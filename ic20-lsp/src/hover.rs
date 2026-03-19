use ic20c::diagnostic::Span;
use ic20c::ir::bound::{
    Block, ElseClause, Expression, ExpressionKind, ForStatement, IfStatement, LetStatement,
    Program as BoundProgram, Statement, SymbolKind as BoundSymbolKind, WhileStatement,
};

use crate::convert::{contains, intrinsic_signature, type_name};

pub struct HoverResult {
    pub text: String,
    pub span: Span,
}

pub fn find_hover_in_bound(program: &BoundProgram, offset: usize) -> Option<HoverResult> {
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
                        BoundSymbolKind::Static(_) =>
                            if info.mutable {
                                "static mut "
                            } else {
                                "static "
                            },
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
