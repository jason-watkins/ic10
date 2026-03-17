use std::collections::HashSet;

use crate::ir::UnaryOperator;
use crate::ir::cfg::{BlockId, Function, Instruction, Operation, Terminator};

/// Removes basic blocks that are not reachable from the entry block via a
/// forward traversal of the CFG edges.
pub(super) fn remove_unreachable_blocks(function: &mut Function) -> bool {
    let reachable = compute_reachable(function);
    let unreachable: HashSet<BlockId> = function
        .blocks
        .iter()
        .map(|block| block.id)
        .filter(|id| !reachable.contains(id))
        .collect();

    if unreachable.is_empty() {
        return false;
    }

    let mut changed = false;

    for block in &mut function.blocks {
        if unreachable.contains(&block.id) {
            continue;
        }
        let original_predecessor_count = block.predecessors.len();
        block
            .predecessors
            .retain(|predecessor| !unreachable.contains(predecessor));
        if block.predecessors.len() != original_predecessor_count {
            changed = true;
        }
        for instruction in &mut block.instructions {
            if let Instruction::Phi { args, .. } = instruction {
                let original_arg_count = args.len();
                args.retain(|(_, block_id)| !unreachable.contains(block_id));
                if args.len() != original_arg_count {
                    changed = true;
                }
            }
        }
    }

    for block in &mut function.blocks {
        if unreachable.contains(&block.id)
            && (!block.instructions.is_empty()
                || !matches!(block.terminator, Terminator::None)
                || !block.successors.is_empty())
        {
            block.instructions.clear();
            block.terminator = Terminator::None;
            block.successors.clear();
            block.predecessors.clear();
            changed = true;
        }
    }

    changed
}

fn compute_reachable(function: &Function) -> HashSet<BlockId> {
    let mut reachable = HashSet::new();
    let mut worklist = vec![function.entry];
    reachable.insert(function.entry);

    while let Some(block_id) = worklist.pop() {
        for &successor in &function.blocks[block_id.0].successors {
            if reachable.insert(successor) {
                worklist.push(successor);
            }
        }
    }

    reachable
}

/// Eliminate empty pass-through blocks: blocks with no instructions and a single
/// unconditional `Jump` to some other block.
///
/// For each such block B → C, every predecessor of B is redirected to jump
/// directly to C, phi arguments in C that came from B are re-attributed to
/// B's predecessors, and B is cleared.  `function.entry` is updated when B
/// was the entry block.  Runs to fixpoint because eliminating one block may
/// expose the next pass-through in the chain.
pub(super) fn merge_empty_blocks(function: &mut Function) -> bool {
    let mut changed = false;
    loop {
        let Some(block_id) = find_empty_pass_through_block(function) else {
            break;
        };
        let Terminator::Jump(target_id) = function.blocks[block_id.0].terminator else {
            unreachable!()
        };

        let predecessors: Vec<BlockId> = function.blocks[block_id.0].predecessors.clone();

        if function.entry == block_id {
            function.entry = target_id;
        }

        for &predecessor_id in &predecessors {
            let predecessor = &mut function.blocks[predecessor_id.0];
            replace_jump_target(&mut predecessor.terminator, block_id, target_id);
            for successor in predecessor.successors.iter_mut() {
                if *successor == block_id {
                    *successor = target_id;
                }
            }
            predecessor.successors.sort_unstable();
            predecessor.successors.dedup();
        }

        {
            let target = &mut function.blocks[target_id.0];
            target
                .predecessors
                .retain(|&predecessor| predecessor != block_id);
            for &predecessor_id in &predecessors {
                if !target.predecessors.contains(&predecessor_id) {
                    target.predecessors.push(predecessor_id);
                }
            }
            for instruction in &mut target.instructions {
                if let Instruction::Phi { args, .. } = instruction
                    && let Some(position) = args.iter().position(|&(_, block)| block == block_id)
                {
                    let (value, _) = args.remove(position);
                    for &predecessor_id in &predecessors {
                        if !args.iter().any(|&(_, block)| block == predecessor_id) {
                            args.push((value, predecessor_id));
                        }
                    }
                }
            }
        }

        let block = &mut function.blocks[block_id.0];
        block.predecessors.clear();
        block.successors.clear();
        block.terminator = Terminator::None;
        changed = true;
    }
    changed
}

/// Coalesce sequential blocks: when block A has a single successor B via an
/// unconditional `Jump`, and B has A as its sole predecessor, merge B into A
/// by appending B's instructions and adopting B's terminator. This eliminates
/// redundant labels and jumps between blocks that always execute in sequence.
pub(super) fn coalesce_blocks(function: &mut Function) -> bool {
    let mut changed = false;
    loop {
        let candidate = find_coalescable_pair(function);
        let Some((block_id, successor_id)) = candidate else {
            break;
        };

        let successor_instructions =
            std::mem::take(&mut function.blocks[successor_id.0].instructions);
        let successor_terminator = std::mem::replace(
            &mut function.blocks[successor_id.0].terminator,
            Terminator::None,
        );
        let successor_successors = std::mem::take(&mut function.blocks[successor_id.0].successors);
        function.blocks[successor_id.0].predecessors.clear();

        let block = &mut function.blocks[block_id.0];
        block.instructions.extend(successor_instructions);
        block.terminator = successor_terminator;
        block.successors = successor_successors;

        let new_successors: Vec<BlockId> = function.blocks[block_id.0].successors.clone();
        for new_successor_id in new_successors {
            let new_successor = &mut function.blocks[new_successor_id.0];
            for predecessor in &mut new_successor.predecessors {
                if *predecessor == successor_id {
                    *predecessor = block_id;
                }
            }
            for instruction in &mut new_successor.instructions {
                if let Instruction::Phi { args, .. } = instruction {
                    for (_, source_block) in args.iter_mut() {
                        if *source_block == successor_id {
                            *source_block = block_id;
                        }
                    }
                }
            }
        }

        changed = true;
    }
    changed
}

fn find_coalescable_pair(function: &Function) -> Option<(BlockId, BlockId)> {
    for block in &function.blocks {
        let Terminator::Jump(successor_id) = block.terminator else {
            continue;
        };
        if successor_id == block.id {
            continue;
        }
        if successor_id == function.entry {
            continue;
        }
        let successor = &function.blocks[successor_id.0];
        if successor.predecessors.len() == 1 && successor.predecessors[0] == block.id {
            return Some((block.id, successor_id));
        }
    }
    None
}

fn find_empty_pass_through_block(function: &Function) -> Option<BlockId> {
    for block in &function.blocks {
        let Terminator::Jump(target) = block.terminator else {
            continue;
        };
        if target != block.id && block.instructions.is_empty() {
            return Some(block.id);
        }
    }
    None
}

fn replace_jump_target(terminator: &mut Terminator, old: BlockId, new: BlockId) {
    match terminator {
        Terminator::Jump(target) if *target == old => *target = new,
        Terminator::Branch {
            true_block,
            false_block,
            ..
        } => {
            if *true_block == old {
                *true_block = new;
            }
            if *false_block == old {
                *false_block = new;
            }
        }
        Terminator::Return(_) | Terminator::None | Terminator::Jump(_) => {}
    }
}

/// Rewrite `Branch { condition: ¬t, true_block: A, false_block: B }` to
/// `Branch { condition: t, true_block: B, false_block: A }`.
///
/// Swapping the successor edges and using the un-negated condition eliminates
/// the `seqz` (logical-NOT) instruction from the critical path, leaving the
/// condition temp free for downstream fusion (e.g. `snan + bnez → bnan`).
/// The original `Not` instruction is left in place; dead-code elimination
/// removes it when it is no longer used.
pub(super) fn invert_negated_branches(function: &mut Function) -> bool {
    use super::utilities::build_def_map;
    use std::collections::HashMap;

    let def_map = build_def_map(function);
    let mut rewrites: HashMap<usize, (usize, usize)> = HashMap::new();

    for (block_index, block) in function.blocks.iter().enumerate() {
        if let Terminator::Branch { condition, .. } = &block.terminator
            && let Some(&(def_block, def_instr)) = def_map.get(condition)
        {
            let defining_instruction = &function.blocks[def_block].instructions[def_instr];
            if let Instruction::Assign {
                operation:
                    Operation::Unary {
                        operator: UnaryOperator::Not,
                        ..
                    },
                ..
            } = defining_instruction
            {
                rewrites.insert(block_index, (def_block, def_instr));
            }
        }
    }

    if rewrites.is_empty() {
        return false;
    }

    for (block_index, (def_block, def_instr)) in rewrites {
        let inner_operand = if let Instruction::Assign {
            operation: Operation::Unary { operand, .. },
            ..
        } = &function.blocks[def_block].instructions[def_instr]
        {
            *operand
        } else {
            unreachable!()
        };

        if let Terminator::Branch {
            condition,
            true_block,
            false_block,
        } = &mut function.blocks[block_index].terminator
        {
            *condition = inner_operand;
            std::mem::swap(true_block, false_block);
        }

        // Keep the successors list in sync: it is ordered [true_block, false_block] by
        // construction and is used for reverse-postorder traversal, which determines the
        // physical block layout and therefore which branch direction can be a fall-through.
        function.blocks[block_index].successors.reverse();
    }

    true
}
