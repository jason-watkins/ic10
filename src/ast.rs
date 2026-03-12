use crate::diagnostic::Span;

/// The three IC20 surface types (§3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
    Bool,
    I53,
    F64,
}

/// A complete IC20 program: an ordered list of top-level items (§1.2).
#[derive(Debug, Clone)]
pub struct Program {
    pub items: Vec<Item>,
    pub span: Span,
}

/// A top-level item: `const`, `device`, or `fn` (§1.2).
#[derive(Debug, Clone)]
pub enum Item {
    Const(ConstDeclaration),
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

/// `device NAME: pin;` — binds a name to a hardware pin (§8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevicePin {
    D0,
    D1,
    D2,
    D3,
    D4,
    D5,
    /// The IC housing itself.
    Db,
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
    /// `loop { … }` (§6.6).
    Loop(LoopStatement),
    /// `while expr { … }` (§6.7).
    While(WhileStatement),
    /// `for ident in expr..expr { … }` (§6.8).
    For(ForStatement),
    /// `break;` (§6.9).
    Break(Span),
    /// `continue;` (§6.10).
    Continue(Span),
    /// `return [expr];` (§6.11).
    Return(ReturnStatement),
    /// `yield;` (§6.12).
    Yield(Span),
    /// `sleep(expr);` (§6.13).
    Sleep(SleepStatement),
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

/// `loop { body }`
#[derive(Debug, Clone)]
pub struct LoopStatement {
    pub body: Block,
    pub span: Span,
}

/// `while cond { body }`
#[derive(Debug, Clone)]
pub struct WhileStatement {
    pub cond: Expression,
    pub body: Block,
    pub span: Span,
}

/// `for var in lower..upper { body }`
#[derive(Debug, Clone)]
pub struct ForStatement {
    pub var: String,
    pub lower: Expression,
    pub upper: Expression,
    pub body: Block,
    pub span: Span,
}

/// `return [expr];`
#[derive(Debug, Clone)]
pub struct ReturnStatement {
    pub value: Option<Expression>,
    pub span: Span,
}

/// `sleep(expr);`
#[derive(Debug, Clone)]
pub struct SleepStatement {
    pub duration: Expression,
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
    /// Built-in math function call (§5.11).
    BuiltinCall(BuiltinFunction, Vec<Expression>),
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
    I53(i64),
    F64(f64),
    Bool(bool),
}

/// Binary operators, in precedence order from low to high (§5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    // Precedence1 – logical OR
    Or,
    // Precedence2 – logical AND
    And,
    // Precedence3 – comparisons (non-associative)
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    // Precedence4 – bitwise OR
    BitOr,
    // Precedence5 – bitwise XOR
    BitXor,
    // Precedence6 – bitwise AND
    BitAnd,
    // Precedence7 – shifts
    Shl,
    Shr,
    // Precedence8 – additive
    Add,
    Sub,
    // Precedence9 – multiplicative
    Mul,
    Div,
    Rem,
}

/// Unary operators (§5.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOperator {
    /// Arithmetic negation `-x` (`i53` or `f64`).
    Neg,
    /// Logical NOT `!x` (`bool`).
    Not,
    /// Bitwise complement `~x` (`i53`).
    BitNot,
}

/// User-defined function call expression.
#[derive(Debug, Clone)]
pub struct CallExpression {
    pub name: String,
    pub args: Vec<Expression>,
    pub span: Span,
}

/// Built-in math/utility functions that map to IC10 instructions (§5.11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinFunction {
    Abs,
    Ceil,
    Floor,
    Round,
    Trunc,
    Sqrt,
    Exp,
    Log,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Pow,
    Min,
    Max,
    Lerp,
    Clamp,
    Rand,
}

/// Batch operation mode (§8.5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchMode {
    Average,
    Sum,
    Minimum,
    Maximum,
    Contents,
}
