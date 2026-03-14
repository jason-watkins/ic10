use std::collections::HashMap;

use crate::cfg::{BlockId, Function, Instruction, Operation, TempId};

use super::utilities::{apply_substitutions, instruction_dest, resolve_substitution_chains};

pub(super) fn copy_propagation(function: &mut Function) -> bool {
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
