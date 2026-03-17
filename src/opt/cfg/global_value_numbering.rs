use std::collections::HashMap;

use crate::ir::cfg::{BlockId, Function, Instruction, Operation, TempId};
use crate::ir::{BinaryOperator, Type, UnaryOperator};

use super::utilities::apply_substitutions;

pub(super) fn global_value_numbering(function: &mut Function) -> bool {
    let mut substitutions: HashMap<TempId, TempId> = HashMap::new();
    let mut value_table: HashMap<ValueExpression, TempId> = HashMap::new();
    let children = function.dominator_tree_children();

    walk_dominator_tree(
        function,
        function.entry,
        &children,
        &mut value_table,
        &mut substitutions,
    );

    if substitutions.is_empty() {
        return false;
    }

    apply_substitutions(function, &substitutions);
    true
}

fn walk_dominator_tree(
    function: &Function,
    block_id: BlockId,
    children: &HashMap<BlockId, Vec<BlockId>>,
    value_table: &mut HashMap<ValueExpression, TempId>,
    substitutions: &mut HashMap<TempId, TempId>,
) {
    let block = &function.blocks[block_id.0];
    let mut added: Vec<ValueExpression> = Vec::new();

    for instruction in &block.instructions {
        if let Instruction::Assign { dest, operation } = instruction
            && let Some(expression) = operation_to_value_expression(operation)
        {
            if let Some(&leader) = value_table.get(&expression) {
                substitutions.insert(*dest, leader);
            } else {
                value_table.insert(expression.clone(), *dest);
                added.push(expression);
            }
        }
    }

    if let Some(block_children) = children.get(&block_id) {
        for &child in block_children {
            walk_dominator_tree(function, child, children, value_table, substitutions);
        }
    }

    for key in added {
        value_table.remove(&key);
    }
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
