use std::collections::HashMap;
use std::fmt::Write;

use crate::cfg::{BlockId, Function, Instruction, Operation, Terminator};

/// Deduplicate structurally identical blocks within a function.
///
/// Two blocks are considered identical when they have the same sequence of
/// instructions and the same terminator, modulo a consistent renaming of their
/// local `TempId`s.  When duplicates are found, all predecessors of the
/// duplicate blocks are redirected to one canonical representative and the
/// duplicates are cleared (later removed by unreachable-block elimination).
///
/// Blocks containing phi instructions are excluded because merging their
/// incoming edges would require inserting new phi arguments.
pub(super) fn deduplicate_blocks(function: &mut Function) -> bool {
    let mut canonical_to_blocks: HashMap<String, Vec<usize>> = HashMap::new();

    for (block_index, block) in function.blocks.iter().enumerate() {
        if matches!(block.terminator, Terminator::None) && block.instructions.is_empty() {
            continue;
        }
        let has_phi = block
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::Phi { .. }));
        if has_phi {
            continue;
        }
        let canonical = canonicalize_block(block);
        canonical_to_blocks
            .entry(canonical)
            .or_default()
            .push(block_index);
    }

    let mut redirect: HashMap<BlockId, BlockId> = HashMap::new();
    for block_indices in canonical_to_blocks.values() {
        if block_indices.len() < 2 {
            continue;
        }
        let canonical_index = if block_indices
            .iter()
            .any(|&index| function.blocks[index].id == function.entry)
        {
            *block_indices
                .iter()
                .find(|&&index| function.blocks[index].id == function.entry)
                .unwrap()
        } else {
            block_indices[0]
        };
        let canonical_id = function.blocks[canonical_index].id;
        for &duplicate_index in block_indices {
            let duplicate_id = function.blocks[duplicate_index].id;
            if duplicate_id != canonical_id {
                redirect.insert(duplicate_id, canonical_id);
            }
        }
    }

    if redirect.is_empty() {
        return false;
    }

    for block in &mut function.blocks {
        let block_id = block.id;
        if redirect.contains_key(&block_id) {
            continue;
        }

        replace_terminator_targets(&mut block.terminator, &redirect);

        for successor in &mut block.successors {
            if let Some(&canonical) = redirect.get(successor) {
                *successor = canonical;
            }
        }
        block.successors.sort_unstable();
        block.successors.dedup();

        for instruction in &mut block.instructions {
            if let Instruction::Phi { args, .. } = instruction {
                // Remove phi args from duplicate blocks when the canonical block
                // already has an entry — the canonical block's value covers all
                // paths that previously went through the duplicate.
                args.retain(|(_, source_block)| !redirect.contains_key(source_block));
            }
        }
    }

    for (&duplicate_id, &canonical_id) in &redirect {
        let predecessors: Vec<BlockId> = function.blocks[duplicate_id.0]
            .predecessors
            .drain(..)
            .collect();
        let canonical = &mut function.blocks[canonical_id.0];
        for predecessor_id in predecessors {
            if !canonical.predecessors.contains(&predecessor_id) {
                canonical.predecessors.push(predecessor_id);
            }
        }

        let duplicate = &mut function.blocks[duplicate_id.0];
        duplicate.instructions.clear();
        duplicate.terminator = Terminator::None;
        duplicate.successors.clear();
    }

    true
}

/// Build a canonical string representation of a block's contents.
///
/// `TempId`s are renumbered sequentially (first appearance = 0, second = 1, …)
/// so that two blocks with identical structure but different concrete `TempId`s
/// produce the same string.
fn canonicalize_block(block: &crate::cfg::BasicBlock) -> String {
    let mut mapping: HashMap<crate::cfg::TempId, usize> = HashMap::new();
    let mut next_index = 0usize;
    let mut output = String::new();

    let mut map = |temp: crate::cfg::TempId,
                   mapping: &mut HashMap<crate::cfg::TempId, usize>,
                   next: &mut usize|
     -> usize {
        *mapping.entry(temp).or_insert_with(|| {
            let index = *next;
            *next += 1;
            index
        })
    };

    for instruction in &block.instructions {
        canonicalize_instruction(
            instruction,
            &mut mapping,
            &mut next_index,
            &mut map,
            &mut output,
        );
        output.push('\n');
    }
    canonicalize_terminator(
        &block.terminator,
        &mut mapping,
        &mut next_index,
        &mut map,
        &mut output,
    );

    output
}

fn canonicalize_instruction(
    instruction: &Instruction,
    mapping: &mut HashMap<crate::cfg::TempId, usize>,
    next_index: &mut usize,
    map: &mut impl FnMut(
        crate::cfg::TempId,
        &mut HashMap<crate::cfg::TempId, usize>,
        &mut usize,
    ) -> usize,
    output: &mut String,
) {
    match instruction {
        Instruction::Assign { dest, operation } => {
            let dest_index = map(*dest, mapping, next_index);
            let _ = write!(output, "assign t{} ", dest_index);
            canonicalize_operation(operation, mapping, next_index, map, output);
        }
        Instruction::LoadDevice { dest, pin, field } => {
            let dest_index = map(*dest, mapping, next_index);
            let _ = write!(output, "load_device t{} {:?} {}", dest_index, pin, field);
        }
        Instruction::StoreDevice { pin, field, source } => {
            let source_index = map(*source, mapping, next_index);
            let _ = write!(output, "store_device {:?} {} t{}", pin, field, source_index);
        }
        Instruction::LoadSlot {
            dest,
            pin,
            slot,
            field,
        } => {
            let dest_index = map(*dest, mapping, next_index);
            let slot_index = map(*slot, mapping, next_index);
            let _ = write!(
                output,
                "load_slot t{} {:?} t{} {}",
                dest_index, pin, slot_index, field
            );
        }
        Instruction::StoreSlot {
            pin,
            slot,
            field,
            source,
        } => {
            let slot_index = map(*slot, mapping, next_index);
            let source_index = map(*source, mapping, next_index);
            let _ = write!(
                output,
                "store_slot {:?} t{} {} t{}",
                pin, slot_index, field, source_index
            );
        }
        Instruction::BatchRead {
            dest,
            hash,
            field,
            mode,
        } => {
            let dest_index = map(*dest, mapping, next_index);
            let hash_index = map(*hash, mapping, next_index);
            let _ = write!(
                output,
                "batch_read t{} t{} {} {:?}",
                dest_index, hash_index, field, mode
            );
        }
        Instruction::BatchWrite { hash, field, value } => {
            let hash_index = map(*hash, mapping, next_index);
            let value_index = map(*value, mapping, next_index);
            let _ = write!(
                output,
                "batch_write t{} {} t{}",
                hash_index, field, value_index
            );
        }
        Instruction::Call {
            dest,
            function,
            args,
        } => {
            if let Some(dest) = dest {
                let dest_index = map(*dest, mapping, next_index);
                let _ = write!(output, "call t{} {:?}", dest_index, function);
            } else {
                let _ = write!(output, "call void {:?}", function);
            }
            for arg in args {
                let arg_index = map(*arg, mapping, next_index);
                let _ = write!(output, " t{}", arg_index);
            }
        }
        Instruction::BuiltinCall {
            dest,
            function,
            args,
        } => {
            let dest_index = map(*dest, mapping, next_index);
            let _ = write!(output, "builtin t{} {:?}", dest_index, function);
            for arg in args {
                let arg_index = map(*arg, mapping, next_index);
                let _ = write!(output, " t{}", arg_index);
            }
        }
        Instruction::Sleep { duration } => {
            let duration_index = map(*duration, mapping, next_index);
            let _ = write!(output, "sleep t{}", duration_index);
        }
        Instruction::Yield => {
            let _ = write!(output, "yield");
        }
        Instruction::Phi { .. } => {
            unreachable!("phi instructions are excluded from deduplication");
        }
    }
}

fn canonicalize_operation(
    operation: &Operation,
    mapping: &mut HashMap<crate::cfg::TempId, usize>,
    next_index: &mut usize,
    map: &mut impl FnMut(
        crate::cfg::TempId,
        &mut HashMap<crate::cfg::TempId, usize>,
        &mut usize,
    ) -> usize,
    output: &mut String,
) {
    match operation {
        Operation::Copy(source) => {
            let source_index = map(*source, mapping, next_index);
            let _ = write!(output, "copy t{}", source_index);
        }
        Operation::Constant(value) => {
            let _ = write!(output, "const {}", f64_canonical(*value));
        }
        Operation::Parameter { index } => {
            let _ = write!(output, "param {}", index);
        }
        Operation::Binary {
            operator,
            left,
            right,
        } => {
            let left_index = map(*left, mapping, next_index);
            let right_index = map(*right, mapping, next_index);
            let _ = write!(
                output,
                "binary {:?} t{} t{}",
                operator, left_index, right_index
            );
        }
        Operation::Unary { operator, operand } => {
            let operand_index = map(*operand, mapping, next_index);
            let _ = write!(output, "unary {:?} t{}", operator, operand_index);
        }
        Operation::Cast {
            operand,
            target_type,
            source_type,
        } => {
            let operand_index = map(*operand, mapping, next_index);
            let _ = write!(
                output,
                "cast t{} {:?} {:?}",
                operand_index, source_type, target_type
            );
        }
        Operation::Select {
            condition,
            if_true,
            if_false,
        } => {
            let condition_index = map(*condition, mapping, next_index);
            let if_true_index = map(*if_true, mapping, next_index);
            let if_false_index = map(*if_false, mapping, next_index);
            let _ = write!(
                output,
                "select t{} t{} t{}",
                condition_index, if_true_index, if_false_index
            );
        }
    }
}

fn canonicalize_terminator(
    terminator: &Terminator,
    mapping: &mut HashMap<crate::cfg::TempId, usize>,
    next_index: &mut usize,
    map: &mut impl FnMut(
        crate::cfg::TempId,
        &mut HashMap<crate::cfg::TempId, usize>,
        &mut usize,
    ) -> usize,
    output: &mut String,
) {
    match terminator {
        Terminator::Jump(target) => {
            let _ = write!(output, "jump {:?}", target);
        }
        Terminator::Branch {
            condition,
            true_block,
            false_block,
        } => {
            let condition_index = map(*condition, mapping, next_index);
            let _ = write!(
                output,
                "branch t{} {:?} {:?}",
                condition_index, true_block, false_block
            );
        }
        Terminator::Return(Some(value)) => {
            let value_index = map(*value, mapping, next_index);
            let _ = write!(output, "return t{}", value_index);
        }
        Terminator::Return(None) => {
            let _ = write!(output, "return void");
        }
        Terminator::None => {
            let _ = write!(output, "none");
        }
    }
}

/// Produce a canonical string for an `f64`, ensuring that `0.0` and `-0.0` are
/// distinguished and NaN values are all mapped to the same representation.
fn f64_canonical(value: f64) -> String {
    if value.is_nan() {
        "NaN".to_string()
    } else {
        format!("{:?}", value.to_bits())
    }
}

fn replace_terminator_targets(terminator: &mut Terminator, redirect: &HashMap<BlockId, BlockId>) {
    match terminator {
        Terminator::Jump(target) => {
            if let Some(&canonical) = redirect.get(target) {
                *target = canonical;
            }
        }
        Terminator::Branch {
            true_block,
            false_block,
            ..
        } => {
            if let Some(&canonical) = redirect.get(true_block) {
                *true_block = canonical;
            }
            if let Some(&canonical) = redirect.get(false_block) {
                *false_block = canonical;
            }
        }
        Terminator::Return(_) | Terminator::None => {}
    }
}
