use std::collections::HashMap;

use crate::ir::cfg::{Function, Instruction, Operation, TempId};
use crate::ir::{BinaryOperator, Intrinsic, UnaryOperator};

/// Simplifies algebraic expressions by applying identities such as `x + 0 → x`,
/// `x * 1 → x`, and `x * 0 → 0`, and by folding constant binary/unary operations.
pub(super) fn algebraic_simplification(function: &mut Function) -> bool {
    let constants = collect_constants(function);
    let mut changed = false;

    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            match instruction {
                Instruction::Assign { operation, .. } => {
                    if let Some(new_operation) = try_simplify_operation(operation, &constants) {
                        *operation = new_operation;
                        changed = true;
                    }
                }
                Instruction::IntrinsicCall {
                    target,
                    function: intrinsic,
                    args,
                } => {
                    if let Some(replacement) =
                        try_simplify_intrinsic(*target, *intrinsic, args, &constants)
                    {
                        *instruction = replacement;
                        changed = true;
                    }
                }
                _ => {}
            }
        }
    }

    changed
}

fn collect_constants(function: &Function) -> HashMap<TempId, f64> {
    let mut constants = HashMap::new();
    for block in &function.blocks {
        for instruction in &block.instructions {
            if let Instruction::Assign {
                target,
                operation: Operation::Constant(value),
            } = instruction
            {
                constants.insert(*target, *value);
            }
        }
    }
    constants
}

fn try_simplify_operation(
    operation: &Operation,
    constants: &HashMap<TempId, f64>,
) -> Option<Operation> {
    if let Operation::Binary {
        operator,
        left,
        right,
    } = operation
    {
        let left_constant = constants.get(left).copied();
        let right_constant = constants.get(right).copied();
        if left_constant.is_some() && right_constant.is_some() {
            return None;
        }
        return try_simplify_binary(*operator, *left, left_constant, *right, right_constant);
    }
    None
}

fn try_simplify_binary(
    operator: BinaryOperator,
    left: TempId,
    left_constant: Option<f64>,
    right: TempId,
    right_constant: Option<f64>,
) -> Option<Operation> {
    match operator {
        BinaryOperator::Add => {
            if right_constant == Some(0.0) {
                return Some(Operation::Copy(left));
            }
            if left_constant == Some(0.0) {
                return Some(Operation::Copy(right));
            }
        }
        BinaryOperator::Sub => {
            if right_constant == Some(0.0) {
                return Some(Operation::Copy(left));
            }
            if left_constant == Some(0.0) {
                return Some(Operation::Unary {
                    operator: UnaryOperator::Neg,
                    operand: right,
                });
            }
        }
        BinaryOperator::Mul => {
            if right_constant == Some(1.0) {
                return Some(Operation::Copy(left));
            }
            if left_constant == Some(1.0) {
                return Some(Operation::Copy(right));
            }
            if right_constant == Some(0.0) {
                return Some(Operation::Constant(0.0));
            }
            if left_constant == Some(0.0) {
                return Some(Operation::Constant(0.0));
            }
        }
        BinaryOperator::Div => {
            if right_constant == Some(1.0) {
                return Some(Operation::Copy(left));
            }
        }
        BinaryOperator::BitOr => {
            if right_constant == Some(0.0) {
                return Some(Operation::Copy(left));
            }
            if left_constant == Some(0.0) {
                return Some(Operation::Copy(right));
            }
        }
        BinaryOperator::BitAnd => {
            if right_constant == Some(0.0) {
                return Some(Operation::Constant(0.0));
            }
            if left_constant == Some(0.0) {
                return Some(Operation::Constant(0.0));
            }
        }
        BinaryOperator::BitXor => {
            if right_constant == Some(0.0) {
                return Some(Operation::Copy(left));
            }
            if left_constant == Some(0.0) {
                return Some(Operation::Copy(right));
            }
        }
        BinaryOperator::Shl | BinaryOperator::Shr => {
            if right_constant == Some(0.0) {
                return Some(Operation::Copy(left));
            }
        }
        BinaryOperator::And => {
            if right_constant == Some(0.0) || left_constant == Some(0.0) {
                return Some(Operation::Constant(0.0));
            }
            if right_constant.is_some_and(|v| v != 0.0) {
                return Some(Operation::Copy(left));
            }
            if left_constant.is_some_and(|v| v != 0.0) {
                return Some(Operation::Copy(right));
            }
        }
        BinaryOperator::Or => {
            if right_constant.is_some_and(|v| v != 0.0) || left_constant.is_some_and(|v| v != 0.0) {
                return Some(Operation::Constant(1.0));
            }
            if right_constant == Some(0.0) {
                return Some(Operation::Copy(left));
            }
            if left_constant == Some(0.0) {
                return Some(Operation::Copy(right));
            }
        }
        _ => {}
    }
    None
}

fn try_simplify_intrinsic(
    target: TempId,
    intrinsic: Intrinsic,
    args: &[TempId],
    constants: &HashMap<TempId, f64>,
) -> Option<Instruction> {
    match (intrinsic, args) {
        (Intrinsic::Min | Intrinsic::Max, &[a, b]) if a == b => Some(Instruction::Assign {
            target,
            operation: Operation::Copy(a),
        }),
        // pow(x, 0) = 1 for all x (IEEE 754 special case, including NaN and ±inf).
        // pow(x, 1) = x for all x. pow(1, x) = 1 for all x (IEEE 754 special case).
        (Intrinsic::Pow, &[base, exponent]) => {
            if constants.get(&exponent).copied() == Some(0.0) {
                Some(Instruction::Assign {
                    target,
                    operation: Operation::Constant(1.0),
                })
            } else if constants.get(&exponent).copied() == Some(1.0) {
                Some(Instruction::Assign {
                    target,
                    operation: Operation::Copy(base),
                })
            } else if constants.get(&base).copied() == Some(1.0) {
                Some(Instruction::Assign {
                    target,
                    operation: Operation::Constant(1.0),
                })
            } else {
                None
            }
        }
        // lerp(a, a, t) = a regardless of t.
        // lerp(a, b, 0) = a; lerp(a, b, 1) = b.
        (Intrinsic::Lerp, &[a, b, t]) => {
            if a == b || constants.get(&t).copied() == Some(0.0) {
                Some(Instruction::Assign {
                    target,
                    operation: Operation::Copy(a),
                })
            } else if constants.get(&t).copied() == Some(1.0) {
                Some(Instruction::Assign {
                    target,
                    operation: Operation::Copy(b),
                })
            } else {
                None
            }
        }
        // clamp(x, v, v) = v for any x.
        (Intrinsic::Clamp, &[_, lo, hi]) if lo == hi => Some(Instruction::Assign {
            target,
            operation: Operation::Copy(lo),
        }),
        _ => None,
    }
}
