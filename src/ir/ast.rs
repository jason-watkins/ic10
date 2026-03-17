//! Unresolved abstract syntax tree — the direct output of the parser.

use crate::diagnostic::Span;

use super::shared::{BatchMode, BinaryOperator, DevicePin, Intrinsic, Type, UnaryOperator};

/// A complete IC20 program: an ordered list of top-level items (§1.2).
#[derive(Debug, Clone)]
pub struct Program {
    pub items: Vec<Item>,
    pub span: Span,
}

/// A top-level item: `const`, `static`, `device`, or `fn` (§1.2).
#[derive(Debug, Clone)]
pub enum Item {
    Const(ConstDeclaration),
    Static(StaticDeclaration),
    Device(DeviceDeclaration),
    Fn(FunctionDeclaration),
}

/// `const NAME: Type = expr;` (§4.3).
#[derive(Debug, Clone)]
pub struct ConstDeclaration {
    pub name: String,
    pub ty: Type,
    pub value: Expression,
    pub span: Span,
}

/// `static [mut] NAME: Type = expr;` — a top-level variable with a fixed stack home (§4.4).
#[derive(Debug, Clone)]
pub struct StaticDeclaration {
    pub name: String,
    pub mutable: bool,
    pub ty: Type,
    pub initializer: Expression,
    pub span: Span,
}

/// `device NAME: pin;` — binds a name to a hardware pin (§8.1).
#[derive(Debug, Clone)]
pub struct DeviceDeclaration {
    pub name: String,
    pub pin: DevicePin,
    pub span: Span,
}

/// `fn NAME(params) -> return_type { body }` (§7.1).
#[derive(Debug, Clone)]
pub struct FunctionDeclaration {
    pub name: String,
    pub params: Vec<Parameter>,
    pub return_type: Option<Type>,
    pub body: Block,
    pub span: Span,
}

/// A single function parameter: `name: Type` (§7.2).
#[derive(Debug, Clone)]
pub struct Parameter {
    pub name: String,
    pub ty: Type,
    pub span: Span,
}

/// A block: `{ statement* }` — also a new lexical scope (§6.4).
#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Statement>,
    pub span: Span,
}

/// A statement (§6).
#[derive(Debug, Clone)]
pub enum Statement {
    /// `let [mut] name [: Type] = expr;` (§6.1).
    Let(LetStatement),
    /// Variable assignment or device field write (§6.2).
    Assign(AssignStatement),
    /// A call expression used as a statement: `f(args);` (§6.3).
    Expression(ExpressionStatement),
    /// `if expr { … } [else { … }]` (§6.5).
    If(IfStatement),
    /// `while cond { … }`; also the desugared form of `loop { … }` (§6.6, §6.7).
    While(WhileStatement),
    /// `for ident in expr..expr { … }` (§6.8).
    For(ForStatement),
    /// `break ['label];` (§6.9).
    Break(BreakStatement),
    /// `continue ['label];` (§6.10).
    Continue(ContinueStatement),
    /// `return [expr];` (§6.11).
    Return(ReturnStatement),
    /// `yield;` (§6.12).
    Yield(Span),
    /// `sleep(expr);` (§6.13).
    Sleep(SleepStatement),
    /// `batch_write(hash_expr, Field, value);` (§8.5.2).
    BatchWrite(BatchWriteStatement),
}

/// `let [mut] name [: Type] = expr;`
#[derive(Debug, Clone)]
pub struct LetStatement {
    pub mutable: bool,
    pub name: String,
    pub ty: Option<Type>,
    pub init: Expression,
    pub span: Span,
}

/// Assignment statement — three forms (§6.2):
/// - `name = expr;`
/// - `name.field = expr;`
/// - `name.slot(expr).field = expr;`
#[derive(Debug, Clone)]
pub struct AssignStatement {
    pub lhs: AssignmentTarget,
    pub rhs: Expression,
    pub span: Span,
}

/// Left-hand side of an assignment.
#[derive(Debug, Clone)]
pub enum AssignmentTarget {
    /// Plain variable: `name`
    Var { name: String, span: Span },
    /// Device logic field: `device.Field`
    DeviceField {
        device: String,
        field: String,
        span: Span,
    },
    /// Device slot field: `device.slot(idx).Field`
    SlotField {
        device: String,
        slot: Expression,
        field: String,
        span: Span,
    },
}

/// A call expression used as a statement.
#[derive(Debug, Clone)]
pub struct ExpressionStatement {
    pub expr: Expression,
    pub span: Span,
}

/// `if cond { then } [else { else_ }]`
#[derive(Debug, Clone)]
pub struct IfStatement {
    pub cond: Expression,
    pub then_block: Block,
    pub else_clause: Option<ElseClause>,
    pub span: Span,
}

/// The `else` part of an `if` statement.
#[derive(Debug, Clone)]
pub enum ElseClause {
    Block(Block),
    If(Box<IfStatement>),
}

/// `['label:] while cond { body }`; also the desugared form of `loop { body }`
#[derive(Debug, Clone)]
pub struct WhileStatement {
    pub label: Option<String>,
    pub cond: Expression,
    pub body: Block,
    pub span: Span,
}

/// `for var in lower..upper { body }` with optional modifiers:
/// - `lower..=upper` for inclusive upper bound
/// - `(lower..upper).rev()` for reverse iteration
/// - `(lower..upper).step_by(n)` for custom step
#[derive(Debug, Clone)]
pub struct ForStatement {
    pub label: Option<String>,
    pub var: String,
    pub lower: Expression,
    pub upper: Expression,
    pub inclusive: bool,
    pub reverse: bool,
    pub step: Option<Expression>,
    pub body: Block,
    pub span: Span,
}

/// `return [expr];`
#[derive(Debug, Clone)]
pub struct ReturnStatement {
    pub value: Option<Expression>,
    pub span: Span,
}

/// `break ['label];`
#[derive(Debug, Clone)]
pub struct BreakStatement {
    pub label: Option<String>,
    pub span: Span,
}

/// `continue ['label];`
#[derive(Debug, Clone)]
pub struct ContinueStatement {
    pub label: Option<String>,
    pub span: Span,
}

/// `sleep(expr);`
#[derive(Debug, Clone)]
pub struct SleepStatement {
    pub duration: Expression,
    pub span: Span,
}

/// `batch_write(hash_expr, Field, value);`
#[derive(Debug, Clone)]
pub struct BatchWriteStatement {
    pub hash_expr: Expression,
    pub field: String,
    pub value: Expression,
    pub span: Span,
}

/// An expression node, always carrying its source span (§5).
#[derive(Debug, Clone)]
pub struct Expression {
    pub kind: ExpressionKind,
    pub span: Span,
}

/// The kind of an expression.
#[derive(Debug, Clone)]
pub enum ExpressionKind {
    /// An integer or float literal (§2.4.3–§2.4.6).
    Literal(LiteralKind),
    /// A variable reference: `name` (§4.1).
    Variable(String),
    /// Binary operation: `lhs op rhs` (§5.2–§5.6).
    Binary(BinaryOperator, Box<Expression>, Box<Expression>),
    /// Unary operation: `op expr` (§5.7).
    Unary(UnaryOperator, Box<Expression>),
    /// Type cast: `expr as Type` (§5.8).
    Cast(Box<Expression>, Type),
    /// User-defined function call: `f(args)` (§5.9).
    Call(CallExpression),
    /// Intrinsic function call (§5.11).
    IntrinsicCall(Intrinsic, Vec<Expression>),
    /// Device logic-field read: `device.Field` (§5.10, §8.2).
    DeviceRead { device: String, field: String },
    /// Device slot-field read: `device.slot(idx).Field` (§5.10, §8.4).
    SlotRead {
        device: String,
        slot: Box<Expression>,
        field: String,
    },
    /// Batch read: `batch_read(hash_expr, field, mode)` (§8.5.1).
    BatchRead {
        hash_expr: Box<Expression>,
        field: String,
        mode: BatchMode,
    },
    /// `select(cond, if_true, if_false)` (§5.12).
    Select {
        cond: Box<Expression>,
        if_true: Box<Expression>,
        if_false: Box<Expression>,
    },
    /// `hash("string")` — evaluated at compile time (§5.13).
    Hash(String),
}

/// Literal value kinds (§2.4.3–§2.4.6).
#[derive(Debug, Clone)]
pub enum LiteralKind {
    /// A 53-bit signed integer literal.
    I53(i64),
    /// A 64-bit floating-point literal.
    F64(f64),
    /// A boolean literal (`true` or `false`).
    Bool(bool),
}

/// User-defined function call expression.
#[derive(Debug, Clone)]
pub struct CallExpression {
    pub name: String,
    pub args: Vec<Expression>,
    pub span: Span,
}
