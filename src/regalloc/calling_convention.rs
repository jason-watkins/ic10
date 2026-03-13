use std::collections::HashMap;

use crate::cfg::{Function, Instruction, TempId, Terminator};

use super::ic10::Register;
use super::liveness::{LinearMap, LinearPosition, LiveRange};

/// Whether a function contains any `Instruction::Call` instructions.
///
/// A leaf function never calls another function and therefore does not need to save or
/// restore the return-address register `ra`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionClass {
    /// No `Call` instructions — `ra` does not need to be saved.
    Leaf,
    /// Contains at least one `Call` instruction — must `push ra` at entry and `pop ra`
    /// before every return.
    NonLeaf,
}

/// Calling-convention pre-assignments and liveness information for a single function.
///
/// Produced by [`analyze_calling_convention`] and consumed by the linear-scan allocator
/// and instruction emitter.
pub struct CallingConventionInfo {
    /// Whether the function is a leaf or non-leaf.
    pub function_class: FunctionClass,
    /// Fixed register pre-assignments derived from the IC20 calling convention:
    ///
    /// - Function parameters → `r0`, `r1`, ... (one per parameter, in declaration order).
    /// - The function's own return value → `r0`.
    /// - Each `Instruction::Call` argument → `r0`, `r1`, ... (at the call site).
    /// - Each `Instruction::Call` destination → `r0` (at the call site).
    ///
    /// The linear-scan allocator must honor every entry: the named temp must be placed in
    /// the named register for its entire live range.  When two entries for the same temp
    /// agree on the register the duplicate is harmless; when they disagree, the allocator
    /// must insert a `Move` to resolve the conflict.
    pub fixed: HashMap<TempId, Register>,
    /// For each call site (keyed by its `LinearPosition`), the temps whose live ranges
    /// straddle the call — i.e., defined strictly before the call and last-used strictly
    /// after it. These temps may be clobbered by the callee and must be spilled/reloaded.
    pub live_across_calls: HashMap<LinearPosition, Vec<TempId>>,
}

/// Map a 0-based argument index to the corresponding general-purpose register.
///
/// IC20's calling convention passes arguments left-to-right in `r0`, `r1`, …  Panics if
/// `index` is >= 16 (the IC10 register file only has `r0`–`r15`).
fn register_for_index(index: usize) -> Register {
    match index {
        0 => Register::R0,
        1 => Register::R1,
        2 => Register::R2,
        3 => Register::R3,
        4 => Register::R4,
        5 => Register::R5,
        6 => Register::R6,
        7 => Register::R7,
        8 => Register::R8,
        9 => Register::R9,
        10 => Register::R10,
        11 => Register::R11,
        12 => Register::R12,
        13 => Register::R13,
        14 => Register::R14,
        15 => Register::R15,
        _ => panic!(
            "argument index {} exceeds available registers (max 15)",
            index
        ),
    }
}

/// Collect the `LinearPosition` of every `Instruction::Call` in `function`.
///
/// Blocks are visited in the order given by `linear_map.block_order`, so the returned
/// positions are monotonically increasing.
pub fn find_call_sites(function: &Function, linear_map: &LinearMap) -> Vec<LinearPosition> {
    let mut positions = Vec::new();
    for &block_id in &linear_map.block_order {
        let block = &function.blocks[block_id.0];
        for (index, instruction) in block.instructions.iter().enumerate() {
            if matches!(instruction, Instruction::Call { .. }) {
                positions.push(linear_map.instruction_positions[&(block_id, index)]);
            }
        }
    }
    positions
}

/// For each call site, compute which temps are live across it.
///
/// A temp is live across a call at position `site` when its live range both starts
/// strictly before `site` (it carries a value into the call) and ends strictly after
/// `site` (that value is needed again after the call returns). Such temps must be
/// spilled before the call and reloaded afterwards.
///
/// Temps whose range ends exactly at `site` (argument temps whose last use is the call)
/// and temps whose range starts exactly at `site` (the call's return-value dest) are
/// intentionally excluded.
pub fn find_live_across_calls(
    call_sites: &[LinearPosition],
    live_ranges: &HashMap<TempId, LiveRange>,
) -> HashMap<LinearPosition, Vec<TempId>> {
    let mut result: HashMap<LinearPosition, Vec<TempId>> = HashMap::new();
    for &site in call_sites {
        let live_across: Vec<TempId> = live_ranges
            .iter()
            .filter(|(_, range)| range.start() < site && range.end() > site)
            .map(|(&temp, _)| temp)
            .collect();
        result.insert(site, live_across);
    }
    result
}

/// Determine whether `function` is a leaf (contains no `Call` instructions) or a
/// non-leaf (contains at least one `Call` instruction).
pub fn classify_function(function: &Function) -> FunctionClass {
    let has_call = function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .any(|instruction| matches!(instruction, Instruction::Call { .. }));
    if has_call {
        FunctionClass::NonLeaf
    } else {
        FunctionClass::Leaf
    }
}

/// Compute the calling-convention pre-assignments and call-site liveness for `function`.
///
/// Must be called after phi deconstruction so that the block graph and instruction lists
/// are in their final form before linearization.
///
/// Pre-assignments produced:
/// - **Parameters** (§5.2): the initial definition temp of each parameter is fixed to
///   `r0`, `r1`, ... in declaration order.
/// - **Return value** (§5.3): every `Terminator::Return(Some(temp))` fixes `temp` to `r0`.
/// - **Call arguments** (§5.4): at each `Instruction::Call`, each argument temp is fixed
///   to `r0`, `r1`, ... in argument order.
/// - **Call return value** (§5.5): the `dest` temp of each `Instruction::Call` is fixed
///   to `r0`.
pub fn analyze_calling_convention(
    function: &Function,
    linear_map: &LinearMap,
    live_ranges: &HashMap<TempId, LiveRange>,
) -> CallingConventionInfo {
    let function_class = classify_function(function);
    let call_sites = find_call_sites(function, linear_map);
    let live_across_calls = find_live_across_calls(&call_sites, live_ranges);

    let mut fixed: HashMap<TempId, Register> = HashMap::new();

    // §5.2 — parameter temps: the definition of each parameter at the function entry block
    // is the temp that the caller places the argument in.
    for (index, &symbol_id) in function.parameters.iter().enumerate() {
        if let Some(definitions) = function.variable_definitions.get(&symbol_id)
            && let Some(&(temp, _)) = definitions
                .iter()
                .find(|&&(_, block)| block == function.entry)
        {
            fixed.insert(temp, register_for_index(index));
        }
    }

    // §5.3 — function return value: the temp returned at each `Terminator::Return` must
    // be in `r0` when the return executes.
    for block in &function.blocks {
        if let Terminator::Return(Some(temp)) = &block.terminator {
            fixed.insert(*temp, Register::R0);
        }
    }

    // §5.4 and §5.5 — call sites.
    for block in &function.blocks {
        for instruction in &block.instructions {
            if let Instruction::Call { dest, args, .. } = instruction {
                for (index, &arg_temp) in args.iter().enumerate() {
                    fixed.insert(arg_temp, register_for_index(index));
                }
                if let Some(dest_temp) = dest {
                    fixed.insert(*dest_temp, Register::R0);
                }
            }
        }
    }

    CallingConventionInfo {
        function_class,
        fixed,
        live_across_calls,
    }
}
