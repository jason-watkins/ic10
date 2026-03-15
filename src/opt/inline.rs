use std::collections::{HashMap, HashSet};

use crate::ir::cfg::{
    BasicBlock, BlockId, BlockRole, Function, Instruction, Operation, Program, TempId, Terminator,
};
use crate::ir::bound::SymbolId;

use super::utilities::{instruction_dest, substitute_in_instruction, substitute_in_terminator};

pub(super) fn inline_functions(program: &mut Program) {
    let call_graph = build_call_graph(program);

    let function_map: HashMap<SymbolId, usize> = program
        .functions
        .iter()
        .enumerate()
        .map(|(index, function)| (function.symbol_id, index))
        .collect();

    let mut inline_targets: HashSet<SymbolId> = HashSet::new();
    for function in &program.functions {
        let call_count = count_calls_to(program, function.symbol_id);
        if should_inline(function, call_count, &call_graph) {
            inline_targets.insert(function.symbol_id);
        }
    }

    if inline_targets.is_empty() {
        return;
    }

    let callee_snapshots: HashMap<SymbolId, Function> = inline_targets
        .iter()
        .filter_map(|&symbol| {
            let index = *function_map.get(&symbol)?;
            Some((symbol, clone_function(&program.functions[index])))
        })
        .collect();

    for function in &mut program.functions {
        loop {
            let call_site = find_inline_candidate(function, &inline_targets);
            let Some((block_index, instruction_index, callee_symbol)) = call_site else {
                break;
            };
            let callee = &callee_snapshots[&callee_symbol];
            inline_call_into_function(function, callee, block_index, instruction_index);
        }
    }

    let still_called: HashSet<SymbolId> = program
        .functions
        .iter()
        .flat_map(|f| {
            f.blocks.iter().flat_map(|b| {
                b.instructions.iter().filter_map(|i| {
                    if let Instruction::Call { function, .. } = i {
                        Some(*function)
                    } else {
                        None
                    }
                })
            })
        })
        .collect();

    program
        .functions
        .retain(|f| f.name == "main" || still_called.contains(&f.symbol_id));
}

fn build_call_graph(program: &Program) -> HashMap<SymbolId, HashSet<SymbolId>> {
    let mut graph: HashMap<SymbolId, HashSet<SymbolId>> = HashMap::new();
    for function in &program.functions {
        let callees = graph.entry(function.symbol_id).or_default();
        for block in &function.blocks {
            for instruction in &block.instructions {
                if let Instruction::Call {
                    function: callee, ..
                } = instruction
                {
                    callees.insert(*callee);
                }
            }
        }
    }
    graph
}

fn is_recursive(symbol: SymbolId, call_graph: &HashMap<SymbolId, HashSet<SymbolId>>) -> bool {
    let mut visited = HashSet::new();
    let mut stack = vec![symbol];
    while let Some(current) = stack.pop() {
        if !visited.insert(current) {
            continue;
        }
        if let Some(callees) = call_graph.get(&current) {
            if callees.contains(&symbol) {
                return true;
            }
            for &callee in callees {
                stack.push(callee);
            }
        }
    }
    false
}

fn count_body_instructions(function: &Function) -> usize {
    let mut count = 0;
    for block in &function.blocks {
        count += block.instructions.len();
        if !matches!(block.terminator, Terminator::None) {
            count += 1;
        }
    }
    count
}

fn count_calls_to(program: &Program, target: SymbolId) -> usize {
    let mut count = 0;
    for function in &program.functions {
        for block in &function.blocks {
            for instruction in &block.instructions {
                if let Instruction::Call {
                    function: callee, ..
                } = instruction
                    && *callee == target
                {
                    count += 1;
                }
            }
        }
    }
    count
}

/// Estimate the IC10 instruction cost of a call site (argument moves, jal, return move,
/// plus potential caller-save/callee-save overhead).
fn call_overhead(arg_count: usize, has_return: bool) -> usize {
    let mut cost = arg_count;
    cost += 1; // jal
    if has_return {
        cost += 1; // move result from r0
    }
    cost += 2; // push/pop ra (non-leaf callee)
    cost
}

fn should_inline(
    callee: &Function,
    call_count: usize,
    call_graph: &HashMap<SymbolId, HashSet<SymbolId>>,
) -> bool {
    if callee.name == "main" {
        return false;
    }
    if is_recursive(callee.symbol_id, call_graph) {
        return false;
    }
    if call_count == 0 {
        return false;
    }

    let body_size = count_body_instructions(callee);
    let has_return = callee.return_type.is_some();
    let overhead_per_call = call_overhead(callee.parameters.len(), has_return);

    let cost_without_inlining = body_size + overhead_per_call * call_count;
    let cost_with_inlining = body_size * call_count;

    cost_with_inlining <= cost_without_inlining
}

fn inline_call_into_function(
    caller: &mut Function,
    callee: &Function,
    call_block_index: usize,
    call_instruction_index: usize,
) {
    let call_instruction =
        caller.blocks[call_block_index].instructions[call_instruction_index].clone();
    let (call_dest, call_args) = match &call_instruction {
        Instruction::Call { dest, args, .. } => (*dest, args.clone()),
        _ => unreachable!("inline_call_into_function called on non-Call instruction"),
    };

    let temp_offset = caller.next_temp;
    let block_offset = caller.blocks.len();

    let callee_block_count = callee.blocks.len();
    let mut max_callee_temp = 0;
    for block in &callee.blocks {
        for instruction in &block.instructions {
            if let Some(dest) = instruction_dest(instruction) {
                max_callee_temp = max_callee_temp.max(dest.0 + 1);
            }
        }
    }

    let remap_temp = |t: TempId| -> TempId { TempId(t.0 + temp_offset) };
    let remap_block = |b: BlockId| -> BlockId { BlockId(b.0 + block_offset) };

    let merge_block_id = BlockId(block_offset + callee_block_count);
    let callee_entry = remap_block(callee.entry);

    let result_temp = call_dest.map(|_| TempId(temp_offset + max_callee_temp));
    let mut next_inline_temp =
        temp_offset + max_callee_temp + if result_temp.is_some() { 1 } else { 0 };
    let mut return_value_temps: Vec<(BlockId, TempId)> = Vec::new();

    let mut inlined_blocks: Vec<BasicBlock> = Vec::with_capacity(callee_block_count);
    for callee_block in &callee.blocks {
        let mut instructions: Vec<Instruction> = Vec::new();

        for instruction in &callee_block.instructions {
            let new_instruction = match instruction {
                Instruction::Assign { dest, operation } => {
                    let remapped_operation = match operation {
                        Operation::Parameter { index } => Operation::Copy(call_args[*index]),
                        Operation::Copy(source) => Operation::Copy(remap_temp(*source)),
                        Operation::Constant(value) => Operation::Constant(*value),
                        Operation::Binary {
                            operator,
                            left,
                            right,
                        } => Operation::Binary {
                            operator: *operator,
                            left: remap_temp(*left),
                            right: remap_temp(*right),
                        },
                        Operation::Unary { operator, operand } => Operation::Unary {
                            operator: *operator,
                            operand: remap_temp(*operand),
                        },
                        Operation::Cast {
                            operand,
                            target_type,
                            source_type,
                        } => Operation::Cast {
                            operand: remap_temp(*operand),
                            target_type: *target_type,
                            source_type: *source_type,
                        },
                        Operation::Select {
                            condition,
                            if_true,
                            if_false,
                        } => Operation::Select {
                            condition: remap_temp(*condition),
                            if_true: remap_temp(*if_true),
                            if_false: remap_temp(*if_false),
                        },
                    };
                    Instruction::Assign {
                        dest: remap_temp(*dest),
                        operation: remapped_operation,
                    }
                }
                Instruction::Phi { dest, args } => Instruction::Phi {
                    dest: remap_temp(*dest),
                    args: args
                        .iter()
                        .map(|&(t, b)| (remap_temp(t), remap_block(b)))
                        .collect(),
                },
                Instruction::LoadDevice { dest, pin, field } => Instruction::LoadDevice {
                    dest: remap_temp(*dest),
                    pin: *pin,
                    field: field.clone(),
                },
                Instruction::StoreDevice { pin, field, source } => Instruction::StoreDevice {
                    pin: *pin,
                    field: field.clone(),
                    source: remap_temp(*source),
                },
                Instruction::LoadSlot {
                    dest,
                    pin,
                    slot,
                    field,
                } => Instruction::LoadSlot {
                    dest: remap_temp(*dest),
                    pin: *pin,
                    slot: remap_temp(*slot),
                    field: field.clone(),
                },
                Instruction::StoreSlot {
                    pin,
                    slot,
                    field,
                    source,
                } => Instruction::StoreSlot {
                    pin: *pin,
                    slot: remap_temp(*slot),
                    field: field.clone(),
                    source: remap_temp(*source),
                },
                Instruction::BatchRead {
                    dest,
                    hash,
                    field,
                    mode,
                } => Instruction::BatchRead {
                    dest: remap_temp(*dest),
                    hash: remap_temp(*hash),
                    field: field.clone(),
                    mode: *mode,
                },
                Instruction::BatchWrite { hash, field, value } => Instruction::BatchWrite {
                    hash: remap_temp(*hash),
                    field: field.clone(),
                    value: remap_temp(*value),
                },
                Instruction::Call {
                    dest,
                    function,
                    args,
                } => Instruction::Call {
                    dest: dest.map(&remap_temp),
                    function: *function,
                    args: args.iter().map(|&a| remap_temp(a)).collect(),
                },
                Instruction::IntrinsicCall {
                    dest,
                    function,
                    args,
                } => Instruction::IntrinsicCall {
                    dest: remap_temp(*dest),
                    function: *function,
                    args: args.iter().map(|&a| remap_temp(a)).collect(),
                },
                Instruction::Sleep { duration } => Instruction::Sleep {
                    duration: remap_temp(*duration),
                },
                Instruction::Yield => Instruction::Yield,
            };
            instructions.push(new_instruction);
        }

        let terminator = match &callee_block.terminator {
            Terminator::Jump(target) => Terminator::Jump(remap_block(*target)),
            Terminator::Branch {
                condition,
                true_block,
                false_block,
            } => Terminator::Branch {
                condition: remap_temp(*condition),
                true_block: remap_block(*true_block),
                false_block: remap_block(*false_block),
            },
            Terminator::Return(value) => {
                if let Some(return_value) = value
                    && call_dest.is_some()
                {
                    let value_temp = TempId(next_inline_temp);
                    next_inline_temp += 1;
                    instructions.push(Instruction::Assign {
                        dest: value_temp,
                        operation: Operation::Copy(remap_temp(*return_value)),
                    });
                    return_value_temps.push((remap_block(callee_block.id), value_temp));
                }
                Terminator::Jump(merge_block_id)
            }
            Terminator::None => Terminator::None,
        };

        inlined_blocks.push(BasicBlock {
            id: remap_block(callee_block.id),
            role: BlockRole::Inlined {
                callee_name: callee.name.clone(),
                original_role: Box::new(callee_block.role.clone()),
            },
            instructions,
            terminator,
            predecessors: callee_block
                .predecessors
                .iter()
                .map(|&b| remap_block(b))
                .collect(),
            successors: callee_block
                .successors
                .iter()
                .map(|&b| remap_block(b))
                .collect(),
        });
    }

    for block in &mut inlined_blocks {
        if let Terminator::Jump(target) = block.terminator
            && target == merge_block_id
            && !block.successors.contains(&merge_block_id)
        {
            block.successors.push(merge_block_id);
        }
    }

    let merge_predecessors: Vec<BlockId> = inlined_blocks
        .iter()
        .filter(|b| b.successors.contains(&merge_block_id))
        .map(|b| b.id)
        .collect();

    let (final_result_temp, merge_instructions) = if return_value_temps.len() > 1 {
        let result = result_temp.unwrap();
        let phi = Instruction::Phi {
            dest: result,
            args: return_value_temps
                .iter()
                .map(|&(block, temp)| (temp, block))
                .collect(),
        };
        (Some(result), vec![phi])
    } else if return_value_temps.len() == 1 {
        (Some(return_value_temps[0].1), vec![])
    } else {
        (result_temp, vec![])
    };

    let call_block_id = caller.blocks[call_block_index].id;
    let post_call_instructions: Vec<Instruction> = caller.blocks[call_block_index]
        .instructions
        .split_off(call_instruction_index + 1);
    caller.blocks[call_block_index].instructions.pop();

    let original_terminator = std::mem::replace(
        &mut caller.blocks[call_block_index].terminator,
        Terminator::Jump(callee_entry),
    );
    let original_successors = std::mem::take(&mut caller.blocks[call_block_index].successors);

    caller.blocks[call_block_index].successors = vec![callee_entry];

    let callee_entry_idx = callee_entry.0 - block_offset;
    inlined_blocks[callee_entry_idx]
        .predecessors
        .push(call_block_id);

    let mut substitutions: HashMap<TempId, TempId> = HashMap::new();
    if let (Some(call_dest_temp), Some(result)) = (call_dest, final_result_temp) {
        substitutions.insert(call_dest_temp, result);
    }

    let mut merge_post_instructions = merge_instructions;
    merge_post_instructions.extend(post_call_instructions);

    if !substitutions.is_empty() {
        for instruction in &mut merge_post_instructions {
            substitute_in_instruction(instruction, &substitutions);
        }
        substitute_in_terminator(&mut original_terminator.clone(), &substitutions);
    }

    let mut final_terminator = original_terminator;
    if !substitutions.is_empty() {
        substitute_in_terminator(&mut final_terminator, &substitutions);
    }

    for &successor_id in &original_successors {
        let successor = &mut caller.blocks[successor_id.0];
        for predecessor in &mut successor.predecessors {
            if *predecessor == call_block_id {
                *predecessor = merge_block_id;
            }
        }
        for instruction in &mut successor.instructions {
            if let Instruction::Phi { args, .. } = instruction {
                for (_, block) in args.iter_mut() {
                    if *block == call_block_id {
                        *block = merge_block_id;
                    }
                }
            }
        }
    }

    let merge_block = BasicBlock {
        id: merge_block_id,
        role: BlockRole::Generic,
        instructions: merge_post_instructions,
        terminator: final_terminator,
        predecessors: merge_predecessors,
        successors: original_successors,
    };

    caller.blocks.extend(inlined_blocks);
    caller.blocks.push(merge_block);

    caller.next_temp = next_inline_temp;
}

fn find_inline_candidate(
    function: &Function,
    inline_targets: &HashSet<SymbolId>,
) -> Option<(usize, usize, SymbolId)> {
    for (block_index, block) in function.blocks.iter().enumerate() {
        for (instruction_index, instruction) in block.instructions.iter().enumerate() {
            if let Instruction::Call {
                function: callee, ..
            } = instruction
                && inline_targets.contains(callee)
            {
                return Some((block_index, instruction_index, *callee));
            }
        }
    }
    None
}

fn clone_function(function: &Function) -> Function {
    Function {
        name: function.name.clone(),
        symbol_id: function.symbol_id,
        parameters: function.parameters.clone(),
        return_type: function.return_type,
        blocks: function
            .blocks
            .iter()
            .map(|block| BasicBlock {
                id: block.id,
                role: block.role.clone(),
                instructions: block.instructions.clone(),
                terminator: block.terminator.clone(),
                predecessors: block.predecessors.clone(),
                successors: block.successors.clone(),
            })
            .collect(),
        entry: function.entry,
        variable_definitions: function.variable_definitions.clone(),
        variable_temps: function.variable_temps.clone(),
        immediate_dominators: function.immediate_dominators.clone(),
        dominance_frontiers: function.dominance_frontiers.clone(),
        next_temp: function.next_temp,
    }
}
