use std::collections::{HashSet, VecDeque};

use crate::cfg::{Function, TempId};

use super::utilities::{build_def_map, has_side_effects, instruction_dest, instruction_uses, terminator_uses};

pub(super) fn dead_code_elimination(function: &mut Function) -> bool {
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
