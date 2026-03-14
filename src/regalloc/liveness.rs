use std::collections::{HashMap, HashSet};

use crate::cfg::{BlockId, Function, Instruction, Operation, TempId, Terminator};

/// A sequential position in the linearized instruction sequence.
///
/// Position 0 is the first instruction of the entry block. Each regular instruction and each
/// block terminator each occupy exactly one position. Labels and other pseudo-instructions are
/// not counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LinearPosition(pub usize);

/// An inclusive range of linear positions covering all instructions and the terminator of a
/// single basic block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinearRange {
    pub start: LinearPosition,
    pub end: LinearPosition,
}

/// A single contiguous live interval `[start, end]` (both endpoints inclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LiveInterval {
    pub start: LinearPosition,
    pub end: LinearPosition,
}

/// The complete live range of a temporary, represented as a sorted, non-overlapping list of
/// `LiveInterval`s.
///
/// Using multiple intervals rather than a single `[start, end]` span avoids overestimating
/// liveness across lifetime holes, which matters for the 128-line IC10 budget: a temp that is
/// defined early and used again much later should not block a register during the gap.
#[derive(Debug, Clone)]
pub struct LiveRange {
    /// Sorted by `start`, non-overlapping.
    pub intervals: Vec<LiveInterval>,
}

impl LiveRange {
    /// Returns `true` if this range is live at `position`.
    pub fn contains(&self, position: LinearPosition) -> bool {
        self.intervals
            .iter()
            .any(|interval| interval.start <= position && position <= interval.end)
    }

    /// The earliest position covered.
    pub fn start(&self) -> LinearPosition {
        self.intervals
            .first()
            .expect("live range must have at least one interval")
            .start
    }

    /// The latest position covered.
    pub fn end(&self) -> LinearPosition {
        self.intervals
            .last()
            .expect("live range must have at least one interval")
            .end
    }
}

/// The result of linearizing a function's CFG.
///
/// Every instruction and terminator in the function is assigned exactly one `LinearPosition`.
/// Positions are assigned in reverse-postorder block layout, with instructions within a block
/// numbered consecutively followed by the block's terminator.
pub struct LinearMap {
    /// The block layout order (reverse-postorder).
    pub block_order: Vec<BlockId>,
    /// Position of the instruction at `(block_id, instruction_index)`. The index is 0-based
    /// into the block's `instructions` slice.
    pub instruction_positions: HashMap<(BlockId, usize), LinearPosition>,
    /// Position of each block's terminator.
    pub terminator_positions: HashMap<BlockId, LinearPosition>,
    /// The position range `[start, end]` (inclusive) spanned by each block, covering all of
    /// its instructions and its terminator.
    pub block_ranges: HashMap<BlockId, LinearRange>,
    /// Total number of positions assigned (equal to the number of instructions + terminators).
    pub total: usize,
}

/// Compute the reverse-postorder traversal of `function`'s CFG.
///
/// DFS postorder visits a block after all of its reachable successors. Reversing that order
/// guarantees that in acyclic regions every block's dominators appear before it, and loop
/// headers appear before loop bodies.
pub fn compute_reverse_postorder(function: &Function) -> Vec<BlockId> {
    let mut visited = HashSet::new();
    let mut postorder = Vec::new();
    dfs_postorder(function, function.entry, &mut visited, &mut postorder);
    postorder.reverse();
    postorder
}

fn dfs_postorder(
    function: &Function,
    block_id: BlockId,
    visited: &mut HashSet<BlockId>,
    postorder: &mut Vec<BlockId>,
) {
    if !visited.insert(block_id) {
        return;
    }
    let block = &function.blocks[block_id.0];
    for &successor in &block.successors {
        dfs_postorder(function, successor, visited, postorder);
    }
    postorder.push(block_id);
}

/// Assign a `LinearPosition` to every instruction and terminator in `function`, laid out in
/// the given `block_order`.
///
/// Within each block, instructions are numbered first (in their original order), followed by
/// the block terminator. The returned `LinearMap` stores all position assignments and the
/// inclusive position range of each block.
pub fn linearize_function(function: &Function, block_order: &[BlockId]) -> LinearMap {
    let mut instruction_positions = HashMap::new();
    let mut terminator_positions = HashMap::new();
    let mut block_ranges = HashMap::new();
    let mut position = 0;

    for &block_id in block_order {
        let block = &function.blocks[block_id.0];
        let start = LinearPosition(position);

        for (index, _) in block.instructions.iter().enumerate() {
            instruction_positions.insert((block_id, index), LinearPosition(position));
            position += 1;
        }

        terminator_positions.insert(block_id, LinearPosition(position));
        position += 1;

        block_ranges.insert(
            block_id,
            LinearRange {
                start,
                end: LinearPosition(position - 1),
            },
        );
    }

    LinearMap {
        block_order: block_order.to_vec(),
        instruction_positions,
        terminator_positions,
        block_ranges,
        total: position,
    }
}

/// Collect the `TempId`s defined by a single instruction (0 or 1 in practice for non-phi).
fn instruction_defs(instruction: &Instruction) -> Vec<TempId> {
    match instruction {
        Instruction::Assign { dest, .. } => vec![*dest],
        Instruction::Phi { dest, .. } => vec![*dest],
        Instruction::LoadDevice { dest, .. } => vec![*dest],
        Instruction::LoadSlot { dest, .. } => vec![*dest],
        Instruction::BatchRead { dest, .. } => vec![*dest],
        Instruction::Call {
            dest: Some(dest), ..
        } => vec![*dest],
        Instruction::BuiltinCall { dest, .. } => vec![*dest],
        Instruction::StoreDevice { .. }
        | Instruction::StoreSlot { .. }
        | Instruction::BatchWrite { .. }
        | Instruction::Call { dest: None, .. }
        | Instruction::Sleep { .. }
        | Instruction::Yield => vec![],
    }
}

/// Collect the `TempId`s read by a single instruction.
pub(crate) fn instruction_uses(instruction: &Instruction) -> Vec<TempId> {
    match instruction {
        Instruction::Assign { operation, .. } => operation_uses(operation),
        Instruction::Phi { args, .. } => args.iter().map(|(temp, _)| *temp).collect(),
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

/// Collect the `TempId`s read by a block terminator.
pub(crate) fn terminator_uses(terminator: &Terminator) -> Vec<TempId> {
    match terminator {
        Terminator::Jump(_) => vec![],
        Terminator::Branch { condition, .. } => vec![*condition],
        Terminator::Return(Some(value)) => vec![*value],
        Terminator::Return(None) | Terminator::None => vec![],
    }
}

/// Compute the live range of every `TempId` defined in `function`.
///
/// Must be called after phi deconstruction so that there are no `Instruction::Phi` nodes.
/// Each temp receives a sorted, non-overlapping list of `LiveInterval`s. Temps that are
/// defined but never used get a single zero-length interval at their definition point.
///
/// Liveness is computed by a backward dataflow pass:
///   live_out[B] = ∪ live_in[successor] for each successor of B
///   live_in[B]  = upward_exposed_uses[B] ∪ (live_out[B] \ defs[B])
///
/// After the fixed-point, for each temp the intervals cover only the actual live portions of
/// the linear sequence, exploiting lifetime holes to reduce register pressure.
pub fn compute_live_ranges(
    function: &Function,
    linear_map: &LinearMap,
) -> HashMap<TempId, LiveRange> {
    let block_order = &linear_map.block_order;

    // Per-block upward-exposed uses and defs, plus per-instruction use/def positions.
    let mut upward_exposed_uses: HashMap<BlockId, HashSet<TempId>> = HashMap::new();
    let mut block_defs: HashMap<BlockId, HashSet<TempId>> = HashMap::new();
    let mut def_block_map: HashMap<TempId, BlockId> = HashMap::new();
    let mut def_position_map: HashMap<TempId, LinearPosition> = HashMap::new();
    // Last position in each block at which a given temp is used.
    let mut last_use_in_block: HashMap<(BlockId, TempId), LinearPosition> = HashMap::new();

    for &block_id in block_order {
        let block = &function.blocks[block_id.0];
        let mut uses: HashSet<TempId> = HashSet::new();
        let mut defs: HashSet<TempId> = HashSet::new();

        for (index, instruction) in block.instructions.iter().enumerate() {
            let position = linear_map.instruction_positions[&(block_id, index)];
            for used_temp in instruction_uses(instruction) {
                if !defs.contains(&used_temp) {
                    uses.insert(used_temp);
                }
                last_use_in_block.insert((block_id, used_temp), position);
            }
            for defined_temp in instruction_defs(instruction) {
                defs.insert(defined_temp);
                def_block_map.insert(defined_temp, block_id);
                def_position_map.insert(defined_temp, position);
            }
        }

        let term_position = linear_map.terminator_positions[&block_id];
        for used_temp in terminator_uses(&block.terminator) {
            if !defs.contains(&used_temp) {
                uses.insert(used_temp);
            }
            last_use_in_block.insert((block_id, used_temp), term_position);
        }

        upward_exposed_uses.insert(block_id, uses);
        block_defs.insert(block_id, defs);
    }

    // Backward dataflow: iterate in reverse block order until the sets stabilize.
    let mut live_in: HashMap<BlockId, HashSet<TempId>> =
        block_order.iter().map(|&id| (id, HashSet::new())).collect();
    let mut live_out: HashMap<BlockId, HashSet<TempId>> =
        block_order.iter().map(|&id| (id, HashSet::new())).collect();

    let mut changed = true;
    while changed {
        changed = false;
        for &block_id in block_order.iter().rev() {
            let block = &function.blocks[block_id.0];

            let new_live_out: HashSet<TempId> = block
                .successors
                .iter()
                .flat_map(|&successor| live_in[&successor].iter().copied())
                .collect();

            let new_live_in: HashSet<TempId> = upward_exposed_uses[&block_id]
                .iter()
                .copied()
                .chain(
                    new_live_out
                        .iter()
                        .copied()
                        .filter(|t| !block_defs[&block_id].contains(t)),
                )
                .collect();

            if new_live_out != live_out[&block_id] {
                *live_out.get_mut(&block_id).unwrap() = new_live_out;
                changed = true;
            }
            if new_live_in != live_in[&block_id] {
                *live_in.get_mut(&block_id).unwrap() = new_live_in;
                changed = true;
            }
        }
    }

    // Build per-temp interval lists from the liveness sets.
    let mut result: HashMap<TempId, LiveRange> = HashMap::new();

    for (&temp, &temp_def_pos) in &def_position_map {
        let temp_def_block = def_block_map[&temp];
        let def_block_range = linear_map.block_ranges[&temp_def_block];

        // Interval for the defining block: starts at the definition, ends at the block boundary
        // if the temp escapes the block, otherwise at the last use within the block.
        let def_block_end = if live_out[&temp_def_block].contains(&temp) {
            def_block_range.end
        } else {
            last_use_in_block
                .get(&(temp_def_block, temp))
                .copied()
                .unwrap_or(temp_def_pos)
        };

        let mut intervals = vec![LiveInterval {
            start: temp_def_pos,
            end: def_block_end,
        }];

        // One interval per block where the temp is live-in (excluding the defining block, which
        // was handled above).
        for &block_id in block_order {
            if block_id == temp_def_block {
                continue;
            }
            if !live_in[&block_id].contains(&temp) {
                continue;
            }
            let block_range = linear_map.block_ranges[&block_id];
            let end = if live_out[&block_id].contains(&temp) {
                block_range.end
            } else {
                last_use_in_block
                    .get(&(block_id, temp))
                    .copied()
                    .unwrap_or(block_range.start)
            };
            intervals.push(LiveInterval {
                start: block_range.start,
                end,
            });
        }

        intervals.sort_by_key(|interval| interval.start);
        result.insert(temp, LiveRange { intervals });
    }

    result
}
