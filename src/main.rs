use std::collections::HashMap;
use std::sync::Mutex;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use ic20c::bind;
use ic20c::diagnostic::{Diagnostic as CompilerDiagnostic, Severity as CompilerSeverity, Span};
use ic20c::ir::ast::{Item, Program as AstProgram};
use ic20c::ir::bound::{
    Block, ElseClause, Expression, ExpressionKind, ForStatement, IfStatement, LetStatement,
    Program as BoundProgram, Statement, SymbolId, SymbolKind as BoundSymbolKind, WhileStatement,
};
use ic20c::ir::{Intrinsic, Type};
use ic20c::parser;

struct DocumentState {
    source: String,
    ast: AstProgram,
    bound: Option<BoundProgram>,
}

struct Backend {
    client: Client,
    documents: Mutex<HashMap<Url, DocumentState>>,
}

impl Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            documents: Mutex::new(HashMap::new()),
        }
    }

    fn analyze(&self, uri: &Url, source: String) -> Vec<CompilerDiagnostic> {
        let mut all_diagnostics = Vec::new();

        let (ast, parse_diagnostics) = parser::parse(&source);
        all_diagnostics.extend(parse_diagnostics);

        let has_parse_errors = all_diagnostics
            .iter()
            .any(|d| d.severity == CompilerSeverity::Error);

        let bound = if !has_parse_errors {
            match bind::bind(&ast) {
                Ok((program, bind_diagnostics)) => {
                    all_diagnostics.extend(bind_diagnostics);
                    Some(program)
                }
                Err(diagnostics) => {
                    all_diagnostics.extend(diagnostics);
                    None
                }
            }
        } else {
            None
        };

        let mut documents = self.documents.lock().unwrap();
        documents.insert(uri.clone(), DocumentState { source, ast, bound });

        all_diagnostics
    }
}

fn span_to_range(source: &str, span: Span) -> Range {
    let start = offset_to_position(source, span.start);
    let end = offset_to_position(source, span.end);
    Range { start, end }
}

fn offset_to_position(source: &str, offset: usize) -> Position {
    let offset = offset.min(source.len());
    let mut line = 0u32;
    let mut col = 0u32;
    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    Position::new(line, col)
}

fn position_to_offset(source: &str, position: Position) -> usize {
    let mut line = 0u32;
    let mut col = 0u32;
    for (i, ch) in source.char_indices() {
        if line == position.line && col == position.character {
            return i;
        }
        if ch == '\n' {
            if line == position.line {
                return i;
            }
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    source.len()
}

fn compiler_to_lsp_diagnostics(
    diagnostics: &[CompilerDiagnostic],
    source: &str,
) -> Vec<Diagnostic> {
    diagnostics
        .iter()
        .map(|d| {
            let severity = match d.severity {
                CompilerSeverity::Error => DiagnosticSeverity::ERROR,
                CompilerSeverity::Warning => DiagnosticSeverity::WARNING,
            };
            Diagnostic {
                range: span_to_range(source, d.span),
                severity: Some(severity),
                source: Some("ic20".to_string()),
                message: d.message.clone(),
                ..Diagnostic::default()
            }
        })
        .collect()
}

fn type_name(ty: &Type) -> &'static str {
    match ty {
        Type::Bool => "bool",
        Type::I53 => "i53",
        Type::F64 => "f64",
        Type::Unit => "()",
    }
}

fn intrinsic_signature(intrinsic: &Intrinsic) -> &'static str {
    match intrinsic {
        Intrinsic::Abs => "fn abs(f64) -> f64",
        Intrinsic::Ceil => "fn ceil(f64) -> f64",
        Intrinsic::Floor => "fn floor(f64) -> f64",
        Intrinsic::Round => "fn round(f64) -> f64",
        Intrinsic::Trunc => "fn trunc(f64) -> f64",
        Intrinsic::Sqrt => "fn sqrt(f64) -> f64",
        Intrinsic::Exp => "fn exp(f64) -> f64",
        Intrinsic::Log => "fn log(f64) -> f64",
        Intrinsic::Sin => "fn sin(f64) -> f64",
        Intrinsic::Cos => "fn cos(f64) -> f64",
        Intrinsic::Tan => "fn tan(f64) -> f64",
        Intrinsic::Asin => "fn asin(f64) -> f64",
        Intrinsic::Acos => "fn acos(f64) -> f64",
        Intrinsic::Atan => "fn atan(f64) -> f64",
        Intrinsic::Atan2 => "fn atan2(y: f64, x: f64) -> f64",
        Intrinsic::Pow => "fn pow(base: f64, exp: f64) -> f64",
        Intrinsic::Min => "fn min(f64, f64) -> f64",
        Intrinsic::Max => "fn max(f64, f64) -> f64",
        Intrinsic::Lerp => "fn lerp(a: f64, b: f64, t: f64) -> f64",
        Intrinsic::Clamp => "fn clamp(x: f64, lo: f64, hi: f64) -> f64",
        Intrinsic::Rand => "fn rand() -> f64",
        Intrinsic::IsNan => "fn is_nan(f64) -> bool",
    }
}

struct HoverResult {
    text: String,
    span: Span,
}

fn find_hover_in_bound(program: &BoundProgram, offset: usize) -> Option<HoverResult> {
    for function in &program.functions {
        if !contains(function.span, offset) {
            continue;
        }
        for param in &function.parameters {
            if contains(param.span, offset) {
                let info = program.symbols.get(param.symbol_id);
                return Some(HoverResult {
                    text: format!(
                        "```ic20\n{}: {}\n```\nParameter of `{}`",
                        info.name,
                        type_name(&info.ty),
                        function.name
                    ),
                    span: param.span,
                });
            }
        }
        if let Some(result) = find_hover_in_block(program, &function.body, offset) {
            return Some(result);
        }
    }
    None
}

fn find_hover_in_block(
    program: &BoundProgram,
    block: &Block,
    offset: usize,
) -> Option<HoverResult> {
    for statement in &block.statements {
        if let Some(result) = find_hover_in_statement(program, statement, offset) {
            return Some(result);
        }
    }
    None
}

fn find_hover_in_statement(
    program: &BoundProgram,
    statement: &Statement,
    offset: usize,
) -> Option<HoverResult> {
    match statement {
        Statement::Let(LetStatement {
            symbol_id, init, ..
        }) => {
            if let Some(result) = find_hover_in_expression(program, init, offset) {
                return Some(result);
            }
            let info = program.symbols.get(*symbol_id);
            let stmt_span = statement.span();
            if contains(stmt_span, offset) {
                return Some(HoverResult {
                    text: format!(
                        "```ic20\nlet {}{}: {}\n```",
                        if info.mutable { "mut " } else { "" },
                        info.name,
                        type_name(&info.ty),
                    ),
                    span: stmt_span,
                });
            }
        }
        Statement::Assign(assign) => {
            if let Some(result) = find_hover_in_expression(program, &assign.value, offset) {
                return Some(result);
            }
        }
        Statement::Expression(expression_statement) => {
            if let Some(result) =
                find_hover_in_expression(program, &expression_statement.expression, offset)
            {
                return Some(result);
            }
        }
        Statement::If(if_statement) => {
            return find_hover_in_if(program, if_statement, offset);
        }
        Statement::While(WhileStatement {
            condition, body, ..
        }) => {
            if let Some(result) = find_hover_in_expression(program, condition, offset) {
                return Some(result);
            }
            if let Some(result) = find_hover_in_block(program, body, offset) {
                return Some(result);
            }
        }
        Statement::For(ForStatement {
            variable,
            body,
            lower,
            upper,
            ..
        }) => {
            let info = program.symbols.get(*variable);
            let stmt_span = statement.span();
            if contains(stmt_span, offset) && !contains(body.span, offset) {
                return Some(HoverResult {
                    text: format!("```ic20\n{}: i53\n```\nFor loop variable", info.name),
                    span: stmt_span,
                });
            }
            if let Some(result) = find_hover_in_expression(program, lower, offset) {
                return Some(result);
            }
            if let Some(result) = find_hover_in_expression(program, upper, offset) {
                return Some(result);
            }
            if let Some(result) = find_hover_in_block(program, body, offset) {
                return Some(result);
            }
        }
        Statement::Return(ret) => {
            if let Some(value) = &ret.value
                && let Some(result) = find_hover_in_expression(program, value, offset)
            {
                return Some(result);
            }
        }
        Statement::Sleep(sleep) => {
            if let Some(result) = find_hover_in_expression(program, &sleep.duration, offset) {
                return Some(result);
            }
        }
        _ => {}
    }
    None
}

fn find_hover_in_if(
    program: &BoundProgram,
    if_statement: &IfStatement,
    offset: usize,
) -> Option<HoverResult> {
    if let Some(result) = find_hover_in_expression(program, &if_statement.condition, offset) {
        return Some(result);
    }
    if let Some(result) = find_hover_in_block(program, &if_statement.then_block, offset) {
        return Some(result);
    }
    if let Some(else_clause) = &if_statement.else_clause {
        match else_clause {
            ElseClause::Block(block) => {
                if let Some(result) = find_hover_in_block(program, block, offset) {
                    return Some(result);
                }
            }
            ElseClause::If(nested) => {
                if let Some(result) = find_hover_in_if(program, nested, offset) {
                    return Some(result);
                }
            }
        }
    }
    None
}

fn find_hover_in_expression(
    program: &BoundProgram,
    expression: &Expression,
    offset: usize,
) -> Option<HoverResult> {
    if !contains(expression.span, offset) {
        return None;
    }

    match &expression.kind {
        ExpressionKind::Variable(symbol_id) => {
            let info = program.symbols.get(*symbol_id);
            Some(HoverResult {
                text: format!(
                    "```ic20\n{}{}: {}\n```",
                    match info.kind {
                        BoundSymbolKind::Local =>
                            if info.mutable {
                                "let mut "
                            } else {
                                "let "
                            },
                        BoundSymbolKind::Parameter => "",
                        BoundSymbolKind::Function => "fn ",
                    },
                    info.name,
                    type_name(&info.ty)
                ),
                span: expression.span,
            })
        }
        ExpressionKind::Call(symbol_id, args) => {
            for arg in args {
                if let Some(result) = find_hover_in_expression(program, arg, offset) {
                    return Some(result);
                }
            }
            let info = program.symbols.get(*symbol_id);
            Some(HoverResult {
                text: format!("```ic20\nfn {}: {}\n```", info.name, type_name(&info.ty)),
                span: expression.span,
            })
        }
        ExpressionKind::IntrinsicCall(intrinsic, args) => {
            for arg in args {
                if let Some(result) = find_hover_in_expression(program, arg, offset) {
                    return Some(result);
                }
            }
            Some(HoverResult {
                text: format!(
                    "```ic20\n{}\n```\nIC10 intrinsic",
                    intrinsic_signature(intrinsic)
                ),
                span: expression.span,
            })
        }
        ExpressionKind::Binary(_, left, right) => {
            if let Some(result) = find_hover_in_expression(program, left, offset) {
                return Some(result);
            }
            if let Some(result) = find_hover_in_expression(program, right, offset) {
                return Some(result);
            }
            Some(HoverResult {
                text: format!("```ic20\n{}\n```", type_name(&expression.ty)),
                span: expression.span,
            })
        }
        ExpressionKind::Unary(_, operand) => {
            if let Some(result) = find_hover_in_expression(program, operand, offset) {
                return Some(result);
            }
            Some(HoverResult {
                text: format!("```ic20\n{}\n```", type_name(&expression.ty)),
                span: expression.span,
            })
        }
        ExpressionKind::Cast(inner, target) => {
            if let Some(result) = find_hover_in_expression(program, inner, offset) {
                return Some(result);
            }
            Some(HoverResult {
                text: format!(
                    "```ic20\n{}\n```\nCast to `{}`",
                    type_name(target),
                    type_name(target)
                ),
                span: expression.span,
            })
        }
        ExpressionKind::DeviceRead { pin, field } => Some(HoverResult {
            text: format!("```ic20\n{:?}.{}: f64\n```\nDevice field read", pin, field),
            span: expression.span,
        }),
        ExpressionKind::SlotRead {
            pin, slot, field, ..
        } => {
            if let Some(result) = find_hover_in_expression(program, slot, offset) {
                return Some(result);
            }
            Some(HoverResult {
                text: format!(
                    "```ic20\n{:?}.slot(...).{}: f64\n```\nSlot field read",
                    pin, field
                ),
                span: expression.span,
            })
        }
        ExpressionKind::BatchRead {
            hash_expr,
            field,
            mode,
            ..
        } => {
            if let Some(result) = find_hover_in_expression(program, hash_expr, offset) {
                return Some(result);
            }
            Some(HoverResult {
                text: format!(
                    "```ic20\nbatch_read(..., {}, {:?}): f64\n```\nBatch read operation",
                    field, mode
                ),
                span: expression.span,
            })
        }
        ExpressionKind::Select {
            condition,
            if_true,
            if_false,
        } => {
            if let Some(result) = find_hover_in_expression(program, condition, offset) {
                return Some(result);
            }
            if let Some(result) = find_hover_in_expression(program, if_true, offset) {
                return Some(result);
            }
            if let Some(result) = find_hover_in_expression(program, if_false, offset) {
                return Some(result);
            }
            Some(HoverResult {
                text: format!(
                    "```ic20\nselect(...): {}\n```\nTernary conditional",
                    type_name(&expression.ty)
                ),
                span: expression.span,
            })
        }
        ExpressionKind::Literal(_) => Some(HoverResult {
            text: format!("```ic20\n{}\n```", type_name(&expression.ty)),
            span: expression.span,
        }),
    }
}

fn contains(span: Span, offset: usize) -> bool {
    offset >= span.start && offset < span.end
}

struct DefinitionLocation {
    span: Span,
}

fn find_definition_in_bound(
    program: &BoundProgram,
    ast: &AstProgram,
    offset: usize,
) -> Option<DefinitionLocation> {
    for function in &program.functions {
        if !contains(function.span, offset) {
            continue;
        }
        if let Some(result) = find_definition_in_block(program, ast, &function.body, offset) {
            return Some(result);
        }
    }
    None
}

fn find_definition_in_block(
    program: &BoundProgram,
    ast: &AstProgram,
    block: &Block,
    offset: usize,
) -> Option<DefinitionLocation> {
    for statement in &block.statements {
        if let Some(result) = find_definition_in_statement(program, ast, statement, offset) {
            return Some(result);
        }
    }
    None
}

fn find_definition_in_statement(
    program: &BoundProgram,
    ast: &AstProgram,
    statement: &Statement,
    offset: usize,
) -> Option<DefinitionLocation> {
    match statement {
        Statement::Let(LetStatement { init, .. }) => {
            find_definition_in_expression(program, ast, init, offset)
        }
        Statement::Assign(assign) => {
            find_definition_in_expression(program, ast, &assign.value, offset)
        }
        Statement::Expression(expression_statement) => {
            find_definition_in_expression(program, ast, &expression_statement.expression, offset)
        }
        Statement::If(if_statement) => find_definition_in_if(program, ast, if_statement, offset),
        Statement::While(WhileStatement {
            condition, body, ..
        }) => {
            if let Some(result) = find_definition_in_expression(program, ast, condition, offset) {
                return Some(result);
            }
            find_definition_in_block(program, ast, body, offset)
        }
        Statement::For(ForStatement {
            lower, upper, body, ..
        }) => {
            if let Some(result) = find_definition_in_expression(program, ast, lower, offset) {
                return Some(result);
            }
            if let Some(result) = find_definition_in_expression(program, ast, upper, offset) {
                return Some(result);
            }
            find_definition_in_block(program, ast, body, offset)
        }
        Statement::Return(ret) => ret
            .value
            .as_ref()
            .and_then(|v| find_definition_in_expression(program, ast, v, offset)),
        Statement::Sleep(sleep) => {
            find_definition_in_expression(program, ast, &sleep.duration, offset)
        }
        _ => None,
    }
}

fn find_definition_in_if(
    program: &BoundProgram,
    ast: &AstProgram,
    if_statement: &IfStatement,
    offset: usize,
) -> Option<DefinitionLocation> {
    if let Some(result) =
        find_definition_in_expression(program, ast, &if_statement.condition, offset)
    {
        return Some(result);
    }
    if let Some(result) = find_definition_in_block(program, ast, &if_statement.then_block, offset) {
        return Some(result);
    }
    if let Some(else_clause) = &if_statement.else_clause {
        match else_clause {
            ElseClause::Block(block) => {
                return find_definition_in_block(program, ast, block, offset);
            }
            ElseClause::If(nested) => {
                return find_definition_in_if(program, ast, nested, offset);
            }
        }
    }
    None
}

fn find_definition_in_expression(
    program: &BoundProgram,
    ast: &AstProgram,
    expression: &Expression,
    offset: usize,
) -> Option<DefinitionLocation> {
    if !contains(expression.span, offset) {
        return None;
    }
    match &expression.kind {
        ExpressionKind::Variable(symbol_id) => find_symbol_definition(program, ast, *symbol_id),
        ExpressionKind::Call(symbol_id, args) => {
            for arg in args {
                if let Some(result) = find_definition_in_expression(program, ast, arg, offset) {
                    return Some(result);
                }
            }
            find_symbol_definition(program, ast, *symbol_id)
        }
        ExpressionKind::IntrinsicCall(_, args) => {
            for arg in args {
                if let Some(result) = find_definition_in_expression(program, ast, arg, offset) {
                    return Some(result);
                }
            }
            None
        }
        ExpressionKind::Binary(_, left, right) => {
            if let Some(result) = find_definition_in_expression(program, ast, left, offset) {
                return Some(result);
            }
            find_definition_in_expression(program, ast, right, offset)
        }
        ExpressionKind::Unary(_, operand) => {
            find_definition_in_expression(program, ast, operand, offset)
        }
        ExpressionKind::Cast(inner, _) => {
            find_definition_in_expression(program, ast, inner, offset)
        }
        ExpressionKind::Select {
            condition,
            if_true,
            if_false,
        } => {
            if let Some(result) = find_definition_in_expression(program, ast, condition, offset) {
                return Some(result);
            }
            if let Some(result) = find_definition_in_expression(program, ast, if_true, offset) {
                return Some(result);
            }
            find_definition_in_expression(program, ast, if_false, offset)
        }
        ExpressionKind::SlotRead { slot, .. } => {
            find_definition_in_expression(program, ast, slot, offset)
        }
        ExpressionKind::BatchRead { hash_expr, .. } => {
            find_definition_in_expression(program, ast, hash_expr, offset)
        }
        _ => None,
    }
}

fn find_symbol_definition(
    program: &BoundProgram,
    ast: &AstProgram,
    symbol_id: SymbolId,
) -> Option<DefinitionLocation> {
    let info = program.symbols.get(symbol_id);

    if info.kind == BoundSymbolKind::Function {
        for function in &program.functions {
            if function.symbol_id == symbol_id {
                return Some(DefinitionLocation {
                    span: function.span,
                });
            }
        }
        return None;
    }

    for function in &program.functions {
        for param in &function.parameters {
            if param.symbol_id == symbol_id {
                return Some(DefinitionLocation { span: param.span });
            }
        }
        if let Some(span) = find_let_in_block(&function.body, symbol_id) {
            return Some(DefinitionLocation { span });
        }
    }

    // Check for const and device declarations in the AST since they're folded away
    // in the bound IR
    for item in &ast.items {
        match item {
            Item::Const(c) if c.name == info.name => {
                return Some(DefinitionLocation { span: c.span });
            }
            Item::Device(d) if d.name == info.name => {
                return Some(DefinitionLocation { span: d.span });
            }
            _ => {}
        }
    }

    None
}

fn find_let_in_block(block: &Block, target: SymbolId) -> Option<Span> {
    for statement in &block.statements {
        if let Some(span) = find_let_in_statement(statement, target) {
            return Some(span);
        }
    }
    None
}

fn find_let_in_statement(statement: &Statement, target: SymbolId) -> Option<Span> {
    match statement {
        Statement::Let(LetStatement {
            symbol_id, span, ..
        }) => {
            if *symbol_id == target {
                return Some(*span);
            }
        }
        Statement::If(if_statement) => {
            return find_let_in_if(if_statement, target);
        }
        Statement::While(WhileStatement { body, .. }) => {
            if let Some(span) = find_let_in_block(body, target) {
                return Some(span);
            }
        }
        Statement::For(ForStatement { body, .. }) => {
            if let Some(span) = find_let_in_block(body, target) {
                return Some(span);
            }
        }
        _ => {}
    }
    None
}

fn find_let_in_if(if_statement: &IfStatement, target: SymbolId) -> Option<Span> {
    if let Some(span) = find_let_in_block(&if_statement.then_block, target) {
        return Some(span);
    }
    if let Some(else_clause) = &if_statement.else_clause {
        match else_clause {
            ElseClause::Block(block) => {
                if let Some(span) = find_let_in_block(block, target) {
                    return Some(span);
                }
            }
            ElseClause::If(nested) => {
                if let Some(span) = find_let_in_if(nested, target) {
                    return Some(span);
                }
            }
        }
    }
    None
}

fn document_symbols_from_ast(ast: &AstProgram, source: &str) -> Vec<DocumentSymbol> {
    let mut symbols = Vec::new();

    for item in &ast.items {
        match item {
            Item::Const(c) => {
                #[allow(deprecated)]
                symbols.push(DocumentSymbol {
                    name: c.name.clone(),
                    detail: Some(format!("const: {}", type_name(&c.ty))),
                    kind: SymbolKind::CONSTANT,
                    tags: None,
                    deprecated: None,
                    range: span_to_range(source, c.span),
                    selection_range: span_to_range(source, c.span),
                    children: None,
                });
            }
            Item::Device(d) => {
                #[allow(deprecated)]
                symbols.push(DocumentSymbol {
                    name: d.name.clone(),
                    detail: Some(format!("device: {:?}", d.pin)),
                    kind: SymbolKind::VARIABLE,
                    tags: None,
                    deprecated: None,
                    range: span_to_range(source, d.span),
                    selection_range: span_to_range(source, d.span),
                    children: None,
                });
            }
            Item::Fn(f) => {
                let params: Vec<String> = f
                    .params
                    .iter()
                    .map(|p| format!("{}: {}", p.name, type_name(&p.ty)))
                    .collect();
                let return_str = f
                    .return_type
                    .as_ref()
                    .map(|t| format!(" -> {}", type_name(t)))
                    .unwrap_or_default();
                let detail = format!("fn({}){}", params.join(", "), return_str);

                let children: Vec<DocumentSymbol> = f
                    .params
                    .iter()
                    .map(|p| {
                        #[allow(deprecated)]
                        DocumentSymbol {
                            name: p.name.clone(),
                            detail: Some(type_name(&p.ty).to_string()),
                            kind: SymbolKind::VARIABLE,
                            tags: None,
                            deprecated: None,
                            range: span_to_range(source, p.span),
                            selection_range: span_to_range(source, p.span),
                            children: None,
                        }
                    })
                    .collect();

                #[allow(deprecated)]
                symbols.push(DocumentSymbol {
                    name: f.name.clone(),
                    detail: Some(detail),
                    kind: SymbolKind::FUNCTION,
                    tags: None,
                    deprecated: None,
                    range: span_to_range(source, f.span),
                    selection_range: span_to_range(source, f.span),
                    children: if children.is_empty() {
                        None
                    } else {
                        Some(children)
                    },
                });
            }
        }
    }

    symbols
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                ..ServerCapabilities::default()
            },
            ..InitializeResult::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "IC20 language server initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let source = params.text_document.text;
        let diagnostics = self.analyze(&uri, source.clone());
        let lsp_diagnostics = compiler_to_lsp_diagnostics(&diagnostics, &source);
        self.client
            .publish_diagnostics(uri, lsp_diagnostics, None)
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        if let Some(change) = params.content_changes.into_iter().last() {
            let source = change.text;
            let diagnostics = self.analyze(&uri, source.clone());
            let lsp_diagnostics = compiler_to_lsp_diagnostics(&diagnostics, &source);
            self.client
                .publish_diagnostics(uri, lsp_diagnostics, None)
                .await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.client
            .publish_diagnostics(uri.clone(), vec![], None)
            .await;
        self.documents.lock().unwrap().remove(&uri);
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let documents = self.documents.lock().unwrap();
        let Some(state) = documents.get(uri) else {
            return Ok(None);
        };

        let offset = position_to_offset(&state.source, position);

        // Try bound IR hover first (more information available)
        if let Some(bound) = &state.bound
            && let Some(result) = find_hover_in_bound(bound, offset)
        {
            return Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: result.text,
                }),
                range: Some(span_to_range(&state.source, result.span)),
            }));
        }

        // Fall back to AST-level hover for consts and devices
        for item in &state.ast.items {
            match item {
                Item::Const(c) if contains(c.span, offset) => {
                    return Ok(Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: format!("```ic20\nconst {}: {}\n```", c.name, type_name(&c.ty)),
                        }),
                        range: Some(span_to_range(&state.source, c.span)),
                    }));
                }
                Item::Device(d) if contains(d.span, offset) => {
                    return Ok(Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: format!("```ic20\ndevice {}: {:?}\n```", d.name, d.pin),
                        }),
                        range: Some(span_to_range(&state.source, d.span)),
                    }));
                }
                _ => {}
            }
        }

        Ok(None)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let documents = self.documents.lock().unwrap();
        let Some(state) = documents.get(uri) else {
            return Ok(None);
        };

        let offset = position_to_offset(&state.source, position);

        if let Some(bound) = &state.bound
            && let Some(def) = find_definition_in_bound(bound, &state.ast, offset)
        {
            let range = span_to_range(&state.source, def.span);
            return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                uri: uri.clone(),
                range,
            })));
        }

        Ok(None)
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = &params.text_document.uri;

        let documents = self.documents.lock().unwrap();
        let Some(state) = documents.get(uri) else {
            return Ok(None);
        };

        let symbols = document_symbols_from_ast(&state.ast, &state.source);
        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
