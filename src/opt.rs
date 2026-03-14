use std::collections::{HashMap, HashSet, VecDeque};

use crate::ast::{BinaryOperator, BuiltinFunction, Type, UnaryOperator};
use crate::cfg::{
    BasicBlock, BlockId, Function, Instruction, Operation, Program, TempId, Terminator,
};
use crate::resolved::SymbolId;

/// Optimize all functions in a CFG program.
///
/// First inlines eligible function calls to reduce overall program size,
/// then runs constant propagation, copy propagation, global value numbering,
/// and dead-code elimination in a fixpoint loop until no further
/// simplifications are possible.
pub fn optimize_program(program: &mut Program) {
    inline_functions(program);
    for function in &mut program.functions {
        optimize(function);
    }
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

    // Cost if not inlined: body_size (function exists once) + overhead_per_call * call_count
    let cost_without_inlining = body_size + overhead_per_call * call_count;

    // Cost if inlined: body_size * call_count (body duplicated at each site), no function overhead
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
                Instruction::BuiltinCall {
                    dest,
                    function,
                    args,
                } => Instruction::BuiltinCall {
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
                    && let Some(result) = result_temp
                {
                    instructions.push(Instruction::Assign {
                        dest: result,
                        operation: Operation::Copy(remap_temp(*return_value)),
                    });
                }
                Terminator::Jump(merge_block_id)
            }
            Terminator::None => Terminator::None,
        };

        inlined_blocks.push(BasicBlock {
            id: remap_block(callee_block.id),
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

    // Return blocks in the callee had no successors. After replacing Return →
    // Jump(merge_block_id), add the merge block as a successor.
    for block in &mut inlined_blocks {
        if let Terminator::Jump(target) = block.terminator
            && target == merge_block_id
            && !block.successors.contains(&merge_block_id)
        {
            block.successors.push(merge_block_id);
        }
    }

    // Collect predecessor ids for merge block (all blocks that return/jump to it)
    let merge_predecessors: Vec<BlockId> = inlined_blocks
        .iter()
        .filter(|b| b.successors.contains(&merge_block_id))
        .map(|b| b.id)
        .collect();

    // If the result temp is used from multiple return points, create a phi node
    let merge_instructions = if let Some(result) = result_temp
        && merge_predecessors.len() > 1
    {
        vec![Instruction::Phi {
            dest: result,
            args: merge_predecessors
                .iter()
                .map(|&pred| (result, pred))
                .collect(),
        }]
    } else {
        vec![]
    };

    // Split the caller's block at the call site
    let call_block_id = caller.blocks[call_block_index].id;
    let post_call_instructions: Vec<Instruction> = caller.blocks[call_block_index]
        .instructions
        .split_off(call_instruction_index + 1);
    // Remove the call instruction itself
    caller.blocks[call_block_index].instructions.pop();

    let original_terminator = std::mem::replace(
        &mut caller.blocks[call_block_index].terminator,
        Terminator::Jump(callee_entry),
    );
    let original_successors = std::mem::take(&mut caller.blocks[call_block_index].successors);

    // Add callee_entry as successor of the call block
    caller.blocks[call_block_index].successors = vec![callee_entry];

    // Add call_block_id as predecessor of callee_entry
    let callee_entry_idx = callee_entry.0 - block_offset;
    inlined_blocks[callee_entry_idx]
        .predecessors
        .push(call_block_id);

    // If the call had a dest, substitute it with result_temp in post-call instructions
    let mut substitutions: HashMap<TempId, TempId> = HashMap::new();
    if let (Some(call_dest_temp), Some(result)) = (call_dest, result_temp) {
        substitutions.insert(call_dest_temp, result);
    }

    let mut merge_post_instructions = merge_instructions;
    merge_post_instructions.extend(post_call_instructions);

    // Apply substitutions to the post-call instructions
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

    // Fix original successors: update their predecessors from call_block_id to merge_block_id
    for &successor_id in &original_successors {
        let successor = &mut caller.blocks[successor_id.0];
        for predecessor in &mut successor.predecessors {
            if *predecessor == call_block_id {
                *predecessor = merge_block_id;
            }
        }
        // Update phi arguments in successor blocks
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

    // Create the merge block
    let merge_block = BasicBlock {
        id: merge_block_id,
        instructions: merge_post_instructions,
        terminator: final_terminator,
        predecessors: merge_predecessors,
        successors: original_successors,
    };

    // Append all inlined blocks and the merge block to the caller
    caller.blocks.extend(inlined_blocks);
    caller.blocks.push(merge_block);

    // Update next_temp
    caller.next_temp = temp_offset + max_callee_temp + if result_temp.is_some() { 1 } else { 0 };
}

fn inline_functions(program: &mut Program) {
    let call_graph = build_call_graph(program);

    let function_map: HashMap<SymbolId, usize> = program
        .functions
        .iter()
        .enumerate()
        .map(|(index, function)| (function.symbol_id, index))
        .collect();

    // Determine which functions to inline. We clone each callee since we'll
    // be modifying callers in-place.
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

    // Snapshot the callees before mutation
    let callee_snapshots: HashMap<SymbolId, Function> = inline_targets
        .iter()
        .filter_map(|&symbol| {
            let index = *function_map.get(&symbol)?;
            Some((symbol, clone_function(&program.functions[index])))
        })
        .collect();

    // Inline into each caller
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

    // Remove functions that were fully inlined and are no longer called
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

fn optimize(function: &mut Function) {
    let mut iterations = 0;
    loop {
        let mut changed = false;
        changed |= constant_propagation(function);
        changed |= copy_propagation(function);
        changed |= global_value_numbering(function);
        changed |= dead_code_elimination(function);
        changed |= remove_unreachable_blocks(function);
        changed |= coalesce_blocks(function);
        changed |= merge_empty_blocks(function);
        if !changed {
            break;
        }
        iterations += 1;
        assert!(
            iterations <= 100,
            "optimization loop failed to converge after {} iterations for function '{}'",
            iterations,
            function.name
        );
    }
}

fn instruction_dest(instruction: &Instruction) -> Option<TempId> {
    match instruction {
        Instruction::Assign { dest, .. }
        | Instruction::Phi { dest, .. }
        | Instruction::LoadDevice { dest, .. }
        | Instruction::LoadSlot { dest, .. }
        | Instruction::BatchRead { dest, .. }
        | Instruction::BuiltinCall { dest, .. } => Some(*dest),
        Instruction::Call { dest, .. } => *dest,
        Instruction::StoreDevice { .. }
        | Instruction::StoreSlot { .. }
        | Instruction::BatchWrite { .. }
        | Instruction::Sleep { .. }
        | Instruction::Yield => None,
    }
}

fn instruction_uses(instruction: &Instruction) -> Vec<TempId> {
    match instruction {
        Instruction::Assign { operation, .. } => operation_uses(operation),
        Instruction::Phi { args, .. } => args.iter().map(|&(temp, _)| temp).collect(),
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

fn terminator_uses(terminator: &Terminator) -> Vec<TempId> {
    match terminator {
        Terminator::Branch { condition, .. } => vec![*condition],
        Terminator::Return(Some(value)) => vec![*value],
        Terminator::Jump(_) | Terminator::Return(None) | Terminator::None => vec![],
    }
}

fn has_side_effects(instruction: &Instruction) -> bool {
    matches!(
        instruction,
        Instruction::StoreDevice { .. }
            | Instruction::StoreSlot { .. }
            | Instruction::BatchWrite { .. }
            | Instruction::Call { .. }
            | Instruction::Sleep { .. }
            | Instruction::Yield
    )
}

fn build_def_map(function: &Function) -> HashMap<TempId, (usize, usize)> {
    let mut map = HashMap::new();
    for (block_index, block) in function.blocks.iter().enumerate() {
        for (instruction_index, instruction) in block.instructions.iter().enumerate() {
            if let Some(dest) = instruction_dest(instruction) {
                map.insert(dest, (block_index, instruction_index));
            }
        }
    }
    map
}

fn apply_substitutions(function: &mut Function, substitutions: &HashMap<TempId, TempId>) {
    if substitutions.is_empty() {
        return;
    }
    let resolved = resolve_substitution_chains(substitutions);
    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            substitute_in_instruction(instruction, &resolved);
        }
        substitute_in_terminator(&mut block.terminator, &resolved);
    }
}

fn resolve_substitution_chains(substitutions: &HashMap<TempId, TempId>) -> HashMap<TempId, TempId> {
    let mut resolved = HashMap::new();
    for &key in substitutions.keys() {
        let mut target = key;
        let mut visited = HashSet::new();
        while let Some(&next) = substitutions.get(&target) {
            if !visited.insert(target) {
                break;
            }
            target = next;
        }
        if target != key {
            resolved.insert(key, target);
        }
    }
    resolved
}

fn substitute_temp(temp: &mut TempId, substitutions: &HashMap<TempId, TempId>) {
    if let Some(&replacement) = substitutions.get(temp) {
        *temp = replacement;
    }
}

fn substitute_in_instruction(
    instruction: &mut Instruction,
    substitutions: &HashMap<TempId, TempId>,
) {
    match instruction {
        Instruction::Assign { operation, .. } => {
            substitute_in_operation(operation, substitutions);
        }
        Instruction::Phi { args, .. } => {
            for (temp, _) in args.iter_mut() {
                substitute_temp(temp, substitutions);
            }
        }
        Instruction::LoadDevice { .. } => {}
        Instruction::StoreDevice { source, .. } => {
            substitute_temp(source, substitutions);
        }
        Instruction::LoadSlot { slot, .. } => {
            substitute_temp(slot, substitutions);
        }
        Instruction::StoreSlot { slot, source, .. } => {
            substitute_temp(slot, substitutions);
            substitute_temp(source, substitutions);
        }
        Instruction::BatchRead { hash, .. } => {
            substitute_temp(hash, substitutions);
        }
        Instruction::BatchWrite { hash, value, .. } => {
            substitute_temp(hash, substitutions);
            substitute_temp(value, substitutions);
        }
        Instruction::Call { args, .. } => {
            for arg in args.iter_mut() {
                substitute_temp(arg, substitutions);
            }
        }
        Instruction::BuiltinCall { args, .. } => {
            for arg in args.iter_mut() {
                substitute_temp(arg, substitutions);
            }
        }
        Instruction::Sleep { duration } => {
            substitute_temp(duration, substitutions);
        }
        Instruction::Yield => {}
    }
}

fn substitute_in_operation(operation: &mut Operation, substitutions: &HashMap<TempId, TempId>) {
    match operation {
        Operation::Copy(source) => substitute_temp(source, substitutions),
        Operation::Constant(_) | Operation::Parameter { .. } => {}
        Operation::Binary { left, right, .. } => {
            substitute_temp(left, substitutions);
            substitute_temp(right, substitutions);
        }
        Operation::Unary { operand, .. } => substitute_temp(operand, substitutions),
        Operation::Cast { operand, .. } => substitute_temp(operand, substitutions),
        Operation::Select {
            condition,
            if_true,
            if_false,
        } => {
            substitute_temp(condition, substitutions);
            substitute_temp(if_true, substitutions);
            substitute_temp(if_false, substitutions);
        }
    }
}

fn substitute_in_terminator(terminator: &mut Terminator, substitutions: &HashMap<TempId, TempId>) {
    match terminator {
        Terminator::Branch { condition, .. } => substitute_temp(condition, substitutions),
        Terminator::Return(Some(value)) => substitute_temp(value, substitutions),
        Terminator::Jump(_) | Terminator::Return(None) | Terminator::None => {}
    }
}

fn constant_propagation(function: &mut Function) -> bool {
    let mut constants: HashMap<TempId, f64> = HashMap::new();
    let mut changed = false;

    for block in &function.blocks {
        for instruction in &block.instructions {
            if let Instruction::Assign {
                dest,
                operation: Operation::Constant(value),
            } = instruction
            {
                constants.insert(*dest, *value);
            }
        }
    }

    loop {
        let mut new_found = false;
        for block in &function.blocks {
            for instruction in &block.instructions {
                let dest = match instruction_dest(instruction) {
                    Some(d) => d,
                    None => continue,
                };
                if constants.contains_key(&dest) {
                    continue;
                }
                if let Some(value) = try_evaluate_constant(instruction, &constants) {
                    constants.insert(dest, value);
                    new_found = true;
                }
            }
        }
        if !new_found {
            break;
        }
    }

    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            if let Some(dest) = instruction_dest(instruction)
                && let Some(&value) = constants.get(&dest)
                && !matches!(
                    instruction,
                    Instruction::Assign {
                        operation: Operation::Constant(_),
                        ..
                    }
                )
            {
                *instruction = Instruction::Assign {
                    dest,
                    operation: Operation::Constant(value),
                };
                changed = true;
            }
        }
    }

    let mut branch_changes: Vec<(usize, BlockId, Option<BlockId>)> = Vec::new();
    for block_index in 0..function.blocks.len() {
        if let Terminator::Branch {
            condition,
            true_block,
            false_block,
        } = &function.blocks[block_index].terminator
            && let Some(&value) = constants.get(condition)
        {
            let (target, other) = if value != 0.0 {
                (*true_block, *false_block)
            } else {
                (*false_block, *true_block)
            };
            let removed = if target != other { Some(other) } else { None };
            branch_changes.push((block_index, target, removed));
        }
    }

    for (block_index, target, removed) in branch_changes {
        let this_block_id = function.blocks[block_index].id;
        function.blocks[block_index].terminator = Terminator::Jump(target);

        match removed {
            Some(removed_block) => {
                function.blocks[block_index]
                    .successors
                    .retain(|s| *s != removed_block);
                function.blocks[removed_block.0]
                    .predecessors
                    .retain(|p| *p != this_block_id);
                for instruction in &mut function.blocks[removed_block.0].instructions {
                    if let Instruction::Phi { args, .. } = instruction {
                        args.retain(|(_, block)| *block != this_block_id);
                    }
                }
            }
            None => {
                function.blocks[block_index].successors.sort();
                function.blocks[block_index].successors.dedup();
                let predecessors = &mut function.blocks[target.0].predecessors;
                if let Some(position) = predecessors.iter().rposition(|p| *p == this_block_id) {
                    predecessors.remove(position);
                }
            }
        }
        changed = true;
    }

    changed
}

fn try_evaluate_constant(
    instruction: &Instruction,
    constants: &HashMap<TempId, f64>,
) -> Option<f64> {
    match instruction {
        Instruction::Assign { operation, .. } => match operation {
            Operation::Constant(value) => Some(*value),
            Operation::Parameter { .. } => None,
            Operation::Copy(source) => constants.get(source).copied(),
            Operation::Binary {
                operator,
                left,
                right,
            } => {
                let left_value = constants.get(left)?;
                let right_value = constants.get(right)?;
                try_fold_binary(*operator, *left_value, *right_value)
            }
            Operation::Unary { operator, operand } => {
                let value = constants.get(operand)?;
                try_fold_unary(*operator, *value)
            }
            Operation::Cast {
                operand,
                target_type,
                source_type,
            } => {
                let value = constants.get(operand)?;
                Some(fold_cast(*value, *source_type, *target_type))
            }
            Operation::Select {
                condition,
                if_true,
                if_false,
            } => {
                if let Some(&condition_value) = constants.get(condition) {
                    if condition_value != 0.0 {
                        constants.get(if_true).copied()
                    } else {
                        constants.get(if_false).copied()
                    }
                } else {
                    let true_value = constants.get(if_true)?;
                    let false_value = constants.get(if_false)?;
                    if true_value.to_bits() == false_value.to_bits() {
                        Some(*true_value)
                    } else {
                        None
                    }
                }
            }
        },
        Instruction::Phi { args, .. } => {
            if args.is_empty() {
                return None;
            }
            let first_value = constants.get(&args[0].0)?;
            for &(temp, _) in &args[1..] {
                let value = constants.get(&temp)?;
                if value.to_bits() != first_value.to_bits() {
                    return None;
                }
            }
            Some(*first_value)
        }
        Instruction::BuiltinCall { function, args, .. } => {
            let constant_args: Option<Vec<f64>> =
                args.iter().map(|a| constants.get(a).copied()).collect();
            let constant_args = constant_args?;
            try_fold_builtin(*function, &constant_args)
        }
        _ => None,
    }
}

fn try_fold_binary(operator: BinaryOperator, left: f64, right: f64) -> Option<f64> {
    let result = match operator {
        BinaryOperator::Add => left + right,
        BinaryOperator::Sub => left - right,
        BinaryOperator::Mul => left * right,
        BinaryOperator::Div => left / right,
        BinaryOperator::Rem => left % right,
        BinaryOperator::Eq => {
            if left == right {
                1.0
            } else {
                0.0
            }
        }
        BinaryOperator::Ne => {
            if left != right {
                1.0
            } else {
                0.0
            }
        }
        BinaryOperator::Lt => {
            if left < right {
                1.0
            } else {
                0.0
            }
        }
        BinaryOperator::Gt => {
            if left > right {
                1.0
            } else {
                0.0
            }
        }
        BinaryOperator::Le => {
            if left <= right {
                1.0
            } else {
                0.0
            }
        }
        BinaryOperator::Ge => {
            if left >= right {
                1.0
            } else {
                0.0
            }
        }
        BinaryOperator::And => {
            if left != 0.0 && right != 0.0 {
                1.0
            } else {
                0.0
            }
        }
        BinaryOperator::Or => {
            if left != 0.0 || right != 0.0 {
                1.0
            } else {
                0.0
            }
        }
        BinaryOperator::BitAnd => ((left as i64) & (right as i64)) as f64,
        BinaryOperator::BitOr => ((left as i64) | (right as i64)) as f64,
        BinaryOperator::BitXor => ((left as i64) ^ (right as i64)) as f64,
        BinaryOperator::Shl => {
            let shift = right as i64;
            if !(0..=63).contains(&shift) {
                return None;
            }
            ((left as i64).wrapping_shl(shift as u32)) as f64
        }
        BinaryOperator::Shr => {
            let shift = right as i64;
            if !(0..=63).contains(&shift) {
                return None;
            }
            ((left as i64).wrapping_shr(shift as u32)) as f64
        }
    };
    Some(result)
}

fn try_fold_unary(operator: UnaryOperator, operand: f64) -> Option<f64> {
    let result = match operator {
        UnaryOperator::Neg => -operand,
        UnaryOperator::Not => {
            if operand == 0.0 {
                1.0
            } else {
                0.0
            }
        }
        UnaryOperator::BitNot => (!(operand as i64)) as f64,
    };
    Some(result)
}

fn fold_cast(value: f64, source: Type, target: Type) -> f64 {
    match (source, target) {
        (Type::I53, Type::F64)
        | (Type::F64, Type::F64)
        | (Type::I53, Type::I53)
        | (Type::Bool, Type::Bool)
        | (Type::Bool, Type::I53)
        | (Type::Bool, Type::F64) => value,
        (Type::F64, Type::I53) => value.trunc(),
        (Type::I53, Type::Bool) | (Type::F64, Type::Bool) => {
            unreachable!("cast to bool is a compile error and should have been rejected by resolve")
        }
        (Type::Unit, _) | (_, Type::Unit) => {
            unreachable!(
                "unit type should not appear in SSA and should have been rejected by resolve"
            )
        }
    }
}

fn try_fold_builtin(builtin: BuiltinFunction, args: &[f64]) -> Option<f64> {
    match builtin {
        BuiltinFunction::Rand => None,
        BuiltinFunction::Abs if args.len() == 1 => Some(args[0].abs()),
        BuiltinFunction::Ceil if args.len() == 1 => Some(args[0].ceil()),
        BuiltinFunction::Floor if args.len() == 1 => Some(args[0].floor()),
        BuiltinFunction::Round if args.len() == 1 => Some(args[0].round()),
        BuiltinFunction::Trunc if args.len() == 1 => Some(args[0].trunc()),
        BuiltinFunction::Sqrt if args.len() == 1 => Some(args[0].sqrt()),
        BuiltinFunction::Exp if args.len() == 1 => Some(args[0].exp()),
        BuiltinFunction::Log if args.len() == 1 => Some(args[0].ln()),
        BuiltinFunction::Sin if args.len() == 1 => Some(args[0].sin()),
        BuiltinFunction::Cos if args.len() == 1 => Some(args[0].cos()),
        BuiltinFunction::Tan if args.len() == 1 => Some(args[0].tan()),
        BuiltinFunction::Asin if args.len() == 1 => Some(args[0].asin()),
        BuiltinFunction::Acos if args.len() == 1 => Some(args[0].acos()),
        BuiltinFunction::Atan if args.len() == 1 => Some(args[0].atan()),
        BuiltinFunction::Atan2 if args.len() == 2 => Some(args[0].atan2(args[1])),
        BuiltinFunction::Pow if args.len() == 2 => Some(args[0].powf(args[1])),
        BuiltinFunction::Min if args.len() == 2 => Some(args[0].min(args[1])),
        BuiltinFunction::Max if args.len() == 2 => Some(args[0].max(args[1])),
        BuiltinFunction::Lerp if args.len() == 3 => Some(args[0] + (args[1] - args[0]) * args[2]),
        BuiltinFunction::Clamp if args.len() == 3 => Some(args[0].max(args[1]).min(args[2])),
        _ => None,
    }
}

fn copy_propagation(function: &mut Function) -> bool {
    let mut substitutions: HashMap<TempId, TempId> = HashMap::new();

    for block in &function.blocks {
        for instruction in &block.instructions {
            match instruction {
                Instruction::Assign {
                    dest,
                    operation: Operation::Copy(source),
                } => {
                    if *dest != *source {
                        substitutions.insert(*dest, *source);
                    }
                }
                Instruction::Phi { dest, args } => {
                    if let Some(single_source) = single_phi_source(args)
                        && *dest != single_source
                    {
                        substitutions.insert(*dest, single_source);
                    }
                }
                _ => {}
            }
        }
    }

    if substitutions.is_empty() {
        return false;
    }

    apply_substitutions(function, &substitutions);

    let resolved = resolve_substitution_chains(&substitutions);
    for block in &mut function.blocks {
        block.instructions.retain(|instruction| {
            if let Some(dest) = instruction_dest(instruction) {
                !resolved.contains_key(&dest)
            } else {
                true
            }
        });
    }

    true
}

fn single_phi_source(args: &[(TempId, BlockId)]) -> Option<TempId> {
    if args.is_empty() {
        return None;
    }
    let first = args[0].0;
    if args.iter().all(|&(temp, _)| temp == first) {
        Some(first)
    } else {
        None
    }
}

fn dead_code_elimination(function: &mut Function) -> bool {
    let def_map = build_def_map(function);
    let mut live: HashSet<TempId> = HashSet::new();
    let mut worklist: VecDeque<TempId> = VecDeque::new();

    for block in &function.blocks {
        for instruction in &block.instructions {
            if has_side_effects(instruction) {
                for temp in instruction_uses(instruction) {
                    if live.insert(temp) {
                        worklist.push_back(temp);
                    }
                }
            }
        }
        for temp in terminator_uses(&block.terminator) {
            if live.insert(temp) {
                worklist.push_back(temp);
            }
        }
    }

    while let Some(temp) = worklist.pop_front() {
        if let Some(&(block_index, instruction_index)) = def_map.get(&temp) {
            let instruction = &function.blocks[block_index].instructions[instruction_index];
            for used_temp in instruction_uses(instruction) {
                if live.insert(used_temp) {
                    worklist.push_back(used_temp);
                }
            }
        }
    }

    let mut changed = false;
    for block in &mut function.blocks {
        let original_length = block.instructions.len();
        block.instructions.retain(|instruction| {
            if has_side_effects(instruction) {
                return true;
            }
            match instruction_dest(instruction) {
                Some(dest) => live.contains(&dest),
                None => true,
            }
        });
        if block.instructions.len() != original_length {
            changed = true;
        }
    }

    changed
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ValueExpression {
    Constant(u64),
    Binary {
        operator: BinaryOperator,
        left: TempId,
        right: TempId,
    },
    Unary {
        operator: UnaryOperator,
        operand: TempId,
    },
    Cast {
        operand: TempId,
        target_type: Type,
        source_type: Type,
    },
    Select {
        condition: TempId,
        if_true: TempId,
        if_false: TempId,
    },
}

fn global_value_numbering(function: &mut Function) -> bool {
    let mut substitutions: HashMap<TempId, TempId> = HashMap::new();

    for block in &function.blocks {
        let mut value_table: HashMap<ValueExpression, TempId> = HashMap::new();

        for instruction in &block.instructions {
            if let Instruction::Assign { dest, operation } = instruction
                && let Some(expression) = operation_to_value_expression(operation)
            {
                if let Some(&leader) = value_table.get(&expression) {
                    substitutions.insert(*dest, leader);
                } else {
                    value_table.insert(expression, *dest);
                }
            }
        }
    }

    if substitutions.is_empty() {
        return false;
    }

    apply_substitutions(function, &substitutions);
    true
}

fn operation_to_value_expression(operation: &Operation) -> Option<ValueExpression> {
    match operation {
        Operation::Constant(value) => Some(ValueExpression::Constant(value.to_bits())),
        Operation::Binary {
            operator,
            left,
            right,
        } => {
            let (left, right) = if is_commutative(*operator) && right.0 < left.0 {
                (*right, *left)
            } else {
                (*left, *right)
            };
            Some(ValueExpression::Binary {
                operator: *operator,
                left,
                right,
            })
        }
        Operation::Unary { operator, operand } => Some(ValueExpression::Unary {
            operator: *operator,
            operand: *operand,
        }),
        Operation::Cast {
            operand,
            target_type,
            source_type,
        } => Some(ValueExpression::Cast {
            operand: *operand,
            target_type: *target_type,
            source_type: *source_type,
        }),
        Operation::Select {
            condition,
            if_true,
            if_false,
        } => Some(ValueExpression::Select {
            condition: *condition,
            if_true: *if_true,
            if_false: *if_false,
        }),
        Operation::Copy(_) => None,
        Operation::Parameter { .. } => None,
    }
}

fn is_commutative(operator: BinaryOperator) -> bool {
    matches!(
        operator,
        BinaryOperator::Add
            | BinaryOperator::Mul
            | BinaryOperator::Eq
            | BinaryOperator::Ne
            | BinaryOperator::And
            | BinaryOperator::Or
            | BinaryOperator::BitAnd
            | BinaryOperator::BitOr
            | BinaryOperator::BitXor
    )
}

fn remove_unreachable_blocks(function: &mut Function) -> bool {
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
fn merge_empty_blocks(function: &mut Function) -> bool {
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
fn coalesce_blocks(function: &mut Function) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg;
    use crate::parser::parse;
    use crate::resolve::resolve;
    use crate::ssa;

    fn build_optimized(source: &str) -> Program {
        let mut program = build_ssa_unoptimized(source);
        optimize_program(&mut program);
        program
    }

    fn build_ssa_unoptimized(source: &str) -> Program {
        let (ast, parse_diagnostics) = parse(source);
        let errors: Vec<_> = parse_diagnostics
            .iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "parse errors: {:#?}", errors);
        let (resolved, _) = resolve(&ast)
            .unwrap_or_else(|diagnostics| panic!("resolve errors: {:#?}", diagnostics));
        let (mut program, _) = cfg::build(&resolved);
        ssa::construct_program(&mut program);
        program
    }

    fn get_function<'a>(program: &'a Program, name: &str) -> &'a Function {
        program
            .functions
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("function '{}' not found", name))
    }

    fn count_instructions(function: &Function) -> usize {
        function
            .blocks
            .iter()
            .map(|block| block.instructions.len())
            .sum()
    }

    fn has_binary_instruction(function: &Function) -> bool {
        function.blocks.iter().any(|block| {
            block.instructions.iter().any(|instruction| {
                matches!(
                    instruction,
                    Instruction::Assign {
                        operation: Operation::Binary { .. },
                        ..
                    }
                )
            })
        })
    }

    fn has_phi_instruction(function: &Function) -> bool {
        function.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Phi { .. }))
        })
    }

    fn count_constants(function: &Function) -> usize {
        function
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter())
            .filter(|instruction| {
                matches!(
                    instruction,
                    Instruction::Assign {
                        operation: Operation::Constant(_),
                        ..
                    }
                )
            })
            .count()
    }

    #[test]
    fn constant_folding_arithmetic() {
        let program = build_optimized("fn main() { let x: i53 = 3 + 4; }");
        let main = get_function(&program, "main");
        assert!(
            !has_binary_instruction(main),
            "binary instruction should be folded away"
        );
    }

    #[test]
    fn constant_folding_nested_arithmetic() {
        let program = build_optimized("fn main() { let x: i53 = (2 + 3) * (4 - 1); }");
        let main = get_function(&program, "main");
        assert!(
            !has_binary_instruction(main),
            "all arithmetic should be folded"
        );
    }

    #[test]
    fn dead_code_elimination_unused_variable() {
        let before = build_ssa_unoptimized("fn main() { let x: i53 = 5; }");
        let after = build_optimized("fn main() { let x: i53 = 5; }");
        let before_count = count_instructions(get_function(&before, "main"));
        let after_count = count_instructions(get_function(&after, "main"));
        assert!(
            after_count < before_count,
            "DCE should reduce instruction count: before={}, after={}",
            before_count,
            after_count
        );
    }

    #[test]
    fn dead_code_elimination_preserves_side_effects() {
        let program = build_optimized(
            r#"
            device sensor: d0;
            fn main() {
                sensor.Setting = 1;
            }
            "#,
        );
        let main = get_function(&program, "main");
        let has_store = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::StoreDevice { .. }))
        });
        assert!(has_store, "device store must be preserved");
    }

    #[test]
    fn copy_propagation_eliminates_copies() {
        let src = r#"fn main() {
            let x: i53 = 1;
            let y: i53 = x;
            let z: i53 = y;
        }"#;
        let before = build_ssa_unoptimized(src);
        let after = build_optimized(src);
        let before_count = count_instructions(get_function(&before, "main"));
        let after_count = count_instructions(get_function(&after, "main"));
        assert!(
            after_count < before_count,
            "copy propagation + DCE should reduce instruction count: before={}, after={}",
            before_count,
            after_count
        );
    }

    #[test]
    fn constant_branch_simplification() {
        let program = build_optimized(
            r#"
            device sensor: d0;
            fn main() {
                if true {
                    sensor.Setting = 1;
                } else {
                    sensor.Setting = 2;
                }
            }
            "#,
        );
        let main = get_function(&program, "main");
        let store_count: usize = main
            .blocks
            .iter()
            .flat_map(|b| b.instructions.iter())
            .filter(|i| matches!(i, Instruction::StoreDevice { .. }))
            .count();
        assert_eq!(
            store_count, 1,
            "dead branch should be eliminated, leaving only one store"
        );
    }

    #[test]
    fn phi_with_same_constant_folded() {
        let program = build_optimized(
            r#"fn main() {
                let mut x: i53 = 1;
                if true {
                    x = 1;
                }
                let y = x;
            }"#,
        );
        let main = get_function(&program, "main");
        assert!(
            !has_phi_instruction(main),
            "phi with identical constant arguments should be eliminated"
        );
    }

    #[test]
    fn gvn_eliminates_duplicate_constants() {
        let before = build_ssa_unoptimized(
            r#"
            device sensor: d0;
            fn main() {
                sensor.Setting = 42;
                sensor.Mode = 42;
            }
            "#,
        );
        let after = build_optimized(
            r#"
            device sensor: d0;
            fn main() {
                sensor.Setting = 42;
                sensor.Mode = 42;
            }
            "#,
        );
        let before_constants = count_constants(get_function(&before, "main"));
        let after_constants = count_constants(get_function(&after, "main"));
        assert!(
            after_constants < before_constants,
            "GVN should deduplicate identical constants: before={}, after={}",
            before_constants,
            after_constants
        );
    }

    #[test]
    fn pipeline_reduces_complex_program() {
        let before = build_ssa_unoptimized(
            r#"
            device sensor: d0;
            fn main() {
                let x: i53 = 2 + 3;
                let y: i53 = x * 2;
                let unused: i53 = 99;
                sensor.Setting = y;
            }
            "#,
        );
        let after = build_optimized(
            r#"
            device sensor: d0;
            fn main() {
                let x: i53 = 2 + 3;
                let y: i53 = x * 2;
                let unused: i53 = 99;
                sensor.Setting = y;
            }
            "#,
        );
        let before_count = count_instructions(get_function(&before, "main"));
        let after_count = count_instructions(get_function(&after, "main"));
        assert!(
            after_count < before_count,
            "optimization pipeline should reduce total instructions: before={}, after={}",
            before_count,
            after_count
        );
    }

    #[test]
    fn yield_preserved_through_optimization() {
        let program = build_optimized(
            r#"fn main() {
                loop {
                    yield;
                }
            }"#,
        );
        let main = get_function(&program, "main");
        let has_yield = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::Yield))
        });
        assert!(has_yield, "yield must be preserved");
    }

    #[test]
    fn sleep_preserved_through_optimization() {
        let program = build_optimized(
            r#"fn main() {
                sleep(1.0);
            }"#,
        );
        let main = get_function(&program, "main");
        let has_sleep = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::Sleep { .. }))
        });
        assert!(has_sleep, "sleep must be preserved");
    }

    #[test]
    fn builtin_constant_folding() {
        let program = build_optimized("fn main() { let x: f64 = sqrt(4.0); }");
        let main = get_function(&program, "main");
        let has_builtin = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::BuiltinCall { .. }))
        });
        assert!(
            !has_builtin,
            "builtin call with constant args should be folded"
        );
    }

    #[test]
    fn loop_with_device_io_preserved() {
        let program = build_optimized(
            r#"
            device sensor: d0;
            device light: d1;
            fn main() {
                loop {
                    let temp = sensor.Temperature;
                    light.Setting = temp;
                    yield;
                }
            }
            "#,
        );
        let main = get_function(&program, "main");
        let has_load = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::LoadDevice { .. }))
        });
        let has_store = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::StoreDevice { .. }))
        });
        assert!(has_load, "device load in loop must be preserved");
        assert!(has_store, "device store in loop must be preserved");
    }

    #[test]
    fn unary_constant_folding() {
        let program = build_optimized("fn main() { let x: i53 = -5; }");
        let main = get_function(&program, "main");
        let has_unary = main.blocks.iter().any(|block| {
            block.instructions.iter().any(|i| {
                matches!(
                    i,
                    Instruction::Assign {
                        operation: Operation::Unary { .. },
                        ..
                    }
                )
            })
        });
        assert!(!has_unary, "unary negation of constant should be folded");
    }

    #[test]
    fn comparison_constant_folding() {
        let program = build_optimized("fn main() { let x: bool = 3 < 5; }");
        let main = get_function(&program, "main");
        assert!(
            !has_binary_instruction(main),
            "constant comparison should be folded"
        );
    }

    mod resolve_substitution_chain_tests {
        use super::super::*;
        use std::collections::HashMap;

        fn t(n: usize) -> TempId {
            TempId(n)
        }

        fn resolve(pairs: &[(usize, usize)]) -> HashMap<TempId, TempId> {
            let map: HashMap<TempId, TempId> = pairs.iter().map(|&(k, v)| (t(k), t(v))).collect();
            resolve_substitution_chains(&map)
        }

        #[test]
        fn empty_map_returns_empty() {
            assert!(resolve(&[]).is_empty());
        }

        #[test]
        fn single_hop_preserved() {
            let result = resolve(&[(1, 2)]);
            assert_eq!(result.get(&t(1)), Some(&t(2)));
            assert_eq!(result.len(), 1);
        }

        #[test]
        fn self_mapping_omitted() {
            let result = resolve(&[(1, 1)]);
            assert!(
                result.is_empty(),
                "self-mapping should be omitted from result"
            );
        }

        #[test]
        fn two_hop_chain_collapsed() {
            // 1 -> 2 -> 3 should produce {1 -> 3, 2 -> 3}
            let result = resolve(&[(1, 2), (2, 3)]);
            assert_eq!(result.get(&t(1)), Some(&t(3)));
            assert_eq!(result.get(&t(2)), Some(&t(3)));
        }

        #[test]
        fn three_hop_chain_collapsed() {
            // 1 -> 2 -> 3 -> 4 should produce {1 -> 4, 2 -> 4, 3 -> 4}
            let result = resolve(&[(1, 2), (2, 3), (3, 4)]);
            assert_eq!(result.get(&t(1)), Some(&t(4)));
            assert_eq!(result.get(&t(2)), Some(&t(4)));
            assert_eq!(result.get(&t(3)), Some(&t(4)));
        }

        #[test]
        fn converging_chains_resolved_to_same_target() {
            // 1 -> 2 -> 3, and 4 -> 2 both converge on 3
            let result = resolve(&[(1, 2), (2, 3), (4, 2)]);
            assert_eq!(result.get(&t(1)), Some(&t(3)));
            assert_eq!(result.get(&t(2)), Some(&t(3)));
            assert_eq!(result.get(&t(4)), Some(&t(3)));
        }

        #[test]
        fn cycle_terminates_without_panic() {
            // 1 -> 2 -> 1: should not loop forever. Neither member has a
            // canonical root outside the cycle, so both resolve back to
            // themselves and are omitted.
            let result = resolve(&[(1, 2), (2, 1)]);
            assert!(
                !result.contains_key(&t(1)) && !result.contains_key(&t(2)),
                "entries in a pure cycle should not produce cross-mappings"
            );
        }

        #[test]
        fn tail_into_cycle_terminates() {
            // 1 -> 2 -> 3 -> 2 (tail leading into a cycle)
            // Key 1: follows 1->2->3->2, hits cycle, terminates at 2 (re-visited). result: 1->2
            // Key 2: follows 2->3->2, hits cycle, terminates at 2. 2==key, omitted.
            // Key 3: follows 3->2->3, hits cycle, terminates at 3. 3==key, omitted.
            let result = resolve(&[(1, 2), (2, 3), (3, 2)]);
            assert_eq!(result.get(&t(1)), Some(&t(2)));
            assert!(!result.contains_key(&t(2)));
            assert!(!result.contains_key(&t(3)));
        }
    }

    #[test]
    fn inline_small_function_called_once() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn helper(x: i53) -> i53 { return x + 1; }
            fn main() { out.Setting = helper(5); }
            "#,
        );
        assert!(
            program.functions.len() == 1,
            "helper should be inlined and removed, leaving only main; found: {:?}",
            program
                .functions
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
        let main = get_function(&program, "main");
        let has_call = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::Call { .. }))
        });
        assert!(!has_call, "call should be inlined away");
    }

    #[test]
    fn inline_constant_propagates_through_inlined_body() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn double(x: i53) -> i53 { return x * 2; }
            fn main() { out.Setting = double(21); }
            "#,
        );
        let main = get_function(&program, "main");
        let has_constant_42 = main.blocks.iter().any(|block| {
            block.instructions.iter().any(|i| {
                matches!(
                    i,
                    Instruction::Assign {
                        operation: Operation::Constant(v),
                        ..
                    } if *v == 42.0
                )
            })
        });
        assert!(
            has_constant_42,
            "double(21) should inline and fold to constant 42"
        );
    }

    #[test]
    fn inline_does_not_inline_recursive_function() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn recurse(x: i53) -> i53 {
                if x < 1 { return 0; }
                return recurse(x - 1) + 1;
            }
            fn main() { out.Setting = recurse(5); }
            "#,
        );
        assert!(
            program.functions.len() == 2,
            "recursive function should not be inlined; found: {:?}",
            program
                .functions
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn inline_preserves_side_effects() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn write_device(x: i53) {
                out.Setting = x;
            }
            fn main() { write_device(42); }
            "#,
        );
        let main = get_function(&program, "main");
        let has_store = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::StoreDevice { .. }))
        });
        assert!(
            has_store,
            "device store from inlined function must be preserved"
        );
    }

    #[test]
    fn inline_function_with_control_flow() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn abs_val(x: f64) -> f64 {
                if x < 0.0 { return -x; }
                return x;
            }
            fn main() { out.Setting = abs_val(-5.0); }
            "#,
        );
        let main = get_function(&program, "main");
        let has_call = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::Call { .. }))
        });
        assert!(!has_call, "abs_val should be inlined");
        let has_constant_5 = main.blocks.iter().any(|block| {
            block.instructions.iter().any(|i| {
                matches!(
                    i,
                    Instruction::Assign {
                        operation: Operation::Constant(v),
                        ..
                    } if *v == 5.0
                )
            })
        });
        assert!(has_constant_5, "abs_val(-5.0) should fold to constant 5.0");
    }

    #[test]
    fn inline_does_not_inline_large_function_called_many_times() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn big(a: f64, b: f64) -> f64 {
                let c = a + b;
                let d = c * a;
                let e = d - b;
                let f = e + c;
                return f * d;
            }
            fn main() {
                out.Setting = big(1.0, 2.0);
                out.Setting = big(3.0, 4.0);
                out.Setting = big(5.0, 6.0);
            }
            "#,
        );
        assert!(
            program.functions.len() == 2,
            "large function called 3 times should not be inlined; found: {:?}",
            program
                .functions
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn inline_void_function() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn set_output(val: f64) {
                out.Setting = val;
            }
            fn main() {
                set_output(10.0);
            }
            "#,
        );
        let main = get_function(&program, "main");
        let has_call = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::Call { .. }))
        });
        assert!(!has_call, "void function called once should be inlined");
        let has_store = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::StoreDevice { .. }))
        });
        assert!(
            has_store,
            "device store from inlined void function must be preserved"
        );
    }

    #[test]
    fn inline_multiple_small_functions() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn add_one(x: i53) -> i53 { return x + 1; }
            fn double(x: i53) -> i53 { return x * 2; }
            fn main() { out.Setting = double(add_one(5)); }
            "#,
        );
        assert!(
            program.functions.len() == 1,
            "both small functions should be inlined; found: {:?}",
            program
                .functions
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
        let main = get_function(&program, "main");
        let has_constant_12 = main.blocks.iter().any(|block| {
            block.instructions.iter().any(|i| {
                matches!(
                    i,
                    Instruction::Assign {
                        operation: Operation::Constant(v),
                        ..
                    } if *v == 12.0
                )
            })
        });
        assert!(
            has_constant_12,
            "double(add_one(5)) = double(6) = 12 should fold to constant"
        );
    }
}
