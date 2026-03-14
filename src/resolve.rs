use std::collections::HashMap;

use crate::ast::DevicePin;
use crate::ast::{
    AssignmentTarget as AstAssignmentTarget, BinaryOperator, Block as AstBlock, BuiltinFunction,
    ElseClause as AstElseClause, Expression as AstExpression, ExpressionKind as AstExpressionKind,
    FunctionDeclaration as AstFunctionDeclaration, IfStatement as AstIfStatement, Item,
    LiteralKind, Program as AstProgram, Statement as AstStatement, Type, UnaryOperator,
};
use crate::crc32::crc32;
use crate::diagnostic::{Diagnostic, Severity, Span};
use crate::resolved::{
    AssignStatement, AssignmentTarget, Block, ElseClause, Expression, ExpressionKind,
    ExpressionStatement, ForStatement, FunctionDeclaration, IfStatement, LetStatement, Parameter,
    Program, ReturnStatement, SleepStatement, Statement, SymbolId, SymbolInfo, SymbolKind,
    SymbolTable, WhileStatement,
};

/// An entry in the scope stack.
#[derive(Clone)]
enum ScopeEntry {
    Symbol(SymbolId),
    Constant(f64, Type),
    Device(DevicePin),
}

/// The compile-time signature of a function, recorded during the top-level pre-pass.
struct FunctionSignature {
    symbol_id: SymbolId,
    return_type: Type,
    parameter_types: Vec<Type>,
}

struct Resolver {
    symbols: SymbolTable,
    scopes: Vec<HashMap<String, ScopeEntry>>,
    function_signatures: HashMap<String, FunctionSignature>,
    diagnostics: Vec<Diagnostic>,
}

impl Resolver {
    fn new() -> Self {
        Self {
            symbols: SymbolTable::default(),
            scopes: Vec::new(),
            function_signatures: HashMap::new(),
            diagnostics: Vec::new(),
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn define(&mut self, name: String, entry: ScopeEntry) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, entry);
        }
    }

    fn lookup(&self, name: &str) -> Option<&ScopeEntry> {
        for scope in self.scopes.iter().rev() {
            if let Some(entry) = scope.get(name) {
                return Some(entry);
            }
        }
        None
    }

    fn allocate_symbol(&mut self, info: SymbolInfo) -> SymbolId {
        self.symbols.push(info)
    }

    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::error(span, message));
    }

    fn warning(&mut self, span: Span, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::warning(span, message));
    }

    /// Evaluate a constant expression to `(f64, Type)`.
    fn eval_const(&mut self, expr: &AstExpression) -> Option<(f64, Type)> {
        match &expr.kind {
            AstExpressionKind::Literal(lit) => match lit {
                LiteralKind::I53(v) => Some((*v as f64, Type::I53)),
                LiteralKind::F64(v) => Some((*v, Type::F64)),
                LiteralKind::Bool(b) => Some((if *b { 1.0 } else { 0.0 }, Type::Bool)),
            },
            AstExpressionKind::Hash(s) => Some((crc32(s), Type::F64)),
            AstExpressionKind::Variable(name) => match self.lookup(name) {
                Some(ScopeEntry::Constant(val, ty)) => {
                    let (val, ty) = (*val, *ty);
                    Some((val, ty))
                }
                _ => {
                    self.error(
                        expr.span,
                        format!("cannot use `{name}` in a constant expression"),
                    );
                    None
                }
            },
            AstExpressionKind::Unary(op, inner) => {
                let (v, ty) = self.eval_const(inner)?;
                match op {
                    UnaryOperator::Neg => Some((-v, ty)),
                    UnaryOperator::Not => {
                        if ty != Type::Bool {
                            self.error(expr.span, "logical `!` requires a bool operand");
                            return None;
                        }
                        Some((if v == 0.0 { 1.0 } else { 0.0 }, Type::Bool))
                    }
                    UnaryOperator::BitNot => {
                        if ty != Type::I53 {
                            self.error(expr.span, "bitwise `~` requires an i53 operand");
                            return None;
                        }
                        Some(((!(v as i64)) as f64, Type::I53))
                    }
                }
            }
            AstExpressionKind::Binary(op, lhs, rhs) => {
                let (lv, left_type) = self.eval_const(lhs)?;
                let (rv, right_type) = self.eval_const(rhs)?;
                if left_type != right_type {
                    self.error(
                        expr.span,
                        format!("type mismatch in constant expression: `{left_type:?}` vs `{right_type:?}`"),
                    );
                    return None;
                }
                eval_binary_const(*op, lv, rv, left_type, expr.span, &mut self.diagnostics)
            }
            AstExpressionKind::Cast(inner, target_type) => {
                let (v, src_type) = self.eval_const(inner)?;
                let target_type = *target_type;
                match (src_type, target_type) {
                    (a, b) if a == b => {
                        self.warning(
                            expr.span,
                            format!("identity cast `{a:?} as {b:?}` has no effect"),
                        );
                        Some((v, target_type))
                    }
                    (Type::I53, Type::F64) => Some((v, Type::F64)),
                    (Type::F64, Type::I53) => Some((v.trunc(), Type::I53)),
                    (Type::Bool, Type::I53) => Some((if v != 0.0 { 1.0 } else { 0.0 }, Type::I53)),
                    (Type::Bool, Type::F64) => Some((if v != 0.0 { 1.0 } else { 0.0 }, Type::F64)),
                    (_, Type::Bool) => {
                        self.error(
                            expr.span,
                            "cannot cast to `bool`; use an explicit comparison",
                        );
                        None
                    }
                    _ => {
                        self.error(
                            expr.span,
                            format!("invalid cast `{src_type:?} as {target_type:?}`"),
                        );
                        None
                    }
                }
            }
            _ => {
                self.error(expr.span, "unsupported expression in constant context");
                None
            }
        }
    }

    /// Top-level pre-pass: register all fn/const/device items before resolving
    /// any function body (enables forward references to functions).
    fn pre_pass(&mut self, program: &AstProgram) {
        for item in &program.items {
            match item {
                Item::Const(c) => {
                    if let Some((value, ty)) = self.eval_const(&c.value) {
                        if ty != c.ty {
                            self.error(
                                c.span,
                                format!(
                                    "const `{}` declared as `{:?}` but value has type `{:?}`",
                                    c.name, c.ty, ty
                                ),
                            );
                        }
                        self.define(c.name.clone(), ScopeEntry::Constant(value, c.ty));
                    }
                }
                Item::Device(d) => {
                    self.define(d.name.clone(), ScopeEntry::Device(d.pin));
                }
                Item::Fn(f) => {
                    if f.params.len() > 8 {
                        self.error(
                            f.span,
                            format!(
                                "function `{}` has {} parameters, but the maximum is 8",
                                f.name,
                                f.params.len()
                            ),
                        );
                    }
                    let return_type = f.return_type.unwrap_or(Type::Unit);
                    let parameter_types: Vec<Type> = f.params.iter().map(|p| p.ty).collect();
                    let symbol_id = self.allocate_symbol(SymbolInfo {
                        name: f.name.clone(),
                        ty: return_type,
                        mutable: false,
                        kind: SymbolKind::Function,
                    });
                    self.define(f.name.clone(), ScopeEntry::Symbol(symbol_id));
                    self.function_signatures.insert(
                        f.name.clone(),
                        FunctionSignature {
                            symbol_id,
                            return_type,
                            parameter_types,
                        },
                    );
                }
            }
        }
    }

    fn resolve_function(&mut self, ast_fn: &AstFunctionDeclaration) -> FunctionDeclaration {
        let sig = self.function_signatures.get(&ast_fn.name).unwrap();
        let function_symbol_id = sig.symbol_id;
        let return_type = sig.return_type;

        self.push_scope();

        let mut parameters = Vec::new();
        for ast_param in &ast_fn.params {
            let symbol_id = self.allocate_symbol(SymbolInfo {
                name: ast_param.name.clone(),
                ty: ast_param.ty,
                mutable: false,
                kind: SymbolKind::Parameter,
            });
            self.define(ast_param.name.clone(), ScopeEntry::Symbol(symbol_id));
            parameters.push(Parameter {
                name: ast_param.name.clone(),
                symbol_id,
                ty: ast_param.ty,
                span: ast_param.span,
            });
        }

        let body = self.resolve_block(&ast_fn.body, return_type);
        self.pop_scope();

        FunctionDeclaration {
            name: ast_fn.name.clone(),
            symbol_id: function_symbol_id,
            parameters,
            return_type: (return_type != Type::Unit).then_some(return_type),
            body,
            span: ast_fn.span,
        }
    }

    fn resolve_block(&mut self, block: &AstBlock, return_type: Type) -> Block {
        self.push_scope();
        let mut statements = Vec::new();
        for stmt in &block.stmts {
            let resolved = self.resolve_statement(stmt, return_type);
            statements.push(resolved);
        }
        self.pop_scope();
        Block {
            statements,
            span: block.span,
        }
    }

    fn resolve_statement(&mut self, stmt: &AstStatement, return_type: Type) -> Statement {
        match stmt {
            AstStatement::Let(s) => {
                let init = self.resolve_expression(&s.init);
                let actual_type = init.ty;

                let final_type = match s.ty {
                    Some(annotation_type) => {
                        if annotation_type != actual_type {
                            self.error(
                                s.init.span,
                                format!(
                                    "type mismatch: declared `{annotation_type:?}` but initializer has type `{actual_type:?}`"
                                ),
                            );
                        }
                        annotation_type
                    }
                    None => actual_type,
                };

                let symbol_id = self.allocate_symbol(SymbolInfo {
                    name: s.name.clone(),
                    ty: final_type,
                    mutable: s.mutable,
                    kind: SymbolKind::Local,
                });
                self.define(s.name.clone(), ScopeEntry::Symbol(symbol_id));
                Statement::Let(LetStatement {
                    symbol_id,
                    init,
                    span: s.span,
                })
            }

            AstStatement::Assign(s) => {
                let value = self.resolve_expression(&s.rhs);
                let target = self.resolve_assignment_target(&s.lhs, value.ty, s.span);
                Statement::Assign(AssignStatement {
                    target,
                    value,
                    span: s.span,
                })
            }

            AstStatement::Expression(s) => {
                let expression = self.resolve_expression(&s.expr);
                Statement::Expression(ExpressionStatement {
                    expression,
                    span: s.span,
                })
            }

            AstStatement::If(s) => Statement::If(self.resolve_if_statement(s, return_type)),

            AstStatement::While(s) => {
                let condition = self.resolve_expression(&s.cond);
                if condition.ty != Type::Bool {
                    self.error(
                        s.cond.span,
                        format!(
                            "`while` condition must be `bool`, found `{:?}`",
                            condition.ty
                        ),
                    );
                }
                let body = self.resolve_block(&s.body, return_type);
                Statement::While(WhileStatement {
                    condition,
                    body,
                    span: s.span,
                })
            }

            AstStatement::For(s) => {
                let lower = self.resolve_expression(&s.lower);
                let upper = self.resolve_expression(&s.upper);
                if lower.ty != Type::I53 {
                    self.error(
                        s.lower.span,
                        format!(
                            "`for` range lower bound must be `i53`, found `{:?}`",
                            lower.ty
                        ),
                    );
                }
                if upper.ty != Type::I53 {
                    self.error(
                        s.upper.span,
                        format!(
                            "`for` range upper bound must be `i53`, found `{:?}`",
                            upper.ty
                        ),
                    );
                }
                self.push_scope();
                let variable = self.allocate_symbol(SymbolInfo {
                    name: s.var.clone(),
                    ty: Type::I53,
                    mutable: true,
                    kind: SymbolKind::Local,
                });
                self.define(s.var.clone(), ScopeEntry::Symbol(variable));
                let body = self.resolve_block(&s.body, return_type);
                self.pop_scope();
                Statement::For(ForStatement {
                    variable,
                    lower,
                    upper,
                    body,
                    span: s.span,
                })
            }

            AstStatement::Break(span) => Statement::Break(*span),
            AstStatement::Continue(span) => Statement::Continue(*span),

            AstStatement::Return(s) => {
                let value = s.value.as_ref().map(|v| self.resolve_expression(v));
                let value_type = value.as_ref().map(|v| v.ty);
                match (return_type, value_type) {
                    (Type::Unit, Some(_)) => {
                        self.error(
                            s.span,
                            "cannot return a value from a function with no return type",
                        );
                    }
                    (expected, None) if expected != Type::Unit => {
                        self.error(
                            s.span,
                            format!("missing return value: expected `{expected:?}`"),
                        );
                    }
                    (expected, Some(actual)) if expected != actual => {
                        self.error(
                            s.span,
                            format!(
                                "return type mismatch: expected `{expected:?}`, found `{actual:?}`"
                            ),
                        );
                    }
                    _ => {}
                }
                Statement::Return(ReturnStatement {
                    value,
                    span: s.span,
                })
            }

            AstStatement::Yield(span) => Statement::Yield(*span),

            AstStatement::Sleep(s) => {
                let duration = self.resolve_expression(&s.duration);
                if duration.ty != Type::F64 {
                    self.error(
                        s.duration.span,
                        format!("`sleep` duration must be `f64`, found `{:?}`", duration.ty),
                    );
                }
                Statement::Sleep(SleepStatement {
                    duration,
                    span: s.span,
                })
            }
        }
    }

    fn resolve_assignment_target(
        &mut self,
        target: &AstAssignmentTarget,
        value_type: Type,
        assignment_span: Span,
    ) -> AssignmentTarget {
        match target {
            AstAssignmentTarget::Var { name, span } => match self.lookup(name) {
                Some(ScopeEntry::Symbol(id)) => {
                    let id = *id;
                    let (is_mutable, expected_type) = {
                        let info = self.symbols.get(id);
                        (info.mutable, info.ty)
                    };
                    if !is_mutable {
                        self.error(
                            *span,
                            format!("cannot assign to immutable variable `{name}`"),
                        );
                    }
                    if expected_type != value_type {
                        self.error(
                            assignment_span,
                            format!(
                                "type mismatch in assignment: variable `{name}` is `{expected_type:?}` but value is `{value_type:?}`"
                            ),
                        );
                    }
                    AssignmentTarget::Variable {
                        symbol_id: id,
                        span: *span,
                    }
                }
                Some(ScopeEntry::Constant(_, _)) => {
                    self.error(*span, format!("cannot assign to constant `{name}`"));
                    AssignmentTarget::Variable {
                        symbol_id: SymbolId(0),
                        span: *span,
                    }
                }
                Some(ScopeEntry::Device(_)) => {
                    self.error(
                        *span,
                        format!("`{name}` is a device; use `{name}.Field = ...` to write"),
                    );
                    AssignmentTarget::Variable {
                        symbol_id: SymbolId(0),
                        span: *span,
                    }
                }
                None => {
                    self.error(*span, format!("undeclared name `{name}`"));
                    AssignmentTarget::Variable {
                        symbol_id: SymbolId(0),
                        span: *span,
                    }
                }
            },

            AstAssignmentTarget::DeviceField {
                device,
                field,
                span,
            } => match self.lookup(device) {
                Some(ScopeEntry::Device(pin)) => {
                    let pin = *pin;
                    AssignmentTarget::DeviceField {
                        pin,
                        field: field.clone(),
                        span: *span,
                    }
                }
                _ => {
                    self.error(*span, format!("`{device}` is not a device"));
                    AssignmentTarget::DeviceField {
                        pin: DevicePin::D0,
                        field: field.clone(),
                        span: *span,
                    }
                }
            },

            AstAssignmentTarget::SlotField {
                device,
                slot,
                field,
                span,
            } => match self.lookup(device) {
                Some(ScopeEntry::Device(pin)) => {
                    let pin = *pin;
                    let slot_resolved = self.resolve_expression(slot);
                    if slot_resolved.ty != Type::I53 {
                        self.error(
                            slot.span,
                            format!("slot index must be `i53`, found `{:?}`", slot_resolved.ty),
                        );
                    }
                    AssignmentTarget::SlotField {
                        pin,
                        slot: slot_resolved,
                        field: field.clone(),
                        span: *span,
                    }
                }
                _ => {
                    self.error(*span, format!("`{device}` is not a device"));
                    let slot_resolved = self.resolve_expression(slot);
                    AssignmentTarget::SlotField {
                        pin: DevicePin::D0,
                        slot: slot_resolved,
                        field: field.clone(),
                        span: *span,
                    }
                }
            },
        }
    }

    fn resolve_if_statement(&mut self, s: &AstIfStatement, return_type: Type) -> IfStatement {
        let condition = self.resolve_expression(&s.cond);
        if condition.ty != Type::Bool {
            self.error(
                s.cond.span,
                format!("`if` condition must be `bool`, found `{:?}`", condition.ty),
            );
        }
        let then_block = self.resolve_block(&s.then_block, return_type);
        let else_clause = s
            .else_clause
            .as_ref()
            .map(|e| self.resolve_else_clause(e, return_type));
        IfStatement {
            condition,
            then_block,
            else_clause,
            span: s.span,
        }
    }

    fn resolve_else_clause(&mut self, else_: &AstElseClause, return_type: Type) -> ElseClause {
        match else_ {
            AstElseClause::Block(block) => {
                ElseClause::Block(self.resolve_block(block, return_type))
            }
            AstElseClause::If(if_stmt) => {
                ElseClause::If(Box::new(self.resolve_if_statement(if_stmt, return_type)))
            }
        }
    }

    fn resolve_expression(&mut self, expr: &AstExpression) -> Expression {
        match &expr.kind {
            AstExpressionKind::Literal(lit) => {
                let (value, ty) = match lit {
                    LiteralKind::I53(v) => (*v as f64, Type::I53),
                    LiteralKind::F64(v) => (*v, Type::F64),
                    LiteralKind::Bool(b) => (if *b { 1.0 } else { 0.0 }, Type::Bool),
                };
                Expression {
                    kind: ExpressionKind::Literal(value),
                    ty,
                    span: expr.span,
                }
            }

            AstExpressionKind::Hash(s) => Expression {
                kind: ExpressionKind::Literal(crc32(s)),
                ty: Type::F64,
                span: expr.span,
            },

            AstExpressionKind::Variable(name) => match self.lookup(name) {
                Some(ScopeEntry::Constant(val, ty)) => {
                    let (val, ty) = (*val, *ty);
                    Expression {
                        kind: ExpressionKind::Literal(val),
                        ty,
                        span: expr.span,
                    }
                }
                Some(ScopeEntry::Symbol(id)) => {
                    let id = *id;
                    let ty = self.symbols.get(id).ty;
                    Expression {
                        kind: ExpressionKind::Variable(id),
                        ty,
                        span: expr.span,
                    }
                }
                Some(ScopeEntry::Device(_)) => {
                    self.error(
                        expr.span,
                        format!("`{name}` is a device; use `{name}.Field` to read"),
                    );
                    Expression {
                        kind: ExpressionKind::Literal(0.0),
                        ty: Type::F64,
                        span: expr.span,
                    }
                }
                None => {
                    self.error(expr.span, format!("undeclared name `{name}`"));
                    Expression {
                        kind: ExpressionKind::Literal(0.0),
                        ty: Type::F64,
                        span: expr.span,
                    }
                }
            },

            AstExpressionKind::Binary(op, lhs, rhs) => {
                let lhs_resolved = self.resolve_expression(lhs);
                let rhs_resolved = self.resolve_expression(rhs);
                let left_type = lhs_resolved.ty;
                let right_type = rhs_resolved.ty;
                let op = *op;
                let result_type =
                    infer_binary_type(op, left_type, right_type, expr.span, &mut self.diagnostics);
                Expression {
                    kind: ExpressionKind::Binary(
                        op,
                        Box::new(lhs_resolved),
                        Box::new(rhs_resolved),
                    ),
                    ty: result_type,
                    span: expr.span,
                }
            }

            AstExpressionKind::Unary(op, inner) => {
                let inner_resolved = self.resolve_expression(inner);
                let inner_type = inner_resolved.ty;
                let op = *op;
                let result_type =
                    infer_unary_type(op, inner_type, expr.span, &mut self.diagnostics);
                Expression {
                    kind: ExpressionKind::Unary(op, Box::new(inner_resolved)),
                    ty: result_type,
                    span: expr.span,
                }
            }

            AstExpressionKind::Cast(inner, target_type) => {
                let inner_resolved = self.resolve_expression(inner);
                let src_type = inner_resolved.ty;
                let target_type = *target_type;
                validate_cast(src_type, target_type, expr.span, &mut self.diagnostics);
                Expression {
                    kind: ExpressionKind::Cast(Box::new(inner_resolved), target_type),
                    ty: target_type,
                    span: expr.span,
                }
            }

            AstExpressionKind::Call(call) => {
                let resolved_args: Vec<Expression> = call
                    .args
                    .iter()
                    .map(|a| self.resolve_expression(a))
                    .collect();
                match self.function_signatures.get(&call.name) {
                    Some(sig) => {
                        let symbol_id = sig.symbol_id;
                        let return_type = sig.return_type;
                        let param_types: Vec<Type> = sig.parameter_types.clone();

                        if resolved_args.len() != param_types.len() {
                            self.error(
                                call.span,
                                format!(
                                    "function `{}` expects {} argument(s), found {}",
                                    call.name,
                                    param_types.len(),
                                    resolved_args.len()
                                ),
                            );
                        } else {
                            for (i, (arg, &param_type)) in
                                resolved_args.iter().zip(param_types.iter()).enumerate()
                            {
                                if arg.ty != param_type {
                                    self.error(
                                        arg.span,
                                        format!(
                                            "argument {} to `{}` has type `{:?}`, expected `{:?}`",
                                            i + 1,
                                            call.name,
                                            arg.ty,
                                            param_type
                                        ),
                                    );
                                }
                            }
                        }
                        let result_type = return_type;
                        Expression {
                            kind: ExpressionKind::Call(symbol_id, resolved_args),
                            ty: result_type,
                            span: expr.span,
                        }
                    }
                    None => {
                        self.error(call.span, format!("unknown function `{}`", call.name));
                        Expression {
                            kind: ExpressionKind::Literal(0.0),
                            ty: Type::F64,
                            span: expr.span,
                        }
                    }
                }
            }

            AstExpressionKind::BuiltinCall(builtin, args) => {
                let resolved_args: Vec<Expression> =
                    args.iter().map(|a| self.resolve_expression(a)).collect();
                let expected = builtin_param_count(*builtin);
                if resolved_args.len() != expected {
                    self.error(
                        expr.span,
                        format!(
                            "built-in `{builtin:?}` expects {expected} argument(s), found {}",
                            resolved_args.len()
                        ),
                    );
                } else {
                    for arg in &resolved_args {
                        if arg.ty != Type::F64 {
                            self.error(
                                arg.span,
                                format!(
                                    "built-in math functions require `f64` arguments, found `{:?}`",
                                    arg.ty
                                ),
                            );
                        }
                    }
                }
                Expression {
                    kind: ExpressionKind::BuiltinCall(*builtin, resolved_args),
                    ty: Type::F64,
                    span: expr.span,
                }
            }

            AstExpressionKind::DeviceRead { device, field } => match self.lookup(device) {
                Some(ScopeEntry::Device(pin)) => {
                    let pin = *pin;
                    Expression {
                        kind: ExpressionKind::DeviceRead {
                            pin,
                            field: field.clone(),
                        },
                        ty: crate::ast::Type::F64,
                        span: expr.span,
                    }
                }
                _ => {
                    self.error(expr.span, format!("`{device}` is not a device"));
                    Expression {
                        kind: ExpressionKind::Literal(0.0),
                        ty: Type::F64,
                        span: expr.span,
                    }
                }
            },

            AstExpressionKind::SlotRead {
                device,
                slot,
                field,
            } => match self.lookup(device) {
                Some(ScopeEntry::Device(pin)) => {
                    let pin = *pin;
                    let slot_resolved = self.resolve_expression(slot);
                    if slot_resolved.ty != Type::I53 {
                        self.error(
                            slot.span,
                            format!("slot index must be `i53`, found `{:?}`", slot_resolved.ty),
                        );
                    }
                    Expression {
                        kind: ExpressionKind::SlotRead {
                            pin,
                            slot: Box::new(slot_resolved),
                            field: field.clone(),
                        },
                        ty: Type::F64,
                        span: expr.span,
                    }
                }
                _ => {
                    self.error(expr.span, format!("`{device}` is not a device"));
                    Expression {
                        kind: ExpressionKind::Literal(0.0),
                        ty: Type::F64,
                        span: expr.span,
                    }
                }
            },

            AstExpressionKind::BatchRead {
                hash_expr,
                field,
                mode,
            } => {
                let hash_resolved = self.resolve_expression(hash_expr);
                if hash_resolved.ty != Type::F64 {
                    self.error(
                        hash_expr.span,
                        format!(
                            "batch_read hash must be `f64`, found `{:?}`",
                            hash_resolved.ty
                        ),
                    );
                }
                Expression {
                    kind: ExpressionKind::BatchRead {
                        hash_expr: Box::new(hash_resolved),
                        field: field.clone(),
                        mode: *mode,
                    },
                    ty: Type::F64,
                    span: expr.span,
                }
            }

            AstExpressionKind::Select {
                cond,
                if_true,
                if_false,
            } => {
                let cond_resolved = self.resolve_expression(cond);
                let true_resolved = self.resolve_expression(if_true);
                let false_resolved = self.resolve_expression(if_false);
                if cond_resolved.ty != Type::Bool {
                    self.error(
                        cond.span,
                        format!(
                            "`select` condition must be `bool`, found `{:?}`",
                            cond_resolved.ty
                        ),
                    );
                }
                if true_resolved.ty != false_resolved.ty {
                    self.error(
                        expr.span,
                        format!(
                            "`select` branches have different types: `{:?}` vs `{:?}`",
                            true_resolved.ty, false_resolved.ty
                        ),
                    );
                }
                let result_type = true_resolved.ty;
                Expression {
                    kind: ExpressionKind::Select {
                        condition: Box::new(cond_resolved),
                        if_true: Box::new(true_resolved),
                        if_false: Box::new(false_resolved),
                    },
                    ty: result_type,
                    span: expr.span,
                }
            }
        }
    }

    fn validate_main(&mut self, program: &AstProgram) {
        let main_fns: Vec<_> = program
            .items
            .iter()
            .filter_map(|item| {
                if let Item::Fn(f) = item {
                    if f.name == "main" { Some(f) } else { None }
                } else {
                    None
                }
            })
            .collect();

        if main_fns.is_empty() {
            self.error(program.span, "program must have a `main` function");
            return;
        }
        for f in &main_fns[1..] {
            self.error(f.span, "duplicate definition of `main`");
        }
        let main = main_fns[0];
        if !main.params.is_empty() {
            self.error(main.span, "`main` must take no parameters");
        }
        if main.return_type.is_some() {
            self.error(main.span, "`main` must have no return type");
        }
    }
}

fn infer_binary_type(
    op: BinaryOperator,
    left_type: Type,
    right_type: Type,
    span: Span,
    diagnostics: &mut Vec<Diagnostic>,
) -> Type {
    use BinaryOperator::*;
    match op {
        Or | And => {
            if left_type != Type::Bool {
                diagnostics.push(Diagnostic::error(
                    span,
                    format!(
                        "logical operator requires `bool` operands, found `{left_type:?}` on left"
                    ),
                ));
            }
            if right_type != Type::Bool {
                diagnostics.push(Diagnostic::error(
                    span,
                    format!(
                        "logical operator requires `bool` operands, found `{right_type:?}` on right"
                    ),
                ));
            }
            Type::Bool
        }
        Eq | Ne | Lt | Gt | Le | Ge => {
            if left_type != right_type {
                diagnostics.push(Diagnostic::error(
                    span,
                    format!(
                        "comparison requires operands of the same type, found `{left_type:?}` and `{right_type:?}`"
                    ),
                ));
            }
            Type::Bool
        }
        BitOr | BitXor | BitAnd | Shl | Shr => {
            if left_type != Type::I53 {
                diagnostics.push(Diagnostic::error(
                    span,
                    format!(
                        "bitwise/shift operator requires `i53` operands, found `{left_type:?}` on left"
                    ),
                ));
            }
            if right_type != Type::I53 {
                diagnostics.push(Diagnostic::error(
                    span,
                    format!(
                        "bitwise/shift operator requires `i53` operands, found `{right_type:?}` on right"
                    ),
                ));
            }
            Type::I53
        }
        Div => {
            if left_type != right_type {
                diagnostics.push(Diagnostic::error(
                    span,
                    format!("type mismatch in `/` operator: `{left_type:?}` and `{right_type:?}`"),
                ));
            }
            if left_type == Type::Bool {
                diagnostics.push(Diagnostic::error(
                    span,
                    "arithmetic operators cannot be applied to `bool`",
                ));
            }
            Type::F64
        }
        Add | Sub | Mul | Rem => {
            if left_type != right_type {
                diagnostics.push(Diagnostic::error(
                    span,
                    format!(
                        "type mismatch in arithmetic operator: `{left_type:?}` and `{right_type:?}`"
                    ),
                ));
            }
            if left_type == Type::Bool {
                diagnostics.push(Diagnostic::error(
                    span,
                    "arithmetic operators cannot be applied to `bool`",
                ));
                return Type::I53;
            }
            left_type
        }
    }
}

fn infer_unary_type(
    op: UnaryOperator,
    operand_type: Type,
    span: Span,
    diagnostics: &mut Vec<Diagnostic>,
) -> Type {
    match op {
        UnaryOperator::Neg => {
            if operand_type == Type::Bool {
                diagnostics.push(Diagnostic::error(
                    span,
                    "unary `-` cannot be applied to `bool`",
                ));
            }
            operand_type
        }
        UnaryOperator::Not => {
            if operand_type != Type::Bool {
                diagnostics.push(Diagnostic::error(
                    span,
                    format!("logical `!` requires `bool` operand, found `{operand_type:?}`"),
                ));
            }
            Type::Bool
        }
        UnaryOperator::BitNot => {
            if operand_type != Type::I53 {
                diagnostics.push(Diagnostic::error(
                    span,
                    format!("bitwise `~` requires `i53` operand, found `{operand_type:?}`"),
                ));
            }
            Type::I53
        }
    }
}

fn validate_cast(src_type: Type, target_type: Type, span: Span, diagnostics: &mut Vec<Diagnostic>) {
    match (src_type, target_type) {
        (a, b) if a == b => {
            diagnostics.push(Diagnostic::warning(
                span,
                format!("identity cast `{a:?} as {b:?}` has no effect"),
            ));
        }
        (Type::I53, Type::F64)
        | (Type::F64, Type::I53)
        | (Type::Bool, Type::I53)
        | (Type::Bool, Type::F64) => {}
        (_, Type::Bool) => {
            diagnostics.push(Diagnostic::error(
                span,
                "cannot cast to `bool`; use an explicit comparison instead",
            ));
        }
        _ => {
            diagnostics.push(Diagnostic::error(
                span,
                format!("invalid cast from `{src_type:?}` to `{target_type:?}`"),
            ));
        }
    }
}

fn eval_binary_const(
    op: BinaryOperator,
    lv: f64,
    rv: f64,
    ty: Type,
    span: Span,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<(f64, Type)> {
    use BinaryOperator::*;
    Some(match op {
        Add => (lv + rv, ty),
        Sub => (lv - rv, ty),
        Mul => (lv * rv, ty),
        Div => (lv / rv, Type::F64),
        Rem => {
            let r = lv - (lv / rv).floor() * rv;
            (r, ty)
        }
        BitOr => ((lv as i64 | rv as i64) as f64, Type::I53),
        BitXor => ((lv as i64 ^ rv as i64) as f64, Type::I53),
        BitAnd => ((lv as i64 & rv as i64) as f64, Type::I53),
        Shl => ((lv as i64).wrapping_shl(rv as u32) as f64, Type::I53),
        Shr => ((lv as i64).wrapping_shr(rv as u32) as f64, Type::I53),
        Eq => (if lv == rv { 1.0 } else { 0.0 }, Type::Bool),
        Ne => (if lv != rv { 1.0 } else { 0.0 }, Type::Bool),
        Lt => (if lv < rv { 1.0 } else { 0.0 }, Type::Bool),
        Gt => (if lv > rv { 1.0 } else { 0.0 }, Type::Bool),
        Le => (if lv <= rv { 1.0 } else { 0.0 }, Type::Bool),
        Ge => (if lv >= rv { 1.0 } else { 0.0 }, Type::Bool),
        Or => {
            if ty != Type::Bool {
                diagnostics.push(Diagnostic::error(
                    span,
                    "logical `||` requires bool operands",
                ));
                return None;
            }
            (if lv != 0.0 || rv != 0.0 { 1.0 } else { 0.0 }, Type::Bool)
        }
        And => {
            if ty != Type::Bool {
                diagnostics.push(Diagnostic::error(
                    span,
                    "logical `&&` requires bool operands",
                ));
                return None;
            }
            (if lv != 0.0 && rv != 0.0 { 1.0 } else { 0.0 }, Type::Bool)
        }
    })
}

fn builtin_param_count(builtin: BuiltinFunction) -> usize {
    use BuiltinFunction::*;
    match builtin {
        Abs | Ceil | Floor | Round | Trunc | Sqrt | Exp | Log | Sin | Cos | Tan | Asin | Acos
        | Atan | Rand => 1,
        Atan2 | Pow | Min | Max => 2,
        Lerp | Clamp => 3,
    }
}

/// Resolve an `ast::Program` to a `resolved::Program`.
///
/// All errors are accumulated before returning. Returns `Err` if any errors
/// were produced; warnings do not prevent a successful result.
pub fn resolve(program: &AstProgram) -> Result<(Program, Vec<Diagnostic>), Vec<Diagnostic>> {
    let mut resolver = Resolver::new();

    resolver.push_scope();
    resolver.pre_pass(program);
    resolver.validate_main(program);

    let functions: Vec<FunctionDeclaration> = program
        .items
        .iter()
        .filter_map(|item| {
            if let Item::Fn(f) = item {
                Some(resolver.resolve_function(f))
            } else {
                None
            }
        })
        .collect();

    resolver.pop_scope();

    if resolver
        .diagnostics
        .iter()
        .any(|d| d.severity == Severity::Error)
    {
        Err(resolver.diagnostics)
    } else {
        Ok((
            Program {
                functions,
                symbols: resolver.symbols,
            },
            resolver.diagnostics,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::resolve;
    use crate::ast::Type;
    use crate::crc32::crc32;
    use crate::diagnostic::Diagnostic;
    use crate::parser::parse;
    use crate::resolved::Program;
    use crate::resolved::{ExpressionKind, Statement, SymbolKind};

    fn resolve_ok(source: &str) -> Program {
        let (ast, parse_diagnostics) = parse(source);
        let errors: Vec<_> = parse_diagnostics
            .iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "parse errors: {:#?}", errors);
        let (program, _) =
            resolve(&ast).unwrap_or_else(|diags| panic!("resolve errors: {:#?}", diags));
        program
    }

    fn resolve_errors(source: &str) -> Vec<Diagnostic> {
        let (ast, _) = parse(source);
        resolve(&ast).unwrap_err()
    }

    fn has_error(source: &str, fragment: &str) -> bool {
        let errors = resolve_errors(source);
        errors.iter().any(|d| d.message.contains(fragment))
    }

    // 4.1 / 4.2 — basic let binding, symbol allocation, scope
    #[test]
    fn let_binding_infers_type() {
        let program = resolve_ok("fn main() { let x = 42; }");
        let func = &program.functions[0];
        assert_eq!(func.name, "main");
        let Statement::Let(s) = &func.body.statements[0] else {
            panic!("expected let");
        };
        let info = program.symbols.get(s.symbol_id);
        assert_eq!(info.ty, Type::I53);
        assert!(!info.mutable);
        assert_eq!(info.kind, SymbolKind::Local);
    }

    #[test]
    fn let_mut_binding() {
        let program = resolve_ok("fn main() { let mut count = 0; }");
        let Statement::Let(s) = &program.functions[0].body.statements[0] else {
            panic!("expected let");
        };
        assert!(program.symbols.get(s.symbol_id).mutable);
    }

    #[test]
    fn let_type_annotation_matches() {
        let program = resolve_ok("fn main() { let x: f64 = 1.0; }");
        let Statement::Let(s) = &program.functions[0].body.statements[0] else {
            panic!("expected let");
        };
        assert_eq!(program.symbols.get(s.symbol_id).ty, Type::F64);
    }

    #[test]
    fn let_type_annotation_mismatch_is_error() {
        assert!(has_error("fn main() { let x: f64 = 42; }", "type mismatch"));
    }

    // 4.3 / 4.4 — const folding
    #[test]
    fn const_folds_to_literal() {
        let program = resolve_ok("const LIMIT: i53 = 10; fn main() { let x = LIMIT; }");
        let Statement::Let(s) = &program.functions[0].body.statements[0] else {
            panic!("expected let");
        };
        let ExpressionKind::Literal(v) = s.init.kind else {
            panic!(
                "expected literal after const folding, got {:?}",
                s.init.kind
            );
        };
        assert_eq!(v, 10.0);
        assert_eq!(s.init.ty, Type::I53);
    }

    #[test]
    fn const_not_emitted_as_function() {
        let program = resolve_ok("const X: i53 = 5; fn main() {}");
        // No symbols with SymbolKind::Function named "X"
        assert!(program.symbols.symbols.iter().all(|s| s.name != "X"));
    }

    // 4.5 — hash folding
    #[test]
    fn hash_folds_to_literal() {
        let program = resolve_ok("fn main() { let h = hash(\"StructureGasSensor\"); }");
        let Statement::Let(s) = &program.functions[0].body.statements[0] else {
            panic!("expected let");
        };
        let ExpressionKind::Literal(v) = s.init.kind else {
            panic!("expected literal");
        };
        assert_eq!(v, crc32("StructureGasSensor"));
        assert_eq!(s.init.ty, Type::F64);
    }

    // 4.6 — device declarations
    #[test]
    fn device_not_emitted_in_resolved_ir() {
        let program = resolve_ok("device sensor: d0; fn main() {}");
        assert!(program.symbols.symbols.iter().all(|s| s.name != "sensor"));
    }

    #[test]
    fn device_read_resolves_to_pin() {
        use crate::ast::DevicePin;
        let program = resolve_ok("device sensor: d0; fn main() { let t = sensor.Temperature; }");
        let Statement::Let(s) = &program.functions[0].body.statements[0] else {
            panic!("expected let");
        };
        let ExpressionKind::DeviceRead { pin, .. } = &s.init.kind else {
            panic!("expected DeviceRead, got {:?}", s.init.kind);
        };
        assert_eq!(*pin, DevicePin::D0);
        assert_eq!(s.init.ty, Type::F64);
    }

    // 4.7 — undeclared name
    #[test]
    fn undeclared_name_is_error() {
        assert!(has_error("fn main() { let x = y; }", "undeclared name `y`"));
    }

    // 4.8 — function call resolution
    #[test]
    fn function_call_resolves() {
        let program = resolve_ok(
            "fn add(a: i53, b: i53) -> i53 { return a + b; } fn main() { let r = add(1, 2); }",
        );
        let Statement::Let(s) = &program.functions[1].body.statements[0] else {
            panic!("expected let");
        };
        assert!(matches!(s.init.kind, ExpressionKind::Call(_, _)));
        assert_eq!(s.init.ty, Type::I53);
    }

    #[test]
    fn unknown_function_is_error() {
        assert!(has_error(
            "fn main() { let r = foo(); }",
            "unknown function `foo`"
        ));
    }

    #[test]
    fn wrong_argument_count_is_error() {
        assert!(has_error(
            "fn f(x: i53) -> i53 { return x; } fn main() { let r = f(1, 2); }",
            "expects 1 argument(s), found 2"
        ));
    }

    #[test]
    fn too_many_parameters_is_error() {
        assert!(has_error(
            "fn many(a: i53, b: i53, c: i53, d: i53, e: i53, f: i53, g: i53, h: i53, i: i53) -> i53 { return a; } fn main() {}",
            "has 9 parameters, but the maximum is 8"
        ));
    }

    #[test]
    fn eight_parameters_is_ok() {
        resolve_ok(
            "fn eight(a: i53, b: i53, c: i53, d: i53, e: i53, f: i53, g: i53, h: i53) -> i53 { return a; } fn main() {}",
        );
    }

    // 4.9 — device assignment target
    #[test]
    fn device_field_write_resolves() {
        use crate::resolved::AssignmentTarget;
        let program = resolve_ok("device heater: d1; fn main() { heater.On = 1.0; }");
        let Statement::Assign(s) = &program.functions[0].body.statements[0] else {
            panic!("expected assign");
        };
        assert!(matches!(s.target, AssignmentTarget::DeviceField { .. }));
    }

    // 4.10 / 4.11 — type inference and checking
    #[test]
    fn division_of_i53_yields_f64() {
        let program = resolve_ok("fn main() { let q = 7 / 2; }");
        let Statement::Let(s) = &program.functions[0].body.statements[0] else {
            panic!("expected let");
        };
        assert_eq!(s.init.ty, Type::F64);
    }

    #[test]
    fn bool_arithmetic_is_error() {
        assert!(has_error(
            "fn main() { let b: bool = true; let x = b + b; }",
            "cannot be applied to `bool`"
        ));
    }

    #[test]
    fn mixed_i53_f64_arithmetic_is_error() {
        assert!(has_error(
            "fn main() { let a = 1; let b = 1.0; let c = a + b; }",
            "type mismatch"
        ));
    }

    #[test]
    fn comparison_produces_bool() {
        let program = resolve_ok("fn main() { let b = 1 < 2; }");
        let Statement::Let(s) = &program.functions[0].body.statements[0] else {
            panic!("expected let");
        };
        assert_eq!(s.init.ty, Type::Bool);
    }

    #[test]
    fn cast_i53_to_f64() {
        let program = resolve_ok("fn main() { let x = 5; let f = x as f64; }");
        let Statement::Let(s) = &program.functions[0].body.statements[1] else {
            panic!("expected let");
        };
        assert_eq!(s.init.ty, Type::F64);
    }

    #[test]
    fn cast_to_bool_is_error() {
        assert!(has_error(
            "fn main() { let x = 1; let b = x as bool; }",
            "cannot cast to `bool`"
        ));
    }

    // 4.12 — mutability checking
    #[test]
    fn assign_to_immutable_is_error() {
        assert!(has_error(
            "fn main() { let x = 0; x = 1; }",
            "cannot assign to immutable variable `x`"
        ));
    }

    #[test]
    fn assign_to_mutable_ok() {
        resolve_ok("fn main() { let mut x = 0; x = 1; }");
    }

    // 4.13 — main validation
    #[test]
    fn missing_main_is_error() {
        assert!(has_error("fn foo() {}", "must have a `main` function"));
    }

    #[test]
    fn main_with_params_is_error() {
        assert!(has_error(
            "fn main(x: i53) {}",
            "`main` must take no parameters"
        ));
    }

    #[test]
    fn main_with_return_type_is_error() {
        assert!(has_error(
            "fn main() -> i53 { return 0; }",
            "`main` must have no return type"
        ));
    }

    // Return type checking
    #[test]
    fn return_type_mismatch_is_error() {
        assert!(has_error(
            "fn f() -> i53 { return 1.0; } fn main() {}",
            "return type mismatch"
        ));
    }

    #[test]
    fn return_from_void_function_is_error() {
        assert!(has_error(
            "fn main() { return 42; }",
            "cannot return a value from a function with no return type"
        ));
    }

    // Forward reference to function
    #[test]
    fn forward_reference_to_function() {
        resolve_ok("fn main() { let r = helper(); } fn helper() -> i53 { return 1; }");
    }

    #[test]
    fn void_function_call_result_is_unit_type() {
        let program = resolve_ok("fn noop() {} fn main() { noop(); }");
        let Statement::Expression(s) = &program.functions[1].body.statements[0] else {
            panic!("expected expression statement");
        };
        assert_eq!(s.expression.ty, Type::Unit);
    }

    #[test]
    fn void_function_result_used_in_bool_context_is_error() {
        assert!(has_error(
            "fn noop() {} fn main() { if noop() {} }",
            "`if` condition must be `bool`"
        ));
    }

    // identity cast — allowed, emits warning
    #[test]
    fn identity_cast_warns_but_succeeds() {
        use crate::diagnostic::Severity;
        let (ast, _) = parse("fn main() { let x = 5; let y = x as i53; }");
        let (program, warnings) =
            resolve(&ast).unwrap_or_else(|diags| panic!("resolve errors: {:#?}", diags));
        assert!(
            warnings
                .iter()
                .any(|d| d.severity == Severity::Warning && d.message.contains("identity cast")),
            "expected identity cast warning, got: {:#?}",
            warnings
        );
        let Statement::Let(s) = &program.functions[0].body.statements[1] else {
            panic!("expected let");
        };
        assert_eq!(s.init.ty, Type::I53);
    }
}
