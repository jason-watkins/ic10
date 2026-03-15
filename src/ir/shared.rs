/// The three IC20 surface types (§3), plus `Unit` for void functions.
///
/// `Unit` is not a surface type — users cannot write it in annotations. It is
/// used internally as the type of void-function call expressions and their
/// symbols so that misusing the result of a void call is caught by the normal
/// type checking rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Type {
    Bool,
    I53,
    F64,
    Unit,
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

/// Binary operators, in precedence order from low to high (§5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOperator {
    /// Arithmetic negation `-x` (`i53` or `f64`).
    Neg,
    /// Logical NOT `!x` (`bool`).
    Not,
    /// Bitwise complement `~x` (`i53`).
    BitNot,
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
