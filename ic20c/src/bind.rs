//! Name resolution and type checking — transforms the untyped AST into the
//! bound IR with resolved symbols, type annotations, and constant folding.

use std::collections::HashMap;

use crate::crc32::crc32;
use crate::diagnostic::{Diagnostic, Severity, Span};
use crate::ir::ast::{
    AssignmentTarget as AstAssignmentTarget, Block as AstBlock, ElseClause as AstElseClause,
    Expression as AstExpression, ExpressionKind as AstExpressionKind,
    FunctionDeclaration as AstFunctionDeclaration, IfStatement as AstIfStatement, Item,
    LiteralKind, Program as AstProgram, Statement as AstStatement,
};
use crate::ir::bound::{
    AssignStatement, AssignmentTarget, BatchWriteStatement, Block, BreakStatement,
    ContinueStatement, ElseClause, Expression, ExpressionKind, ExpressionStatement, ForStatement,
    FunctionDeclaration, IfStatement, LetStatement, Parameter, Program, ReturnStatement,
    SleepStatement, Statement, StaticId, StaticInitializer, StaticVariable, SymbolId, SymbolInfo,
    SymbolKind, SymbolTable, WhileStatement,
};
use crate::ir::{BinaryOperator, DevicePin, Intrinsic, Type, UnaryOperator};

/// An entry in the scope stack, representing one binding visible at a given point.
#[derive(Clone)]
enum ScopeEntry {
    /// A local variable, parameter, or function — resolved via `SymbolId`.
    Symbol(SymbolId),
    /// A `const` declaration, already folded to a value and type.
    Constant(f64, Type),
    /// A `device` declaration bound to a hardware pin.
    Device(DevicePin),
    /// A `static` variable, referencing its `SymbolId` in the symbol table.
    Static(SymbolId),
}

/// The compile-time signature of a function, recorded during the top-level pre-pass
/// so that function calls can be type-checked before the callee's body is bound.
struct FunctionSignature {
    /// Symbol table entry for this function.
    symbol_id: SymbolId,
    /// The function's return type (`Unit` for void functions).
    return_type: Type,
    /// Types of each parameter, in declaration order.
    parameter_types: Vec<Type>,
}

/// The binder: performs name resolution, type checking, and constant folding,
/// transforming an `ast::Program` into a `bound::Program`.
struct Binder {
    /// Global symbol table shared across all scopes.
    symbols: SymbolTable,
    /// Stack of lexical scopes (innermost last).
    scopes: Vec<HashMap<String, ScopeEntry>>,
    /// Pre-registered function signatures for forward-reference support.
    function_signatures: HashMap<String, FunctionSignature>,
    /// Bound static variable declarations.
    statics: Vec<StaticVariable>,
    /// Bound initializer expressions for static variables.
    static_initializers: Vec<StaticInitializer>,
    /// Accumulated diagnostics (errors and warnings).
    diagnostics: Vec<Diagnostic>,
}

impl Binder {
    /// Creates a new binder with empty state.
    fn new() -> Self {
        Self {
            symbols: SymbolTable::default(),
            scopes: Vec::new(),
            function_signatures: HashMap::new(),
            statics: Vec::new(),
            static_initializers: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    /// Push a new lexical scope onto the scope stack.
    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    /// Pop the innermost lexical scope.
    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Define a name in the current (innermost) scope.
    fn define(&mut self, name: String, entry: ScopeEntry) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, entry);
        }
    }

    /// Look up a name through the scope stack, from innermost to outermost.
    /// Returns `None` if the name is not defined in any enclosing scope.
    fn lookup(&self, name: &str) -> Option<&ScopeEntry> {
        for scope in self.scopes.iter().rev() {
            if let Some(entry) = scope.get(name) {
                return Some(entry);
            }
        }
        None
    }

    /// Allocate a new symbol in the symbol table and return its `SymbolId`.
    fn allocate_symbol(&mut self, info: SymbolInfo) -> SymbolId {
        self.symbols.push(info)
    }

    /// Emit an error diagnostic.
    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::error(span, message));
    }

    /// Emit a warning diagnostic.
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
                Item::Static(_) => {}
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

    /// Bind static variable declarations in source order (§4.4).
    ///
    /// Each static's initializer is bound with full expression resolution (including
    /// device reads and function calls). A static may reference constants, devices,
    /// functions, and any static declared earlier in the source file.
    fn bind_statics(&mut self, program: &AstProgram) {
        let mut next_address: u16 = 511;
        for item in &program.items {
            if let Item::Static(s) = item {
                let static_id = StaticId(self.statics.len());
                let address = next_address;
                next_address = next_address.saturating_sub(1);

                let init = self.bind_expression(&s.initializer);
                let actual_type = init.ty;

                let is_device_read = matches!(
                    init.kind,
                    ExpressionKind::DeviceRead { .. }
                        | ExpressionKind::SlotRead { .. }
                        | ExpressionKind::BatchRead { .. }
                );

                let (init, _) = if actual_type == s.ty {
                    (init, s.ty)
                } else if is_device_read
                    && matches!(
                        (actual_type, s.ty),
                        (Type::F64, Type::Bool) | (Type::F64, Type::I53)
                    )
                {
                    let span = s.initializer.span;
                    let coerced = Expression {
                        kind: ExpressionKind::Cast(Box::new(init), s.ty),
                        ty: s.ty,
                        span,
                    };
                    (coerced, s.ty)
                } else {
                    self.error(
                        s.initializer.span,
                        format!(
                            "type mismatch: static `{}` declared as `{:?}` but initializer has type `{:?}`",
                            s.name, s.ty, actual_type
                        ),
                    );
                    (init, s.ty)
                };

                let symbol_id = self.allocate_symbol(SymbolInfo {
                    name: s.name.clone(),
                    ty: s.ty,
                    mutable: s.mutable,
                    kind: SymbolKind::Static(static_id),
                });
                self.define(s.name.clone(), ScopeEntry::Static(symbol_id));

                self.statics.push(StaticVariable {
                    name: s.name.clone(),
                    mutable: s.mutable,
                    ty: s.ty,
                    address,
                });
                self.static_initializers.push(StaticInitializer {
                    static_id,
                    expression: init,
                    span: s.span,
                });
            }
        }
    }

    /// Bind a function declaration: resolve parameters, bind the body, and
    /// return the fully resolved `FunctionDeclaration`.
    fn bind_function(&mut self, ast_fn: &AstFunctionDeclaration) -> FunctionDeclaration {
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

        let body = self.bind_block(&ast_fn.body, return_type);
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

    /// Bind a block: push a new scope, bind each statement, pop the scope.
    fn bind_block(&mut self, block: &AstBlock, return_type: Type) -> Block {
        self.push_scope();
        let mut statements = Vec::new();
        for stmt in &block.stmts {
            let bound = self.bind_statement(stmt, return_type);
            statements.push(bound);
        }
        self.pop_scope();
        Block {
            statements,
            span: block.span,
        }
    }

    /// Bind a single statement, performing type checking on all sub-expressions.
    ///
    /// `return_type` is the enclosing function's return type, used to validate
    /// `return` statements.
    fn bind_statement(&mut self, stmt: &AstStatement, return_type: Type) -> Statement {
        match stmt {
            AstStatement::Let(s) => {
                let init = self.bind_expression(&s.init);
                let actual_type = init.ty;

                let is_device_read = matches!(
                    init.kind,
                    ExpressionKind::DeviceRead { .. }
                        | ExpressionKind::SlotRead { .. }
                        | ExpressionKind::BatchRead { .. }
                );

                let (init, final_type) = match s.ty {
                    Some(annotation_type) => {
                        if annotation_type == actual_type {
                            (init, annotation_type)
                        } else if is_device_read
                            && matches!(
                                (actual_type, annotation_type),
                                (Type::F64, Type::Bool) | (Type::F64, Type::I53)
                            )
                        {
                            let span = s.init.span;
                            let coerced = Expression {
                                kind: ExpressionKind::Cast(Box::new(init), annotation_type),
                                ty: annotation_type,
                                span,
                            };
                            (coerced, annotation_type)
                        } else {
                            self.error(
                                s.init.span,
                                format!(
                                    "type mismatch: declared `{annotation_type:?}` but initializer has type `{actual_type:?}`"
                                ),
                            );
                            (init, annotation_type)
                        }
                    }
                    None => (init, actual_type),
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
                let value = self.bind_expression(&s.rhs);
                let target = self.bind_assignment_target(&s.lhs, value.ty, s.span);
                Statement::Assign(AssignStatement {
                    target,
                    value,
                    span: s.span,
                })
            }

            AstStatement::Expression(s) => {
                let expression = self.bind_expression(&s.expr);
                Statement::Expression(ExpressionStatement {
                    expression,
                    span: s.span,
                })
            }

            AstStatement::If(s) => Statement::If(self.bind_if_statement(s, return_type)),

            AstStatement::While(s) => {
                let condition = self.bind_expression(&s.cond);
                if condition.ty != Type::Bool {
                    self.error(
                        s.cond.span,
                        format!(
                            "`while` condition must be `bool`, found `{:?}`",
                            condition.ty
                        ),
                    );
                }
                let body = self.bind_block(&s.body, return_type);
                Statement::While(WhileStatement {
                    label: s.label.clone(),
                    condition,
                    body,
                    span: s.span,
                })
            }

            AstStatement::For(s) => {
                let lower = self.bind_expression(&s.lower);
                let upper = self.bind_expression(&s.upper);
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
                let step = s.step.as_ref().map(|step_expr| {
                    let bound_step = self.bind_expression(step_expr);
                    if bound_step.ty != Type::I53 {
                        self.error(
                            step_expr.span,
                            format!(
                                "`for` range step must be `i53`, found `{:?}`",
                                bound_step.ty
                            ),
                        );
                    }
                    bound_step
                });
                self.push_scope();
                let variable = self.allocate_symbol(SymbolInfo {
                    name: s.var.clone(),
                    ty: Type::I53,
                    mutable: true,
                    kind: SymbolKind::Local,
                });
                self.define(s.var.clone(), ScopeEntry::Symbol(variable));
                let body = self.bind_block(&s.body, return_type);
                self.pop_scope();
                Statement::For(ForStatement {
                    label: s.label.clone(),
                    variable,
                    lower,
                    upper,
                    inclusive: s.inclusive,
                    reverse: s.reverse,
                    step,
                    body,
                    span: s.span,
                })
            }

            AstStatement::Break(s) => Statement::Break(BreakStatement {
                label: s.label.clone(),
                span: s.span,
            }),
            AstStatement::Continue(s) => Statement::Continue(ContinueStatement {
                label: s.label.clone(),
                span: s.span,
            }),

            AstStatement::Return(s) => {
                let value = s.value.as_ref().map(|v| self.bind_expression(v));
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
                let duration = self.bind_expression(&s.duration);
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

            AstStatement::BatchWrite(s) => {
                let hash_bound = self.bind_expression(&s.hash_expr);
                if hash_bound.ty != Type::F64 {
                    self.error(
                        s.hash_expr.span,
                        format!(
                            "batch_write hash must be `f64`, found `{:?}`",
                            hash_bound.ty
                        ),
                    );
                }
                let value_bound = self.bind_expression(&s.value);
                Statement::BatchWrite(BatchWriteStatement {
                    hash_expr: hash_bound,
                    field: s.field.clone(),
                    value: value_bound,
                    span: s.span,
                })
            }
        }
    }

    /// Bind an assignment target, checking mutability and type compatibility.
    ///
    /// `value_type` is the type of the right-hand side of the assignment.
    fn bind_assignment_target(
        &mut self,
        target: &AstAssignmentTarget,
        value_type: Type,
        assignment_span: Span,
    ) -> AssignmentTarget {
        match target {
            AstAssignmentTarget::Var { name, span } => match self.lookup(name) {
                Some(ScopeEntry::Symbol(id) | ScopeEntry::Static(id)) => {
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
                    let slot_bound = self.bind_expression(slot);
                    if slot_bound.ty != Type::I53 {
                        self.error(
                            slot.span,
                            format!("slot index must be `i53`, found `{:?}`", slot_bound.ty),
                        );
                    }
                    AssignmentTarget::SlotField {
                        pin,
                        slot: slot_bound,
                        field: field.clone(),
                        span: *span,
                    }
                }
                _ => {
                    self.error(*span, format!("`{device}` is not a device"));
                    let slot_bound = self.bind_expression(slot);
                    AssignmentTarget::SlotField {
                        pin: DevicePin::D0,
                        slot: slot_bound,
                        field: field.clone(),
                        span: *span,
                    }
                }
            },
        }
    }

    /// Bind an `if` statement: type-check the condition (must be `bool`),
    /// bind both branches, and bind any `else` clause.
    fn bind_if_statement(&mut self, s: &AstIfStatement, return_type: Type) -> IfStatement {
        let condition = self.bind_expression(&s.cond);
        if condition.ty != Type::Bool {
            self.error(
                s.cond.span,
                format!("`if` condition must be `bool`, found `{:?}`", condition.ty),
            );
        }
        let then_block = self.bind_block(&s.then_block, return_type);
        let else_clause = s
            .else_clause
            .as_ref()
            .map(|e| self.bind_else_clause(e, return_type));
        IfStatement {
            condition,
            then_block,
            else_clause,
            span: s.span,
        }
    }

    /// Bind an `else` clause (either a block or a chained `else if`).
    fn bind_else_clause(&mut self, else_: &AstElseClause, return_type: Type) -> ElseClause {
        match else_ {
            AstElseClause::Block(block) => ElseClause::Block(self.bind_block(block, return_type)),
            AstElseClause::If(if_stmt) => {
                ElseClause::If(Box::new(self.bind_if_statement(if_stmt, return_type)))
            }
        }
    }

    /// Bind an expression: resolve names, type-check, and return the
    /// resolved `Expression` with its computed type.
    fn bind_expression(&mut self, expr: &AstExpression) -> Expression {
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
                Some(ScopeEntry::Symbol(id) | ScopeEntry::Static(id)) => {
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
                let lhs_bound = self.bind_expression(lhs);
                let rhs_bound = self.bind_expression(rhs);
                let left_type = lhs_bound.ty;
                let right_type = rhs_bound.ty;
                let op = *op;
                let result_type =
                    infer_binary_type(op, left_type, right_type, expr.span, &mut self.diagnostics);
                Expression {
                    kind: ExpressionKind::Binary(op, Box::new(lhs_bound), Box::new(rhs_bound)),
                    ty: result_type,
                    span: expr.span,
                }
            }

            AstExpressionKind::Unary(op, inner) => {
                let inner_bound = self.bind_expression(inner);
                let inner_type = inner_bound.ty;
                let op = *op;
                let result_type =
                    infer_unary_type(op, inner_type, expr.span, &mut self.diagnostics);
                Expression {
                    kind: ExpressionKind::Unary(op, Box::new(inner_bound)),
                    ty: result_type,
                    span: expr.span,
                }
            }

            AstExpressionKind::Cast(inner, target_type) => {
                let inner_bound = self.bind_expression(inner);
                let src_type = inner_bound.ty;
                let target_type = *target_type;
                validate_cast(src_type, target_type, expr.span, &mut self.diagnostics);
                Expression {
                    kind: ExpressionKind::Cast(Box::new(inner_bound), target_type),
                    ty: target_type,
                    span: expr.span,
                }
            }

            AstExpressionKind::Call(call) => {
                let bound_args: Vec<Expression> =
                    call.args.iter().map(|a| self.bind_expression(a)).collect();
                match self.function_signatures.get(&call.name) {
                    Some(sig) => {
                        let symbol_id = sig.symbol_id;
                        let return_type = sig.return_type;
                        let param_types: Vec<Type> = sig.parameter_types.clone();

                        if bound_args.len() != param_types.len() {
                            self.error(
                                call.span,
                                format!(
                                    "function `{}` expects {} argument(s), found {}",
                                    call.name,
                                    param_types.len(),
                                    bound_args.len()
                                ),
                            );
                        } else {
                            for (i, (arg, &param_type)) in
                                bound_args.iter().zip(param_types.iter()).enumerate()
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
                            kind: ExpressionKind::Call(symbol_id, bound_args),
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

            AstExpressionKind::IntrinsicCall(intrinsic, args) => {
                let bound_args: Vec<Expression> =
                    args.iter().map(|a| self.bind_expression(a)).collect();
                let expected = intrinsic_param_count(*intrinsic);
                if bound_args.len() != expected {
                    self.error(
                        expr.span,
                        format!(
                            "intrinsic `{intrinsic:?}` expects {expected} argument(s), found {}",
                            bound_args.len()
                        ),
                    );
                } else {
                    for arg in &bound_args {
                        if arg.ty != Type::F64 {
                            self.error(
                                arg.span,
                                format!(
                                    "intrinsic functions require `f64` arguments, found `{:?}`",
                                    arg.ty
                                ),
                            );
                        }
                    }
                }
                Expression {
                    kind: ExpressionKind::IntrinsicCall(*intrinsic, bound_args),
                    ty: intrinsic_return_type(*intrinsic),
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
                        ty: crate::ir::Type::F64,
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
                    let slot_bound = self.bind_expression(slot);
                    if slot_bound.ty != Type::I53 {
                        self.error(
                            slot.span,
                            format!("slot index must be `i53`, found `{:?}`", slot_bound.ty),
                        );
                    }
                    Expression {
                        kind: ExpressionKind::SlotRead {
                            pin,
                            slot: Box::new(slot_bound),
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
                let hash_bound = self.bind_expression(hash_expr);
                if hash_bound.ty != Type::F64 {
                    self.error(
                        hash_expr.span,
                        format!("batch_read hash must be `f64`, found `{:?}`", hash_bound.ty),
                    );
                }
                Expression {
                    kind: ExpressionKind::BatchRead {
                        hash_expr: Box::new(hash_bound),
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
                let cond_bound = self.bind_expression(cond);
                let true_bound = self.bind_expression(if_true);
                let false_bound = self.bind_expression(if_false);
                if cond_bound.ty != Type::Bool {
                    self.error(
                        cond.span,
                        format!(
                            "`select` condition must be `bool`, found `{:?}`",
                            cond_bound.ty
                        ),
                    );
                }
                if true_bound.ty != false_bound.ty {
                    self.error(
                        expr.span,
                        format!(
                            "`select` branches have different types: `{:?}` vs `{:?}`",
                            true_bound.ty, false_bound.ty
                        ),
                    );
                }
                let result_type = true_bound.ty;
                Expression {
                    kind: ExpressionKind::Select {
                        condition: Box::new(cond_bound),
                        if_true: Box::new(true_bound),
                        if_false: Box::new(false_bound),
                    },
                    ty: result_type,
                    span: expr.span,
                }
            }
        }
    }

    /// Validate that the program has exactly one `main` function with no
    /// parameters and no return type.
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

/// Infer the result type of a binary operation, emitting diagnostics for
/// type mismatches. Returns the result type.
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

/// Infer the result type of a unary operation, emitting diagnostics for
/// type mismatches.
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

/// Validate a type cast and emit diagnostics for invalid or identity casts.
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

/// Evaluate a binary operation on two constant `f64` values at compile time.
/// Returns `None` and emits a diagnostic if the operation is invalid for the given type.
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

/// Returns the result type of an intrinsic function (always `F64` except
/// `IsNan` which returns `Bool`).
fn intrinsic_return_type(intrinsic: Intrinsic) -> Type {
    match intrinsic {
        Intrinsic::IsNan => Type::Bool,
        _ => Type::F64,
    }
}

/// Returns the expected number of arguments for an intrinsic function.
fn intrinsic_param_count(intrinsic: Intrinsic) -> usize {
    use Intrinsic::*;
    match intrinsic {
        Abs | Ceil | Floor | Round | Trunc | Sqrt | Exp | Log | Sin | Cos | Tan | Asin | Acos
        | Atan | Rand | IsNan => 1,
        Atan2 | Pow | Min | Max => 2,
        Lerp | Clamp => 3,
    }
}

/// Bind an `ast::Program` to a `bound::Program`.
///
/// All errors are accumulated before returning. Returns `Err` if any errors
/// were produced; warnings do not prevent a successful result.
pub fn bind(program: &AstProgram) -> Result<(Program, Vec<Diagnostic>), Vec<Diagnostic>> {
    let mut binder = Binder::new();

    binder.push_scope();
    binder.pre_pass(program);
    binder.bind_statics(program);
    binder.validate_main(program);

    let functions: Vec<FunctionDeclaration> = program
        .items
        .iter()
        .filter_map(|item| {
            if let Item::Fn(f) = item {
                Some(binder.bind_function(f))
            } else {
                None
            }
        })
        .collect();

    binder.pop_scope();

    if binder
        .diagnostics
        .iter()
        .any(|d| d.severity == Severity::Error)
    {
        Err(binder.diagnostics)
    } else {
        Ok((
            Program {
                functions,
                statics: binder.statics,
                static_initializers: binder.static_initializers,
                symbols: binder.symbols,
            },
            binder.diagnostics,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::bind;
    use crate::crc32::crc32;
    use crate::diagnostic::Diagnostic;
    use crate::ir::Type;
    use crate::ir::bound::Program;
    use crate::ir::bound::{ExpressionKind, Statement, SymbolKind};
    use crate::parser::parse;

    fn bind_ok(source: &str) -> Program {
        let (ast, parse_diagnostics) = parse(source);
        let errors: Vec<_> = parse_diagnostics
            .iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "parse errors: {:#?}", errors);
        let (program, _) = bind(&ast).unwrap_or_else(|diags| panic!("bind errors: {:#?}", diags));
        program
    }

    fn bind_errors(source: &str) -> Vec<Diagnostic> {
        let (ast, _) = parse(source);
        bind(&ast).unwrap_err()
    }

    fn has_error(source: &str, fragment: &str) -> bool {
        let errors = bind_errors(source);
        errors.iter().any(|d| d.message.contains(fragment))
    }

    // 4.1 / 4.2 — basic let binding, symbol allocation, scope
    #[test]
    fn let_binding_infers_type() {
        let program = bind_ok("fn main() { let x = 42; }");
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
        let program = bind_ok("fn main() { let mut count = 0; }");
        let Statement::Let(s) = &program.functions[0].body.statements[0] else {
            panic!("expected let");
        };
        assert!(program.symbols.get(s.symbol_id).mutable);
    }

    #[test]
    fn let_type_annotation_matches() {
        let program = bind_ok("fn main() { let x: f64 = 1.0; }");
        let Statement::Let(s) = &program.functions[0].body.statements[0] else {
            panic!("expected let");
        };
        assert_eq!(program.symbols.get(s.symbol_id).ty, Type::F64);
    }

    #[test]
    fn let_type_annotation_mismatch_is_error() {
        let errors = bind_errors("fn main() { let x: f64 = 42; }");
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].message,
            "type mismatch: declared `F64` but initializer has type `I53`"
        );
    }

    #[test]
    fn f64_to_bool_without_device_read_is_error() {
        let errors = bind_errors("fn main() { let x: f64 = 1.0; let b: bool = x; }");
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].message,
            "type mismatch: declared `Bool` but initializer has type `F64`"
        );
    }

    #[test]
    fn f64_to_i53_without_device_read_is_error() {
        let errors = bind_errors("fn main() { let x: f64 = 1.0; let n: i53 = x; }");
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].message,
            "type mismatch: declared `I53` but initializer has type `F64`"
        );
    }

    // 4.3 / 4.4 — const folding
    #[test]
    fn const_folds_to_literal() {
        let program = bind_ok("const LIMIT: i53 = 10; fn main() { let x = LIMIT; }");
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
        let program = bind_ok("const X: i53 = 5; fn main() {}");
        // No symbols with SymbolKind::Function named "X"
        assert!(program.symbols.symbols.iter().all(|s| s.name != "X"));
    }

    // 4.5 — hash folding
    #[test]
    fn hash_folds_to_literal() {
        let program = bind_ok("fn main() { let h = hash(\"StructureGasSensor\"); }");
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
    fn device_not_emitted_in_bound_ir() {
        let program = bind_ok("device sensor: d0; fn main() {}");
        assert!(program.symbols.symbols.iter().all(|s| s.name != "sensor"));
    }

    #[test]
    fn device_read_resolves_to_pin() {
        use crate::ir::DevicePin;
        let program = bind_ok("device sensor: d0; fn main() { let t = sensor.Temperature; }");
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
        let program = bind_ok(
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
        bind_ok(
            "fn eight(a: i53, b: i53, c: i53, d: i53, e: i53, f: i53, g: i53, h: i53) -> i53 { return a; } fn main() {}",
        );
    }

    // 4.9 — device assignment target
    #[test]
    fn device_field_write_resolves() {
        use crate::ir::bound::AssignmentTarget;
        let program = bind_ok("device heater: d1; fn main() { heater.On = 1.0; }");
        let Statement::Assign(s) = &program.functions[0].body.statements[0] else {
            panic!("expected assign");
        };
        assert!(matches!(s.target, AssignmentTarget::DeviceField { .. }));
    }

    #[test]
    fn device_field_write_accepts_bool() {
        bind_ok("device light: d0; fn main() { light.On = true; }");
    }

    #[test]
    fn device_field_write_accepts_i53() {
        bind_ok("device light: d0; fn main() { light.On = 1; }");
    }

    #[test]
    fn device_read_with_bool_annotation() {
        let program = bind_ok("device sensor: d0; fn main() { let on: bool = sensor.On; }");
        let Statement::Let(s) = &program.functions[0].body.statements[0] else {
            panic!("expected let");
        };
        assert_eq!(s.init.ty, Type::Bool);
        assert!(
            matches!(s.init.kind, ExpressionKind::Cast(_, Type::Bool)),
            "expected implicit cast to bool, got {:?}",
            s.init.kind
        );
        assert_eq!(program.symbols.get(s.symbol_id).ty, Type::Bool);
    }

    #[test]
    fn device_read_with_i53_annotation() {
        let program = bind_ok("device sensor: d0; fn main() { let count: i53 = sensor.Count; }");
        let Statement::Let(s) = &program.functions[0].body.statements[0] else {
            panic!("expected let");
        };
        assert_eq!(s.init.ty, Type::I53);
        assert!(
            matches!(s.init.kind, ExpressionKind::Cast(_, Type::I53)),
            "expected implicit cast to i53, got {:?}",
            s.init.kind
        );
        assert_eq!(program.symbols.get(s.symbol_id).ty, Type::I53);
    }

    // 4.10 / 4.11 — type inference and checking
    #[test]
    fn division_of_i53_yields_f64() {
        let program = bind_ok("fn main() { let q = 7 / 2; }");
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
        let errors = bind_errors("fn main() { let a = 1; let b = 1.0; let c = a + b; }");
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].message,
            "type mismatch in arithmetic operator: `I53` and `F64`"
        );
    }

    #[test]
    fn comparison_produces_bool() {
        let program = bind_ok("fn main() { let b = 1 < 2; }");
        let Statement::Let(s) = &program.functions[0].body.statements[0] else {
            panic!("expected let");
        };
        assert_eq!(s.init.ty, Type::Bool);
    }

    #[test]
    fn cast_i53_to_f64() {
        let program = bind_ok("fn main() { let x = 5; let f = x as f64; }");
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
        bind_ok("fn main() { let mut x = 0; x = 1; }");
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
        bind_ok("fn main() { let r = helper(); } fn helper() -> i53 { return 1; }");
    }

    #[test]
    fn void_function_call_result_is_unit_type() {
        let program = bind_ok("fn noop() {} fn main() { noop(); }");
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
            bind(&ast).unwrap_or_else(|diags| panic!("bind errors: {:#?}", diags));
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

    #[test]
    fn duplicate_variable_in_same_scope_is_allowed() {
        let program = bind_ok("fn main() { let x = 5; let x = 10; }");
        let func = &program.functions[0];
        assert_eq!(func.body.statements.len(), 2);
    }

    #[test]
    fn variable_shadowing_in_nested_scope() {
        let program = bind_ok("fn main() { let x: i53 = 5; if true { let x: f64 = 3.0; } }");
        let func = &program.functions[0];
        let Statement::Let(outer) = &func.body.statements[0] else {
            panic!("expected let statement");
        };
        assert_eq!(program.symbols.get(outer.symbol_id).ty, Type::I53);
        let Statement::If(if_stmt) = &func.body.statements[1] else {
            panic!("expected if statement");
        };
        let Statement::Let(inner) = &if_stmt.then_block.statements[0] else {
            panic!("expected inner let statement");
        };
        assert_eq!(program.symbols.get(inner.symbol_id).ty, Type::F64);
        assert_ne!(
            outer.symbol_id, inner.symbol_id,
            "shadowed variable must get a distinct symbol ID"
        );
    }

    #[test]
    fn recursive_function_call_binds_correctly() {
        let program = bind_ok(
            "fn countdown(n: i53) -> i53 { if n <= 0 { return 0; } return countdown(n - 1); } fn main() { countdown(10); }",
        );
        let countdown_fn = &program.functions[0];
        let Statement::Return(ret) = &countdown_fn.body.statements[1] else {
            panic!("expected return as second statement of countdown");
        };
        let value = ret.value.as_ref().expect("return should have a value");
        assert!(
            matches!(value.kind, ExpressionKind::Call(_, _)),
            "recursive call should bind to a Call expression, got {:?}",
            value.kind
        );
    }

    #[test]
    fn multiple_return_paths_type_mismatch_is_error() {
        let errors =
            bind_errors("fn f() -> i53 { if true { return 1; } return 2.0; } fn main() {}");
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].message,
            "return type mismatch: expected `I53`, found `F64`"
        );
    }

    #[test]
    fn for_loop_variable_infers_i53_type() {
        let program = bind_ok("device out: d0; fn main() { for i in 0..10 { out.Setting = i; } }");
        let func = &program.functions[0];
        if let Statement::For(f) = &func.body.statements[0] {
            let info = program.symbols.get(f.variable);
            assert_eq!(info.ty, Type::I53);
        } else {
            panic!("expected for statement");
        }
    }

    #[test]
    fn batch_write_binding() {
        let program = bind_ok(r#"fn main() { batch_write(hash("StructureWallType"), On, 1.0); }"#);
        let func = &program.functions[0];
        let Statement::BatchWrite(s) = &func.body.statements[0] else {
            panic!("expected BatchWrite statement");
        };
        assert!(
            matches!(s.hash_expr.kind, ExpressionKind::Literal(_)),
            "hash() should fold to a literal"
        );
    }

    #[test]
    fn is_nan_intrinsic_requires_f64_argument() {
        let errors = bind_errors("fn main() { let x = is_nan(true); }");
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].message,
            "intrinsic functions require `f64` arguments, found `Bool`"
        );
    }

    #[test]
    fn is_nan_intrinsic_result_is_bool() {
        let program = bind_ok("fn main() { let x: f64 = 1.0; let y: bool = is_nan(x); }");
        let func = &program.functions[0];
        let Statement::Let(s) = &func.body.statements[1] else {
            panic!("expected let");
        };
        assert_eq!(s.init.ty, Type::Bool);
    }

    #[test]
    fn select_intrinsic_requires_bool_condition() {
        let errors = bind_errors("fn main() { let x = select(1, 2.0, 3.0); }");
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].message,
            "`select` condition must be `bool`, found `I53`"
        );
    }

    #[test]
    fn select_intrinsic_branches_must_match_type() {
        let errors = bind_errors("fn main() { let x = select(true, 1, 2.0); }");
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].message,
            "`select` branches have different types: `I53` vs `F64`"
        );
    }

    // const eval_const — bool literal
    #[test]
    fn const_bool_literal_evaluates() {
        let program = bind_ok("const ENABLED: bool = true; fn main() {}");
        assert!(program.symbols.symbols.iter().all(|s| s.name != "ENABLED"));
    }

    // const eval_const — hash expression
    #[test]
    fn const_hash_expression_evaluates() {
        let program = bind_ok(r#"const H: f64 = hash("StructureGasSensor"); fn main() {}"#);
        // Hash constants are folded away; they should not appear as symbols.
        assert!(program.symbols.symbols.iter().all(|s| s.name != "H"));
    }

    // const eval_const — reference to another const
    #[test]
    fn const_references_another_const() {
        let program = bind_ok("const A: i53 = 5; const B: i53 = A; fn main() { let x = B; }");
        let Statement::Let(s) = &program.functions[0].body.statements[0] else {
            panic!("expected let");
        };
        let ExpressionKind::Literal(v) = s.init.kind else {
            panic!("expected literal from const folding");
        };
        assert_eq!(v, 5.0);
    }

    // const eval_const — non-const variable in const context
    #[test]
    fn const_with_non_const_variable_is_error() {
        assert!(has_error(
            "const C: i53 = undeclared; fn main() {}",
            "cannot use `undeclared` in a constant expression"
        ));
    }

    // const eval_const — unary negation
    #[test]
    fn const_unary_neg_evaluates() {
        let program = bind_ok("const N: i53 = -5; fn main() { let x = N; }");
        let Statement::Let(s) = &program.functions[0].body.statements[0] else {
            panic!("expected let");
        };
        let ExpressionKind::Literal(v) = s.init.kind else {
            panic!("expected literal");
        };
        assert_eq!(v, -5.0);
    }

    // const eval_const — unary logical not on bool
    #[test]
    fn const_unary_not_bool_evaluates() {
        let program = bind_ok("const B: bool = !false; fn main() { let x = B; }");
        let Statement::Let(s) = &program.functions[0].body.statements[0] else {
            panic!("expected let");
        };
        let ExpressionKind::Literal(v) = s.init.kind else {
            panic!("expected literal");
        };
        assert_eq!(v, 1.0);
    }

    // const eval_const — unary logical not on non-bool is an error
    #[test]
    fn const_unary_not_on_non_bool_is_error() {
        assert!(has_error(
            "const C: bool = !5; fn main() {}",
            "logical `!` requires a bool operand"
        ));
    }

    // const eval_const — unary bitwise not on i53
    #[test]
    fn const_unary_bitnot_i53_evaluates() {
        let program = bind_ok("const N: i53 = ~0; fn main() { let x = N; }");
        let Statement::Let(s) = &program.functions[0].body.statements[0] else {
            panic!("expected let");
        };
        let ExpressionKind::Literal(v) = s.init.kind else {
            panic!("expected literal");
        };
        assert_eq!(v, (!0i64) as f64);
    }

    // const eval_const — unary bitwise not on non-i53 is an error
    #[test]
    fn const_unary_bitnot_on_non_i53_is_error() {
        assert!(has_error(
            "const C: bool = ~true; fn main() {}",
            "bitwise `~` requires an i53 operand"
        ));
    }

    // const eval_const — binary expression
    #[test]
    fn const_binary_expression_evaluates() {
        let program = bind_ok("const C: i53 = 3 + 4; fn main() { let x = C; }");
        let Statement::Let(s) = &program.functions[0].body.statements[0] else {
            panic!("expected let");
        };
        let ExpressionKind::Literal(v) = s.init.kind else {
            panic!("expected literal");
        };
        assert_eq!(v, 7.0);
    }

    // const eval_const — binary type mismatch is an error
    #[test]
    fn const_binary_type_mismatch_is_error() {
        assert!(has_error(
            "const C: f64 = 1 + 1.0; fn main() {}",
            "type mismatch in constant expression"
        ));
    }

    // const eval_const — logical || on non-bool is an error
    #[test]
    fn const_logical_or_on_non_bool_is_error() {
        assert!(has_error(
            "const C: bool = 1 || 2; fn main() {}",
            "logical `||` requires bool operands"
        ));
    }

    // const eval_const — logical && on non-bool is an error
    #[test]
    fn const_logical_and_on_non_bool_is_error() {
        assert!(has_error(
            "const C: bool = 1 && 2; fn main() {}",
            "logical `&&` requires bool operands"
        ));
    }

    // const eval_const — cast I53 -> F64
    #[test]
    fn const_cast_i53_to_f64_evaluates() {
        let program = bind_ok("const C: f64 = 5 as f64; fn main() { let x = C; }");
        let Statement::Let(s) = &program.functions[0].body.statements[0] else {
            panic!("expected let");
        };
        let ExpressionKind::Literal(v) = s.init.kind else {
            panic!("expected literal");
        };
        assert_eq!(v, 5.0);
        assert_eq!(s.init.ty, Type::F64);
    }

    // const eval_const — cast F64 -> I53 (truncation)
    #[test]
    fn const_cast_f64_to_i53_truncates() {
        let program = bind_ok("const C: i53 = 3.9 as i53; fn main() { let x = C; }");
        let Statement::Let(s) = &program.functions[0].body.statements[0] else {
            panic!("expected let");
        };
        let ExpressionKind::Literal(v) = s.init.kind else {
            panic!("expected literal");
        };
        assert_eq!(v, 3.0);
        assert_eq!(s.init.ty, Type::I53);
    }

    // const eval_const — cast Bool -> I53
    #[test]
    fn const_cast_bool_to_i53_evaluates() {
        let program = bind_ok("const C: i53 = true as i53; fn main() { let x = C; }");
        let Statement::Let(s) = &program.functions[0].body.statements[0] else {
            panic!("expected let");
        };
        let ExpressionKind::Literal(v) = s.init.kind else {
            panic!("expected literal");
        };
        assert_eq!(v, 1.0);
        assert_eq!(s.init.ty, Type::I53);
    }

    // const eval_const — cast Bool -> F64
    #[test]
    fn const_cast_bool_to_f64_evaluates() {
        let program = bind_ok("const C: f64 = false as f64; fn main() { let x = C; }");
        let Statement::Let(s) = &program.functions[0].body.statements[0] else {
            panic!("expected let");
        };
        let ExpressionKind::Literal(v) = s.init.kind else {
            panic!("expected literal");
        };
        assert_eq!(v, 0.0);
        assert_eq!(s.init.ty, Type::F64);
    }

    // const eval_const — cast to Bool is an error
    #[test]
    fn const_cast_to_bool_is_error() {
        assert!(has_error(
            "const C: bool = 1 as bool; fn main() {}",
            "cannot cast to `bool`"
        ));
    }

    // const eval_const — identity cast emits a warning
    #[test]
    fn const_identity_cast_emits_warning() {
        use crate::diagnostic::Severity;
        let (ast, _) = parse("const C: i53 = 5 as i53; fn main() {}");
        let (_program, warnings) =
            bind(&ast).unwrap_or_else(|diags| panic!("bind errors: {diags:#?}"));
        assert!(
            warnings
                .iter()
                .any(|d| d.severity == Severity::Warning && d.message.contains("identity cast")),
            "expected identity cast warning, got: {warnings:#?}",
        );
    }

    // const eval_const — unsupported expression (intrinsic call) in constant context
    #[test]
    fn const_unsupported_expression_is_error() {
        assert!(has_error(
            "const C: f64 = abs(1.0); fn main() {}",
            "unsupported expression in constant context"
        ));
    }

    // pre_pass — const declared type does not match value type
    #[test]
    fn const_declared_type_mismatch_is_error() {
        assert!(has_error(
            "const C: i53 = 1.5; fn main() {}",
            "declared as `I53` but value has type `F64`"
        ));
    }

    // for loop — lower bound must be i53
    #[test]
    fn for_loop_lower_bound_not_i53_is_error() {
        assert!(has_error(
            "fn main() { for i in 1.0..10 {} }",
            "lower bound must be `i53`"
        ));
    }

    // for loop — upper bound must be i53
    #[test]
    fn for_loop_upper_bound_not_i53_is_error() {
        assert!(has_error(
            "fn main() { for i in 0..10.0 {} }",
            "upper bound must be `i53`"
        ));
    }

    // for loop — step must be i53
    #[test]
    fn for_loop_step_not_i53_is_error() {
        assert!(has_error(
            "fn main() { for i in (0..10).step_by(1.0) {} }",
            "step must be `i53`"
        ));
    }

    // yield statement binds without error
    #[test]
    fn yield_statement_is_valid() {
        bind_ok("fn main() { yield; }");
    }

    // sleep — duration must be f64
    #[test]
    fn sleep_with_non_f64_duration_is_error() {
        assert!(has_error(
            "fn main() { sleep(1); }",
            "`sleep` duration must be `f64`"
        ));
    }

    // batch_write — hash must be f64
    #[test]
    fn batch_write_with_non_f64_hash_is_error() {
        assert!(has_error(
            r#"fn main() { batch_write(1, On, 1.0); }"#,
            "batch_write hash must be `f64`"
        ));
    }

    // assignment target — cannot assign to a constant
    #[test]
    fn assign_to_constant_is_error() {
        assert!(has_error(
            "const X: i53 = 5; fn main() { X = 10; }",
            "cannot assign to constant `X`"
        ));
    }

    // assignment target — cannot assign to a device directly
    #[test]
    fn assign_to_device_directly_is_error() {
        assert!(has_error(
            "device d: d0; fn main() { d = 1.0; }",
            "is a device; use `d.Field = ...` to write"
        ));
    }

    // assignment target — undeclared name in assignment
    #[test]
    fn assign_to_undeclared_name_is_error() {
        assert!(has_error("fn main() { z = 1; }", "undeclared name `z`"));
    }

    // assignment target — device field on non-device
    #[test]
    fn assign_to_device_field_on_non_device_is_error() {
        assert!(has_error(
            "fn main() { let x = 1.0; x.Setting = 1.0; }",
            "`x` is not a device"
        ));
    }

    // assignment target — slot field write on non-device
    #[test]
    fn slot_field_write_on_non_device_is_error() {
        assert!(has_error(
            "fn main() { let x = 1.0; x.Occupancy(0).Setting = 1.0; }",
            "`x` is not a device"
        ));
    }

    // assignment target — slot index must be i53
    #[test]
    fn slot_field_write_slot_not_i53_is_error() {
        assert!(has_error(
            "device d: d0; fn main() { d.Occupancy(1.0).Setting = 1.0; }",
            "slot index must be `i53`"
        ));
    }

    // expression binding — device used as a plain variable read
    #[test]
    fn device_used_as_plain_variable_is_error() {
        assert!(has_error(
            "device sensor: d0; fn main() { let x = sensor; }",
            "`sensor` is a device; use `sensor.Field` to read"
        ));
    }

    // expression binding — device read on non-device
    #[test]
    fn device_field_read_on_non_device_is_error() {
        assert!(has_error(
            "fn main() { let x = 1.0; let y = x.Temperature; }",
            "`x` is not a device"
        ));
    }

    // expression binding — slot read on non-device
    #[test]
    fn slot_read_on_non_device_is_error() {
        assert!(has_error(
            "fn main() { let x = 1.0; let y = x.Occupancy(0).Temperature; }",
            "`x` is not a device"
        ));
    }

    // expression binding — slot read slot index not i53
    #[test]
    fn slot_read_slot_index_not_i53_is_error() {
        assert!(has_error(
            "device d: d0; fn main() { let y = d.Occupancy(1.0).Temperature; }",
            "slot index must be `i53`"
        ));
    }

    // expression binding — batch_read hash must be f64
    #[test]
    fn batch_read_hash_not_f64_is_error() {
        assert!(has_error(
            "fn main() { let y = batch_read(1, Temperature, Average); }",
            "batch_read hash must be `f64`"
        ));
    }

    // call — argument type mismatch
    #[test]
    fn call_argument_type_mismatch_is_error() {
        assert!(has_error(
            "fn add(x: i53, y: i53) -> i53 { return x + y; } fn main() { add(1, 2.0); }",
            "argument 2 to `add` has type `F64`, expected `I53`"
        ));
    }

    // intrinsic call — wrong argument count
    #[test]
    fn intrinsic_call_wrong_arg_count_is_error() {
        assert!(has_error(
            "fn main() { let x = abs(1.0, 2.0); }",
            "`Abs` expects 1 argument(s), found 2"
        ));
    }

    // intrinsic call — non-f64 argument
    #[test]
    fn intrinsic_call_non_f64_arg_is_error() {
        assert!(has_error(
            "fn main() { let x = pow(1, 2.0); }",
            "intrinsic functions require `f64` arguments, found `I53`"
        ));
    }

    // validate_main — duplicate main is an error
    #[test]
    fn duplicate_main_is_error() {
        assert!(has_error(
            "fn main() {} fn main() {}",
            "duplicate definition of `main`"
        ));
    }

    // validate_main — cast from void function result is an invalid cast
    #[test]
    fn cast_from_unit_to_numeric_is_error() {
        assert!(has_error(
            "fn noop() {} fn main() { let x = noop() as i53; }",
            "invalid cast from `Unit`"
        ));
    }

    // infer_binary_type — logical operator requires bool on left
    #[test]
    fn logical_or_lhs_not_bool_is_error() {
        assert!(has_error(
            "fn main() { let b: bool = true; let x = 1 || b; }",
            "logical operator requires `bool` operands, found `I53` on left"
        ));
    }

    // infer_binary_type — logical operator requires bool on right
    #[test]
    fn logical_or_rhs_not_bool_is_error() {
        assert!(has_error(
            "fn main() { let b: bool = true; let x = b || 1; }",
            "logical operator requires `bool` operands, found `I53` on right"
        ));
    }

    // infer_binary_type — comparison requires operands of the same type
    #[test]
    fn comparison_with_mismatched_types_is_error() {
        assert!(has_error(
            "fn main() { let x = 1 < 2.0; }",
            "comparison requires operands of the same type"
        ));
    }

    // infer_binary_type — bitwise operator left operand must be i53
    #[test]
    fn bitwise_op_lhs_not_i53_is_error() {
        assert!(has_error(
            "fn main() { let x = 1.0 << 2; }",
            "bitwise/shift operator requires `i53` operands, found `F64` on left"
        ));
    }

    // infer_binary_type — bitwise operator right operand must be i53
    #[test]
    fn bitwise_op_rhs_not_i53_is_error() {
        assert!(has_error(
            "fn main() { let x = 2 >> 1.0; }",
            "bitwise/shift operator requires `i53` operands, found `F64` on right"
        ));
    }

    // infer_binary_type — arithmetic on bool is an error
    #[test]
    fn arithmetic_on_bool_is_error() {
        assert!(has_error(
            "fn main() { let x = true + false; }",
            "arithmetic operators cannot be applied to `bool`"
        ));
    }

    // infer_unary_type — unary negation on bool is an error
    #[test]
    fn unary_neg_on_bool_is_error() {
        assert!(has_error(
            "fn main() { let b: bool = true; let x = -b; }",
            "unary `-` cannot be applied to `bool`"
        ));
    }

    // infer_unary_type — logical not on non-bool is an error
    #[test]
    fn logical_not_on_non_bool_is_error() {
        assert!(has_error(
            "fn main() { let x = 1; let y = !x; }",
            "logical `!` requires `bool` operand, found `I53`"
        ));
    }

    // infer_unary_type — bitwise not on non-i53 is an error
    #[test]
    fn bitwise_not_on_non_i53_is_error() {
        assert!(has_error(
            "fn main() { let b: bool = true; let y = ~b; }",
            "bitwise `~` requires `i53` operand, found `Bool`"
        ));
    }

    // division yields f64 regardless of operand types
    #[test]
    fn division_type_mismatch_is_error() {
        assert!(has_error(
            "fn main() { let x = 1 / 1.0; }",
            "type mismatch in `/` operator: `I53` and `F64`"
        ));
    }

    // Type coercion: missing return value from non-void function
    #[test]
    fn missing_return_value_is_error() {
        assert!(has_error(
            "fn f() -> i53 { return; } fn main() {}",
            "missing return value: expected `I53`"
        ));
    }
}
