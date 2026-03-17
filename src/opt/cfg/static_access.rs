use std::collections::HashMap;

use crate::ir::bound::StaticId;
use crate::ir::cfg::{Function, Instruction, Operation, TempId};

/// Per-block state tracking the most recent known value for each static variable.
///
/// A `Known::Loaded(temp)` entry means `temp` holds the current value of a static
/// and can be reused instead of emitting another `LoadStatic`. A `Known::Stored(temp)`
/// entry means we just stored `temp` into the static, so a subsequent load can forward
/// that value directly.
enum Known {
    Loaded(TempId),
    Stored(TempId),
}

/// Optimizes static variable loads and stores both within and across basic blocks.
///
/// Cross-block: a forward dataflow analysis (available-expressions with intersection
/// at join points) computes which static values are provably available at each block
/// entry. A `LoadStatic` whose static is already available is replaced with a `Copy`.
///
/// Intra-block optimizations applied on top of the cross-block initial state:
///
/// 1. **Redundant load elimination** — if a static was already loaded (locally or
///    from a predecessor) and nothing has invalidated the cached value, replace the
///    second load with a copy.
/// 2. **Store-to-load forwarding** — if a value was just stored to a static, a
///    subsequent load returns that value directly (copy instead of `get db`).
/// 3. **Dead store elimination** — if a static is stored twice with no intervening
///    load or invalidation, the first store is removed.
///
/// An entry is invalidated by `Call` (the callee may access any static), `Yield`,
/// or `Sleep` (control returns to the runtime).
pub(super) fn optimize_static_access(function: &mut Function) -> bool {
    let available_in = compute_available_in(function);
    let mut changed = false;

    for block in &mut function.blocks {
        let mut known: HashMap<StaticId, Known> = available_in[block.id.0]
            .iter()
            .map(|(&static_id, &temp)| (static_id, Known::Loaded(temp)))
            .collect();
        // Indices of StoreStatic instructions that are candidates for dead-store
        // elimination. Cleared when the stored static is read or invalidated.
        let mut last_store_index: HashMap<StaticId, usize> = HashMap::new();
        let mut dead_indices: Vec<usize> = Vec::new();

        for index in 0..block.instructions.len() {
            match &block.instructions[index] {
                Instruction::LoadStatic { dest, static_id } => {
                    let static_id = *static_id;
                    let dest = *dest;
                    if let Some(entry) = known.get(&static_id) {
                        let source = match *entry {
                            Known::Loaded(temp) | Known::Stored(temp) => temp,
                        };
                        block.instructions[index] = Instruction::Assign {
                            dest,
                            operation: Operation::Copy(source),
                        };
                        changed = true;
                    }
                    // Whether rewritten or not, record that `dest` now holds this static.
                    known.insert(static_id, Known::Loaded(dest));
                    // A load observes the static, so the preceding store is no longer dead.
                    last_store_index.remove(&static_id);
                }

                Instruction::StoreStatic { static_id, source } => {
                    let static_id = *static_id;
                    let source = *source;
                    // If there's an earlier store to the same static that hasn't been
                    // observed, it's dead.
                    if let Some(prev_index) = last_store_index.remove(&static_id) {
                        dead_indices.push(prev_index);
                        changed = true;
                    }
                    known.insert(static_id, Known::Stored(source));
                    last_store_index.insert(static_id, index);
                }

                Instruction::Call { .. } | Instruction::Yield | Instruction::Sleep { .. } => {
                    known.clear();
                    last_store_index.clear();
                }

                _ => {}
            }
        }

        if !dead_indices.is_empty() {
            dead_indices.sort_unstable();
            let mut remove_set = dead_indices
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>();
            block.instructions = block
                .instructions
                .drain(..)
                .enumerate()
                .filter(|(i, _)| !remove_set.remove(i))
                .map(|(_, instruction)| instruction)
                .collect();
        }
    }

    changed
}

/// Forward dataflow analysis computing available static values at each block entry.
///
/// The domain is `HashMap<StaticId, TempId>` — a static is available if every path
/// from the function entry to the block provides the same temp for that static.
/// The meet operator is intersection (keeping only entries that agree on both key
/// and value across all predecessors). `None` represents the top element (all statics
/// available), used as the initial state for non-entry blocks.
fn compute_available_in(function: &Function) -> Vec<HashMap<StaticId, TempId>> {
    let block_count = function.blocks.len();
    let mut available_in: Vec<Option<HashMap<StaticId, TempId>>> = vec![None; block_count];
    let mut available_out: Vec<Option<HashMap<StaticId, TempId>>> = vec![None; block_count];

    let entry = function.entry.0;
    available_in[entry] = Some(HashMap::new());
    available_out[entry] = Some(transfer(
        &function.blocks[entry].instructions,
        &HashMap::new(),
    ));

    loop {
        let mut progress = false;
        for block_index in 0..block_count {
            if block_index == entry {
                continue;
            }

            let predecessors = &function.blocks[block_index].predecessors;
            if predecessors.is_empty() {
                if available_in[block_index].is_none() {
                    available_in[block_index] = Some(HashMap::new());
                    available_out[block_index] = Some(transfer(
                        &function.blocks[block_index].instructions,
                        &HashMap::new(),
                    ));
                    progress = true;
                }
                continue;
            }

            let mut new_in: Option<HashMap<StaticId, TempId>> = None;
            let mut all_top = true;
            for &predecessor in predecessors {
                if let Some(out) = &available_out[predecessor.0] {
                    all_top = false;
                    new_in = Some(match new_in {
                        None => out.clone(),
                        Some(accumulated) => intersect(&accumulated, out),
                    });
                }
            }

            if all_top {
                continue;
            }

            if available_in[block_index] == new_in {
                continue;
            }

            let concrete = new_in.as_ref().cloned().unwrap_or_default();
            available_out[block_index] = Some(transfer(
                &function.blocks[block_index].instructions,
                &concrete,
            ));
            available_in[block_index] = new_in;
            progress = true;
        }
        if !progress {
            break;
        }
    }

    available_in
        .into_iter()
        .map(|option| option.unwrap_or_default())
        .collect()
}

fn transfer(
    instructions: &[Instruction],
    available_in: &HashMap<StaticId, TempId>,
) -> HashMap<StaticId, TempId> {
    let mut available = available_in.clone();
    for instruction in instructions {
        match instruction {
            Instruction::LoadStatic { dest, static_id } => {
                available.insert(*static_id, *dest);
            }
            Instruction::StoreStatic { static_id, source } => {
                available.insert(*static_id, *source);
            }
            Instruction::Call { .. } | Instruction::Yield | Instruction::Sleep { .. } => {
                available.clear();
            }
            _ => {}
        }
    }
    available
}

fn intersect(
    a: &HashMap<StaticId, TempId>,
    b: &HashMap<StaticId, TempId>,
) -> HashMap<StaticId, TempId> {
    a.iter()
        .filter(|&(&static_id, &temp)| b.get(&static_id) == Some(&temp))
        .map(|(&static_id, &temp)| (static_id, temp))
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::ir::bound::StaticId;
    use crate::ir::bound::SymbolId;
    use crate::ir::cfg::{
        BasicBlock, BlockId, BlockRole, Function, Instruction, Operation, TempId, Terminator,
    };
    use std::collections::HashMap;

    use super::optimize_static_access;

    fn make_function(instructions: Vec<Instruction>) -> Function {
        Function {
            name: "test".to_string(),
            symbol_id: SymbolId(0),
            parameters: Vec::new(),
            return_type: None,
            blocks: vec![BasicBlock {
                id: BlockId(0),
                role: BlockRole::Entry,
                instructions,
                terminator: Terminator::Return(None),
                predecessors: Vec::new(),
                successors: Vec::new(),
            }],
            entry: BlockId(0),
            variable_definitions: HashMap::new(),
            variable_temps: HashMap::new(),
            immediate_dominators: HashMap::new(),
            dominance_frontiers: HashMap::new(),
            next_temp: 100,
        }
    }

    #[test]
    fn redundant_load_eliminated() {
        let mut function = make_function(vec![
            Instruction::LoadStatic {
                dest: TempId(0),
                static_id: StaticId(0),
            },
            Instruction::LoadStatic {
                dest: TempId(1),
                static_id: StaticId(0),
            },
        ]);
        let changed = optimize_static_access(&mut function);
        assert!(changed);
        assert!(matches!(
            &function.blocks[0].instructions[1],
            Instruction::Assign {
                dest: TempId(1),
                operation: Operation::Copy(TempId(0))
            }
        ));
    }

    #[test]
    fn store_to_load_forwarded() {
        let mut function = make_function(vec![
            Instruction::Assign {
                dest: TempId(0),
                operation: Operation::Constant(42.0),
            },
            Instruction::StoreStatic {
                static_id: StaticId(0),
                source: TempId(0),
            },
            Instruction::LoadStatic {
                dest: TempId(1),
                static_id: StaticId(0),
            },
        ]);
        let changed = optimize_static_access(&mut function);
        assert!(changed);
        assert!(matches!(
            &function.blocks[0].instructions[2],
            Instruction::Assign {
                dest: TempId(1),
                operation: Operation::Copy(TempId(0))
            }
        ));
    }

    #[test]
    fn dead_store_eliminated() {
        let mut function = make_function(vec![
            Instruction::Assign {
                dest: TempId(0),
                operation: Operation::Constant(1.0),
            },
            Instruction::StoreStatic {
                static_id: StaticId(0),
                source: TempId(0),
            },
            Instruction::Assign {
                dest: TempId(1),
                operation: Operation::Constant(2.0),
            },
            Instruction::StoreStatic {
                static_id: StaticId(0),
                source: TempId(1),
            },
        ]);
        let changed = optimize_static_access(&mut function);
        assert!(changed);
        assert_eq!(function.blocks[0].instructions.len(), 3);
        assert!(matches!(
            &function.blocks[0].instructions[2],
            Instruction::StoreStatic {
                static_id: StaticId(0),
                source: TempId(1)
            }
        ));
    }

    #[test]
    fn call_invalidates_known_values() {
        let mut function = make_function(vec![
            Instruction::LoadStatic {
                dest: TempId(0),
                static_id: StaticId(0),
            },
            Instruction::Call {
                dest: None,
                function: SymbolId(1),
                args: Vec::new(),
            },
            Instruction::LoadStatic {
                dest: TempId(1),
                static_id: StaticId(0),
            },
        ]);
        let changed = optimize_static_access(&mut function);
        assert!(!changed);
        assert!(matches!(
            &function.blocks[0].instructions[2],
            Instruction::LoadStatic {
                dest: TempId(1),
                static_id: StaticId(0)
            }
        ));
    }

    #[test]
    fn yield_invalidates_known_values() {
        let mut function = make_function(vec![
            Instruction::LoadStatic {
                dest: TempId(0),
                static_id: StaticId(0),
            },
            Instruction::Yield,
            Instruction::LoadStatic {
                dest: TempId(1),
                static_id: StaticId(0),
            },
        ]);
        let changed = optimize_static_access(&mut function);
        assert!(!changed);
    }

    #[test]
    fn different_statics_tracked_independently() {
        let mut function = make_function(vec![
            Instruction::LoadStatic {
                dest: TempId(0),
                static_id: StaticId(0),
            },
            Instruction::LoadStatic {
                dest: TempId(1),
                static_id: StaticId(1),
            },
            Instruction::LoadStatic {
                dest: TempId(2),
                static_id: StaticId(0),
            },
            Instruction::LoadStatic {
                dest: TempId(3),
                static_id: StaticId(1),
            },
        ]);
        let changed = optimize_static_access(&mut function);
        assert!(changed);
        assert!(matches!(
            &function.blocks[0].instructions[2],
            Instruction::Assign {
                dest: TempId(2),
                operation: Operation::Copy(TempId(0))
            }
        ));
        assert!(matches!(
            &function.blocks[0].instructions[3],
            Instruction::Assign {
                dest: TempId(3),
                operation: Operation::Copy(TempId(1))
            }
        ));
    }

    #[test]
    fn store_to_different_static_does_not_invalidate() {
        let mut function = make_function(vec![
            Instruction::LoadStatic {
                dest: TempId(0),
                static_id: StaticId(0),
            },
            Instruction::Assign {
                dest: TempId(1),
                operation: Operation::Constant(1.0),
            },
            Instruction::StoreStatic {
                static_id: StaticId(1),
                source: TempId(1),
            },
            Instruction::LoadStatic {
                dest: TempId(2),
                static_id: StaticId(0),
            },
        ]);
        let changed = optimize_static_access(&mut function);
        assert!(changed);
        assert!(matches!(
            &function.blocks[0].instructions[3],
            Instruction::Assign {
                dest: TempId(2),
                operation: Operation::Copy(TempId(0))
            }
        ));
    }

    #[test]
    fn dead_store_not_eliminated_when_load_intervenes() {
        let mut function = make_function(vec![
            Instruction::Assign {
                dest: TempId(0),
                operation: Operation::Constant(1.0),
            },
            Instruction::StoreStatic {
                static_id: StaticId(0),
                source: TempId(0),
            },
            Instruction::LoadStatic {
                dest: TempId(1),
                static_id: StaticId(0),
            },
            Instruction::Assign {
                dest: TempId(2),
                operation: Operation::Constant(2.0),
            },
            Instruction::StoreStatic {
                static_id: StaticId(0),
                source: TempId(2),
            },
        ]);
        let changed = optimize_static_access(&mut function);
        assert!(changed); // The load is forwarded, but the first store is kept.
        assert_eq!(function.blocks[0].instructions.len(), 5);
        assert!(matches!(
            &function.blocks[0].instructions[1],
            Instruction::StoreStatic {
                static_id: StaticId(0),
                source: TempId(0)
            }
        ));
    }

    fn make_two_block_function(
        block0_instructions: Vec<Instruction>,
        block0_terminator: Terminator,
        block1_instructions: Vec<Instruction>,
        block1_terminator: Terminator,
    ) -> Function {
        Function {
            name: "test".to_string(),
            symbol_id: SymbolId(0),
            parameters: Vec::new(),
            return_type: None,
            blocks: vec![
                BasicBlock {
                    id: BlockId(0),
                    role: BlockRole::Entry,
                    instructions: block0_instructions,
                    terminator: block0_terminator,
                    predecessors: Vec::new(),
                    successors: vec![BlockId(1)],
                },
                BasicBlock {
                    id: BlockId(1),
                    role: BlockRole::Generic,
                    instructions: block1_instructions,
                    terminator: block1_terminator,
                    predecessors: vec![BlockId(0)],
                    successors: Vec::new(),
                },
            ],
            entry: BlockId(0),
            variable_definitions: HashMap::new(),
            variable_temps: HashMap::new(),
            immediate_dominators: HashMap::new(),
            dominance_frontiers: HashMap::new(),
            next_temp: 100,
        }
    }

    #[test]
    fn cross_block_load_forwarded() {
        let mut function = make_two_block_function(
            vec![Instruction::LoadStatic {
                dest: TempId(0),
                static_id: StaticId(0),
            }],
            Terminator::Jump(BlockId(1)),
            vec![Instruction::LoadStatic {
                dest: TempId(1),
                static_id: StaticId(0),
            }],
            Terminator::Return(None),
        );
        let changed = optimize_static_access(&mut function);
        assert!(changed);
        assert!(matches!(
            &function.blocks[1].instructions[0],
            Instruction::Assign {
                dest: TempId(1),
                operation: Operation::Copy(TempId(0))
            }
        ));
    }

    #[test]
    fn cross_block_store_to_load_forwarded() {
        let mut function = make_two_block_function(
            vec![
                Instruction::Assign {
                    dest: TempId(0),
                    operation: Operation::Constant(42.0),
                },
                Instruction::StoreStatic {
                    static_id: StaticId(0),
                    source: TempId(0),
                },
            ],
            Terminator::Jump(BlockId(1)),
            vec![Instruction::LoadStatic {
                dest: TempId(1),
                static_id: StaticId(0),
            }],
            Terminator::Return(None),
        );
        let changed = optimize_static_access(&mut function);
        assert!(changed);
        assert!(matches!(
            &function.blocks[1].instructions[0],
            Instruction::Assign {
                dest: TempId(1),
                operation: Operation::Copy(TempId(0))
            }
        ));
    }

    #[test]
    fn cross_block_call_invalidates() {
        let mut function = make_two_block_function(
            vec![
                Instruction::LoadStatic {
                    dest: TempId(0),
                    static_id: StaticId(0),
                },
                Instruction::Call {
                    dest: None,
                    function: SymbolId(1),
                    args: Vec::new(),
                },
            ],
            Terminator::Jump(BlockId(1)),
            vec![Instruction::LoadStatic {
                dest: TempId(1),
                static_id: StaticId(0),
            }],
            Terminator::Return(None),
        );
        let changed = optimize_static_access(&mut function);
        assert!(!changed);
    }

    #[test]
    fn cross_block_diamond_both_paths_agree() {
        // Block 0 loads S0 → T0, branches to blocks 1 and 2.
        // Neither block 1 nor 2 kills S0. Both flow to block 3.
        // Block 3's load of S0 should be replaced with Copy(T0).
        let mut function = Function {
            name: "test".to_string(),
            symbol_id: SymbolId(0),
            parameters: Vec::new(),
            return_type: None,
            blocks: vec![
                BasicBlock {
                    id: BlockId(0),
                    role: BlockRole::Entry,
                    instructions: vec![Instruction::LoadStatic {
                        dest: TempId(0),
                        static_id: StaticId(0),
                    }],
                    terminator: Terminator::Branch {
                        condition: TempId(0),
                        true_block: BlockId(1),
                        false_block: BlockId(2),
                    },
                    predecessors: Vec::new(),
                    successors: vec![BlockId(1), BlockId(2)],
                },
                BasicBlock {
                    id: BlockId(1),
                    role: BlockRole::Generic,
                    instructions: Vec::new(),
                    terminator: Terminator::Jump(BlockId(3)),
                    predecessors: vec![BlockId(0)],
                    successors: vec![BlockId(3)],
                },
                BasicBlock {
                    id: BlockId(2),
                    role: BlockRole::Generic,
                    instructions: Vec::new(),
                    terminator: Terminator::Jump(BlockId(3)),
                    predecessors: vec![BlockId(0)],
                    successors: vec![BlockId(3)],
                },
                BasicBlock {
                    id: BlockId(3),
                    role: BlockRole::Generic,
                    instructions: vec![Instruction::LoadStatic {
                        dest: TempId(1),
                        static_id: StaticId(0),
                    }],
                    terminator: Terminator::Return(None),
                    predecessors: vec![BlockId(1), BlockId(2)],
                    successors: Vec::new(),
                },
            ],
            entry: BlockId(0),
            variable_definitions: HashMap::new(),
            variable_temps: HashMap::new(),
            immediate_dominators: HashMap::new(),
            dominance_frontiers: HashMap::new(),
            next_temp: 100,
        };
        let changed = optimize_static_access(&mut function);
        assert!(changed);
        assert!(matches!(
            &function.blocks[3].instructions[0],
            Instruction::Assign {
                dest: TempId(1),
                operation: Operation::Copy(TempId(0))
            }
        ));
    }

    #[test]
    fn cross_block_diamond_one_path_kills() {
        // Block 0 loads S0 → T0, branches to blocks 1 and 2.
        // Block 1 has a Call (kills S0). Both flow to block 3.
        // Block 3's load of S0 must NOT be replaced.
        let mut function = Function {
            name: "test".to_string(),
            symbol_id: SymbolId(0),
            parameters: Vec::new(),
            return_type: None,
            blocks: vec![
                BasicBlock {
                    id: BlockId(0),
                    role: BlockRole::Entry,
                    instructions: vec![Instruction::LoadStatic {
                        dest: TempId(0),
                        static_id: StaticId(0),
                    }],
                    terminator: Terminator::Branch {
                        condition: TempId(0),
                        true_block: BlockId(1),
                        false_block: BlockId(2),
                    },
                    predecessors: Vec::new(),
                    successors: vec![BlockId(1), BlockId(2)],
                },
                BasicBlock {
                    id: BlockId(1),
                    role: BlockRole::Generic,
                    instructions: vec![Instruction::Call {
                        dest: None,
                        function: SymbolId(1),
                        args: Vec::new(),
                    }],
                    terminator: Terminator::Jump(BlockId(3)),
                    predecessors: vec![BlockId(0)],
                    successors: vec![BlockId(3)],
                },
                BasicBlock {
                    id: BlockId(2),
                    role: BlockRole::Generic,
                    instructions: Vec::new(),
                    terminator: Terminator::Jump(BlockId(3)),
                    predecessors: vec![BlockId(0)],
                    successors: vec![BlockId(3)],
                },
                BasicBlock {
                    id: BlockId(3),
                    role: BlockRole::Generic,
                    instructions: vec![Instruction::LoadStatic {
                        dest: TempId(1),
                        static_id: StaticId(0),
                    }],
                    terminator: Terminator::Return(None),
                    predecessors: vec![BlockId(1), BlockId(2)],
                    successors: Vec::new(),
                },
            ],
            entry: BlockId(0),
            variable_definitions: HashMap::new(),
            variable_temps: HashMap::new(),
            immediate_dominators: HashMap::new(),
            dominance_frontiers: HashMap::new(),
            next_temp: 100,
        };
        let changed = optimize_static_access(&mut function);
        assert!(!changed);
    }

    #[test]
    fn cross_block_chain_forwarding() {
        // Block 0 → Block 1 → Block 2, all unconditional.
        // Block 0 loads S0 → T0. Block 2 loads S0 → should become Copy(T0).
        let mut function = Function {
            name: "test".to_string(),
            symbol_id: SymbolId(0),
            parameters: Vec::new(),
            return_type: None,
            blocks: vec![
                BasicBlock {
                    id: BlockId(0),
                    role: BlockRole::Entry,
                    instructions: vec![Instruction::LoadStatic {
                        dest: TempId(0),
                        static_id: StaticId(0),
                    }],
                    terminator: Terminator::Jump(BlockId(1)),
                    predecessors: Vec::new(),
                    successors: vec![BlockId(1)],
                },
                BasicBlock {
                    id: BlockId(1),
                    role: BlockRole::Generic,
                    instructions: Vec::new(),
                    terminator: Terminator::Jump(BlockId(2)),
                    predecessors: vec![BlockId(0)],
                    successors: vec![BlockId(2)],
                },
                BasicBlock {
                    id: BlockId(2),
                    role: BlockRole::Generic,
                    instructions: vec![Instruction::LoadStatic {
                        dest: TempId(1),
                        static_id: StaticId(0),
                    }],
                    terminator: Terminator::Return(None),
                    predecessors: vec![BlockId(1)],
                    successors: Vec::new(),
                },
            ],
            entry: BlockId(0),
            variable_definitions: HashMap::new(),
            variable_temps: HashMap::new(),
            immediate_dominators: HashMap::new(),
            dominance_frontiers: HashMap::new(),
            next_temp: 100,
        };
        let changed = optimize_static_access(&mut function);
        assert!(changed);
        assert!(matches!(
            &function.blocks[2].instructions[0],
            Instruction::Assign {
                dest: TempId(1),
                operation: Operation::Copy(TempId(0))
            }
        ));
    }
}
