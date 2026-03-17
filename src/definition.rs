use ic20c::diagnostic::Span;
use ic20c::ir::ast::{Item, Program as AstProgram};
use ic20c::ir::bound::{
    Block, ElseClause, Expression, ExpressionKind, ForStatement, IfStatement, LetStatement,
    Program as BoundProgram, Statement, SymbolId, SymbolKind as BoundSymbolKind, WhileStatement,
};

use crate::convert::contains;

pub struct DefinitionLocation {
    pub span: Span,
}

pub fn find_definition_in_bound(
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
            Item::Static(s) if s.name == info.name => {
                return Some(DefinitionLocation { span: s.span });
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
