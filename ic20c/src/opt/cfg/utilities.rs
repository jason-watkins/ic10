use std::collections::{HashMap, HashSet};

use crate::ir::cfg::{Function, Instruction, Operation, TempId, Terminator};

/// Returns the `TempId` defined by this instruction, if any.
///
/// Instructions that only produce side effects (stores, sleep, yield) return `None`.
pub(super) fn instruction_target(instruction: &Instruction) -> Option<TempId> {
    match instruction {
        Instruction::Assign { target, .. }
        | Instruction::Phi { target, .. }
        | Instruction::LoadDevice { target, .. }
        | Instruction::LoadSlot { target, .. }
        | Instruction::BatchRead { target, .. }
        | Instruction::IntrinsicCall { target, .. }
        | Instruction::LoadStatic { target, .. } => Some(*target),
        Instruction::Call { target, .. } => *target,
        Instruction::StoreDevice { .. }
        | Instruction::StoreSlot { .. }
        | Instruction::BatchWrite { .. }
        | Instruction::StoreStatic { .. }
        | Instruction::Sleep { .. }
        | Instruction::Yield => None,
    }
}

/// Collects all `TempId`s read by this instruction.
pub(super) fn instruction_uses(instruction: &Instruction) -> Vec<TempId> {
    match instruction {
        Instruction::Assign { operation, .. } => operation_uses(operation),
        Instruction::Phi { args, .. } => args.iter().map(|&(temp, _)| temp).collect(),
        Instruction::LoadDevice { .. } => vec![],
        Instruction::StoreDevice { source, .. } => vec![*source],
        Instruction::LoadStatic { .. } => vec![],
        Instruction::StoreStatic { source, .. } => vec![*source],
        Instruction::LoadSlot { slot, .. } => vec![*slot],
        Instruction::StoreSlot { slot, source, .. } => vec![*slot, *source],
        Instruction::BatchRead { hash, .. } => vec![*hash],
        Instruction::BatchWrite { hash, value, .. } => vec![*hash, *value],
        Instruction::Call { args, .. } => args.clone(),
        Instruction::IntrinsicCall { args, .. } => args.clone(),
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

/// Collects all `TempId`s read by this terminator.
pub(super) fn terminator_uses(terminator: &Terminator) -> Vec<TempId> {
    match terminator {
        Terminator::Branch { condition, .. } => vec![*condition],
        Terminator::Return(Some(value)) => vec![*value],
        Terminator::Jump(_) | Terminator::Return(None) | Terminator::None => vec![],
    }
}

/// Returns `true` if the instruction has observable side effects (device/static
/// stores, function calls, sleep, yield) and therefore must not be removed by
/// dead code elimination.
pub(super) fn has_side_effects(instruction: &Instruction) -> bool {
    matches!(
        instruction,
        Instruction::StoreDevice { .. }
            | Instruction::StoreSlot { .. }
            | Instruction::BatchWrite { .. }
            | Instruction::StoreStatic { .. }
            | Instruction::Call { .. }
            | Instruction::Sleep { .. }
            | Instruction::Yield
    )
}

/// Builds a map from each defined `TempId` to its definition site as a
/// `(block_index, instruction_index)` pair.
pub(super) fn build_def_map(function: &Function) -> HashMap<TempId, (usize, usize)> {
    let mut map = HashMap::new();
    for (block_index, block) in function.blocks.iter().enumerate() {
        for (instruction_index, instruction) in block.instructions.iter().enumerate() {
            if let Some(target) = instruction_target(instruction) {
                map.insert(target, (block_index, instruction_index));
            }
        }
    }
    map
}

/// Rewrites every use of a substituted `TempId` in the function according to
/// the given substitution map, first resolving transitive chains (A→B→C
/// becomes A→C).
pub(super) fn apply_substitutions(
    function: &mut Function,
    substitutions: &HashMap<TempId, TempId>,
) {
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

/// Resolves transitive substitution chains so that every key maps directly to
/// its final target. Cycles are detected and broken.
pub(super) fn resolve_substitution_chains(
    substitutions: &HashMap<TempId, TempId>,
) -> HashMap<TempId, TempId> {
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

/// Applies the substitution map to every operand `TempId` inside the instruction.
/// Target (defined) temps are not rewritten.
pub(super) fn substitute_in_instruction(
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
        Instruction::LoadStatic { .. } => {}
        Instruction::StoreStatic { source, .. } => {
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
        Instruction::IntrinsicCall { args, .. } => {
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

/// Applies the substitution map to the condition operand of a `Branch`
/// terminator or the return value of a `Return`.
pub(super) fn substitute_in_terminator(
    terminator: &mut Terminator,
    substitutions: &HashMap<TempId, TempId>,
) {
    match terminator {
        Terminator::Branch { condition, .. } => substitute_temp(condition, substitutions),
        Terminator::Return(Some(value)) => substitute_temp(value, substitutions),
        Terminator::Jump(_) | Terminator::Return(None) | Terminator::None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let result = resolve(&[(1, 2), (2, 3)]);
        assert_eq!(result.get(&t(1)), Some(&t(3)));
        assert_eq!(result.get(&t(2)), Some(&t(3)));
    }

    #[test]
    fn three_hop_chain_collapsed() {
        let result = resolve(&[(1, 2), (2, 3), (3, 4)]);
        assert_eq!(result.get(&t(1)), Some(&t(4)));
        assert_eq!(result.get(&t(2)), Some(&t(4)));
        assert_eq!(result.get(&t(3)), Some(&t(4)));
    }

    #[test]
    fn converging_chains_resolved_to_same_target() {
        let result = resolve(&[(1, 2), (2, 3), (4, 2)]);
        assert_eq!(result.get(&t(1)), Some(&t(3)));
        assert_eq!(result.get(&t(2)), Some(&t(3)));
        assert_eq!(result.get(&t(4)), Some(&t(3)));
    }

    #[test]
    fn cycle_terminates_without_panic() {
        let result = resolve(&[(1, 2), (2, 1)]);
        assert!(
            !result.contains_key(&t(1)) && !result.contains_key(&t(2)),
            "entries in a pure cycle should not produce cross-mappings"
        );
    }

    #[test]
    fn tail_into_cycle_terminates() {
        let result = resolve(&[(1, 2), (2, 3), (3, 2)]);
        assert_eq!(result.get(&t(1)), Some(&t(2)));
        assert!(!result.contains_key(&t(2)));
        assert!(!result.contains_key(&t(3)));
    }
}
