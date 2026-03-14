use std::collections::{HashMap, HashSet, VecDeque};

use crate::ast::{BinaryOperator, BuiltinFunction, Type, UnaryOperator};
use crate::cfg::{BlockId, Function, Instruction, Operation, Program, TempId, Terminator};

/// Optimize all functions in a CFG program.
///
/// Runs constant propagation, copy propagation, global value numbering,
/// and dead-code elimination in a fixpoint loop until no further
/// simplifications are possible.
pub fn optimize_program(program: &mut Program) {
    for function in &mut program.functions {
        optimize(function);
    }
}

fn optimize(function: &mut Function) {
    let mut iterations = 0;
    loop {
        let mut changed = false;
        changed |= constant_propagation(function);
        changed |= copy_propagation(function);
        changed |= global_value_numbering(function);
        changed |= dead_code_elimination(function);
        changed |= remove_unreachable_blocks(function);
        changed |= merge_empty_blocks(function);
        if !changed {
            break;
        }
        iterations += 1;
        assert!(
            iterations <= 100,
            "optimization loop failed to converge after {} iterations for function '{}'",
            iterations,
            function.name
        );
    }
}

fn instruction_dest(instruction: &Instruction) -> Option<TempId> {
    match instruction {
        Instruction::Assign { dest, .. }
        | Instruction::Phi { dest, .. }
        | Instruction::LoadDevice { dest, .. }
        | Instruction::LoadSlot { dest, .. }
        | Instruction::BatchRead { dest, .. }
        | Instruction::BuiltinCall { dest, .. } => Some(*dest),
        Instruction::Call { dest, .. } => *dest,
        Instruction::StoreDevice { .. }
        | Instruction::StoreSlot { .. }
        | Instruction::BatchWrite { .. }
        | Instruction::Sleep { .. }
        | Instruction::Yield => None,
    }
}

fn instruction_uses(instruction: &Instruction) -> Vec<TempId> {
    match instruction {
        Instruction::Assign { operation, .. } => operation_uses(operation),
        Instruction::Phi { args, .. } => args.iter().map(|&(temp, _)| temp).collect(),
        Instruction::LoadDevice { .. } => vec![],
        Instruction::StoreDevice { source, .. } => vec![*source],
        Instruction::LoadSlot { slot, .. } => vec![*slot],
        Instruction::StoreSlot { slot, source, .. } => vec![*slot, *source],
        Instruction::BatchRead { hash, .. } => vec![*hash],
        Instruction::BatchWrite { hash, value, .. } => vec![*hash, *value],
        Instruction::Call { args, .. } => args.clone(),
        Instruction::BuiltinCall { args, .. } => args.clone(),
        Instruction::Sleep { duration } => vec![*duration],
        Instruction::Yield => vec![],
    }
}

fn operation_uses(operation: &Operation) -> Vec<TempId> {
    match operation {
        Operation::Copy(source) => vec![*source],
        Operation::Constant(_) | Operation::Parameter { .. } => vec![],
        Operation::Binary { left, right, .. } => vec![*left, *right],
        Operation::Unary { operand, .. } => vec![*operand],
        Operation::Cast { operand, .. } => vec![*operand],
        Operation::Select {
            condition,
            if_true,
            if_false,
        } => vec![*condition, *if_true, *if_false],
    }
}

fn terminator_uses(terminator: &Terminator) -> Vec<TempId> {
    match terminator {
        Terminator::Branch { condition, .. } => vec![*condition],
        Terminator::Return(Some(value)) => vec![*value],
        Terminator::Jump(_) | Terminator::Return(None) | Terminator::None => vec![],
    }
}

fn has_side_effects(instruction: &Instruction) -> bool {
    matches!(
        instruction,
        Instruction::StoreDevice { .. }
            | Instruction::StoreSlot { .. }
            | Instruction::BatchWrite { .. }
            | Instruction::Call { .. }
            | Instruction::Sleep { .. }
            | Instruction::Yield
    )
}

fn build_def_map(function: &Function) -> HashMap<TempId, (usize, usize)> {
    let mut map = HashMap::new();
    for (block_index, block) in function.blocks.iter().enumerate() {
        for (instruction_index, instruction) in block.instructions.iter().enumerate() {
            if let Some(dest) = instruction_dest(instruction) {
                map.insert(dest, (block_index, instruction_index));
            }
        }
    }
    map
}

fn apply_substitutions(function: &mut Function, substitutions: &HashMap<TempId, TempId>) {
    if substitutions.is_empty() {
        return;
    }
    let resolved = resolve_substitution_chains(substitutions);
    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            substitute_in_instruction(instruction, &resolved);
        }
        substitute_in_terminator(&mut block.terminator, &resolved);
    }
}

fn resolve_substitution_chains(substitutions: &HashMap<TempId, TempId>) -> HashMap<TempId, TempId> {
    let mut resolved = HashMap::new();
    for &key in substitutions.keys() {
        let mut target = key;
        let mut visited = HashSet::new();
        while let Some(&next) = substitutions.get(&target) {
            if !visited.insert(target) {
                break;
            }
            target = next;
        }
        if target != key {
            resolved.insert(key, target);
        }
    }
    resolved
}

fn substitute_temp(temp: &mut TempId, substitutions: &HashMap<TempId, TempId>) {
    if let Some(&replacement) = substitutions.get(temp) {
        *temp = replacement;
    }
}

fn substitute_in_instruction(
    instruction: &mut Instruction,
    substitutions: &HashMap<TempId, TempId>,
) {
    match instruction {
        Instruction::Assign { operation, .. } => {
            substitute_in_operation(operation, substitutions);
        }
        Instruction::Phi { args, .. } => {
            for (temp, _) in args.iter_mut() {
                substitute_temp(temp, substitutions);
            }
        }
        Instruction::LoadDevice { .. } => {}
        Instruction::StoreDevice { source, .. } => {
            substitute_temp(source, substitutions);
        }
        Instruction::LoadSlot { slot, .. } => {
            substitute_temp(slot, substitutions);
        }
        Instruction::StoreSlot { slot, source, .. } => {
            substitute_temp(slot, substitutions);
            substitute_temp(source, substitutions);
        }
        Instruction::BatchRead { hash, .. } => {
            substitute_temp(hash, substitutions);
        }
        Instruction::BatchWrite { hash, value, .. } => {
            substitute_temp(hash, substitutions);
            substitute_temp(value, substitutions);
        }
        Instruction::Call { args, .. } => {
            for arg in args.iter_mut() {
                substitute_temp(arg, substitutions);
            }
        }
        Instruction::BuiltinCall { args, .. } => {
            for arg in args.iter_mut() {
                substitute_temp(arg, substitutions);
            }
        }
        Instruction::Sleep { duration } => {
            substitute_temp(duration, substitutions);
        }
        Instruction::Yield => {}
    }
}

fn substitute_in_operation(operation: &mut Operation, substitutions: &HashMap<TempId, TempId>) {
    match operation {
        Operation::Copy(source) => substitute_temp(source, substitutions),
        Operation::Constant(_) | Operation::Parameter { .. } => {}
        Operation::Binary { left, right, .. } => {
            substitute_temp(left, substitutions);
            substitute_temp(right, substitutions);
        }
        Operation::Unary { operand, .. } => substitute_temp(operand, substitutions),
        Operation::Cast { operand, .. } => substitute_temp(operand, substitutions),
        Operation::Select {
            condition,
            if_true,
            if_false,
        } => {
            substitute_temp(condition, substitutions);
            substitute_temp(if_true, substitutions);
            substitute_temp(if_false, substitutions);
        }
    }
}

fn substitute_in_terminator(terminator: &mut Terminator, substitutions: &HashMap<TempId, TempId>) {
    match terminator {
        Terminator::Branch { condition, .. } => substitute_temp(condition, substitutions),
        Terminator::Return(Some(value)) => substitute_temp(value, substitutions),
        Terminator::Jump(_) | Terminator::Return(None) | Terminator::None => {}
    }
}

fn constant_propagation(function: &mut Function) -> bool {
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

fn copy_propagation(function: &mut Function) -> bool {
    let mut substitutions: HashMap<TempId, TempId> = HashMap::new();

    for block in &function.blocks {
        for instruction in &block.instructions {
            match instruction {
                Instruction::Assign {
                    dest,
                    operation: Operation::Copy(source),
                } => {
                    if *dest != *source {
                        substitutions.insert(*dest, *source);
                    }
                }
                Instruction::Phi { dest, args } => {
                    if let Some(single_source) = single_phi_source(args)
                        && *dest != single_source
                    {
                        substitutions.insert(*dest, single_source);
                    }
                }
                _ => {}
            }
        }
    }

    if substitutions.is_empty() {
        return false;
    }

    apply_substitutions(function, &substitutions);

    let resolved = resolve_substitution_chains(&substitutions);
    for block in &mut function.blocks {
        block.instructions.retain(|instruction| {
            if let Some(dest) = instruction_dest(instruction) {
                !resolved.contains_key(&dest)
            } else {
                true
            }
        });
    }

    true
}

fn single_phi_source(args: &[(TempId, BlockId)]) -> Option<TempId> {
    if args.is_empty() {
        return None;
    }
    let first = args[0].0;
    if args.iter().all(|&(temp, _)| temp == first) {
        Some(first)
    } else {
        None
    }
}

fn dead_code_elimination(function: &mut Function) -> bool {
    let def_map = build_def_map(function);
    let mut live: HashSet<TempId> = HashSet::new();
    let mut worklist: VecDeque<TempId> = VecDeque::new();

    for block in &function.blocks {
        for instruction in &block.instructions {
            if has_side_effects(instruction) {
                for temp in instruction_uses(instruction) {
                    if live.insert(temp) {
                        worklist.push_back(temp);
                    }
                }
            }
        }
        for temp in terminator_uses(&block.terminator) {
            if live.insert(temp) {
                worklist.push_back(temp);
            }
        }
    }

    while let Some(temp) = worklist.pop_front() {
        if let Some(&(block_index, instruction_index)) = def_map.get(&temp) {
            let instruction = &function.blocks[block_index].instructions[instruction_index];
            for used_temp in instruction_uses(instruction) {
                if live.insert(used_temp) {
                    worklist.push_back(used_temp);
                }
            }
        }
    }

    let mut changed = false;
    for block in &mut function.blocks {
        let original_length = block.instructions.len();
        block.instructions.retain(|instruction| {
            if has_side_effects(instruction) {
                return true;
            }
            match instruction_dest(instruction) {
                Some(dest) => live.contains(&dest),
                None => true,
            }
        });
        if block.instructions.len() != original_length {
            changed = true;
        }
    }

    changed
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

fn global_value_numbering(function: &mut Function) -> bool {
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

fn remove_unreachable_blocks(function: &mut Function) -> bool {
    let reachable = compute_reachable(function);
    let unreachable: HashSet<BlockId> = function
        .blocks
        .iter()
        .map(|block| block.id)
        .filter(|id| !reachable.contains(id))
        .collect();

    if unreachable.is_empty() {
        return false;
    }

    let mut changed = false;

    for block in &mut function.blocks {
        if unreachable.contains(&block.id) {
            continue;
        }
        let original_predecessor_count = block.predecessors.len();
        block
            .predecessors
            .retain(|predecessor| !unreachable.contains(predecessor));
        if block.predecessors.len() != original_predecessor_count {
            changed = true;
        }
        for instruction in &mut block.instructions {
            if let Instruction::Phi { args, .. } = instruction {
                let original_arg_count = args.len();
                args.retain(|(_, block_id)| !unreachable.contains(block_id));
                if args.len() != original_arg_count {
                    changed = true;
                }
            }
        }
    }

    for block in &mut function.blocks {
        if unreachable.contains(&block.id)
            && (!block.instructions.is_empty()
                || !matches!(block.terminator, Terminator::None)
                || !block.successors.is_empty())
        {
            block.instructions.clear();
            block.terminator = Terminator::None;
            block.successors.clear();
            block.predecessors.clear();
            changed = true;
        }
    }

    changed
}

fn compute_reachable(function: &Function) -> HashSet<BlockId> {
    let mut reachable = HashSet::new();
    let mut worklist = vec![function.entry];
    reachable.insert(function.entry);

    while let Some(block_id) = worklist.pop() {
        for &successor in &function.blocks[block_id.0].successors {
            if reachable.insert(successor) {
                worklist.push(successor);
            }
        }
    }

    reachable
}

/// Eliminate empty pass-through blocks: blocks with no instructions and a single
/// unconditional `Jump` to some other block.
///
/// For each such block B → C, every predecessor of B is redirected to jump
/// directly to C, phi arguments in C that came from B are re-attributed to
/// B's predecessors, and B is cleared.  `function.entry` is updated when B
/// was the entry block.  Runs to fixpoint because eliminating one block may
/// expose the next pass-through in the chain.
fn merge_empty_blocks(function: &mut Function) -> bool {
    let mut changed = false;
    loop {
        let Some(block_id) = find_empty_pass_through_block(function) else {
            break;
        };
        let Terminator::Jump(target_id) = function.blocks[block_id.0].terminator else {
            unreachable!()
        };

        let predecessors: Vec<BlockId> = function.blocks[block_id.0].predecessors.clone();

        if function.entry == block_id {
            function.entry = target_id;
        }

        for &predecessor_id in &predecessors {
            let predecessor = &mut function.blocks[predecessor_id.0];
            replace_jump_target(&mut predecessor.terminator, block_id, target_id);
            for successor in predecessor.successors.iter_mut() {
                if *successor == block_id {
                    *successor = target_id;
                }
            }
            predecessor.successors.sort_unstable();
            predecessor.successors.dedup();
        }

        {
            let target = &mut function.blocks[target_id.0];
            target
                .predecessors
                .retain(|&predecessor| predecessor != block_id);
            for &predecessor_id in &predecessors {
                if !target.predecessors.contains(&predecessor_id) {
                    target.predecessors.push(predecessor_id);
                }
            }
            for instruction in &mut target.instructions {
                if let Instruction::Phi { args, .. } = instruction
                    && let Some(position) = args.iter().position(|&(_, block)| block == block_id)
                {
                    let (value, _) = args.remove(position);
                    for &predecessor_id in &predecessors {
                        if !args.iter().any(|&(_, block)| block == predecessor_id) {
                            args.push((value, predecessor_id));
                        }
                    }
                }
            }
        }

        let block = &mut function.blocks[block_id.0];
        block.predecessors.clear();
        block.successors.clear();
        block.terminator = Terminator::None;
        changed = true;
    }
    changed
}

fn find_empty_pass_through_block(function: &Function) -> Option<BlockId> {
    for block in &function.blocks {
        let Terminator::Jump(target) = block.terminator else {
            continue;
        };
        if target != block.id && block.instructions.is_empty() {
            return Some(block.id);
        }
    }
    None
}

fn replace_jump_target(terminator: &mut Terminator, old: BlockId, new: BlockId) {
    match terminator {
        Terminator::Jump(target) if *target == old => *target = new,
        Terminator::Branch {
            true_block,
            false_block,
            ..
        } => {
            if *true_block == old {
                *true_block = new;
            }
            if *false_block == old {
                *false_block = new;
            }
        }
        Terminator::Return(_) | Terminator::None | Terminator::Jump(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg;
    use crate::parser::parse;
    use crate::resolve::resolve;
    use crate::ssa;

    fn build_optimized(source: &str) -> Program {
        let mut program = build_ssa_unoptimized(source);
        optimize_program(&mut program);
        program
    }

    fn build_ssa_unoptimized(source: &str) -> Program {
        let (ast, parse_diagnostics) = parse(source);
        let errors: Vec<_> = parse_diagnostics
            .iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "parse errors: {:#?}", errors);
        let (resolved, _) = resolve(&ast)
            .unwrap_or_else(|diagnostics| panic!("resolve errors: {:#?}", diagnostics));
        let (mut program, _) = cfg::build(&resolved);
        ssa::construct_program(&mut program);
        program
    }

    fn get_function<'a>(program: &'a Program, name: &str) -> &'a Function {
        program
            .functions
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("function '{}' not found", name))
    }

    fn count_instructions(function: &Function) -> usize {
        function
            .blocks
            .iter()
            .map(|block| block.instructions.len())
            .sum()
    }

    fn has_binary_instruction(function: &Function) -> bool {
        function.blocks.iter().any(|block| {
            block.instructions.iter().any(|instruction| {
                matches!(
                    instruction,
                    Instruction::Assign {
                        operation: Operation::Binary { .. },
                        ..
                    }
                )
            })
        })
    }

    fn has_phi_instruction(function: &Function) -> bool {
        function.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Phi { .. }))
        })
    }

    fn count_constants(function: &Function) -> usize {
        function
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter())
            .filter(|instruction| {
                matches!(
                    instruction,
                    Instruction::Assign {
                        operation: Operation::Constant(_),
                        ..
                    }
                )
            })
            .count()
    }

    #[test]
    fn constant_folding_arithmetic() {
        let program = build_optimized("fn main() { let x: i53 = 3 + 4; }");
        let main = get_function(&program, "main");
        assert!(
            !has_binary_instruction(main),
            "binary instruction should be folded away"
        );
    }

    #[test]
    fn constant_folding_nested_arithmetic() {
        let program = build_optimized("fn main() { let x: i53 = (2 + 3) * (4 - 1); }");
        let main = get_function(&program, "main");
        assert!(
            !has_binary_instruction(main),
            "all arithmetic should be folded"
        );
    }

    #[test]
    fn dead_code_elimination_unused_variable() {
        let before = build_ssa_unoptimized("fn main() { let x: i53 = 5; }");
        let after = build_optimized("fn main() { let x: i53 = 5; }");
        let before_count = count_instructions(get_function(&before, "main"));
        let after_count = count_instructions(get_function(&after, "main"));
        assert!(
            after_count < before_count,
            "DCE should reduce instruction count: before={}, after={}",
            before_count,
            after_count
        );
    }

    #[test]
    fn dead_code_elimination_preserves_side_effects() {
        let program = build_optimized(
            r#"
            device sensor: d0;
            fn main() {
                sensor.Setting = 1;
            }
            "#,
        );
        let main = get_function(&program, "main");
        let has_store = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::StoreDevice { .. }))
        });
        assert!(has_store, "device store must be preserved");
    }

    #[test]
    fn copy_propagation_eliminates_copies() {
        let src = r#"fn main() {
            let x: i53 = 1;
            let y: i53 = x;
            let z: i53 = y;
        }"#;
        let before = build_ssa_unoptimized(src);
        let after = build_optimized(src);
        let before_count = count_instructions(get_function(&before, "main"));
        let after_count = count_instructions(get_function(&after, "main"));
        assert!(
            after_count < before_count,
            "copy propagation + DCE should reduce instruction count: before={}, after={}",
            before_count,
            after_count
        );
    }

    #[test]
    fn constant_branch_simplification() {
        let program = build_optimized(
            r#"
            device sensor: d0;
            fn main() {
                if true {
                    sensor.Setting = 1;
                } else {
                    sensor.Setting = 2;
                }
            }
            "#,
        );
        let main = get_function(&program, "main");
        let store_count: usize = main
            .blocks
            .iter()
            .flat_map(|b| b.instructions.iter())
            .filter(|i| matches!(i, Instruction::StoreDevice { .. }))
            .count();
        assert_eq!(
            store_count, 1,
            "dead branch should be eliminated, leaving only one store"
        );
    }

    #[test]
    fn phi_with_same_constant_folded() {
        let program = build_optimized(
            r#"fn main() {
                let mut x: i53 = 1;
                if true {
                    x = 1;
                }
                let y = x;
            }"#,
        );
        let main = get_function(&program, "main");
        assert!(
            !has_phi_instruction(main),
            "phi with identical constant arguments should be eliminated"
        );
    }

    #[test]
    fn gvn_eliminates_duplicate_constants() {
        let before = build_ssa_unoptimized(
            r#"
            device sensor: d0;
            fn main() {
                sensor.Setting = 42;
                sensor.Mode = 42;
            }
            "#,
        );
        let after = build_optimized(
            r#"
            device sensor: d0;
            fn main() {
                sensor.Setting = 42;
                sensor.Mode = 42;
            }
            "#,
        );
        let before_constants = count_constants(get_function(&before, "main"));
        let after_constants = count_constants(get_function(&after, "main"));
        assert!(
            after_constants < before_constants,
            "GVN should deduplicate identical constants: before={}, after={}",
            before_constants,
            after_constants
        );
    }

    #[test]
    fn pipeline_reduces_complex_program() {
        let before = build_ssa_unoptimized(
            r#"
            device sensor: d0;
            fn main() {
                let x: i53 = 2 + 3;
                let y: i53 = x * 2;
                let unused: i53 = 99;
                sensor.Setting = y;
            }
            "#,
        );
        let after = build_optimized(
            r#"
            device sensor: d0;
            fn main() {
                let x: i53 = 2 + 3;
                let y: i53 = x * 2;
                let unused: i53 = 99;
                sensor.Setting = y;
            }
            "#,
        );
        let before_count = count_instructions(get_function(&before, "main"));
        let after_count = count_instructions(get_function(&after, "main"));
        assert!(
            after_count < before_count,
            "optimization pipeline should reduce total instructions: before={}, after={}",
            before_count,
            after_count
        );
    }

    #[test]
    fn yield_preserved_through_optimization() {
        let program = build_optimized(
            r#"fn main() {
                loop {
                    yield;
                }
            }"#,
        );
        let main = get_function(&program, "main");
        let has_yield = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::Yield))
        });
        assert!(has_yield, "yield must be preserved");
    }

    #[test]
    fn sleep_preserved_through_optimization() {
        let program = build_optimized(
            r#"fn main() {
                sleep(1.0);
            }"#,
        );
        let main = get_function(&program, "main");
        let has_sleep = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::Sleep { .. }))
        });
        assert!(has_sleep, "sleep must be preserved");
    }

    #[test]
    fn builtin_constant_folding() {
        let program = build_optimized("fn main() { let x: f64 = sqrt(4.0); }");
        let main = get_function(&program, "main");
        let has_builtin = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::BuiltinCall { .. }))
        });
        assert!(
            !has_builtin,
            "builtin call with constant args should be folded"
        );
    }

    #[test]
    fn loop_with_device_io_preserved() {
        let program = build_optimized(
            r#"
            device sensor: d0;
            device light: d1;
            fn main() {
                loop {
                    let temp = sensor.Temperature;
                    light.Setting = temp;
                    yield;
                }
            }
            "#,
        );
        let main = get_function(&program, "main");
        let has_load = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::LoadDevice { .. }))
        });
        let has_store = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::StoreDevice { .. }))
        });
        assert!(has_load, "device load in loop must be preserved");
        assert!(has_store, "device store in loop must be preserved");
    }

    #[test]
    fn unary_constant_folding() {
        let program = build_optimized("fn main() { let x: i53 = -5; }");
        let main = get_function(&program, "main");
        let has_unary = main.blocks.iter().any(|block| {
            block.instructions.iter().any(|i| {
                matches!(
                    i,
                    Instruction::Assign {
                        operation: Operation::Unary { .. },
                        ..
                    }
                )
            })
        });
        assert!(!has_unary, "unary negation of constant should be folded");
    }

    #[test]
    fn comparison_constant_folding() {
        let program = build_optimized("fn main() { let x: bool = 3 < 5; }");
        let main = get_function(&program, "main");
        assert!(
            !has_binary_instruction(main),
            "constant comparison should be folded"
        );
    }

    mod resolve_substitution_chain_tests {
        use super::super::*;
        use std::collections::HashMap;

        fn t(n: usize) -> TempId {
            TempId(n)
        }

        fn resolve(pairs: &[(usize, usize)]) -> HashMap<TempId, TempId> {
            let map: HashMap<TempId, TempId> = pairs.iter().map(|&(k, v)| (t(k), t(v))).collect();
            resolve_substitution_chains(&map)
        }

        #[test]
        fn empty_map_returns_empty() {
            assert!(resolve(&[]).is_empty());
        }

        #[test]
        fn single_hop_preserved() {
            let result = resolve(&[(1, 2)]);
            assert_eq!(result.get(&t(1)), Some(&t(2)));
            assert_eq!(result.len(), 1);
        }

        #[test]
        fn self_mapping_omitted() {
            let result = resolve(&[(1, 1)]);
            assert!(
                result.is_empty(),
                "self-mapping should be omitted from result"
            );
        }

        #[test]
        fn two_hop_chain_collapsed() {
            // 1 -> 2 -> 3 should produce {1 -> 3, 2 -> 3}
            let result = resolve(&[(1, 2), (2, 3)]);
            assert_eq!(result.get(&t(1)), Some(&t(3)));
            assert_eq!(result.get(&t(2)), Some(&t(3)));
        }

        #[test]
        fn three_hop_chain_collapsed() {
            // 1 -> 2 -> 3 -> 4 should produce {1 -> 4, 2 -> 4, 3 -> 4}
            let result = resolve(&[(1, 2), (2, 3), (3, 4)]);
            assert_eq!(result.get(&t(1)), Some(&t(4)));
            assert_eq!(result.get(&t(2)), Some(&t(4)));
            assert_eq!(result.get(&t(3)), Some(&t(4)));
        }

        #[test]
        fn converging_chains_resolved_to_same_target() {
            // 1 -> 2 -> 3, and 4 -> 2 both converge on 3
            let result = resolve(&[(1, 2), (2, 3), (4, 2)]);
            assert_eq!(result.get(&t(1)), Some(&t(3)));
            assert_eq!(result.get(&t(2)), Some(&t(3)));
            assert_eq!(result.get(&t(4)), Some(&t(3)));
        }

        #[test]
        fn cycle_terminates_without_panic() {
            // 1 -> 2 -> 1: should not loop forever. Neither member has a
            // canonical root outside the cycle, so both resolve back to
            // themselves and are omitted.
            let result = resolve(&[(1, 2), (2, 1)]);
            assert!(
                !result.contains_key(&t(1)) && !result.contains_key(&t(2)),
                "entries in a pure cycle should not produce cross-mappings"
            );
        }

        #[test]
        fn tail_into_cycle_terminates() {
            // 1 -> 2 -> 3 -> 2 (tail leading into a cycle)
            // Key 1: follows 1->2->3->2, hits cycle, terminates at 2 (re-visited). result: 1->2
            // Key 2: follows 2->3->2, hits cycle, terminates at 2. 2==key, omitted.
            // Key 3: follows 3->2->3, hits cycle, terminates at 3. 3==key, omitted.
            let result = resolve(&[(1, 2), (2, 3), (3, 2)]);
            assert_eq!(result.get(&t(1)), Some(&t(2)));
            assert!(!result.contains_key(&t(2)));
            assert!(!result.contains_key(&t(3)));
        }
    }
}
