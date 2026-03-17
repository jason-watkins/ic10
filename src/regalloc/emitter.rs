use std::collections::{HashMap, HashSet};

use crate::ir::DevicePin;
use crate::ir::bound::{StaticVariable, SymbolTable};
use crate::ir::cfg::{
    BasicBlock, BlockId, BlockRole, Function, Instruction, Operation, TempId, Terminator,
};
use crate::ir::{BinaryOperator, Intrinsic, Type, UnaryOperator};

use super::allocator::{AllocationResult, SpillRecord};
use super::calling_convention::{CallingConventionInfo, FunctionClass};
use super::ic10::{IC10Function, IC10Instruction, JumpTarget, Operand, Register};
use super::liveness::{LinearMap, LinearPosition};

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
    statics: &'a [StaticVariable],
    spills: &'a [SpillRecord],
    output: Vec<IC10Instruction>,
    /// Spills indexed by the position at which the push should be emitted.
    spills_at: HashMap<LinearPosition, Vec<&'a SpillRecord>>,
    /// Reloads indexed by the position at which the pop should be emitted.
    reloads_at: HashMap<LinearPosition, Vec<&'a SpillRecord>>,
    /// Call site metadata collected during emission, consumed by the caller-save post-pass.
    call_sites: Vec<EmittedCallSite>,
    /// The value of every constant temp. `operand_of` returns `Operand::Literal` for
    /// these so that constants are inlined directly into arithmetic, comparison, branch,
    /// and call instructions, and `lower_assign` suppresses the `move rN imm` that would
    /// otherwise materialise them into a register.
    constant_values: HashMap<TempId, f64>,
}

/// Collect the value of every constant temp in the function.
///
/// Used by `operand_of` to return `Operand::Literal` at every use site so that
/// constants are inlined directly into instructions rather than materialised into a
/// register first.
fn build_constant_values(function: &Function) -> HashMap<TempId, f64> {
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
    constants
}

/// Convert a snake_case identifier to camelCase so that generated IC10 labels
/// contain no underscores (which the in-game editor rejects).
///
/// Each word after the first has its initial letter uppercased; leading/trailing/
/// consecutive underscores are collapsed and dropped.
fn snake_case_to_camel_case(name: &str) -> String {
    let mut result = String::with_capacity(name.len());
    let mut capitalise_next = false;
    for character in name.chars() {
        if character == '_' {
            capitalise_next = true;
        } else if capitalise_next {
            result.extend(character.to_uppercase());
            capitalise_next = false;
        } else {
            result.push(character);
        }
    }
    result
}

impl<'a> Emitter<'a> {
    fn new(
        function: &'a Function,
        block_order: &'a [BlockId],
        linear_map: &'a LinearMap,
        result: &'a AllocationResult,
        calling_convention: &'a CallingConventionInfo,
        symbols: &'a SymbolTable,
        statics: &'a [StaticVariable],
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
            statics,
            spills: &result.spills,
            output: Vec::new(),
            spills_at,
            reloads_at,
            call_sites: Vec::new(),
            constant_values: build_constant_values(function),
        }
    }

    fn register_of(&self, temp: TempId) -> Register {
        *self
            .assignments
            .get(&temp)
            .unwrap_or_else(|| panic!("no register assignment for {:?}", temp))
    }

    fn operand_of(&self, temp: TempId) -> Operand {
        if let Some(&value) = self.constant_values.get(&temp) {
            return Operand::Literal(value);
        }
        Operand::Register(self.register_of(temp))
    }

    fn block(&self, block_id: BlockId) -> &BasicBlock {
        &self.function.blocks[block_id.0]
    }

    fn block_label(&self, block_id: BlockId) -> String {
        let prefix = snake_case_to_camel_case(&self.function.name);
        self.block_label_with_prefix(
            block_id,
            &prefix,
            &self.function.blocks[block_id.0].role.clone(),
        )
    }

    fn block_label_with_prefix(&self, block_id: BlockId, prefix: &str, role: &BlockRole) -> String {
        match role {
            BlockRole::Entry => prefix.to_owned(),
            BlockRole::LoopStart(n) => format!("{}Loop{}Start", prefix, n),
            BlockRole::LoopBody(n) => format!("{}Loop{}Body", prefix, n),
            BlockRole::LoopContinue(n) => format!("{}Loop{}Continue", prefix, n),
            BlockRole::LoopPreHeader(n) => format!("{}Loop{}PreHeader", prefix, n),
            BlockRole::LoopEnd(n) => format!("{}Loop{}End", prefix, n),
            BlockRole::IfTrue(n) => format!("{}If{}True", prefix, n),
            BlockRole::IfFalse(n) => format!("{}If{}False", prefix, n),
            BlockRole::IfEnd(n) => format!("{}If{}End", prefix, n),
            BlockRole::Generic => format!("{}Block{}", prefix, block_id.0),
            BlockRole::Inlined {
                callee_name,
                original_role,
            } => {
                let callee_prefix = snake_case_to_camel_case(callee_name);
                match original_role.as_ref() {
                    // Entry and Generic fall back to block ID to avoid duplicate labels when
                    // the same callee is inlined at multiple call sites.
                    BlockRole::Entry | BlockRole::Generic => {
                        format!("{}Block{}", callee_prefix, block_id.0)
                    }
                    other => self.block_label_with_prefix(block_id, &callee_prefix, other),
                }
            }
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

            for (instruction_index, instruction) in instructions.iter().enumerate() {
                let position =
                    self.linear_map.instruction_positions[&(block_id, instruction_index)];

                self.emit_spills_at(position);
                self.emit_reloads_at(position);
                self.lower_instruction(instruction, position);
            }

            let terminator_position = self.linear_map.terminator_positions[&block_id];
            self.emit_spills_at(terminator_position);
            self.emit_reloads_at(terminator_position);

            let next_block = block_order.get(block_index + 1).copied();
            let terminator = self.block(block_id).terminator.clone();
            self.lower_terminator(&terminator, next_block, is_entry, is_non_leaf);
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
            Instruction::IntrinsicCall {
                dest,
                function,
                args,
            } => {
                self.lower_intrinsic_call(*dest, *function, args);
            }
            Instruction::Sleep { duration } => {
                self.emit(IC10Instruction::Sleep(self.operand_of(*duration)));
            }
            Instruction::Yield => {
                self.emit(IC10Instruction::Yield);
            }
            Instruction::LoadStatic { dest, static_id } => {
                let address = self.statics[static_id.0].address;
                self.emit(IC10Instruction::Get(
                    self.register_of(*dest),
                    DevicePin::Db,
                    Operand::Literal(address as f64),
                ));
            }
            Instruction::StoreStatic { static_id, source } => {
                let address = self.statics[static_id.0].address;
                self.emit(IC10Instruction::Poke(
                    Operand::Literal(address as f64),
                    self.operand_of(*source),
                ));
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
            Operation::Constant(_) => {
                // Inlined as a literal at every use site via `operand_of`; no register
                // materialisation is needed.
            }
            Operation::Copy(source) => {
                let source_operand = self.operand_of(*source);
                if source_operand != Operand::Register(dest_register) {
                    self.emit(IC10Instruction::Move(dest_register, source_operand));
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
        let source_operand = self.operand_of(operand);
        match target_type {
            Type::I53 => {
                self.emit(IC10Instruction::Trunc(dest, source_operand));
            }
            _ => {
                if source_operand != Operand::Register(dest) {
                    self.emit(IC10Instruction::Move(dest, source_operand));
                }
            }
        }
    }

    fn lower_intrinsic_call(&mut self, dest: TempId, function: Intrinsic, args: &[TempId]) {
        let dest_register = self.register_of(dest);
        let instruction = match function {
            Intrinsic::Abs => IC10Instruction::Abs(dest_register, self.operand_of(args[0])),
            Intrinsic::Ceil => IC10Instruction::Ceil(dest_register, self.operand_of(args[0])),
            Intrinsic::Floor => IC10Instruction::Floor(dest_register, self.operand_of(args[0])),
            Intrinsic::Round => IC10Instruction::Round(dest_register, self.operand_of(args[0])),
            Intrinsic::Trunc => IC10Instruction::Trunc(dest_register, self.operand_of(args[0])),
            Intrinsic::Sqrt => IC10Instruction::Sqrt(dest_register, self.operand_of(args[0])),
            Intrinsic::Exp => IC10Instruction::Exp(dest_register, self.operand_of(args[0])),
            Intrinsic::Log => IC10Instruction::Log(dest_register, self.operand_of(args[0])),
            Intrinsic::Sin => IC10Instruction::Sin(dest_register, self.operand_of(args[0])),
            Intrinsic::Cos => IC10Instruction::Cos(dest_register, self.operand_of(args[0])),
            Intrinsic::Tan => IC10Instruction::Tan(dest_register, self.operand_of(args[0])),
            Intrinsic::Asin => IC10Instruction::Asin(dest_register, self.operand_of(args[0])),
            Intrinsic::Acos => IC10Instruction::Acos(dest_register, self.operand_of(args[0])),
            Intrinsic::Atan => IC10Instruction::Atan(dest_register, self.operand_of(args[0])),
            Intrinsic::Atan2 => IC10Instruction::Atan2(
                dest_register,
                self.operand_of(args[0]),
                self.operand_of(args[1]),
            ),
            Intrinsic::Pow => IC10Instruction::Pow(
                dest_register,
                self.operand_of(args[0]),
                self.operand_of(args[1]),
            ),
            Intrinsic::Min => IC10Instruction::Min(
                dest_register,
                self.operand_of(args[0]),
                self.operand_of(args[1]),
            ),
            Intrinsic::Max => IC10Instruction::Max(
                dest_register,
                self.operand_of(args[0]),
                self.operand_of(args[1]),
            ),
            Intrinsic::Lerp => IC10Instruction::Lerp(
                dest_register,
                self.operand_of(args[0]),
                self.operand_of(args[1]),
                self.operand_of(args[2]),
            ),
            Intrinsic::Clamp => {
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
            Intrinsic::Rand => IC10Instruction::Rand(dest_register),
            Intrinsic::IsNan => IC10Instruction::Snan(dest_register, self.operand_of(args[0])),
        };
        self.emit(instruction);
    }

    fn lower_call(
        &mut self,
        dest: Option<TempId>,
        function_symbol: crate::ir::bound::SymbolId,
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
            let source_operand = self.operand_of(arg);
            if source_operand != Operand::Register(target_register) {
                self.emit(IC10Instruction::Move(target_register, source_operand));
            }
        }

        let camel_function_name = snake_case_to_camel_case(&function_name);
        self.emit(IC10Instruction::JumpAndLink(JumpTarget::Label(
            camel_function_name.clone(),
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
            callee_name: camel_function_name,
            arg_count: args.len(),
            live_across_registers,
        });
    }

    fn lower_terminator(
        &mut self,
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
                self.lower_branch(*condition, *true_block, *false_block, next_block);
            }
            Terminator::Return(value) => {
                if is_entry {
                    self.emit(IC10Instruction::HaltAndCatchFire);
                } else {
                    if let Some(temp) = value {
                        let source_operand = self.operand_of(*temp);
                        if source_operand != Operand::Register(Register::R0) {
                            self.emit(IC10Instruction::Move(Register::R0, source_operand));
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
        condition: TempId,
        true_block: BlockId,
        false_block: BlockId,
        next_block: Option<BlockId>,
    ) {
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
        IC10Instruction::BranchDeviceSetAndLink(pin, target) => {
            IC10Instruction::BranchDeviceSetAndLink(pin, resolve_target(target, labels))
        }
        IC10Instruction::BranchDeviceNotSetAndLink(pin, target) => {
            IC10Instruction::BranchDeviceNotSetAndLink(pin, resolve_target(target, labels))
        }
        IC10Instruction::BranchDeviceNotValidLoad(pin, field, target) => {
            IC10Instruction::BranchDeviceNotValidLoad(pin, field, resolve_target(target, labels))
        }
        IC10Instruction::BranchDeviceNotValidStore(pin, field, target) => {
            IC10Instruction::BranchDeviceNotValidStore(pin, field, resolve_target(target, labels))
        }
        IC10Instruction::BranchApproximateEqual {
            left,
            right,
            epsilon,
            target,
        } => IC10Instruction::BranchApproximateEqual {
            left,
            right,
            epsilon,
            target: resolve_target(target, labels),
        },
        IC10Instruction::BranchApproximateZero {
            value,
            epsilon,
            target,
        } => IC10Instruction::BranchApproximateZero {
            value,
            epsilon,
            target: resolve_target(target, labels),
        },
        IC10Instruction::BranchNotApproximateEqual {
            left,
            right,
            epsilon,
            target,
        } => IC10Instruction::BranchNotApproximateEqual {
            left,
            right,
            epsilon,
            target: resolve_target(target, labels),
        },
        IC10Instruction::BranchNotApproximateZero {
            value,
            epsilon,
            target,
        } => IC10Instruction::BranchNotApproximateZero {
            value,
            epsilon,
            target: resolve_target(target, labels),
        },
        IC10Instruction::BranchNaN(a, target) => {
            IC10Instruction::BranchNaN(a, resolve_target(target, labels))
        }
        IC10Instruction::BranchEqualAndLink(a, b, target) => {
            IC10Instruction::BranchEqualAndLink(a, b, resolve_target(target, labels))
        }
        IC10Instruction::BranchEqualZeroAndLink(a, target) => {
            IC10Instruction::BranchEqualZeroAndLink(a, resolve_target(target, labels))
        }
        IC10Instruction::BranchNotEqualAndLink(a, b, target) => {
            IC10Instruction::BranchNotEqualAndLink(a, b, resolve_target(target, labels))
        }
        IC10Instruction::BranchNotEqualZeroAndLink(a, target) => {
            IC10Instruction::BranchNotEqualZeroAndLink(a, resolve_target(target, labels))
        }
        IC10Instruction::BranchGreaterThanAndLink(a, b, target) => {
            IC10Instruction::BranchGreaterThanAndLink(a, b, resolve_target(target, labels))
        }
        IC10Instruction::BranchGreaterThanZeroAndLink(a, target) => {
            IC10Instruction::BranchGreaterThanZeroAndLink(a, resolve_target(target, labels))
        }
        IC10Instruction::BranchGreaterEqualAndLink(a, b, target) => {
            IC10Instruction::BranchGreaterEqualAndLink(a, b, resolve_target(target, labels))
        }
        IC10Instruction::BranchGreaterEqualZeroAndLink(a, target) => {
            IC10Instruction::BranchGreaterEqualZeroAndLink(a, resolve_target(target, labels))
        }
        IC10Instruction::BranchLessThanAndLink(a, b, target) => {
            IC10Instruction::BranchLessThanAndLink(a, b, resolve_target(target, labels))
        }
        IC10Instruction::BranchLessThanZeroAndLink(a, target) => {
            IC10Instruction::BranchLessThanZeroAndLink(a, resolve_target(target, labels))
        }
        IC10Instruction::BranchLessEqualAndLink(a, b, target) => {
            IC10Instruction::BranchLessEqualAndLink(a, b, resolve_target(target, labels))
        }
        IC10Instruction::BranchLessEqualZeroAndLink(a, target) => {
            IC10Instruction::BranchLessEqualZeroAndLink(a, resolve_target(target, labels))
        }
        IC10Instruction::BranchApproximateEqualAndLink(a, b, c, target) => {
            IC10Instruction::BranchApproximateEqualAndLink(a, b, c, resolve_target(target, labels))
        }
        IC10Instruction::BranchApproximateZeroAndLink(a, b, target) => {
            IC10Instruction::BranchApproximateZeroAndLink(a, b, resolve_target(target, labels))
        }
        IC10Instruction::BranchNotApproximateEqualAndLink(a, b, c, target) => {
            IC10Instruction::BranchNotApproximateEqualAndLink(
                a,
                b,
                c,
                resolve_target(target, labels),
            )
        }
        IC10Instruction::BranchNotApproximateZeroAndLink(a, b, target) => {
            IC10Instruction::BranchNotApproximateZeroAndLink(a, b, resolve_target(target, labels))
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
    statics: &[StaticVariable],
) -> (IC10Function, Vec<EmittedCallSite>) {
    let emitter = Emitter::new(
        function,
        block_order,
        linear_map,
        result,
        calling_convention,
        symbols,
        statics,
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

        let camel_name = snake_case_to_camel_case(&function.name);
        clobber.insert(camel_name.clone(), direct_writes);
        callees.insert(camel_name, targets);
    }

    loop {
        let mut changed = false;
        for function in functions {
            let camel_name = snake_case_to_camel_case(&function.name);
            if let Some(targets) = callees.get(&camel_name) {
                let callee_registers: HashSet<Register> = targets
                    .iter()
                    .flat_map(|target| clobber.get(target).cloned().unwrap_or_default())
                    .collect();

                let function_set = clobber.get_mut(&camel_name).unwrap();
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
