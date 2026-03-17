use ic20c::diagnostic::Span;
use ic20c::ir::ast::{Item, Program as AstProgram};
use ic20c::ir::bound::{
    AssignmentTarget, Block, ElseClause, Expression, ExpressionKind, IfStatement,
    Program as BoundProgram, Statement, SymbolId, SymbolKind as BoundSymbolKind, WhileStatement,
};

use crate::convert::contains;

fn is_identifier_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn find_name_span_from(source: &str, from: usize, name: &str) -> Option<Span> {
    let bytes = source.as_bytes();
    let name_bytes = name.as_bytes();
    let name_len = name.len();
    if name_len == 0 {
        return None;
    }
    let mut i = from;
    while i + name_len <= bytes.len() {
        if bytes[i..i + name_len] == *name_bytes {
            let before_ok = i == 0 || !is_identifier_char(bytes[i - 1]);
            let after_ok = i + name_len >= bytes.len() || !is_identifier_char(bytes[i + name_len]);
            if before_ok && after_ok {
                return Some(Span::new(i, i + name_len));
            }
        }
        i += 1;
    }
    None
}

fn collect_text_spans(source: &str, name: &str) -> Vec<Span> {
    let bytes = source.as_bytes();
    let name_bytes = name.as_bytes();
    let name_len = name.len();
    let mut spans = Vec::new();
    if name_len == 0 {
        return spans;
    }
    let mut i = 0;
    while i + name_len <= bytes.len() {
        if bytes[i..i + name_len] == *name_bytes {
            let before_ok = i == 0 || !is_identifier_char(bytes[i - 1]);
            let after_ok = i + name_len >= bytes.len() || !is_identifier_char(bytes[i + name_len]);
            if before_ok && after_ok {
                spans.push(Span::new(i, i + name_len));
            }
        }
        i += 1;
    }
    spans
}

fn find_symbol_at_offset(
    program: &BoundProgram,
    source: &str,
    offset: usize,
) -> Option<(SymbolId, Span)> {
    for function in &program.functions {
        if !contains(function.span, offset) {
            continue;
        }
        if let Some(name_span) = find_name_span_from(source, function.span.start, &function.name)
            && contains(name_span, offset)
        {
            return Some((function.symbol_id, name_span));
        }
        for param in &function.parameters {
            let name_span = Span::new(param.span.start, param.span.start + param.name.len());
            if contains(name_span, offset) {
                return Some((param.symbol_id, name_span));
            }
        }
        if let Some(result) = find_rename_symbol_in_block(program, source, &function.body, offset) {
            return Some(result);
        }
    }
    for init in &program.static_initializers {
        if let Some(result) =
            find_rename_symbol_in_expression(program, source, &init.expression, offset)
        {
            return Some(result);
        }
    }
    None
}

fn find_rename_symbol_in_block(
    program: &BoundProgram,
    source: &str,
    block: &Block,
    offset: usize,
) -> Option<(SymbolId, Span)> {
    if !contains(block.span, offset) {
        return None;
    }
    for statement in &block.statements {
        if let Some(result) = find_rename_symbol_in_statement(program, source, statement, offset) {
            return Some(result);
        }
    }
    None
}

fn find_rename_symbol_in_statement(
    program: &BoundProgram,
    source: &str,
    statement: &Statement,
    offset: usize,
) -> Option<(SymbolId, Span)> {
    match statement {
        Statement::Let(let_stmt) => {
            let name = &program.symbols.get(let_stmt.symbol_id).name;
            if let Some(name_span) = find_name_span_from(source, let_stmt.span.start, name)
                && contains(name_span, offset)
            {
                return Some((let_stmt.symbol_id, name_span));
            }
            find_rename_symbol_in_expression(program, source, &let_stmt.init, offset)
        }
        Statement::Assign(assign) => {
            match &assign.target {
                AssignmentTarget::Variable { symbol_id, span } => {
                    if contains(*span, offset) {
                        return Some((*symbol_id, *span));
                    }
                }
                AssignmentTarget::SlotField { slot, .. } => {
                    if let Some(result) =
                        find_rename_symbol_in_expression(program, source, slot, offset)
                    {
                        return Some(result);
                    }
                }
                AssignmentTarget::DeviceField { .. } => {}
            }
            find_rename_symbol_in_expression(program, source, &assign.value, offset)
        }
        Statement::Expression(expression_statement) => find_rename_symbol_in_expression(
            program,
            source,
            &expression_statement.expression,
            offset,
        ),
        Statement::If(if_statement) => {
            find_rename_symbol_in_if(program, source, if_statement, offset)
        }
        Statement::While(WhileStatement {
            condition, body, ..
        }) => {
            if let Some(result) =
                find_rename_symbol_in_expression(program, source, condition, offset)
            {
                return Some(result);
            }
            find_rename_symbol_in_block(program, source, body, offset)
        }
        Statement::For(for_stmt) => {
            let name = &program.symbols.get(for_stmt.variable).name;
            if let Some(name_span) = find_name_span_from(source, for_stmt.span.start, name)
                && contains(name_span, offset)
            {
                return Some((for_stmt.variable, name_span));
            }
            if let Some(result) =
                find_rename_symbol_in_expression(program, source, &for_stmt.lower, offset)
            {
                return Some(result);
            }
            if let Some(result) =
                find_rename_symbol_in_expression(program, source, &for_stmt.upper, offset)
            {
                return Some(result);
            }
            if let Some(step) = &for_stmt.step
                && let Some(result) =
                    find_rename_symbol_in_expression(program, source, step, offset)
            {
                return Some(result);
            }
            find_rename_symbol_in_block(program, source, &for_stmt.body, offset)
        }
        Statement::Return(ret) => ret
            .value
            .as_ref()
            .and_then(|v| find_rename_symbol_in_expression(program, source, v, offset)),
        Statement::Sleep(sleep) => {
            find_rename_symbol_in_expression(program, source, &sleep.duration, offset)
        }
        Statement::BatchWrite(bw) => {
            if let Some(result) =
                find_rename_symbol_in_expression(program, source, &bw.hash_expr, offset)
            {
                return Some(result);
            }
            find_rename_symbol_in_expression(program, source, &bw.value, offset)
        }
        _ => None,
    }
}

fn find_rename_symbol_in_if(
    program: &BoundProgram,
    source: &str,
    if_statement: &IfStatement,
    offset: usize,
) -> Option<(SymbolId, Span)> {
    if let Some(result) =
        find_rename_symbol_in_expression(program, source, &if_statement.condition, offset)
    {
        return Some(result);
    }
    if let Some(result) =
        find_rename_symbol_in_block(program, source, &if_statement.then_block, offset)
    {
        return Some(result);
    }
    if let Some(else_clause) = &if_statement.else_clause {
        match else_clause {
            ElseClause::Block(block) => {
                return find_rename_symbol_in_block(program, source, block, offset);
            }
            ElseClause::If(nested) => {
                return find_rename_symbol_in_if(program, source, nested, offset);
            }
        }
    }
    None
}

#[allow(clippy::only_used_in_recursion)]
fn find_rename_symbol_in_expression(
    program: &BoundProgram,
    source: &str,
    expression: &Expression,
    offset: usize,
) -> Option<(SymbolId, Span)> {
    if !contains(expression.span, offset) {
        return None;
    }
    match &expression.kind {
        ExpressionKind::Variable(symbol_id) => Some((*symbol_id, expression.span)),
        ExpressionKind::Call(symbol_id, args) => {
            for arg in args {
                if let Some(result) = find_rename_symbol_in_expression(program, source, arg, offset)
                {
                    return Some(result);
                }
            }
            let name = &program.symbols.get(*symbol_id).name;
            let name_span = Span::new(expression.span.start, expression.span.start + name.len());
            Some((*symbol_id, name_span))
        }
        ExpressionKind::IntrinsicCall(_, args) => {
            for arg in args {
                if let Some(result) = find_rename_symbol_in_expression(program, source, arg, offset)
                {
                    return Some(result);
                }
            }
            None
        }
        ExpressionKind::Binary(_, left, right) => {
            if let Some(result) = find_rename_symbol_in_expression(program, source, left, offset) {
                return Some(result);
            }
            find_rename_symbol_in_expression(program, source, right, offset)
        }
        ExpressionKind::Unary(_, operand) => {
            find_rename_symbol_in_expression(program, source, operand, offset)
        }
        ExpressionKind::Cast(inner, _) => {
            find_rename_symbol_in_expression(program, source, inner, offset)
        }
        ExpressionKind::Select {
            condition,
            if_true,
            if_false,
        } => {
            if let Some(result) =
                find_rename_symbol_in_expression(program, source, condition, offset)
            {
                return Some(result);
            }
            if let Some(result) = find_rename_symbol_in_expression(program, source, if_true, offset)
            {
                return Some(result);
            }
            find_rename_symbol_in_expression(program, source, if_false, offset)
        }
        ExpressionKind::SlotRead { slot, .. } => {
            find_rename_symbol_in_expression(program, source, slot, offset)
        }
        ExpressionKind::BatchRead { hash_expr, .. } => {
            find_rename_symbol_in_expression(program, source, hash_expr, offset)
        }
        _ => None,
    }
}

fn collect_all_symbol_spans(
    program: &BoundProgram,
    ast: &AstProgram,
    source: &str,
    symbol_id: SymbolId,
) -> Vec<Span> {
    let info = program.symbols.get(symbol_id);
    let name = info.name.clone();
    let mut spans = Vec::new();

    if info.kind == BoundSymbolKind::Function {
        for function in &program.functions {
            if function.symbol_id == symbol_id {
                if let Some(span) = find_name_span_from(source, function.span.start, &name) {
                    spans.push(span);
                }
                break;
            }
        }
    }

    if matches!(info.kind, BoundSymbolKind::Static(_)) {
        for item in &ast.items {
            if let Item::Static(s) = item
                && s.name == name
                && let Some(span) = find_name_span_from(source, s.span.start, &name)
            {
                spans.push(span);
            }
        }
    }

    for function in &program.functions {
        for param in &function.parameters {
            if param.symbol_id == symbol_id {
                spans.push(Span::new(param.span.start, param.span.start + name.len()));
            }
        }
        collect_spans_in_block(source, &function.body, symbol_id, &name, &mut spans);
    }

    for init in &program.static_initializers {
        collect_spans_in_expression(source, &init.expression, symbol_id, &name, &mut spans);
    }

    spans.sort_by_key(|s| s.start);
    spans.dedup_by_key(|s| s.start);
    spans
}

fn collect_spans_in_block(
    source: &str,
    block: &Block,
    symbol_id: SymbolId,
    name: &str,
    spans: &mut Vec<Span>,
) {
    for statement in &block.statements {
        collect_spans_in_statement(source, statement, symbol_id, name, spans);
    }
}

fn collect_spans_in_statement(
    source: &str,
    statement: &Statement,
    symbol_id: SymbolId,
    name: &str,
    spans: &mut Vec<Span>,
) {
    match statement {
        Statement::Let(let_stmt) => {
            if let_stmt.symbol_id == symbol_id
                && let Some(span) = find_name_span_from(source, let_stmt.span.start, name)
            {
                spans.push(span);
            }
            collect_spans_in_expression(source, &let_stmt.init, symbol_id, name, spans);
        }
        Statement::Assign(assign) => {
            match &assign.target {
                AssignmentTarget::Variable {
                    symbol_id: id,
                    span,
                } => {
                    if *id == symbol_id {
                        spans.push(*span);
                    }
                }
                AssignmentTarget::SlotField { slot, .. } => {
                    collect_spans_in_expression(source, slot, symbol_id, name, spans);
                }
                AssignmentTarget::DeviceField { .. } => {}
            }
            collect_spans_in_expression(source, &assign.value, symbol_id, name, spans);
        }
        Statement::Expression(expression_statement) => {
            collect_spans_in_expression(
                source,
                &expression_statement.expression,
                symbol_id,
                name,
                spans,
            );
        }
        Statement::If(if_statement) => {
            collect_spans_in_if(source, if_statement, symbol_id, name, spans);
        }
        Statement::While(WhileStatement {
            condition, body, ..
        }) => {
            collect_spans_in_expression(source, condition, symbol_id, name, spans);
            collect_spans_in_block(source, body, symbol_id, name, spans);
        }
        Statement::For(for_stmt) => {
            if for_stmt.variable == symbol_id
                && let Some(span) = find_name_span_from(source, for_stmt.span.start, name)
            {
                spans.push(span);
            }
            collect_spans_in_expression(source, &for_stmt.lower, symbol_id, name, spans);
            collect_spans_in_expression(source, &for_stmt.upper, symbol_id, name, spans);
            if let Some(step) = &for_stmt.step {
                collect_spans_in_expression(source, step, symbol_id, name, spans);
            }
            collect_spans_in_block(source, &for_stmt.body, symbol_id, name, spans);
        }
        Statement::Return(ret) => {
            if let Some(value) = &ret.value {
                collect_spans_in_expression(source, value, symbol_id, name, spans);
            }
        }
        Statement::Sleep(sleep) => {
            collect_spans_in_expression(source, &sleep.duration, symbol_id, name, spans);
        }
        Statement::BatchWrite(bw) => {
            collect_spans_in_expression(source, &bw.hash_expr, symbol_id, name, spans);
            collect_spans_in_expression(source, &bw.value, symbol_id, name, spans);
        }
        _ => {}
    }
}

fn collect_spans_in_if(
    source: &str,
    if_statement: &IfStatement,
    symbol_id: SymbolId,
    name: &str,
    spans: &mut Vec<Span>,
) {
    collect_spans_in_expression(source, &if_statement.condition, symbol_id, name, spans);
    collect_spans_in_block(source, &if_statement.then_block, symbol_id, name, spans);
    if let Some(else_clause) = &if_statement.else_clause {
        match else_clause {
            ElseClause::Block(block) => {
                collect_spans_in_block(source, block, symbol_id, name, spans);
            }
            ElseClause::If(nested) => {
                collect_spans_in_if(source, nested, symbol_id, name, spans);
            }
        }
    }
}

#[allow(clippy::only_used_in_recursion)]
fn collect_spans_in_expression(
    source: &str,
    expression: &Expression,
    symbol_id: SymbolId,
    name: &str,
    spans: &mut Vec<Span>,
) {
    match &expression.kind {
        ExpressionKind::Variable(id) => {
            if *id == symbol_id {
                spans.push(expression.span);
            }
        }
        ExpressionKind::Call(id, args) => {
            if *id == symbol_id {
                spans.push(Span::new(
                    expression.span.start,
                    expression.span.start + name.len(),
                ));
            }
            for arg in args {
                collect_spans_in_expression(source, arg, symbol_id, name, spans);
            }
        }
        ExpressionKind::IntrinsicCall(_, args) => {
            for arg in args {
                collect_spans_in_expression(source, arg, symbol_id, name, spans);
            }
        }
        ExpressionKind::Binary(_, left, right) => {
            collect_spans_in_expression(source, left, symbol_id, name, spans);
            collect_spans_in_expression(source, right, symbol_id, name, spans);
        }
        ExpressionKind::Unary(_, operand) => {
            collect_spans_in_expression(source, operand, symbol_id, name, spans);
        }
        ExpressionKind::Cast(inner, _) => {
            collect_spans_in_expression(source, inner, symbol_id, name, spans);
        }
        ExpressionKind::Select {
            condition,
            if_true,
            if_false,
        } => {
            collect_spans_in_expression(source, condition, symbol_id, name, spans);
            collect_spans_in_expression(source, if_true, symbol_id, name, spans);
            collect_spans_in_expression(source, if_false, symbol_id, name, spans);
        }
        ExpressionKind::SlotRead { slot, .. } => {
            collect_spans_in_expression(source, slot, symbol_id, name, spans);
        }
        ExpressionKind::BatchRead { hash_expr, .. } => {
            collect_spans_in_expression(source, hash_expr, symbol_id, name, spans);
        }
        _ => {}
    }
}

pub fn find_rename_target(
    program: &BoundProgram,
    ast: &AstProgram,
    source: &str,
    offset: usize,
) -> Option<(String, Span, Vec<Span>)> {
    if let Some((symbol_id, name_span)) = find_symbol_at_offset(program, source, offset) {
        let name = program.symbols.get(symbol_id).name.clone();
        let all_spans = collect_all_symbol_spans(program, ast, source, symbol_id);
        return Some((name, name_span, all_spans));
    }
    for item in &ast.items {
        match item {
            Item::Const(c) => {
                if let Some(name_span) = find_name_span_from(source, c.span.start, &c.name)
                    && contains(name_span, offset)
                {
                    return Some((
                        c.name.clone(),
                        name_span,
                        collect_text_spans(source, &c.name),
                    ));
                }
            }
            Item::Device(d) => {
                if let Some(name_span) = find_name_span_from(source, d.span.start, &d.name)
                    && contains(name_span, offset)
                {
                    return Some((
                        d.name.clone(),
                        name_span,
                        collect_text_spans(source, &d.name),
                    ));
                }
            }
            _ => {}
        }
    }
    None
}
