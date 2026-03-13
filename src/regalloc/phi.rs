use std::collections::HashMap;

use crate::cfg::{BasicBlock, BlockId, Function, Instruction, Operation, TempId, Terminator};

/// Eliminate all phi instructions in `function` by inserting explicit copy instructions
/// at the end of predecessor blocks.
///
/// Critical edges are split first so that copies can always be placed unambiguously.
/// When multiple phis in the same block produce a set of simultaneous copies for a single
/// predecessor, the copies are sequenced to handle cyclic dependencies correctly.
pub fn deconstruct_phis(function: &mut Function) {
    split_critical_edges(function);

    let mut copies_per_edge: HashMap<(BlockId, BlockId), Vec<(TempId, TempId)>> = HashMap::new();

    for block in &function.blocks {
        for instruction in &block.instructions {
            if let Instruction::Phi { dest, args } = instruction {
                for &(source, predecessor) in args {
                    copies_per_edge
                        .entry((block.id, predecessor))
                        .or_default()
                        .push((*dest, source));
                }
            }
        }
    }

    for ((_target, predecessor), copies) in &copies_per_edge {
        let sequenced = sequence_parallel_copies(copies, function);
        for (dest, source) in sequenced {
            function.blocks[predecessor.0]
                .instructions
                .push(Instruction::Assign {
                    dest,
                    operation: Operation::Copy(source),
                });
        }
    }

    for block in &mut function.blocks {
        block
            .instructions
            .retain(|instruction| !matches!(instruction, Instruction::Phi { .. }));
    }
}

/// Split all critical edges that carry phi arguments.
///
/// An edge from `predecessor` to `successor` is critical when `predecessor` has multiple
/// successors and `successor` has multiple predecessors. Inserting copies at the end of
/// such a predecessor would affect all outgoing paths, not just the one to `successor`.
/// Splitting the edge introduces a new block that contains only the copies and an
/// unconditional jump.
fn split_critical_edges(function: &mut Function) {
    let mut edges_to_split: Vec<(BlockId, BlockId)> = Vec::new();

    for block in &function.blocks {
        let has_phis = block
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::Phi { .. }));
        if !has_phis || block.predecessors.len() <= 1 {
            continue;
        }
        for &predecessor in &block.predecessors {
            if function.blocks[predecessor.0].successors.len() > 1 {
                edges_to_split.push((predecessor, block.id));
            }
        }
    }

    for (predecessor, successor) in edges_to_split {
        split_edge(function, predecessor, successor);
    }
}

/// Insert a new empty block on the edge from `predecessor` to `successor`, updating all
/// predecessor/successor lists, terminators, and phi arguments.
fn split_edge(function: &mut Function, predecessor: BlockId, successor: BlockId) {
    let new_id = BlockId(function.blocks.len());
    function.blocks.push(BasicBlock {
        id: new_id,
        instructions: Vec::new(),
        terminator: Terminator::Jump(successor),
        predecessors: vec![predecessor],
        successors: vec![successor],
    });

    for target in &mut function.blocks[predecessor.0].successors {
        if *target == successor {
            *target = new_id;
        }
    }
    rewrite_terminator_target(
        &mut function.blocks[predecessor.0].terminator,
        successor,
        new_id,
    );

    for source in &mut function.blocks[successor.0].predecessors {
        if *source == predecessor {
            *source = new_id;
        }
    }
    for instruction in &mut function.blocks[successor.0].instructions {
        if let Instruction::Phi { args, .. } = instruction {
            for (_, block_id) in args.iter_mut() {
                if *block_id == predecessor {
                    *block_id = new_id;
                }
            }
        }
    }
}

/// Rewrite branch/jump targets in a terminator, replacing `old_target` with `new_target`.
fn rewrite_terminator_target(
    terminator: &mut Terminator,
    old_target: BlockId,
    new_target: BlockId,
) {
    match terminator {
        Terminator::Jump(target) if *target == old_target => {
            *target = new_target;
        }
        Terminator::Branch {
            true_block,
            false_block,
            ..
        } => {
            if *true_block == old_target {
                *true_block = new_target;
            }
            if *false_block == old_target {
                *false_block = new_target;
            }
        }
        _ => {}
    }
}

/// Sequence a set of simultaneous copies into an order that can be executed sequentially.
///
/// When no copy's destination is used as a source by another copy, the copies can be emitted
/// in any order. When there are dependencies (e.g. `a <- b` and `b <- c`), dependent copies
/// are emitted after the ones that read their destination. Cycles (e.g. `a <- b` and `b <- a`)
/// are broken by introducing a fresh temporary.
pub(crate) fn sequence_parallel_copies(
    copies: &[(TempId, TempId)],
    function: &mut Function,
) -> Vec<(TempId, TempId)> {
    let mut remaining: Vec<(TempId, TempId)> = copies
        .iter()
        .filter(|(dest, source)| dest != source)
        .copied()
        .collect();

    let mut result = Vec::new();

    loop {
        if remaining.is_empty() {
            break;
        }

        let ready = remaining
            .iter()
            .position(|(dest, _)| !remaining.iter().any(|(_, source)| source == dest));

        if let Some(index) = ready {
            result.push(remaining.remove(index));
        } else {
            let (dest, _) = remaining[0];
            let temporary = function.fresh_temp();
            result.push((temporary, dest));
            for (_, source) in remaining.iter_mut() {
                if *source == dest {
                    *source = temporary;
                }
            }
        }
    }

    result
}
