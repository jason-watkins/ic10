//! Types and operators shared across all IR layers (AST, bound, CFG).

/// The three IC20 surface types (§3), plus `Unit` for void functions.
///
/// `Unit` is not a surface type — users cannot write it in annotations. It is
/// used internally as the type of void-function call expressions and their
/// symbols so that misusing the result of a void call is caught by the normal
/// type checking rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Type {
    /// Boolean truth value (`true` / `false`).
    Bool,
    /// 53-bit signed integer, stored in the integer-representable range of an `f64`.
    I53,
    /// 64-bit IEEE 754 floating-point number.
    F64,
    /// The unit type, used internally for void-function return types.
    Unit,
}

/// `device NAME: pin;` — binds a name to a hardware pin (§8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevicePin {
    /// Hardware pin 0.
    D0,
    /// Hardware pin 1.
    D1,
    /// Hardware pin 2.
    D2,
    /// Hardware pin 3.
    D3,
    /// Hardware pin 4.
    D4,
    /// Hardware pin 5.
    D5,
    /// The IC housing itself.
    Db,
}

/// Binary operators, in precedence order from low to high (§5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinaryOperator {
    /// Logical OR `||` (precedence 1, lowest).
    Or,
    /// Logical AND `&&` (precedence 2).
    And,
    /// Equality `==` (precedence 3, non-associative).
    Eq,
    /// Inequality `!=` (precedence 3).
    Ne,
    /// Less than `<` (precedence 3).
    Lt,
    /// Greater than `>` (precedence 3).
    Gt,
    /// Less than or equal `<=` (precedence 3).
    Le,
    /// Greater than or equal `>=` (precedence 3).
    Ge,
    /// Bitwise OR `|` (precedence 4).
    BitOr,
    /// Bitwise XOR `^` (precedence 5).
    BitXor,
    /// Bitwise AND `&` (precedence 6).
    BitAnd,
    /// Left shift `<<` (precedence 7).
    Shl,
    /// Right shift `>>` (precedence 7).
    Shr,
    /// Addition `+` (precedence 8).
    Add,
    /// Subtraction `-` (precedence 8).
    Sub,
    /// Multiplication `*` (precedence 9, highest).
    Mul,
    /// Division `/` (precedence 9).
    Div,
    /// Remainder `%` (precedence 9).
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

/// Intrinsic math/utility functions that map directly to IC10 instructions (§5.11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intrinsic {
    /// Absolute value.
    Abs,
    /// Round toward positive infinity.
    Ceil,
    /// Round toward negative infinity.
    Floor,
    /// Round to nearest integer (half-to-even).
    Round,
    /// Round toward zero.
    Trunc,
    /// Square root.
    Sqrt,
    /// Exponential (e^x).
    Exp,
    /// Natural logarithm (ln x).
    Log,
    /// Sine (radians).
    Sin,
    /// Cosine (radians).
    Cos,
    /// Tangent (radians).
    Tan,
    /// Arcsine (result in radians).
    Asin,
    /// Arccosine (result in radians).
    Acos,
    /// Arctangent (result in radians).
    Atan,
    /// Two-argument arctangent `atan2(y, x)` (result in radians).
    Atan2,
    /// Exponentiation `base.pow(exponent)`.
    Pow,
    /// Minimum of two values.
    Min,
    /// Maximum of two values.
    Max,
    /// Linear interpolation `lerp(a, b, t)` = `a + t * (b - a)`.
    Lerp,
    /// Clamp `value` to `[min, max]`.
    Clamp,
    /// Random number in `[0, 1)`. Takes no arguments.
    Rand,
    /// Returns `1.0` if the argument is NaN, `0.0` otherwise.
    IsNan,
}

/// Batch operation mode (§8.5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchMode {
    /// Average of all matching device values.
    Average,
    /// Sum of all matching device values.
    Sum,
    /// Minimum of all matching device values.
    Minimum,
    /// Maximum of all matching device values.
    Maximum,
    /// Count of matching devices.
    Contents,
}
