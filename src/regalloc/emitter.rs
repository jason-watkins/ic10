use std::collections::{HashMap, HashSet};

use crate::ast::{BinaryOperator, BuiltinFunction, Type, UnaryOperator};
use crate::cfg::{BasicBlock, BlockId, Function, Instruction, Operation, TempId, Terminator};
use crate::resolved::SymbolTable;

use super::allocator::{AllocationResult, SpillRecord};
use super::calling_convention::{CallingConventionInfo, FunctionClass};
use super::ic10::{IC10Function, IC10Instruction, JumpTarget, Operand, Register};
use super::liveness::{LinearMap, LinearPosition, instruction_uses, terminator_uses};

/// Metadata about a call site emitted into the IC10 instruction stream.
///
/// Used by the caller-save post-pass to insert push/pop instructions around calls.
pub struct EmittedCallSite {
    /// Index in the instruction list where the call sequence begins (before arg-setup moves).
    pub sequence_start: usize,
    /// Index in the instruction list after the call sequence ends (after return-value move).
    pub sequence_end: usize,
    /// Name of the function being called.
    pub callee_name: String,
    /// Number of arguments passed to the callee.
    pub arg_count: usize,
    /// Registers that hold live-across values at this call site and may need caller-saving.
    /// Already filtered to exclude temps that are pressure-spilled (on the stack) at this
    /// point.
    pub live_across_registers: Vec<Register>,
}

/// Context for emitting IC10 instructions from one function's SSA IR.
struct Emitter<'a> {
    function: &'a Function,
    block_order: &'a [BlockId],
    linear_map: &'a LinearMap,
    assignments: &'a HashMap<TempId, Register>,
    calling_convention: &'a CallingConventionInfo,
    symbols: &'a SymbolTable,
    spills: &'a [SpillRecord],
    output: Vec<IC10Instruction>,
    /// Spills indexed by the position at which the push should be emitted.
    spills_at: HashMap<LinearPosition, Vec<&'a SpillRecord>>,
    /// Reloads indexed by the position at which the pop should be emitted.
    reloads_at: HashMap<LinearPosition, Vec<&'a SpillRecord>>,
    /// Call site metadata collected during emission, consumed by the caller-save post-pass.
    call_sites: Vec<EmittedCallSite>,
    /// Constant temps whose only uses are as `Instruction::Call` arguments.
    ///
    /// For these, `lower_assign` is suppressed and `lower_call` emits the literal value
    /// directly into the target register, avoiding an intermediate `move rN imm` / `move
    /// r0 rN` pair.
    constants_for_call_args: HashMap<TempId, f64>,
}

/// Build the set of constant temps that should be inlined directly as literals at call
/// sites rather than being materialised into a register first.
///
/// A constant temp qualifies when every one of its uses is as an argument to a
/// `Instruction::Call`.  For such temps `lower_assign` is suppressed and `lower_call`
/// emits `move r<n> <literal>` directly, collapsing the otherwise two-instruction
/// `move rN imm` / `move r0 rN` sequence into a single `move r0 imm`.
fn build_constants_for_call_args(function: &Function) -> HashMap<TempId, f64> {
    let mut constants: HashMap<TempId, f64> = HashMap::new();
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

    let mut has_non_call_use: HashSet<TempId> = HashSet::new();
    for block in &function.blocks {
        for instruction in &block.instructions {
            match instruction {
                Instruction::Call { .. } => {}
                _ => {
                    for temp in instruction_uses(instruction) {
                        if constants.contains_key(&temp) {
                            has_non_call_use.insert(temp);
                        }
                    }
                }
            }
        }
        for temp in terminator_uses(&block.terminator) {
            if constants.contains_key(&temp) {
                has_non_call_use.insert(temp);
            }
        }
    }

    constants
        .into_iter()
        .filter(|(temp, _)| !has_non_call_use.contains(temp))
        .collect()
}

impl<'a> Emitter<'a> {
    fn new(
        function: &'a Function,
        block_order: &'a [BlockId],
        linear_map: &'a LinearMap,
        result: &'a AllocationResult,
        calling_convention: &'a CallingConventionInfo,
        symbols: &'a SymbolTable,
    ) -> Self {
        let mut spills_at: HashMap<LinearPosition, Vec<&SpillRecord>> = HashMap::new();
        let mut reloads_at: HashMap<LinearPosition, Vec<&SpillRecord>> = HashMap::new();
        for record in &result.spills {
            spills_at
                .entry(record.spill_position)
                .or_default()
                .push(record);
            if let Some(reload_position) = record.reload_position {
                reloads_at.entry(reload_position).or_default().push(record);
            }
        }

        Emitter {
            function,
            block_order,
            linear_map,
            assignments: &result.assignments,
            calling_convention,
            symbols,
            spills: &result.spills,
            output: Vec::new(),
            spills_at,
            reloads_at,
            call_sites: Vec::new(),
            constants_for_call_args: build_constants_for_call_args(function),
        }
    }

    fn register_of(&self, temp: TempId) -> Register {
        *self
            .assignments
            .get(&temp)
            .unwrap_or_else(|| panic!("no register assignment for {:?}", temp))
    }

    fn operand_of(&self, temp: TempId) -> Operand {
        Operand::Register(self.register_of(temp))
    }

    fn block(&self, block_id: BlockId) -> &BasicBlock {
        &self.function.blocks[block_id.0]
    }

    fn block_label(&self, block_id: BlockId) -> String {
        if block_id == self.function.entry {
            self.function.name.clone()
        } else {
            format!("{}_{}", self.function.name, block_id.0)
        }
    }

    fn emit(&mut self, instruction: IC10Instruction) {
        self.output.push(instruction);
    }

    fn emit_spills_at(&mut self, position: LinearPosition) {
        if let Some(records) = self.spills_at.get(&position) {
            for record in records {
                self.output
                    .push(IC10Instruction::Push(Operand::Register(record.register)));
            }
        }
    }

    fn emit_reloads_at(&mut self, position: LinearPosition) {
        if let Some(records) = self.reloads_at.get(&position) {
            for record in records.iter().rev() {
                self.output.push(IC10Instruction::Pop(record.register));
            }
        }
    }

    /// Returns `true` if `temp` has been pressure-spilled to the stack and not yet reloaded
    /// at the given linear `position`.  Such temps do not need caller-save push/pop because
    /// their values are already safe on the stack.
    fn is_on_stack_at(&self, temp: TempId, position: LinearPosition) -> bool {
        self.spills.iter().any(|record| {
            record.temp == temp
                && record.spill_position <= position
                && record
                    .reload_position
                    .is_none_or(|reload| reload > position)
        })
    }

    fn emit_function(mut self) -> (IC10Function, Vec<EmittedCallSite>) {
        let is_entry = self.function.name == "main";
        let is_non_leaf = self.calling_convention.function_class == FunctionClass::NonLeaf;

        let block_order: Vec<BlockId> = self.block_order.to_vec();

        for (block_index, &block_id) in block_order.iter().enumerate() {
            self.emit(IC10Instruction::Label(self.block_label(block_id)));

            if block_index == 0 && is_non_leaf && !is_entry {
                self.emit(IC10Instruction::Push(Operand::Register(Register::Ra)));
            }

            let instructions = self.block(block_id).instructions.clone();
            let instruction_count = instructions.len();

            for (instruction_index, instruction) in instructions.iter().enumerate() {
                let position =
                    self.linear_map.instruction_positions[&(block_id, instruction_index)];

                self.emit_spills_at(position);
                self.emit_reloads_at(position);

                let should_suppress = self.should_suppress_for_branch_fusion(
                    block_id,
                    instruction_index,
                    instruction_count,
                );

                if !should_suppress {
                    self.lower_instruction(instruction, position);
                }
            }

            let terminator_position = self.linear_map.terminator_positions[&block_id];
            self.emit_spills_at(terminator_position);
            self.emit_reloads_at(terminator_position);

            let next_block = block_order.get(block_index + 1).copied();
            let terminator = self.block(block_id).terminator.clone();
            self.lower_terminator(block_id, &terminator, next_block, is_entry, is_non_leaf);
        }

        let call_sites = self.call_sites;
        (
            IC10Function {
                name: self.function.name.clone(),
                instructions: self.output,
                is_entry,
            },
            call_sites,
        )
    }

    fn should_suppress_for_branch_fusion(
        &self,
        block_id: BlockId,
        instruction_index: usize,
        instruction_count: usize,
    ) -> bool {
        if instruction_index != instruction_count - 1 {
            return false;
        }
        let block = self.block(block_id);
        let Terminator::Branch { condition, .. } = &block.terminator else {
            return false;
        };
        let instruction = &block.instructions[instruction_index];
        if let Instruction::Assign {
            dest,
            operation:
                Operation::Binary {
                    operator,
                    left,
                    right,
                },
        } = instruction
            && dest == condition
            && is_fusible_comparison(operator)
        {
            let left_reg = self.assignments.get(left);
            let right_reg = self.assignments.get(right);
            return left_reg.is_some() && right_reg.is_some();
        }
        false
    }

    fn lower_instruction(&mut self, instruction: &Instruction, position: LinearPosition) {
        match instruction {
            Instruction::Assign { dest, operation } => {
                self.lower_assign(*dest, operation);
            }
            Instruction::Phi { .. } => {
                unreachable!("phi instructions should have been deconstructed before emission");
            }
            Instruction::LoadDevice { dest, pin, field } => {
                self.emit(IC10Instruction::Load(
                    self.register_of(*dest),
                    *pin,
                    field.clone(),
                ));
            }
            Instruction::StoreDevice { pin, field, source } => {
                self.emit(IC10Instruction::Store(
                    *pin,
                    field.clone(),
                    self.operand_of(*source),
                ));
            }
            Instruction::LoadSlot {
                dest,
                pin,
                slot,
                field,
            } => {
                self.emit(IC10Instruction::LoadSlot(
                    self.register_of(*dest),
                    *pin,
                    self.operand_of(*slot),
                    field.clone(),
                ));
            }
            Instruction::StoreSlot {
                pin,
                slot,
                field,
                source,
            } => {
                self.emit(IC10Instruction::StoreSlot(
                    *pin,
                    self.operand_of(*slot),
                    field.clone(),
                    self.operand_of(*source),
                ));
            }
            Instruction::BatchRead {
                dest,
                hash,
                field,
                mode,
            } => {
                self.emit(IC10Instruction::BatchLoad {
                    dest: self.register_of(*dest),
                    device_hash: self.operand_of(*hash),
                    logic_type: field.clone(),
                    batch_mode: *mode,
                });
            }
            Instruction::BatchWrite { hash, field, value } => {
                self.emit(IC10Instruction::BatchStore {
                    device_hash: self.operand_of(*hash),
                    logic_type: field.clone(),
                    source: self.operand_of(*value),
                });
            }
            Instruction::Call {
                dest,
                function,
                args,
            } => {
                self.lower_call(*dest, *function, args, position);
            }
            Instruction::BuiltinCall {
                dest,
                function,
                args,
            } => {
                self.lower_builtin_call(*dest, *function, args);
            }
            Instruction::Sleep { duration } => {
                self.emit(IC10Instruction::Sleep(self.operand_of(*duration)));
            }
            Instruction::Yield => {
                self.emit(IC10Instruction::Yield);
            }
        }
    }

    fn lower_assign(&mut self, dest: TempId, operation: &Operation) {
        let dest_register = self.register_of(dest);
        match operation {
            Operation::Parameter { .. } => {
                // The caller already deposited the argument in the pre-coloured register;
                // no instruction is needed.
                let _ = dest_register;
            }
            Operation::Constant(value) => {
                if self.constants_for_call_args.contains_key(&dest) {
                    // This constant will be emitted as a literal directly at the call
                    // site; no register materialisation is needed here.
                    return;
                }
                self.emit(IC10Instruction::Move(
                    dest_register,
                    Operand::Literal(*value),
                ));
            }
            Operation::Copy(source) => {
                let source_register = self.register_of(*source);
                if source_register != dest_register {
                    self.emit(IC10Instruction::Move(
                        dest_register,
                        Operand::Register(source_register),
                    ));
                }
            }
            Operation::Binary {
                operator,
                left,
                right,
            } => {
                self.lower_binary(dest_register, *operator, *left, *right);
            }
            Operation::Unary { operator, operand } => {
                self.lower_unary(dest_register, *operator, *operand);
            }
            Operation::Cast {
                operand,
                target_type,
                ..
            } => {
                self.lower_cast(dest_register, *operand, target_type);
            }
            Operation::Select {
                condition,
                if_true,
                if_false,
            } => {
                self.emit(IC10Instruction::Select(
                    dest_register,
                    self.operand_of(*condition),
                    self.operand_of(*if_true),
                    self.operand_of(*if_false),
                ));
            }
        }
    }

    fn lower_binary(
        &mut self,
        dest: Register,
        operator: BinaryOperator,
        left: TempId,
        right: TempId,
    ) {
        let left_operand = self.operand_of(left);
        let right_operand = self.operand_of(right);
        let instruction = match operator {
            BinaryOperator::Add => IC10Instruction::Add(dest, left_operand, right_operand),
            BinaryOperator::Sub => IC10Instruction::Sub(dest, left_operand, right_operand),
            BinaryOperator::Mul => IC10Instruction::Mul(dest, left_operand, right_operand),
            BinaryOperator::Div => IC10Instruction::Div(dest, left_operand, right_operand),
            BinaryOperator::Rem => IC10Instruction::Mod(dest, left_operand, right_operand),
            BinaryOperator::Eq => IC10Instruction::Seq(dest, left_operand, right_operand),
            BinaryOperator::Ne => IC10Instruction::Sne(dest, left_operand, right_operand),
            BinaryOperator::Lt => IC10Instruction::Slt(dest, left_operand, right_operand),
            BinaryOperator::Gt => IC10Instruction::Sgt(dest, left_operand, right_operand),
            BinaryOperator::Le => IC10Instruction::Sle(dest, left_operand, right_operand),
            BinaryOperator::Ge => IC10Instruction::Sge(dest, left_operand, right_operand),
            BinaryOperator::And => {
                // Logical AND: `a && b` → `a * b` (both are 0 or 1)
                IC10Instruction::Mul(dest, left_operand, right_operand)
            }
            BinaryOperator::Or => {
                // Logical OR: `a || b` → `sne dest (a + b) 0` but simpler: `or` works
                // since booleans are 0/1 and bitwise OR gives correct result
                IC10Instruction::Or(dest, left_operand, right_operand)
            }
            BinaryOperator::BitAnd => IC10Instruction::And(dest, left_operand, right_operand),
            BinaryOperator::BitOr => IC10Instruction::Or(dest, left_operand, right_operand),
            BinaryOperator::BitXor => IC10Instruction::Xor(dest, left_operand, right_operand),
            BinaryOperator::Shl => IC10Instruction::Sll(dest, left_operand, right_operand),
            BinaryOperator::Shr => IC10Instruction::Sra(dest, left_operand, right_operand),
        };
        self.emit(instruction);
    }

    fn lower_unary(&mut self, dest: Register, operator: UnaryOperator, operand: TempId) {
        let operand_value = self.operand_of(operand);
        match operator {
            UnaryOperator::Neg => {
                self.emit(IC10Instruction::Sub(
                    dest,
                    Operand::Literal(0.0),
                    operand_value,
                ));
            }
            UnaryOperator::Not => {
                self.emit(IC10Instruction::Seqz(dest, operand_value));
            }
            UnaryOperator::BitNot => {
                self.emit(IC10Instruction::Not(dest, operand_value));
            }
        }
    }

    fn lower_cast(&mut self, dest: Register, operand: TempId, target_type: &Type) {
        let source_register = self.register_of(operand);
        match target_type {
            Type::I53 => {
                self.emit(IC10Instruction::Trunc(
                    dest,
                    Operand::Register(source_register),
                ));
            }
            _ => {
                if source_register != dest {
                    self.emit(IC10Instruction::Move(
                        dest,
                        Operand::Register(source_register),
                    ));
                }
            }
        }
    }

    fn lower_builtin_call(&mut self, dest: TempId, function: BuiltinFunction, args: &[TempId]) {
        let dest_register = self.register_of(dest);
        let instruction = match function {
            BuiltinFunction::Abs => IC10Instruction::Abs(dest_register, self.operand_of(args[0])),
            BuiltinFunction::Ceil => IC10Instruction::Ceil(dest_register, self.operand_of(args[0])),
            BuiltinFunction::Floor => {
                IC10Instruction::Floor(dest_register, self.operand_of(args[0]))
            }
            BuiltinFunction::Round => {
                IC10Instruction::Round(dest_register, self.operand_of(args[0]))
            }
            BuiltinFunction::Trunc => {
                IC10Instruction::Trunc(dest_register, self.operand_of(args[0]))
            }
            BuiltinFunction::Sqrt => IC10Instruction::Sqrt(dest_register, self.operand_of(args[0])),
            BuiltinFunction::Exp => IC10Instruction::Exp(dest_register, self.operand_of(args[0])),
            BuiltinFunction::Log => IC10Instruction::Log(dest_register, self.operand_of(args[0])),
            BuiltinFunction::Sin => IC10Instruction::Sin(dest_register, self.operand_of(args[0])),
            BuiltinFunction::Cos => IC10Instruction::Cos(dest_register, self.operand_of(args[0])),
            BuiltinFunction::Tan => IC10Instruction::Tan(dest_register, self.operand_of(args[0])),
            BuiltinFunction::Asin => IC10Instruction::Asin(dest_register, self.operand_of(args[0])),
            BuiltinFunction::Acos => IC10Instruction::Acos(dest_register, self.operand_of(args[0])),
            BuiltinFunction::Atan => IC10Instruction::Atan(dest_register, self.operand_of(args[0])),
            BuiltinFunction::Atan2 => IC10Instruction::Atan2(
                dest_register,
                self.operand_of(args[0]),
                self.operand_of(args[1]),
            ),
            BuiltinFunction::Pow => IC10Instruction::Pow(
                dest_register,
                self.operand_of(args[0]),
                self.operand_of(args[1]),
            ),
            BuiltinFunction::Min => IC10Instruction::Min(
                dest_register,
                self.operand_of(args[0]),
                self.operand_of(args[1]),
            ),
            BuiltinFunction::Max => IC10Instruction::Max(
                dest_register,
                self.operand_of(args[0]),
                self.operand_of(args[1]),
            ),
            BuiltinFunction::Lerp => IC10Instruction::Lerp(
                dest_register,
                self.operand_of(args[0]),
                self.operand_of(args[1]),
                self.operand_of(args[2]),
            ),
            BuiltinFunction::Clamp => {
                // clamp(x, min, max) → max(min, min(x, max))
                // Uses two instructions: min then max.
                self.emit(IC10Instruction::Min(
                    dest_register,
                    self.operand_of(args[0]),
                    self.operand_of(args[2]),
                ));
                IC10Instruction::Max(
                    dest_register,
                    Operand::Register(dest_register),
                    self.operand_of(args[1]),
                )
            }
            BuiltinFunction::Rand => IC10Instruction::Rand(dest_register),
        };
        self.emit(instruction);
    }

    fn lower_call(
        &mut self,
        dest: Option<TempId>,
        function_symbol: crate::resolved::SymbolId,
        args: &[TempId],
        position: LinearPosition,
    ) {
        let function_name = self.symbols.get(function_symbol).name.clone();

        let live_across_registers: Vec<Register> = self
            .calling_convention
            .live_across_calls
            .get(&position)
            .map(|temps| {
                temps
                    .iter()
                    .filter(|&&temp| !self.is_on_stack_at(temp, position))
                    .filter_map(|&temp| self.assignments.get(&temp).copied())
                    .collect()
            })
            .unwrap_or_default();

        let sequence_start = self.output.len();

        for (index, &arg) in args.iter().enumerate() {
            let target_register = register_for_index(index);
            if let Some(&value) = self.constants_for_call_args.get(&arg) {
                self.emit(IC10Instruction::Move(
                    target_register,
                    Operand::Literal(value),
                ));
            } else {
                let source_register = self.register_of(arg);
                if source_register != target_register {
                    self.emit(IC10Instruction::Move(
                        target_register,
                        Operand::Register(source_register),
                    ));
                }
            }
        }

        self.emit(IC10Instruction::JumpAndLink(JumpTarget::Label(
            function_name.clone(),
        )));

        if let Some(dest_temp) = dest {
            let dest_register = self.register_of(dest_temp);
            if dest_register != Register::R0 {
                self.emit(IC10Instruction::Move(
                    dest_register,
                    Operand::Register(Register::R0),
                ));
            }
        }

        let sequence_end = self.output.len();

        self.call_sites.push(EmittedCallSite {
            sequence_start,
            sequence_end,
            callee_name: function_name,
            arg_count: args.len(),
            live_across_registers,
        });
    }

    fn lower_terminator(
        &mut self,
        block_id: BlockId,
        terminator: &Terminator,
        next_block: Option<BlockId>,
        is_entry: bool,
        is_non_leaf: bool,
    ) {
        match terminator {
            Terminator::Jump(target) => {
                if Some(*target) != next_block {
                    self.emit(IC10Instruction::Jump(JumpTarget::Label(
                        self.block_label(*target),
                    )));
                }
            }
            Terminator::Branch {
                condition,
                true_block,
                false_block,
            } => {
                self.lower_branch(block_id, *condition, *true_block, *false_block, next_block);
            }
            Terminator::Return(value) => {
                if is_entry {
                    self.emit(IC10Instruction::HaltAndCatchFire);
                } else {
                    if let Some(temp) = value {
                        let source_register = self.register_of(*temp);
                        if source_register != Register::R0 {
                            self.emit(IC10Instruction::Move(
                                Register::R0,
                                Operand::Register(source_register),
                            ));
                        }
                    }
                    if is_non_leaf {
                        self.emit(IC10Instruction::Pop(Register::Ra));
                    }
                    self.emit(IC10Instruction::Jump(JumpTarget::Register(Register::Ra)));
                }
            }
            Terminator::None => {
                panic!("encountered placeholder terminator during emission");
            }
        }
    }

    fn lower_branch(
        &mut self,
        block_id: BlockId,
        condition: TempId,
        true_block: BlockId,
        false_block: BlockId,
        next_block: Option<BlockId>,
    ) {
        let block = self.block(block_id);
        if let Some(last_instruction) = block.instructions.last()
            && let Instruction::Assign {
                dest,
                operation:
                    Operation::Binary {
                        operator,
                        left,
                        right,
                    },
            } = last_instruction
            && *dest == condition
            && is_fusible_comparison(operator)
        {
            let left_operand = self.operand_of(*left);
            let right_operand = self.operand_of(*right);
            self.emit_fused_branch(
                *operator,
                left_operand,
                right_operand,
                true_block,
                false_block,
                next_block,
            );
            return;
        }

        let condition_operand = self.operand_of(condition);
        if Some(false_block) == next_block {
            self.emit(IC10Instruction::BranchNotEqualZero(
                condition_operand,
                JumpTarget::Label(self.block_label(true_block)),
            ));
        } else if Some(true_block) == next_block {
            self.emit(IC10Instruction::BranchEqualZero(
                condition_operand,
                JumpTarget::Label(self.block_label(false_block)),
            ));
        } else {
            self.emit(IC10Instruction::BranchNotEqualZero(
                condition_operand,
                JumpTarget::Label(self.block_label(true_block)),
            ));
            self.emit(IC10Instruction::Jump(JumpTarget::Label(
                self.block_label(false_block),
            )));
        }
    }

    fn emit_fused_branch(
        &mut self,
        operator: BinaryOperator,
        left: Operand,
        right: Operand,
        true_block: BlockId,
        false_block: BlockId,
        next_block: Option<BlockId>,
    ) {
        let true_label = JumpTarget::Label(self.block_label(true_block));
        let false_label = JumpTarget::Label(self.block_label(false_block));

        if Some(false_block) == next_block {
            let branch = match operator {
                BinaryOperator::Eq => IC10Instruction::BranchEqual(left, right, true_label),
                BinaryOperator::Ne => IC10Instruction::BranchNotEqual(left, right, true_label),
                BinaryOperator::Lt => IC10Instruction::BranchLessThan(left, right, true_label),
                BinaryOperator::Gt => IC10Instruction::BranchGreaterThan(left, right, true_label),
                BinaryOperator::Le => IC10Instruction::BranchLessEqual(left, right, true_label),
                BinaryOperator::Ge => IC10Instruction::BranchGreaterEqual(left, right, true_label),
                _ => unreachable!("non-comparison operator in fused branch"),
            };
            self.emit(branch);
        } else if Some(true_block) == next_block {
            let inverted = match operator {
                BinaryOperator::Eq => IC10Instruction::BranchNotEqual(left, right, false_label),
                BinaryOperator::Ne => IC10Instruction::BranchEqual(left, right, false_label),
                BinaryOperator::Lt => IC10Instruction::BranchGreaterEqual(left, right, false_label),
                BinaryOperator::Gt => IC10Instruction::BranchLessEqual(left, right, false_label),
                BinaryOperator::Le => IC10Instruction::BranchGreaterThan(left, right, false_label),
                BinaryOperator::Ge => IC10Instruction::BranchLessThan(left, right, false_label),
                _ => unreachable!("non-comparison operator in fused branch"),
            };
            self.emit(inverted);
        } else {
            let branch = match operator {
                BinaryOperator::Eq => IC10Instruction::BranchEqual(left, right, true_label),
                BinaryOperator::Ne => IC10Instruction::BranchNotEqual(left, right, true_label),
                BinaryOperator::Lt => IC10Instruction::BranchLessThan(left, right, true_label),
                BinaryOperator::Gt => IC10Instruction::BranchGreaterThan(left, right, true_label),
                BinaryOperator::Le => IC10Instruction::BranchLessEqual(left, right, true_label),
                BinaryOperator::Ge => IC10Instruction::BranchGreaterEqual(left, right, true_label),
                _ => unreachable!("non-comparison operator in fused branch"),
            };
            self.emit(branch);
            self.emit(IC10Instruction::Jump(false_label));
        }
    }
}

fn is_fusible_comparison(operator: &BinaryOperator) -> bool {
    matches!(
        operator,
        BinaryOperator::Eq
            | BinaryOperator::Ne
            | BinaryOperator::Lt
            | BinaryOperator::Gt
            | BinaryOperator::Le
            | BinaryOperator::Ge
    )
}

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
        _ => panic!("argument index {} exceeds maximum of 8 parameters", index),
    }
}

/// Resolve all symbolic labels in an instruction list to absolute line numbers.
///
/// First pass: scan instructions counting non-`Label` lines, recording each label's position.
/// Second pass: rewrite all `JumpTarget::Label` references to `JumpTarget::Line`, then remove
/// all `Label` pseudo-instructions.
pub fn resolve_labels(instructions: Vec<IC10Instruction>) -> Vec<IC10Instruction> {
    let mut label_positions: HashMap<String, u32> = HashMap::new();
    let mut line_number: u32 = 0;
    for instruction in &instructions {
        if let IC10Instruction::Label(name) = instruction {
            label_positions.insert(name.clone(), line_number);
        } else {
            line_number += 1;
        }
    }

    instructions
        .into_iter()
        .filter_map(|instruction| {
            if matches!(instruction, IC10Instruction::Label(_)) {
                return None;
            }
            Some(rewrite_jump_targets(instruction, &label_positions))
        })
        .collect()
}

fn resolve_target(target: JumpTarget, labels: &HashMap<String, u32>) -> JumpTarget {
    match target {
        JumpTarget::Label(name) => {
            let line = labels
                .get(&name)
                .unwrap_or_else(|| panic!("unresolved label: {}", name));
            JumpTarget::Line(*line)
        }
        already_resolved => already_resolved,
    }
}

fn rewrite_jump_targets(
    instruction: IC10Instruction,
    labels: &HashMap<String, u32>,
) -> IC10Instruction {
    match instruction {
        IC10Instruction::Jump(target) => IC10Instruction::Jump(resolve_target(target, labels)),
        IC10Instruction::JumpAndLink(target) => {
            IC10Instruction::JumpAndLink(resolve_target(target, labels))
        }
        IC10Instruction::BranchEqual(a, b, target) => {
            IC10Instruction::BranchEqual(a, b, resolve_target(target, labels))
        }
        IC10Instruction::BranchEqualZero(a, target) => {
            IC10Instruction::BranchEqualZero(a, resolve_target(target, labels))
        }
        IC10Instruction::BranchNotEqual(a, b, target) => {
            IC10Instruction::BranchNotEqual(a, b, resolve_target(target, labels))
        }
        IC10Instruction::BranchNotEqualZero(a, target) => {
            IC10Instruction::BranchNotEqualZero(a, resolve_target(target, labels))
        }
        IC10Instruction::BranchGreaterThan(a, b, target) => {
            IC10Instruction::BranchGreaterThan(a, b, resolve_target(target, labels))
        }
        IC10Instruction::BranchGreaterThanZero(a, target) => {
            IC10Instruction::BranchGreaterThanZero(a, resolve_target(target, labels))
        }
        IC10Instruction::BranchGreaterEqual(a, b, target) => {
            IC10Instruction::BranchGreaterEqual(a, b, resolve_target(target, labels))
        }
        IC10Instruction::BranchGreaterEqualZero(a, target) => {
            IC10Instruction::BranchGreaterEqualZero(a, resolve_target(target, labels))
        }
        IC10Instruction::BranchLessThan(a, b, target) => {
            IC10Instruction::BranchLessThan(a, b, resolve_target(target, labels))
        }
        IC10Instruction::BranchLessThanZero(a, target) => {
            IC10Instruction::BranchLessThanZero(a, resolve_target(target, labels))
        }
        IC10Instruction::BranchLessEqual(a, b, target) => {
            IC10Instruction::BranchLessEqual(a, b, resolve_target(target, labels))
        }
        IC10Instruction::BranchLessEqualZero(a, target) => {
            IC10Instruction::BranchLessEqualZero(a, resolve_target(target, labels))
        }
        IC10Instruction::BranchDeviceSet(pin, target) => {
            IC10Instruction::BranchDeviceSet(pin, resolve_target(target, labels))
        }
        IC10Instruction::BranchDeviceNotSet(pin, target) => {
            IC10Instruction::BranchDeviceNotSet(pin, resolve_target(target, labels))
        }
        IC10Instruction::BranchDeviceNotValidLoad(pin, field, target) => {
            IC10Instruction::BranchDeviceNotValidLoad(pin, field, resolve_target(target, labels))
        }
        IC10Instruction::BranchDeviceNotValidStore(pin, field, target) => {
            IC10Instruction::BranchDeviceNotValidStore(pin, field, resolve_target(target, labels))
        }
        other => other,
    }
}

/// Emit IC10 instructions for a single function.
///
/// Returns the emitted function and metadata about each call site, which the caller-save
/// post-pass uses to insert push/pop instructions.
pub fn emit_function(
    function: &Function,
    block_order: &[BlockId],
    linear_map: &LinearMap,
    result: &AllocationResult,
    calling_convention: &CallingConventionInfo,
    symbols: &SymbolTable,
) -> (IC10Function, Vec<EmittedCallSite>) {
    let emitter = Emitter::new(
        function,
        block_order,
        linear_map,
        result,
        calling_convention,
        symbols,
    );
    emitter.emit_function()
}

/// Compute which general-purpose registers (R0–R15) each function may write, transitively
/// including the effects of any functions it calls.
///
/// Since IC20 programs contain no indirect calls, the full call graph is statically known.
/// Recursion (cycles in the call graph) is handled by fixpoint iteration: each function's
/// clobber set starts with the registers it writes directly, then callee clobber sets are
/// unioned in repeatedly until convergence.
pub fn compute_clobber_sets(functions: &[IC10Function]) -> HashMap<String, HashSet<Register>> {
    let mut clobber: HashMap<String, HashSet<Register>> = HashMap::new();
    let mut callees: HashMap<String, Vec<String>> = HashMap::new();

    for function in functions {
        let mut direct_writes: HashSet<Register> = HashSet::new();
        let mut targets: Vec<String> = Vec::new();

        for instruction in &function.instructions {
            if let Some(register) = instruction.written_register()
                && register.is_general_purpose()
            {
                direct_writes.insert(register);
            }
            if let IC10Instruction::JumpAndLink(JumpTarget::Label(name)) = instruction {
                targets.push(name.clone());
            }
        }

        clobber.insert(function.name.clone(), direct_writes);
        callees.insert(function.name.clone(), targets);
    }

    loop {
        let mut changed = false;
        for function in functions {
            if let Some(targets) = callees.get(&function.name) {
                let callee_registers: HashSet<Register> = targets
                    .iter()
                    .flat_map(|target| clobber.get(target).cloned().unwrap_or_default())
                    .collect();

                let function_set = clobber.get_mut(&function.name).unwrap();
                let old_size = function_set.len();
                function_set.extend(callee_registers);
                if function_set.len() > old_size {
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    clobber
}

/// Insert caller-save push/pop instructions around call sites in a function.
///
/// For each call site, determines which live-across registers are actually clobbered by the
/// callee (or overwritten by argument-setup moves) and emits `push`/`pop` pairs around the
/// call sequence.  Registers that the callee does not clobber are left alone, saving IC10
/// lines.
///
/// Push instructions are inserted before the argument-setup moves; pop instructions (in
/// reverse order, respecting LIFO stack discipline) are inserted after the return-value move.
pub fn insert_caller_saves(
    function: &mut IC10Function,
    call_sites: &[EmittedCallSite],
    clobber_sets: &HashMap<String, HashSet<Register>>,
) {
    if call_sites.is_empty() {
        return;
    }

    let empty_set: HashSet<Register> = HashSet::new();
    let mut offset: usize = 0;

    for site in call_sites {
        let callee_clobber = clobber_sets.get(&site.callee_name).unwrap_or(&empty_set);

        let mut arg_registers: HashSet<Register> = HashSet::new();
        for index in 0..site.arg_count {
            arg_registers.insert(register_for_index(index));
        }

        let combined_clobber: HashSet<&Register> = callee_clobber.union(&arg_registers).collect();

        let registers_to_save: Vec<Register> = site
            .live_across_registers
            .iter()
            .filter(|register| register.is_caller_saved())
            .filter(|register| combined_clobber.contains(register))
            .copied()
            .collect();

        if registers_to_save.is_empty() {
            continue;
        }

        let push_index = site.sequence_start + offset;
        let pop_index = site.sequence_end + offset + registers_to_save.len();

        let pushes: Vec<IC10Instruction> = registers_to_save
            .iter()
            .map(|&register| IC10Instruction::Push(Operand::Register(register)))
            .collect();

        let pops: Vec<IC10Instruction> = registers_to_save
            .iter()
            .rev()
            .map(|&register| IC10Instruction::Pop(register))
            .collect();

        let push_count = pushes.len();
        let pop_count = pops.len();

        for (i, push) in pushes.into_iter().enumerate() {
            function.instructions.insert(push_index + i, push);
        }
        for (i, pop) in pops.into_iter().enumerate() {
            function.instructions.insert(pop_index + i, pop);
        }

        offset += push_count + pop_count;
    }
}

/// Insert callee-save push/pop instructions for registers `r8`–`r15` in a function.
///
/// Scans the function's instruction list to find which callee-saved registers are
/// written, then inserts `push` at the function entry (after the label and any `push ra`)
/// and `pop` before each return sequence (`pop ra` + `j ra` or just `j ra`).
///
/// Entry functions (`main`) do not need callee-saves because they are not called via `jal`.
pub fn insert_callee_saves(function: &mut IC10Function) {
    if function.is_entry {
        return;
    }

    let mut used_callee_saved: Vec<Register> = function
        .instructions
        .iter()
        .filter_map(|instruction| instruction.written_register())
        .filter(|register| register.is_callee_saved())
        .collect::<std::collections::HashSet<Register>>()
        .into_iter()
        .collect();

    if used_callee_saved.is_empty() {
        return;
    }

    used_callee_saved.sort();

    let entry_insert_index = function
        .instructions
        .iter()
        .position(|instruction| {
            !matches!(instruction, IC10Instruction::Label(_))
                && !matches!(
                    instruction,
                    IC10Instruction::Push(Operand::Register(Register::Ra))
                )
        })
        .unwrap_or(function.instructions.len());

    let pushes: Vec<IC10Instruction> = used_callee_saved
        .iter()
        .map(|&register| IC10Instruction::Push(Operand::Register(register)))
        .collect();
    let push_count = pushes.len();
    for (i, push) in pushes.into_iter().enumerate() {
        function.instructions.insert(entry_insert_index + i, push);
    }

    let pops: Vec<IC10Instruction> = used_callee_saved
        .iter()
        .rev()
        .map(|&register| IC10Instruction::Pop(register))
        .collect();

    let mut index = push_count;
    while index < function.instructions.len() {
        if matches!(
            &function.instructions[index],
            IC10Instruction::Jump(JumpTarget::Register(Register::Ra))
        ) {
            let pop_before = if index > 0
                && matches!(
                    &function.instructions[index - 1],
                    IC10Instruction::Pop(Register::Ra)
                ) {
                index - 1
            } else {
                index
            };
            for (i, pop) in pops.iter().enumerate() {
                function.instructions.insert(pop_before + i, pop.clone());
            }
            index += pops.len() + 1;
        } else {
            index += 1;
        }
    }
}
