use std::collections::HashMap;

use crate::ast::{BinaryOperator, Type, UnaryOperator};
use crate::cfg::{Function, Instruction, Operation, TempId};

use super::utilities::apply_substitutions;

pub(super) fn global_value_numbering(function: &mut Function) -> bool {
    let mut substitutions: HashMap<TempId, TempId> = HashMap::new();

    for block in &function.blocks {
        let mut value_table: HashMap<ValueExpression, TempId> = HashMap::new();

        for instruction in &block.instructions {
            if let Instruction::Assign { dest, operation } = instruction
                && let Some(expression) = operation_to_value_expression(operation)
            {
                if let Some(&leader) = value_table.get(&expression) {
                    substitutions.insert(*dest, leader);
                } else {
                    value_table.insert(expression, *dest);
                }
            }
        }
    }

    if substitutions.is_empty() {
        return false;
    }

    apply_substitutions(function, &substitutions);
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ValueExpression {
    Constant(u64),
    Binary {
        operator: BinaryOperator,
        left: TempId,
        right: TempId,
    },
    Unary {
        operator: UnaryOperator,
        operand: TempId,
    },
    Cast {
        operand: TempId,
        target_type: Type,
        source_type: Type,
    },
    Select {
        condition: TempId,
        if_true: TempId,
        if_false: TempId,
    },
}

fn operation_to_value_expression(operation: &Operation) -> Option<ValueExpression> {
    match operation {
        Operation::Constant(value) => Some(ValueExpression::Constant(value.to_bits())),
        Operation::Binary {
            operator,
            left,
            right,
        } => {
            let (left, right) = if is_commutative(*operator) && right.0 < left.0 {
                (*right, *left)
            } else {
                (*left, *right)
            };
            Some(ValueExpression::Binary {
                operator: *operator,
                left,
                right,
            })
        }
        Operation::Unary { operator, operand } => Some(ValueExpression::Unary {
            operator: *operator,
            operand: *operand,
        }),
        Operation::Cast {
            operand,
            target_type,
            source_type,
        } => Some(ValueExpression::Cast {
            operand: *operand,
            target_type: *target_type,
            source_type: *source_type,
        }),
        Operation::Select {
            condition,
            if_true,
            if_false,
        } => Some(ValueExpression::Select {
            condition: *condition,
            if_true: *if_true,
            if_false: *if_false,
        }),
        Operation::Copy(_) => None,
        Operation::Parameter { .. } => None,
    }
}

fn is_commutative(operator: BinaryOperator) -> bool {
    matches!(
        operator,
        BinaryOperator::Add
            | BinaryOperator::Mul
            | BinaryOperator::Eq
            | BinaryOperator::Ne
            | BinaryOperator::And
            | BinaryOperator::Or
            | BinaryOperator::BitAnd
            | BinaryOperator::BitOr
            | BinaryOperator::BitXor
    )
}
