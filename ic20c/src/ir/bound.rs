//! Bound (name-resolved, type-annotated) IR — the output of the bind pass.

use crate::diagnostic::Span;

use super::shared::{BatchMode, BinaryOperator, DevicePin, Intrinsic, Type, UnaryOperator};

/// An opaque index into a `SymbolTable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SymbolId(pub usize);

/// An opaque index into the program's static variable list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StaticId(pub usize);

/// The bound, type-annotated program — output of the bind pass.
#[derive(Debug, Clone)]
pub struct Program {
    pub functions: Vec<FunctionDeclaration>,
    pub statics: Vec<StaticVariable>,
    pub static_initializers: Vec<StaticInitializer>,
    pub symbols: SymbolTable,
}

/// A resolved static variable declaration (§4.4).
#[derive(Debug, Clone)]
pub struct StaticVariable {
    pub name: String,
    pub mutable: bool,
    pub ty: Type,
    pub address: u16,
}

/// A static variable's initializer expression, bound and type-checked.
#[derive(Debug, Clone)]
pub struct StaticInitializer {
    pub static_id: StaticId,
    pub expression: Expression,
    pub span: Span,
}

/// Maps `SymbolId → SymbolInfo` for every let-binding, parameter, and function symbol.
#[derive(Debug, Clone, Default)]
pub struct SymbolTable {
    pub symbols: Vec<SymbolInfo>,
}

impl SymbolTable {
    /// Inserts a new symbol and returns its `SymbolId`.
    pub fn push(&mut self, info: SymbolInfo) -> SymbolId {
        let id = SymbolId(self.symbols.len());
        self.symbols.push(info);
        id
    }

    /// Returns a shared reference to the `SymbolInfo` for `id`.
    ///
    /// # Panics
    ///
    /// Panics if `id` is out of bounds.
    pub fn get(&self, id: SymbolId) -> &SymbolInfo {
        &self.symbols[id.0]
    }

    /// Returns a mutable reference to the `SymbolInfo` for `id`.
    ///
    /// # Panics
    ///
    /// Panics if `id` is out of bounds.
    pub fn get_mut(&mut self, id: SymbolId) -> &mut SymbolInfo {
        &mut self.symbols[id.0]
    }
}

/// Metadata for a single symbol (local variable, parameter, or function).
#[derive(Debug, Clone)]
pub struct SymbolInfo {
    /// Original source name (for diagnostics and code generation).
    pub name: String,
    /// Resolved type of this symbol.
    pub ty: Type,
    /// Whether the symbol was declared with `mut`.
    pub mutable: bool,
    /// Disambiguates locals, parameters, functions, and statics.
    pub kind: SymbolKind,
}

/// The kind of a symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    /// A `let` binding inside a function body.
    Local,
    /// A function parameter.
    Parameter,
    /// A user-defined function name.
    Function,
    /// A `static` variable, indexed by `StaticId` into the program's statics list.
    Static(StaticId),
}

/// A resolved function declaration.
#[derive(Debug, Clone)]
pub struct FunctionDeclaration {
    pub name: String,
    pub symbol_id: SymbolId,
    pub parameters: Vec<Parameter>,
    pub return_type: Option<Type>,
    pub body: Block,
    pub span: Span,
}

/// A resolved parameter.
#[derive(Debug, Clone)]
pub struct Parameter {
    pub name: String,
    pub symbol_id: SymbolId,
    pub ty: Type,
    pub span: Span,
}

/// A resolved block (sequence of statements).
#[derive(Debug, Clone)]
pub struct Block {
    pub statements: Vec<Statement>,
    pub span: Span,
}

/// A resolved statement.
#[derive(Debug, Clone)]
pub enum Statement {
    /// `let [mut] name [: Type] = expr;`
    Let(LetStatement),
    /// Variable or device-field assignment.
    Assign(AssignStatement),
    /// A call expression used as a statement.
    Expression(ExpressionStatement),
    /// `if cond { … } [else { … }]`
    If(IfStatement),
    /// `while cond { … }`
    While(WhileStatement),
    /// `for var in range { … }`
    For(ForStatement),
    /// `break ['label];`
    Break(BreakStatement),
    /// `continue ['label];`
    Continue(ContinueStatement),
    /// `return [expr];`
    Return(ReturnStatement),
    /// `yield;`
    Yield(Span),
    /// `sleep(expr);`
    Sleep(SleepStatement),
    /// `batch_write(hash_expr, Field, value);`
    BatchWrite(BatchWriteStatement),
}

impl Statement {
    /// Returns the source span of this statement, regardless of variant.
    pub fn span(&self) -> Span {
        match self {
            Statement::Let(s) => s.span,
            Statement::Assign(s) => s.span,
            Statement::Expression(s) => s.span,
            Statement::If(s) => s.span,
            Statement::While(s) => s.span,
            Statement::For(s) => s.span,
            Statement::Break(s) => s.span,
            Statement::Continue(s) => s.span,
            Statement::Return(s) => s.span,
            Statement::Yield(span) => *span,
            Statement::Sleep(s) => s.span,
            Statement::BatchWrite(s) => s.span,
        }
    }
}

/// `let [mut] name [: Type] = expr;`
#[derive(Debug, Clone)]
pub struct LetStatement {
    pub symbol_id: SymbolId,
    pub init: Expression,
    pub span: Span,
}

/// Assignment to a variable or device field.
#[derive(Debug, Clone)]
pub struct AssignStatement {
    pub target: AssignmentTarget,
    pub value: Expression,
    pub span: Span,
}

/// Resolved assignment target.
#[derive(Debug, Clone)]
pub enum AssignmentTarget {
    /// Assignment to a local variable or parameter.
    Variable { symbol_id: SymbolId, span: Span },
    /// Assignment to a device logic field: `device.Field = expr;`
    DeviceField {
        pin: DevicePin,
        field: String,
        span: Span,
    },
    /// Assignment to a device slot field: `device.slot(idx).Field = expr;`
    SlotField {
        pin: DevicePin,
        slot: Expression,
        field: String,
        span: Span,
    },
}

/// A call expression used as a statement.
#[derive(Debug, Clone)]
pub struct ExpressionStatement {
    pub expression: Expression,
    pub span: Span,
}

/// `if cond { then } [else { else_ }]`
#[derive(Debug, Clone)]
pub struct IfStatement {
    pub condition: Expression,
    pub then_block: Block,
    pub else_clause: Option<ElseClause>,
    pub span: Span,
}

/// The `else` branch.
#[derive(Debug, Clone)]
pub enum ElseClause {
    /// A plain `else { … }` block.
    Block(Block),
    /// An `else if …` chain.
    If(Box<IfStatement>),
}

/// `while cond { body }`
#[derive(Debug, Clone)]
pub struct WhileStatement {
    pub label: Option<String>,
    pub condition: Expression,
    pub body: Block,
    pub span: Span,
}

/// `for var in lower..upper { body }` — loop variable resolved to a `SymbolId`.
#[derive(Debug, Clone)]
pub struct ForStatement {
    pub label: Option<String>,
    pub variable: SymbolId,
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

/// `batch_write(hash_expr, Field, value);` (§8.5.2).
#[derive(Debug, Clone)]
pub struct BatchWriteStatement {
    pub hash_expr: Expression,
    pub field: String,
    pub value: Expression,
    pub span: Span,
}

/// A resolved, type-annotated expression.
#[derive(Debug, Clone)]
pub struct Expression {
    pub kind: ExpressionKind,
    /// Always present — the type checker ensures this is never unset.
    pub ty: Type,
    pub span: Span,
}

/// The kind of a resolved expression.
#[derive(Debug, Clone)]
pub enum ExpressionKind {
    /// All literals (integer, float, bool, folded consts, hashes) represented as `f64`.
    /// The `ty` field on the enclosing `Expression` distinguishes i53/f64/bool.
    Literal(f64),
    /// A reference to a local variable or parameter.
    Variable(SymbolId),
    /// Binary operation: `lhs op rhs`.
    Binary(BinaryOperator, Box<Expression>, Box<Expression>),
    /// Unary operation: `op expr`.
    Unary(UnaryOperator, Box<Expression>),
    /// Type cast: `expr as Type`.
    Cast(Box<Expression>, Type),
    /// User-defined function call. The `SymbolId` refers to the function symbol.
    Call(SymbolId, Vec<Expression>),
    /// Intrinsic function call.
    IntrinsicCall(Intrinsic, Vec<Expression>),
    /// Device logic-field read: `device.Field`.
    DeviceRead { pin: DevicePin, field: String },
    /// Device slot-field read: `device.slot(idx).Field`.
    SlotRead {
        pin: DevicePin,
        slot: Box<Expression>,
        field: String,
    },
    /// Batch read: `batch_read(hash_expr, field, mode)`.
    BatchRead {
        hash_expr: Box<Expression>,
        field: String,
        mode: BatchMode,
    },
    /// `select(cond, if_true, if_false)`.
    Select {
        condition: Box<Expression>,
        if_true: Box<Expression>,
        if_false: Box<Expression>,
    },
}
