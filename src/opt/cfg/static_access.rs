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

/// Optimizes static variable loads and stores within each basic block.
///
/// Three local (intra-block) optimizations:
///
/// 1. **Redundant load elimination** — if a static was already loaded and nothing
///    has invalidated the cached value, replace the second load with a copy.
/// 2. **Store-to-load forwarding** — if a value was just stored to a static, a
///    subsequent load returns that value directly (copy instead of `get db`).
/// 3. **Dead store elimination** — if a static is stored twice with no intervening
///    load or invalidation, the first store is removed.
///
/// An entry is invalidated by `Call` (the callee may access any static), `Yield`,
/// or `Sleep` (control returns to the runtime).
pub(super) fn optimize_static_access(function: &mut Function) -> bool {
    let mut changed = false;

    for block in &mut function.blocks {
        let mut known: HashMap<StaticId, Known> = HashMap::new();
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
}
