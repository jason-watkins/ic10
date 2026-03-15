use std::collections::HashMap;

use crate::ir::cfg::{BlockId, Function, Instruction, Operation, TempId, Terminator};
use crate::ir::{BinaryOperator, BuiltinFunction, Type, UnaryOperator};

use super::utilities::instruction_dest;

pub(super) fn constant_propagation(function: &mut Function) -> bool {
    let mut constants: HashMap<TempId, f64> = HashMap::new();
    let mut changed = false;

    for block in &function.blocks {
        for instruction in &block.instructions {
            if let Instruction::Assign {
                dest,
                operation: Operation::Constant(value),
            } = instruction
            {
                constants.insert(*dest, *value);
            }
        }
    }

    loop {
        let mut new_found = false;
        for block in &function.blocks {
            for instruction in &block.instructions {
                let dest = match instruction_dest(instruction) {
                    Some(d) => d,
                    None => continue,
                };
                if constants.contains_key(&dest) {
                    continue;
                }
                if let Some(value) = try_evaluate_constant(instruction, &constants) {
                    constants.insert(dest, value);
                    new_found = true;
                }
            }
        }
        if !new_found {
            break;
        }
    }

    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            if let Some(dest) = instruction_dest(instruction)
                && let Some(&value) = constants.get(&dest)
                && !matches!(
                    instruction,
                    Instruction::Assign {
                        operation: Operation::Constant(_),
                        ..
                    }
                )
            {
                *instruction = Instruction::Assign {
                    dest,
                    operation: Operation::Constant(value),
                };
                changed = true;
            }
        }
    }

    let mut branch_changes: Vec<(usize, BlockId, Option<BlockId>)> = Vec::new();
    for block_index in 0..function.blocks.len() {
        if let Terminator::Branch {
            condition,
            true_block,
            false_block,
        } = &function.blocks[block_index].terminator
            && let Some(&value) = constants.get(condition)
        {
            let (target, other) = if value != 0.0 {
                (*true_block, *false_block)
            } else {
                (*false_block, *true_block)
            };
            let removed = if target != other { Some(other) } else { None };
            branch_changes.push((block_index, target, removed));
        }
    }

    for (block_index, target, removed) in branch_changes {
        let this_block_id = function.blocks[block_index].id;
        function.blocks[block_index].terminator = Terminator::Jump(target);

        match removed {
            Some(removed_block) => {
                function.blocks[block_index]
                    .successors
                    .retain(|s| *s != removed_block);
                function.blocks[removed_block.0]
                    .predecessors
                    .retain(|p| *p != this_block_id);
                for instruction in &mut function.blocks[removed_block.0].instructions {
                    if let Instruction::Phi { args, .. } = instruction {
                        args.retain(|(_, block)| *block != this_block_id);
                    }
                }
            }
            None => {
                function.blocks[block_index].successors.sort();
                function.blocks[block_index].successors.dedup();
                let predecessors = &mut function.blocks[target.0].predecessors;
                if let Some(position) = predecessors.iter().rposition(|p| *p == this_block_id) {
                    predecessors.remove(position);
                }
            }
        }
        changed = true;
    }

    changed
}

fn try_evaluate_constant(
    instruction: &Instruction,
    constants: &HashMap<TempId, f64>,
) -> Option<f64> {
    match instruction {
        Instruction::Assign { operation, .. } => match operation {
            Operation::Constant(value) => Some(*value),
            Operation::Parameter { .. } => None,
            Operation::Copy(source) => constants.get(source).copied(),
            Operation::Binary {
                operator,
                left,
                right,
            } => {
                let left_value = constants.get(left)?;
                let right_value = constants.get(right)?;
                try_fold_binary(*operator, *left_value, *right_value)
            }
            Operation::Unary { operator, operand } => {
                let value = constants.get(operand)?;
                try_fold_unary(*operator, *value)
            }
            Operation::Cast {
                operand,
                target_type,
                source_type,
            } => {
                let value = constants.get(operand)?;
                Some(fold_cast(*value, *source_type, *target_type))
            }
            Operation::Select {
                condition,
                if_true,
                if_false,
            } => {
                if let Some(&condition_value) = constants.get(condition) {
                    if condition_value != 0.0 {
                        constants.get(if_true).copied()
                    } else {
                        constants.get(if_false).copied()
                    }
                } else {
                    let true_value = constants.get(if_true)?;
                    let false_value = constants.get(if_false)?;
                    if true_value.to_bits() == false_value.to_bits() {
                        Some(*true_value)
                    } else {
                        None
                    }
                }
            }
        },
        Instruction::Phi { args, .. } => {
            if args.is_empty() {
                return None;
            }
            let first_value = constants.get(&args[0].0)?;
            for &(temp, _) in &args[1..] {
                let value = constants.get(&temp)?;
                if value.to_bits() != first_value.to_bits() {
                    return None;
                }
            }
            Some(*first_value)
        }
        Instruction::BuiltinCall { function, args, .. } => {
            let constant_args: Option<Vec<f64>> =
                args.iter().map(|a| constants.get(a).copied()).collect();
            let constant_args = constant_args?;
            try_fold_builtin(*function, &constant_args)
        }
        _ => None,
    }
}

fn try_fold_binary(operator: BinaryOperator, left: f64, right: f64) -> Option<f64> {
    let result = match operator {
        BinaryOperator::Add => left + right,
        BinaryOperator::Sub => left - right,
        BinaryOperator::Mul => left * right,
        BinaryOperator::Div => left / right,
        BinaryOperator::Rem => left % right,
        BinaryOperator::Eq => {
            if left == right {
                1.0
            } else {
                0.0
            }
        }
        BinaryOperator::Ne => {
            if left != right {
                1.0
            } else {
                0.0
            }
        }
        BinaryOperator::Lt => {
            if left < right {
                1.0
            } else {
                0.0
            }
        }
        BinaryOperator::Gt => {
            if left > right {
                1.0
            } else {
                0.0
            }
        }
        BinaryOperator::Le => {
            if left <= right {
                1.0
            } else {
                0.0
            }
        }
        BinaryOperator::Ge => {
            if left >= right {
                1.0
            } else {
                0.0
            }
        }
        BinaryOperator::And => {
            if left != 0.0 && right != 0.0 {
                1.0
            } else {
                0.0
            }
        }
        BinaryOperator::Or => {
            if left != 0.0 || right != 0.0 {
                1.0
            } else {
                0.0
            }
        }
        BinaryOperator::BitAnd => ((left as i64) & (right as i64)) as f64,
        BinaryOperator::BitOr => ((left as i64) | (right as i64)) as f64,
        BinaryOperator::BitXor => ((left as i64) ^ (right as i64)) as f64,
        BinaryOperator::Shl => {
            let shift = right as i64;
            if !(0..=63).contains(&shift) {
                return None;
            }
            ((left as i64).wrapping_shl(shift as u32)) as f64
        }
        BinaryOperator::Shr => {
            let shift = right as i64;
            if !(0..=63).contains(&shift) {
                return None;
            }
            ((left as i64).wrapping_shr(shift as u32)) as f64
        }
    };
    Some(result)
}

fn try_fold_unary(operator: UnaryOperator, operand: f64) -> Option<f64> {
    let result = match operator {
        UnaryOperator::Neg => -operand,
        UnaryOperator::Not => {
            if operand == 0.0 {
                1.0
            } else {
                0.0
            }
        }
        UnaryOperator::BitNot => (!(operand as i64)) as f64,
    };
    Some(result)
}

fn fold_cast(value: f64, source: Type, target: Type) -> f64 {
    match (source, target) {
        (Type::I53, Type::F64)
        | (Type::F64, Type::F64)
        | (Type::I53, Type::I53)
        | (Type::Bool, Type::Bool)
        | (Type::Bool, Type::I53)
        | (Type::Bool, Type::F64) => value,
        (Type::F64, Type::I53) => value.trunc(),
        (Type::I53, Type::Bool) | (Type::F64, Type::Bool) => {
            unreachable!("cast to bool is a compile error and should have been rejected by resolve")
        }
        (Type::Unit, _) | (_, Type::Unit) => {
            unreachable!(
                "unit type should not appear in SSA and should have been rejected by resolve"
            )
        }
    }
}

fn try_fold_builtin(builtin: BuiltinFunction, args: &[f64]) -> Option<f64> {
    match builtin {
        BuiltinFunction::Rand => None,
        BuiltinFunction::Abs if args.len() == 1 => Some(args[0].abs()),
        BuiltinFunction::Ceil if args.len() == 1 => Some(args[0].ceil()),
        BuiltinFunction::Floor if args.len() == 1 => Some(args[0].floor()),
        BuiltinFunction::Round if args.len() == 1 => Some(args[0].round()),
        BuiltinFunction::Trunc if args.len() == 1 => Some(args[0].trunc()),
        BuiltinFunction::Sqrt if args.len() == 1 => Some(args[0].sqrt()),
        BuiltinFunction::Exp if args.len() == 1 => Some(args[0].exp()),
        BuiltinFunction::Log if args.len() == 1 => Some(args[0].ln()),
        BuiltinFunction::Sin if args.len() == 1 => Some(args[0].sin()),
        BuiltinFunction::Cos if args.len() == 1 => Some(args[0].cos()),
        BuiltinFunction::Tan if args.len() == 1 => Some(args[0].tan()),
        BuiltinFunction::Asin if args.len() == 1 => Some(args[0].asin()),
        BuiltinFunction::Acos if args.len() == 1 => Some(args[0].acos()),
        BuiltinFunction::Atan if args.len() == 1 => Some(args[0].atan()),
        BuiltinFunction::Atan2 if args.len() == 2 => Some(args[0].atan2(args[1])),
        BuiltinFunction::Pow if args.len() == 2 => Some(args[0].powf(args[1])),
        BuiltinFunction::Min if args.len() == 2 => Some(args[0].min(args[1])),
        BuiltinFunction::Max if args.len() == 2 => Some(args[0].max(args[1])),
        BuiltinFunction::Lerp if args.len() == 3 => Some(args[0] + (args[1] - args[0]) * args[2]),
        BuiltinFunction::Clamp if args.len() == 3 => Some(args[0].max(args[1]).min(args[2])),
        _ => None,
    }
}
