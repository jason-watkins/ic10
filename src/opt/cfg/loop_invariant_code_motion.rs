use std::collections::{HashMap, HashSet};

use crate::ir::Intrinsic;
use crate::ir::bound::StaticId;
use crate::ir::cfg::{
    BasicBlock, BlockId, BlockRole, Function, Instruction, Operation, TempId, Terminator,
};

use super::utilities::{build_def_map, instruction_target, instruction_uses};

/// Hoists loop-invariant instructions into pre-header blocks.
///
/// An instruction is loop-invariant when all of its operands are defined
/// outside the loop or are themselves loop-invariant. For each natural loop,
/// a dedicated pre-header block is created (or reused) and qualifying
/// instructions are moved there.
pub(super) fn loop_invariant_code_motion(function: &mut Function) -> bool {
    let loops = find_natural_loops(function);
    if loops.is_empty() {
        return false;
    }

    let mut changed = false;
    for natural_loop in &loops {
        changed |= hoist_invariants(function, natural_loop);
    }
    changed
}

struct NaturalLoop {
    header: BlockId,
    blocks: HashSet<BlockId>,
}

fn find_natural_loops(function: &Function) -> Vec<NaturalLoop> {
    let mut loops: Vec<NaturalLoop> = Vec::new();
    let mut header_indices: HashMap<BlockId, usize> = HashMap::new();

    for block in &function.blocks {
        for &successor in &block.successors {
            if function.dominates(successor, block.id) {
                if let Some(&index) = header_indices.get(&successor) {
                    let body = compute_loop_body(function, successor, block.id);
                    loops[index].blocks.extend(body);
                } else {
                    let body = compute_loop_body(function, successor, block.id);
                    header_indices.insert(successor, loops.len());
                    loops.push(NaturalLoop {
                        header: successor,
                        blocks: body,
                    });
                }
            }
        }
    }

    loops
}

fn compute_loop_body(
    function: &Function,
    header: BlockId,
    back_edge_source: BlockId,
) -> HashSet<BlockId> {
    let mut body = HashSet::new();
    body.insert(header);
    if header == back_edge_source {
        return body;
    }
    let mut worklist = vec![back_edge_source];
    body.insert(back_edge_source);
    while let Some(block_id) = worklist.pop() {
        for &predecessor in &function.blocks[block_id.0].predecessors {
            if body.insert(predecessor) {
                worklist.push(predecessor);
            }
        }
    }
    body
}

fn hoist_invariants(function: &mut Function, natural_loop: &NaturalLoop) -> bool {
    let def_map = build_def_map(function);
    let loop_block_indices: HashSet<usize> = natural_loop.blocks.iter().map(|id| id.0).collect();

    let has_calls = natural_loop.blocks.iter().any(|&block_id| {
        function.blocks[block_id.0]
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::Call { .. }))
    });

    let written_statics: HashSet<StaticId> = natural_loop
        .blocks
        .iter()
        .flat_map(|&block_id| {
            function.blocks[block_id.0]
                .instructions
                .iter()
                .filter_map(|instruction| {
                    if let Instruction::StoreStatic { static_id, .. } = instruction {
                        Some(*static_id)
                    } else {
                        None
                    }
                })
        })
        .collect();

    let mut invariant_temps: HashSet<TempId> = HashSet::new();
    loop {
        let mut progress = false;
        for &block_id in &natural_loop.blocks {
            for instruction in &function.blocks[block_id.0].instructions {
                let target = match instruction_target(instruction) {
                    Some(d) => d,
                    None => continue,
                };
                if invariant_temps.contains(&target) {
                    continue;
                }
                if !is_hoistable(instruction, has_calls, &written_statics) {
                    continue;
                }
                let all_operands_invariant =
                    instruction_uses(instruction).iter().all(|used_temp| {
                        match def_map.get(used_temp) {
                            Some(&(block_index, _)) => {
                                !loop_block_indices.contains(&block_index)
                                    || invariant_temps.contains(used_temp)
                            }
                            None => true,
                        }
                    });
                if all_operands_invariant {
                    invariant_temps.insert(target);
                    progress = true;
                }
            }
        }
        if !progress {
            break;
        }
    }

    if invariant_temps.is_empty() {
        return false;
    }

    let preheader_id = ensure_preheader(function, natural_loop);

    let mut hoisted: Vec<Instruction> = Vec::new();
    for block_index in 0..function.blocks.len() {
        if !natural_loop.blocks.contains(&BlockId(block_index)) {
            continue;
        }
        for instruction in &function.blocks[block_index].instructions {
            if let Some(target) = instruction_target(instruction)
                && invariant_temps.contains(&target)
            {
                hoisted.push(instruction.clone());
            }
        }
    }

    for block_index in 0..function.blocks.len() {
        if !natural_loop.blocks.contains(&BlockId(block_index)) {
            continue;
        }
        function.blocks[block_index]
            .instructions
            .retain(|instruction| match instruction_target(instruction) {
                Some(target) => !invariant_temps.contains(&target),
                None => true,
            });
    }

    function.blocks[preheader_id.0].instructions.extend(hoisted);

    true
}

fn is_hoistable(
    instruction: &Instruction,
    loop_has_calls: bool,
    written_statics: &HashSet<StaticId>,
) -> bool {
    match instruction {
        Instruction::Assign { operation, .. } => match operation {
            Operation::Copy(_)
            | Operation::Constant(_)
            | Operation::Binary { .. }
            | Operation::Unary { .. }
            | Operation::Cast { .. }
            | Operation::Select { .. } => true,
            Operation::Parameter { .. } => false,
        },
        Instruction::IntrinsicCall { function, .. } => *function != Intrinsic::Rand,
        Instruction::LoadStatic { static_id, .. } => {
            !loop_has_calls && !written_statics.contains(static_id)
        }
        _ => false,
    }
}

fn ensure_preheader(function: &mut Function, natural_loop: &NaturalLoop) -> BlockId {
    let header = natural_loop.header;

    let mut entry_predecessors = Vec::new();
    for &predecessor in &function.blocks[header.0].predecessors {
        if !natural_loop.blocks.contains(&predecessor) {
            entry_predecessors.push(predecessor);
        }
    }

    if entry_predecessors.len() == 1 {
        let candidate = entry_predecessors[0];
        if function.blocks[candidate.0].successors == [header] {
            return candidate;
        }
    }

    let loop_index = match &function.blocks[header.0].role {
        BlockRole::LoopStart(n) => *n,
        _ => 0,
    };

    let preheader_id = BlockId(function.blocks.len());
    let preheader = BasicBlock {
        id: preheader_id,
        role: BlockRole::LoopPreHeader(loop_index),
        instructions: Vec::new(),
        terminator: Terminator::Jump(header),
        predecessors: entry_predecessors.clone(),
        successors: vec![header],
    };
    function.blocks.push(preheader);

    for &predecessor_id in &entry_predecessors {
        let predecessor = &mut function.blocks[predecessor_id.0];
        for successor in &mut predecessor.successors {
            if *successor == header {
                *successor = preheader_id;
            }
        }
        match &mut predecessor.terminator {
            Terminator::Jump(target) if *target == header => {
                *target = preheader_id;
            }
            Terminator::Branch {
                true_block,
                false_block,
                ..
            } => {
                if *true_block == header {
                    *true_block = preheader_id;
                }
                if *false_block == header {
                    *false_block = preheader_id;
                }
            }
            _ => {}
        }
    }

    let header_block = &mut function.blocks[header.0];
    header_block
        .predecessors
        .retain(|p| !entry_predecessors.contains(p));
    header_block.predecessors.push(preheader_id);

    for instruction in &mut function.blocks[header.0].instructions {
        if let Instruction::Phi { args, .. } = instruction {
            for (_, block_id) in args.iter_mut() {
                if entry_predecessors.contains(block_id) {
                    *block_id = preheader_id;
                }
            }
        }
    }

    preheader_id
}
