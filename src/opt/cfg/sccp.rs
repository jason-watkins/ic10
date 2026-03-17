use std::collections::{HashMap, HashSet, VecDeque};

use crate::ir::cfg::{BlockId, Function, Instruction, Operation, TempId, Terminator};
use crate::ir::{BinaryOperator, Intrinsic, Type, UnaryOperator};

use super::utilities::instruction_dest;

#[derive(Debug, Clone, Copy)]
enum LatticeValue {
    Top,
    Constant(f64),
    Bottom,
}

impl LatticeValue {
    fn meet(self, other: LatticeValue) -> LatticeValue {
        match (self, other) {
            (LatticeValue::Top, v) | (v, LatticeValue::Top) => v,
            (LatticeValue::Bottom, _) | (_, LatticeValue::Bottom) => LatticeValue::Bottom,
            (LatticeValue::Constant(a), LatticeValue::Constant(b)) => {
                if a.to_bits() == b.to_bits() {
                    LatticeValue::Constant(a)
                } else {
                    LatticeValue::Bottom
                }
            }
        }
    }
}

pub(super) fn sccp(function: &mut Function) -> bool {
    let num_blocks = function.blocks.len();

    let mut values: HashMap<TempId, LatticeValue> = HashMap::new();
    let mut reachable = vec![false; num_blocks];
    let mut cfg_worklist: VecDeque<(BlockId, BlockId)> = VecDeque::new();
    let mut ssa_worklist: VecDeque<TempId> = VecDeque::new();

    let def_block = build_temp_block_map(function);
    let use_map = build_use_map(function);
    let correlated = collect_correlated_values(function);

    reachable[function.entry.0] = true;
    for instruction in &function.blocks[function.entry.0].instructions {
        update_value(instruction, &mut values, &mut ssa_worklist);
    }
    evaluate_terminator(function.entry, function, &values, &mut cfg_worklist);

    while !cfg_worklist.is_empty() || !ssa_worklist.is_empty() {
        while let Some((source, target)) = cfg_worklist.pop_front() {
            let first_visit = !reachable[target.0];
            reachable[target.0] = true;

            if let Some(facts) = correlated.get(&(source, target)) {
                for &(temp, value) in facts {
                    if update_lattice(temp, LatticeValue::Constant(value), &mut values) {
                        ssa_worklist.push_back(temp);
                    }
                }
            }

            let block = &function.blocks[target.0];
            for instruction in &block.instructions {
                if let Instruction::Phi { dest, args, .. } = instruction {
                    let new_value = evaluate_phi(args, &values, &reachable);
                    if update_lattice(*dest, new_value, &mut values) {
                        ssa_worklist.push_back(*dest);
                    }
                }
            }

            if first_visit {
                for instruction in &function.blocks[target.0].instructions {
                    if !matches!(instruction, Instruction::Phi { .. }) {
                        update_value(instruction, &mut values, &mut ssa_worklist);
                    }
                }
                evaluate_terminator(target, function, &values, &mut cfg_worklist);
            }
        }

        while let Some(temp) = ssa_worklist.pop_front() {
            if let Some(uses) = use_map.get(&temp) {
                for &(block_id, instruction_index) in uses {
                    if !reachable[block_id.0] {
                        continue;
                    }
                    let instruction = &function.blocks[block_id.0].instructions[instruction_index];
                    if let Instruction::Phi { dest, args, .. } = instruction {
                        let new_value = evaluate_phi(args, &values, &reachable);
                        if update_lattice(*dest, new_value, &mut values) {
                            ssa_worklist.push_back(*dest);
                        }
                    } else {
                        update_value(instruction, &mut values, &mut ssa_worklist);
                    }
                }
            }
            if let Some(&block_id) = def_block.get(&temp)
                && reachable[block_id.0]
            {
                let block = &function.blocks[block_id.0];
                if terminator_uses_temp(&block.terminator, temp) {
                    evaluate_terminator(block_id, function, &values, &mut cfg_worklist);
                }
            }
        }
    }

    apply_results(function, &values, &reachable)
}

/// Pre-scan branches for equality comparisons. For `branch(x == c, T, F)`:
///   - on edge → T, `x` is known to equal `c`
///   - on edge → F with `x != c`, `x` is *not* constrained (overdefined)
///     Only constant RHS/LHS values produce usable facts.
fn collect_correlated_values(
    function: &Function,
) -> HashMap<(BlockId, BlockId), Vec<(TempId, f64)>> {
    let mut map: HashMap<(BlockId, BlockId), Vec<(TempId, f64)>> = HashMap::new();

    let def_map = super::utilities::build_def_map(function);

    for block in &function.blocks {
        let Terminator::Branch {
            condition,
            true_block,
            false_block,
        } = &block.terminator
        else {
            continue;
        };

        let Some(&(def_block_index, def_instr_index)) = def_map.get(condition) else {
            continue;
        };
        let defining = &function.blocks[def_block_index].instructions[def_instr_index];
        let Instruction::Assign {
            operation:
                Operation::Binary {
                    operator,
                    left,
                    right,
                },
            ..
        } = defining
        else {
            continue;
        };

        let left_const = resolve_constant(&def_map, function, *left);
        let right_const = resolve_constant(&def_map, function, *right);

        match operator {
            BinaryOperator::Eq => {
                if let Some(value) = right_const {
                    map.entry((block.id, *true_block))
                        .or_default()
                        .push((*left, value));
                }
                if let Some(value) = left_const {
                    map.entry((block.id, *true_block))
                        .or_default()
                        .push((*right, value));
                }
            }
            BinaryOperator::Ne => {
                if let Some(value) = right_const {
                    map.entry((block.id, *false_block))
                        .or_default()
                        .push((*left, value));
                }
                if let Some(value) = left_const {
                    map.entry((block.id, *false_block))
                        .or_default()
                        .push((*right, value));
                }
            }
            _ => {}
        }
    }

    map
}

fn resolve_constant(
    def_map: &HashMap<TempId, (usize, usize)>,
    function: &Function,
    temp: TempId,
) -> Option<f64> {
    let &(block_index, instr_index) = def_map.get(&temp)?;
    if let Instruction::Assign {
        operation: Operation::Constant(value),
        ..
    } = &function.blocks[block_index].instructions[instr_index]
    {
        Some(*value)
    } else {
        None
    }
}

fn evaluate_phi(
    args: &[(TempId, BlockId)],
    values: &HashMap<TempId, LatticeValue>,
    reachable: &[bool],
) -> LatticeValue {
    let mut result = LatticeValue::Top;
    for &(temp, block) in args {
        if !reachable[block.0] {
            continue;
        }
        let arg_value = values.get(&temp).copied().unwrap_or(LatticeValue::Top);
        result = result.meet(arg_value);
    }
    result
}

fn update_lattice(
    temp: TempId,
    new_value: LatticeValue,
    values: &mut HashMap<TempId, LatticeValue>,
) -> bool {
    let old = values.get(&temp).copied().unwrap_or(LatticeValue::Top);
    let merged = old.meet(new_value);
    let changed = !same_lattice(old, merged);
    if changed {
        values.insert(temp, merged);
    }
    changed
}

fn same_lattice(a: LatticeValue, b: LatticeValue) -> bool {
    match (a, b) {
        (LatticeValue::Top, LatticeValue::Top) => true,
        (LatticeValue::Bottom, LatticeValue::Bottom) => true,
        (LatticeValue::Constant(x), LatticeValue::Constant(y)) => x.to_bits() == y.to_bits(),
        _ => false,
    }
}

fn update_value(
    instruction: &Instruction,
    values: &mut HashMap<TempId, LatticeValue>,
    ssa_worklist: &mut VecDeque<TempId>,
) {
    let new_value = match instruction {
        Instruction::Assign { operation, .. } => evaluate_operation(operation, values),
        Instruction::Phi { .. } => return,
        Instruction::IntrinsicCall {
            function: intrinsic,
            args,
            ..
        } => evaluate_intrinsic(*intrinsic, args, values),
        Instruction::LoadDevice { .. }
        | Instruction::LoadSlot { .. }
        | Instruction::BatchRead { .. }
        | Instruction::Call { .. }
        | Instruction::LoadStatic { .. } => LatticeValue::Bottom,
        Instruction::StoreDevice { .. }
        | Instruction::StoreSlot { .. }
        | Instruction::BatchWrite { .. }
        | Instruction::StoreStatic { .. }
        | Instruction::Sleep { .. }
        | Instruction::Yield => return,
    };

    if let Some(dest) = instruction_dest(instruction)
        && update_lattice(dest, new_value, values)
    {
        ssa_worklist.push_back(dest);
    }
}

fn evaluate_operation(
    operation: &Operation,
    values: &HashMap<TempId, LatticeValue>,
) -> LatticeValue {
    match operation {
        Operation::Constant(v) => LatticeValue::Constant(*v),
        Operation::Parameter { .. } => LatticeValue::Bottom,
        Operation::Copy(source) => values.get(source).copied().unwrap_or(LatticeValue::Top),
        Operation::Binary {
            operator,
            left,
            right,
        } => {
            let left_value = values.get(left).copied().unwrap_or(LatticeValue::Top);
            let right_value = values.get(right).copied().unwrap_or(LatticeValue::Top);
            evaluate_binary(*operator, left_value, right_value)
        }
        Operation::Unary { operator, operand } => {
            let operand_value = values.get(operand).copied().unwrap_or(LatticeValue::Top);
            evaluate_unary(*operator, operand_value)
        }
        Operation::Cast {
            operand,
            target_type,
            source_type,
        } => {
            let operand_value = values.get(operand).copied().unwrap_or(LatticeValue::Top);
            evaluate_cast(operand_value, *source_type, *target_type)
        }
        Operation::Select {
            condition,
            if_true,
            if_false,
        } => {
            let condition_value = values.get(condition).copied().unwrap_or(LatticeValue::Top);
            let true_value = values.get(if_true).copied().unwrap_or(LatticeValue::Top);
            let false_value = values.get(if_false).copied().unwrap_or(LatticeValue::Top);
            evaluate_select(condition_value, true_value, false_value)
        }
    }
}

fn evaluate_binary(
    operator: BinaryOperator,
    left: LatticeValue,
    right: LatticeValue,
) -> LatticeValue {
    match (left, right) {
        (LatticeValue::Constant(l), LatticeValue::Constant(r)) => {
            match try_fold_binary(operator, l, r) {
                Some(v) => LatticeValue::Constant(v),
                None => LatticeValue::Bottom,
            }
        }
        (LatticeValue::Bottom, _) | (_, LatticeValue::Bottom) => {
            match operator {
                BinaryOperator::Mul => {
                    if let LatticeValue::Constant(v) = left
                        && v == 0.0
                    {
                        return LatticeValue::Constant(0.0);
                    }
                    if let LatticeValue::Constant(v) = right
                        && v == 0.0
                    {
                        return LatticeValue::Constant(0.0);
                    }
                }
                BinaryOperator::And => {
                    if let LatticeValue::Constant(v) = left
                        && v == 0.0
                    {
                        return LatticeValue::Constant(0.0);
                    }
                    if let LatticeValue::Constant(v) = right
                        && v == 0.0
                    {
                        return LatticeValue::Constant(0.0);
                    }
                }
                BinaryOperator::Or => {
                    if let LatticeValue::Constant(v) = left
                        && v != 0.0
                    {
                        return LatticeValue::Constant(1.0);
                    }
                    if let LatticeValue::Constant(v) = right
                        && v != 0.0
                    {
                        return LatticeValue::Constant(1.0);
                    }
                }
                _ => {}
            }
            LatticeValue::Bottom
        }
        _ => LatticeValue::Top,
    }
}

fn evaluate_unary(operator: UnaryOperator, operand: LatticeValue) -> LatticeValue {
    match operand {
        LatticeValue::Constant(v) => match try_fold_unary(operator, v) {
            Some(r) => LatticeValue::Constant(r),
            None => LatticeValue::Bottom,
        },
        LatticeValue::Bottom => LatticeValue::Bottom,
        LatticeValue::Top => LatticeValue::Top,
    }
}

fn evaluate_cast(operand: LatticeValue, source: Type, target: Type) -> LatticeValue {
    match operand {
        LatticeValue::Constant(v) => LatticeValue::Constant(fold_cast(v, source, target)),
        LatticeValue::Bottom => LatticeValue::Bottom,
        LatticeValue::Top => LatticeValue::Top,
    }
}

fn evaluate_select(
    condition: LatticeValue,
    true_value: LatticeValue,
    false_value: LatticeValue,
) -> LatticeValue {
    match condition {
        LatticeValue::Constant(c) => {
            if c != 0.0 {
                true_value
            } else {
                false_value
            }
        }
        LatticeValue::Bottom => true_value.meet(false_value),
        LatticeValue::Top => LatticeValue::Top,
    }
}

fn evaluate_intrinsic(
    intrinsic: Intrinsic,
    args: &[TempId],
    values: &HashMap<TempId, LatticeValue>,
) -> LatticeValue {
    if matches!(intrinsic, Intrinsic::Rand) {
        return LatticeValue::Bottom;
    }
    let mut has_top = false;
    let mut constant_args = Vec::with_capacity(args.len());
    for arg in args {
        match values.get(arg).copied().unwrap_or(LatticeValue::Top) {
            LatticeValue::Constant(v) => constant_args.push(v),
            LatticeValue::Bottom => return LatticeValue::Bottom,
            LatticeValue::Top => {
                has_top = true;
                break;
            }
        }
    }
    if has_top {
        return LatticeValue::Top;
    }
    match try_fold_intrinsic(intrinsic, &constant_args) {
        Some(v) => LatticeValue::Constant(v),
        None => LatticeValue::Bottom,
    }
}

fn evaluate_terminator(
    block_id: BlockId,
    function: &Function,
    values: &HashMap<TempId, LatticeValue>,
    cfg_worklist: &mut VecDeque<(BlockId, BlockId)>,
) {
    let block = &function.blocks[block_id.0];
    match &block.terminator {
        Terminator::Jump(target) => {
            cfg_worklist.push_back((block_id, *target));
        }
        Terminator::Branch {
            condition,
            true_block,
            false_block,
        } => {
            let condition_value = values.get(condition).copied().unwrap_or(LatticeValue::Top);
            match condition_value {
                LatticeValue::Constant(v) => {
                    if v != 0.0 {
                        cfg_worklist.push_back((block_id, *true_block));
                    } else {
                        cfg_worklist.push_back((block_id, *false_block));
                    }
                }
                LatticeValue::Bottom => {
                    cfg_worklist.push_back((block_id, *true_block));
                    cfg_worklist.push_back((block_id, *false_block));
                }
                LatticeValue::Top => {}
            }
        }
        Terminator::Return(_) | Terminator::None => {}
    }
}

fn terminator_uses_temp(terminator: &Terminator, temp: TempId) -> bool {
    match terminator {
        Terminator::Branch { condition, .. } => *condition == temp,
        Terminator::Return(Some(value)) => *value == temp,
        Terminator::Jump(_) | Terminator::Return(None) | Terminator::None => false,
    }
}

fn apply_results(
    function: &mut Function,
    values: &HashMap<TempId, LatticeValue>,
    reachable: &[bool],
) -> bool {
    let mut changed = false;

    for block in &mut function.blocks {
        if !reachable[block.id.0] {
            continue;
        }
        for instruction in &mut block.instructions {
            if let Some(dest) = instruction_dest(instruction)
                && let Some(LatticeValue::Constant(value)) = values.get(&dest)
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
                    operation: Operation::Constant(*value),
                };
                changed = true;
            }
        }
    }

    let mut branch_changes: Vec<(usize, BlockId, Option<BlockId>)> = Vec::new();
    for (block_index, block) in function.blocks.iter().enumerate() {
        if !reachable[block.id.0] {
            continue;
        }
        if let Terminator::Branch {
            condition,
            true_block,
            false_block,
        } = &block.terminator
            && let Some(LatticeValue::Constant(value)) = values.get(condition)
        {
            let (target, other) = if *value != 0.0 {
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

    let unreachable_ids: HashSet<BlockId> = function
        .blocks
        .iter()
        .filter(|block| !reachable[block.id.0])
        .map(|block| block.id)
        .collect();

    if !unreachable_ids.is_empty() {
        for block in &mut function.blocks {
            if !reachable[block.id.0] {
                if !block.instructions.is_empty()
                    || !matches!(block.terminator, Terminator::None)
                    || !block.successors.is_empty()
                {
                    block.instructions.clear();
                    block.terminator = Terminator::None;
                    block.successors.clear();
                    block.predecessors.clear();
                    changed = true;
                }
                continue;
            }
            let original_predecessor_count = block.predecessors.len();
            block
                .predecessors
                .retain(|predecessor| !unreachable_ids.contains(predecessor));
            if block.predecessors.len() != original_predecessor_count {
                changed = true;
            }
            for instruction in &mut block.instructions {
                if let Instruction::Phi { args, .. } = instruction {
                    let original_arg_count = args.len();
                    args.retain(|(_, block_id)| !unreachable_ids.contains(block_id));
                    if args.len() != original_arg_count {
                        changed = true;
                    }
                }
            }
        }
    }

    changed
}

fn build_temp_block_map(function: &Function) -> HashMap<TempId, BlockId> {
    let mut map = HashMap::new();
    for block in &function.blocks {
        for instruction in &block.instructions {
            if let Some(dest) = instruction_dest(instruction) {
                map.insert(dest, block.id);
            }
        }
    }
    map
}

fn build_use_map(function: &Function) -> HashMap<TempId, Vec<(BlockId, usize)>> {
    let mut map: HashMap<TempId, Vec<(BlockId, usize)>> = HashMap::new();
    for block in &function.blocks {
        for (index, instruction) in block.instructions.iter().enumerate() {
            for temp in instruction_uses(instruction) {
                map.entry(temp).or_default().push((block.id, index));
            }
        }
    }
    map
}

fn instruction_uses(instruction: &Instruction) -> Vec<TempId> {
    super::utilities::instruction_uses(instruction)
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
            if value != 0.0 {
                1.0
            } else {
                0.0
            }
        }
        (Type::Unit, _) | (_, Type::Unit) => {
            unreachable!(
                "unit type should not appear in SSA and should have been rejected by resolve"
            )
        }
    }
}

fn try_fold_intrinsic(intrinsic: Intrinsic, args: &[f64]) -> Option<f64> {
    match intrinsic {
        Intrinsic::Rand => None,
        Intrinsic::Abs if args.len() == 1 => Some(args[0].abs()),
        Intrinsic::Ceil if args.len() == 1 => Some(args[0].ceil()),
        Intrinsic::Floor if args.len() == 1 => Some(args[0].floor()),
        Intrinsic::Round if args.len() == 1 => Some(args[0].round()),
        Intrinsic::Trunc if args.len() == 1 => Some(args[0].trunc()),
        Intrinsic::Sqrt if args.len() == 1 => Some(args[0].sqrt()),
        Intrinsic::Exp if args.len() == 1 => Some(args[0].exp()),
        Intrinsic::Log if args.len() == 1 => Some(args[0].ln()),
        Intrinsic::Sin if args.len() == 1 => Some(args[0].sin()),
        Intrinsic::Cos if args.len() == 1 => Some(args[0].cos()),
        Intrinsic::Tan if args.len() == 1 => Some(args[0].tan()),
        Intrinsic::Asin if args.len() == 1 => Some(args[0].asin()),
        Intrinsic::Acos if args.len() == 1 => Some(args[0].acos()),
        Intrinsic::Atan if args.len() == 1 => Some(args[0].atan()),
        Intrinsic::Atan2 if args.len() == 2 => Some(args[0].atan2(args[1])),
        Intrinsic::Pow if args.len() == 2 => Some(args[0].powf(args[1])),
        Intrinsic::Min if args.len() == 2 => Some(args[0].min(args[1])),
        Intrinsic::Max if args.len() == 2 => Some(args[0].max(args[1])),
        Intrinsic::Lerp if args.len() == 3 => Some(args[0] + (args[1] - args[0]) * args[2]),
        Intrinsic::Clamp if args.len() == 3 => Some(args[0].max(args[1]).min(args[2])),
        Intrinsic::IsNan if args.len() == 1 => Some(if args[0].is_nan() { 1.0 } else { 0.0 }),
        _ => None,
    }
}
