use std::collections::HashMap;

use crate::ir::cfg::{BlockId, Function, Instruction, Operation, TempId};

use super::utilities::{apply_substitutions, instruction_target, resolve_substitution_chains};

pub(super) fn copy_propagation(function: &mut Function) -> bool {
    let mut substitutions: HashMap<TempId, TempId> = HashMap::new();

    for block in &function.blocks {
        for instruction in &block.instructions {
            match instruction {
                Instruction::Assign {
                    target,
                    operation: Operation::Copy(source),
                } => {
                    if *target != *source {
                        substitutions.insert(*target, *source);
                    }
                }
                Instruction::Phi { target, args } => {
                    if let Some(single_source) = single_phi_source(args)
                        && *target != single_source
                    {
                        substitutions.insert(*target, single_source);
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
            if let Some(target) = instruction_target(instruction) {
                !resolved.contains_key(&target)
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
