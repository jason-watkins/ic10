use crate::ast::{BatchMode, BinaryOperator, BuiltinFunction, DevicePin, Type, UnaryOperator};
use crate::diagnostic::Span;

/// An opaque index into a `SymbolTable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SymbolId(pub usize);

/// The resolved, type-annotated program — output of the resolve pass.
#[derive(Debug, Clone)]
pub struct Program {
    pub functions: Vec<FunctionDeclaration>,
    pub symbols: SymbolTable,
}

/// Maps `SymbolId → SymbolInfo` for every let-binding, parameter, and function symbol.
#[derive(Debug, Clone, Default)]
pub struct SymbolTable {
    pub symbols: Vec<SymbolInfo>,
}

impl SymbolTable {
    pub fn push(&mut self, info: SymbolInfo) -> SymbolId {
        let id = SymbolId(self.symbols.len());
        self.symbols.push(info);
        id
    }

    pub fn get(&self, id: SymbolId) -> &SymbolInfo {
        &self.symbols[id.0]
    }

    pub fn get_mut(&mut self, id: SymbolId) -> &mut SymbolInfo {
        &mut self.symbols[id.0]
    }
}

/// Metadata for a single symbol (local variable, parameter, or function).
#[derive(Debug, Clone)]
pub struct SymbolInfo {
    /// Original source name (for diagnostics and code generation).
    pub name: String,
    pub ty: Type,
    pub mutable: bool,
    pub kind: SymbolKind,
}

/// The kind of a symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Local,
    Parameter,
    Function,
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
    Let(LetStatement),
    Assign(AssignStatement),
    Expression(ExpressionStatement),
    If(IfStatement),
    While(WhileStatement),
    For(ForStatement),
    Break(Span),
    Continue(Span),
    Return(ReturnStatement),
    Yield(Span),
    Sleep(SleepStatement),
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
    Variable {
        symbol_id: SymbolId,
        span: Span,
    },
    DeviceField {
        pin: DevicePin,
        field: String,
        span: Span,
    },
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
    Block(Block),
    If(Box<IfStatement>),
}

/// `while cond { body }`
#[derive(Debug, Clone)]
pub struct WhileStatement {
    pub condition: Expression,
    pub body: Block,
    pub span: Span,
}

/// `for var in lower..upper { body }` — loop variable resolved to a `SymbolId`.
#[derive(Debug, Clone)]
pub struct ForStatement {
    pub variable: SymbolId,
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
    Binary(BinaryOperator, Box<Expression>, Box<Expression>),
    Unary(UnaryOperator, Box<Expression>),
    Cast(Box<Expression>, Type),
    Call(SymbolId, Vec<Expression>),
    BuiltinCall(BuiltinFunction, Vec<Expression>),
    DeviceRead {
        pin: DevicePin,
        field: String,
    },
    SlotRead {
        pin: DevicePin,
        slot: Box<Expression>,
        field: String,
    },
    BatchRead {
        hash_expr: Box<Expression>,
        field: String,
        mode: BatchMode,
    },
    Select {
        condition: Box<Expression>,
        if_true: Box<Expression>,
        if_false: Box<Expression>,
    },
}
