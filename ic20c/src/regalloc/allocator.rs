use std::collections::HashMap;

use crate::diagnostic::{Diagnostic, Span};
use crate::ir::cfg::{Function, TempId};

use super::calling_convention::{CallingConventionInfo, FunctionClass};
use super::ic10::Register;
use super::liveness::{LinearMap, LinearPosition, LiveRange, instruction_uses, terminator_uses};

/// The sixteen general-purpose registers available for allocation.
const ALLOCATABLE_REGISTERS: [Register; 16] = [
    Register::R0,
    Register::R1,
    Register::R2,
    Register::R3,
    Register::R4,
    Register::R5,
    Register::R6,
    Register::R7,
    Register::R8,
    Register::R9,
    Register::R10,
    Register::R11,
    Register::R12,
    Register::R13,
    Register::R14,
    Register::R15,
];

/// Tracks which physical registers are currently free for allocation.
struct RegisterPool {
    /// `available[i]` is `true` when `ALLOCATABLE_REGISTERS[i]` is not in use.
    available: [bool; 16],
    /// Preferred search order for `allocate_any`. Indices into `ALLOCATABLE_REGISTERS`.
    preference_order: [usize; 16],
}

/// Leaf functions prefer r0–r7 (scratch/caller-saved), then r8–r15.
const LEAF_PREFERENCE: [usize; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

/// Non-leaf functions prefer r8–r15 (callee-saved), then r0–r7.
const NON_LEAF_PREFERENCE: [usize; 16] = [8, 9, 10, 11, 12, 13, 14, 15, 0, 1, 2, 3, 4, 5, 6, 7];

impl RegisterPool {
    fn new(function_class: FunctionClass) -> Self {
        let preference_order = match function_class {
            FunctionClass::Leaf => LEAF_PREFERENCE,
            FunctionClass::NonLeaf => NON_LEAF_PREFERENCE,
        };
        RegisterPool {
            available: [true; 16],
            preference_order,
        }
    }

    fn is_free(&self, register: Register) -> bool {
        Self::index_of(register).is_some_and(|i| self.available[i])
    }

    fn allocate(&mut self, register: Register) {
        if let Some(index) = Self::index_of(register) {
            debug_assert!(
                self.available[index],
                "register {:?} already allocated",
                register
            );
            self.available[index] = false;
        }
    }

    fn free(&mut self, register: Register) {
        if let Some(index) = Self::index_of(register) {
            self.available[index] = true;
        }
    }

    /// Pick any free register, using the preference order for the function class.
    fn allocate_any(&mut self) -> Option<Register> {
        for &index in &self.preference_order {
            if self.available[index] {
                self.available[index] = false;
                return Some(ALLOCATABLE_REGISTERS[index]);
            }
        }
        None
    }

    fn index_of(register: Register) -> Option<usize> {
        match register {
            Register::R0 => Some(0),
            Register::R1 => Some(1),
            Register::R2 => Some(2),
            Register::R3 => Some(3),
            Register::R4 => Some(4),
            Register::R5 => Some(5),
            Register::R6 => Some(6),
            Register::R7 => Some(7),
            Register::R8 => Some(8),
            Register::R9 => Some(9),
            Register::R10 => Some(10),
            Register::R11 => Some(11),
            Register::R12 => Some(12),
            Register::R13 => Some(13),
            Register::R14 => Some(14),
            Register::R15 => Some(15),
            Register::Ra | Register::Sp => None,
        }
    }
}

/// A record of a single spill (push) and its corresponding reload (pop).
///
/// During code generation, emit `push register` before `spill_position` and
/// `pop register` before `reload_position`.
#[derive(Debug, Clone)]
pub struct SpillRecord {
    pub temp: TempId,
    pub register: Register,
    /// Emit `push register` immediately before this position.
    pub spill_position: LinearPosition,
    /// Emit `pop register` immediately before this position, or `None` if the temp has
    /// no remaining uses after the spill.
    pub reload_position: Option<LinearPosition>,
}

/// The result of register allocation for a single function.
pub struct AllocationResult {
    /// Mapping from each live temp to its assigned physical register.
    pub assignments: HashMap<TempId, Register>,
    /// Spill/reload pairs to be inserted during code generation.
    pub spills: Vec<SpillRecord>,
    /// Maximum number of values simultaneously on the stack due to spilling.
    pub max_stack_depth: usize,
}

/// A live range that is currently occupying a physical register.
struct ActiveEntry {
    temp: TempId,
    register: Register,
    range_end: LinearPosition,
}

/// Compute the sorted list of use positions for every `TempId` in the function.
fn compute_use_positions(
    function: &Function,
    linear_map: &LinearMap,
) -> HashMap<TempId, Vec<LinearPosition>> {
    let mut positions: HashMap<TempId, Vec<LinearPosition>> = HashMap::new();

    for &block_id in &linear_map.block_order {
        let block = &function.blocks[block_id.0];
        for (index, instruction) in block.instructions.iter().enumerate() {
            let position = linear_map.instruction_positions[&(block_id, index)];
            for used in instruction_uses(instruction) {
                positions.entry(used).or_default().push(position);
            }
        }
        let terminator_position = linear_map.terminator_positions[&block_id];
        for used in terminator_uses(&block.terminator) {
            positions.entry(used).or_default().push(terminator_position);
        }
    }

    for list in positions.values_mut() {
        list.sort();
        list.dedup();
    }

    positions
}

/// Return the first use position of `temp` that is strictly after `position`.
fn next_use_after(
    temp: TempId,
    position: LinearPosition,
    use_positions: &HashMap<TempId, Vec<LinearPosition>>,
) -> Option<LinearPosition> {
    use_positions
        .get(&temp)
        .and_then(|list| list.iter().copied().find(|&p| p > position))
}

/// Perform linear-scan register allocation for a single function.
///
/// Assigns a physical register to every temp in `live_ranges`, respecting the
/// calling-convention pre-assignments in `calling_convention.fixed` as preferences.
/// When register pressure exceeds 16, the temp with the farthest next use is spilled
/// to the stack with a push/pop pair.
pub fn allocate_function(
    function: &Function,
    linear_map: &LinearMap,
    live_ranges: &HashMap<TempId, LiveRange>,
    calling_convention: &CallingConventionInfo,
) -> Result<AllocationResult, Vec<Diagnostic>> {
    let use_positions = compute_use_positions(function, linear_map);

    // Process ranges in start-position order; break ties by temp id for determinism.
    let mut sorted_ranges: Vec<(TempId, &LiveRange)> = live_ranges
        .iter()
        .map(|(&temp, range)| (temp, range))
        .collect();
    sorted_ranges.sort_by_key(|(temp, range)| (range.start(), temp.0));

    let mut pool = RegisterPool::new(calling_convention.function_class);
    let mut active: Vec<ActiveEntry> = Vec::new();
    let mut assignments: HashMap<TempId, Register> = HashMap::new();
    let mut spills: Vec<SpillRecord> = Vec::new();

    for &(temp, range) in &sorted_ranges {
        let position = range.start();

        // Release registers from temps whose live ranges ended before this point.
        expire_old_ranges(&mut active, &mut pool, position);

        let preferred = calling_convention.fixed.get(&temp).copied();

        if let Some(preferred_register) = preferred {
            // Fast path: the calling-convention register is free, grab it directly.
            if pool.is_free(preferred_register) {
                pool.allocate(preferred_register);
                assignments.insert(temp, preferred_register);
                insert_active_sorted(
                    &mut active,
                    ActiveEntry {
                        temp,
                        register: preferred_register,
                        range_end: range.end(),
                    },
                );
                continue;
            }

            // The preferred register is held by another temp. Evict it to an
            // alternative register if one is available, otherwise spill it to the stack.
            if let Some(occupant_index) =
                active.iter().position(|e| e.register == preferred_register)
            {
                let occupant = active.remove(occupant_index);

                if let Some(alternative) = pool.allocate_any() {
                    // Relocate the occupant so this temp can claim its preferred register.
                    assignments.insert(occupant.temp, alternative);
                    insert_active_sorted(
                        &mut active,
                        ActiveEntry {
                            temp: occupant.temp,
                            register: alternative,
                            range_end: occupant.range_end,
                        },
                    );
                    assignments.insert(temp, preferred_register);
                    insert_active_sorted(
                        &mut active,
                        ActiveEntry {
                            temp,
                            register: preferred_register,
                            range_end: range.end(),
                        },
                    );
                    continue;
                }

                // No free alternative register. Forcibly spilling the occupant here could
                // violate stack LIFO order: the occupant's next use may be earlier than
                // something already deeper on the spill stack, making it impossible to pop
                // in the correct order at reload time. Restore the occupant and fall through
                // to spill_farthest, which always picks the farthest-use victim and
                // therefore guarantees properly nested (non-crossing) spill intervals.
                insert_active_sorted(
                    &mut active,
                    ActiveEntry {
                        temp: occupant.temp,
                        register: occupant.register,
                        range_end: occupant.range_end,
                    },
                );
            }
        }

        // No calling-convention constraint — take any free register.
        if let Some(register) = pool.allocate_any() {
            assignments.insert(temp, register);
            insert_active_sorted(
                &mut active,
                ActiveEntry {
                    temp,
                    register,
                    range_end: range.end(),
                },
            );
            continue;
        }

        // All registers are live. Spill whichever temp is used farthest in the future.
        spill_farthest(
            temp,
            range,
            position,
            &mut active,
            &mut pool,
            &mut assignments,
            &mut spills,
            &use_positions,
        )?;
    }

    // Spills whose temp is never used again need no reload; drop them entirely.
    spills.retain(|record| record.reload_position.is_some());

    let max_stack_depth = compute_max_stack_depth(&spills);

    Ok(AllocationResult {
        assignments,
        spills,
        max_stack_depth,
    })
}

/// Remove entries from the active set whose ranges have ended before `position`, returning
/// their registers to the pool.
fn expire_old_ranges(
    active: &mut Vec<ActiveEntry>,
    pool: &mut RegisterPool,
    position: LinearPosition,
) {
    let mut index = 0;
    while index < active.len() {
        if active[index].range_end <= position {
            let entry = active.remove(index);
            pool.free(entry.register);
        } else {
            index += 1;
        }
    }
}

/// Insert an entry into the active list, keeping it sorted by `range_end` ascending.
fn insert_active_sorted(active: &mut Vec<ActiveEntry>, entry: ActiveEntry) {
    let position = active
        .iter()
        .position(|e| e.range_end > entry.range_end)
        .unwrap_or(active.len());
    active.insert(position, entry);
}

/// Spill the active entry with the farthest next use to free a register for `temp`.
///
/// If the current temp's range extends beyond every active entry, the current temp is
/// itself spilled instead.
#[allow(clippy::too_many_arguments)]
fn spill_farthest(
    temp: TempId,
    range: &LiveRange,
    position: LinearPosition,
    active: &mut Vec<ActiveEntry>,
    pool: &mut RegisterPool,
    assignments: &mut HashMap<TempId, Register>,
    spills: &mut Vec<SpillRecord>,
    use_positions: &HashMap<TempId, Vec<LinearPosition>>,
) -> Result<(), Vec<Diagnostic>> {
    let victim_index = active
        .iter()
        .enumerate()
        .max_by_key(|(_, entry)| {
            next_use_after(entry.temp, position, use_positions).unwrap_or(LinearPosition(0))
        })
        .map(|(index, _)| index);

    let Some(victim_index) = victim_index else {
        return Err(vec![Diagnostic::error(
            Span::new(0, 0),
            "register allocation failed: no register available and no candidate to spill",
        )]);
    };

    let victim = &active[victim_index];
    let victim_next_use =
        next_use_after(victim.temp, position, use_positions).unwrap_or(LinearPosition(0));
    let current_next_use =
        next_use_after(temp, position, use_positions).unwrap_or(LinearPosition(0));

    if victim_next_use > current_next_use {
        let victim = active.remove(victim_index);
        pool.free(victim.register);

        let reload = next_use_after(victim.temp, position, use_positions);
        spills.push(SpillRecord {
            temp: victim.temp,
            register: victim.register,
            spill_position: position,
            reload_position: reload,
        });

        pool.allocate(victim.register);
        assignments.insert(temp, victim.register);
        insert_active_sorted(
            active,
            ActiveEntry {
                temp,
                register: victim.register,
                range_end: range.end(),
            },
        );
    } else {
        let register = active[victim_index].register;
        let reload = next_use_after(temp, position, use_positions);
        spills.push(SpillRecord {
            temp,
            register,
            spill_position: position,
            reload_position: reload,
        });
        assignments.insert(temp, register);
    }

    Ok(())
}

/// Compute the maximum number of values simultaneously resident on the stack.
fn compute_max_stack_depth(spills: &[SpillRecord]) -> usize {
    let mut events: Vec<(LinearPosition, i32)> = Vec::new();
    for spill in spills {
        events.push((spill.spill_position, 1));
        if let Some(reload) = spill.reload_position {
            events.push((reload, -1));
        }
    }
    events.sort_by_key(|&(position, delta)| (position, delta));

    let mut depth: i32 = 0;
    let mut max_depth: i32 = 0;
    for (_, delta) in events {
        depth += delta;
        max_depth = max_depth.max(depth);
    }
    max_depth.max(0) as usize
}

/// Given a set of register-to-register moves required at a call boundary, produce a
/// sequential move list that correctly handles overlapping assignments and cycles.
///
/// Each element of `moves` is `(source_register, destination_register)`. The returned
/// list can be executed in order to achieve the parallel move semantics. Cycles are
/// broken using a scratch register from `available_scratch`.
pub fn resolve_parallel_moves(
    moves: &[(Register, Register)],
    available_scratch: Option<Register>,
) -> Vec<(Register, Register)> {
    let mut remaining: Vec<(Register, Register)> = moves
        .iter()
        .filter(|(source, destination)| source != destination)
        .copied()
        .collect();
    let mut result = Vec::new();

    loop {
        if remaining.is_empty() {
            break;
        }

        let ready = remaining.iter().position(|(_, destination)| {
            !remaining.iter().any(|(source, _)| source == destination)
        });

        if let Some(index) = ready {
            result.push(remaining.remove(index));
        } else if let Some(scratch) = available_scratch {
            let (source, _) = remaining[0];
            result.push((source, scratch));
            for (s, _) in remaining.iter_mut() {
                if *s == source {
                    *s = scratch;
                }
            }
        } else {
            for item in remaining.drain(..) {
                result.push(item);
            }
            break;
        }
    }

    result
}
