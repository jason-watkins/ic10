use crate::ast::{BatchMode, DevicePin};

/// A physical IC10 general-purpose or special register.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Register {
    R0,
    R1,
    R2,
    R3,
    R4,
    R5,
    R6,
    R7,
    R8,
    R9,
    R10,
    R11,
    R12,
    R13,
    R14,
    R15,
    /// Return address register — set by `jal` and `b?al` branch-and-link instructions.
    Ra,
    /// Stack pointer — incremented by `push`, decremented by `pop`.
    Sp,
}

impl Register {
    /// Returns `true` for general-purpose registers `R0`–`R15`.
    pub fn is_general_purpose(&self) -> bool {
        !matches!(self, Register::Ra | Register::Sp)
    }

    /// Returns `true` for caller-saved (scratch) registers `R0`–`R7`.
    pub fn is_caller_saved(&self) -> bool {
        matches!(
            self,
            Register::R0
                | Register::R1
                | Register::R2
                | Register::R3
                | Register::R4
                | Register::R5
                | Register::R6
                | Register::R7
        )
    }

    /// Returns `true` for callee-saved registers `R8`–`R15`.
    pub fn is_callee_saved(&self) -> bool {
        matches!(
            self,
            Register::R8
                | Register::R9
                | Register::R10
                | Register::R11
                | Register::R12
                | Register::R13
                | Register::R14
                | Register::R15
        )
    }
}

/// An instruction source operand — either a physical register or an inline literal constant.
#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    Register(Register),
    Literal(f64),
}

/// A jump or branch target.
///
/// During instruction emission targets are symbolic label names.
/// After label resolution every `Label` is rewritten to a `Line`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JumpTarget {
    /// Symbolic label name — replaced by an absolute line number during resolution.
    Label(String),
    /// Resolved absolute line number in the final IC10 output.
    Line(u32),
    /// Register-indirect jump target — the destination line number is read from the
    /// register at runtime (e.g. `j ra` to return from a function).
    Register(Register),
}

/// A flat IC10 instruction. Every variant (except `Label`) corresponds to exactly one
/// emitted IC10 assembly line. `Label` is an internal pseudo-instruction that is stripped
/// during label resolution.
///
/// Tuple field order mirrors IC10 assembly argument order: destination register first,
/// then source operands left-to-right as they appear in the mnemonic.
#[derive(Debug, Clone)]
pub enum IC10Instruction {
    Abs(Register, Operand),
    Add(Register, Operand, Operand),
    Sub(Register, Operand, Operand),
    Mul(Register, Operand, Operand),
    Div(Register, Operand, Operand),
    Mod(Register, Operand, Operand),
    Pow(Register, Operand, Operand),
    Exp(Register, Operand),
    Log(Register, Operand),
    Sqrt(Register, Operand),
    Max(Register, Operand, Operand),
    Min(Register, Operand, Operand),
    Ceil(Register, Operand),
    Floor(Register, Operand),
    Round(Register, Operand),
    Trunc(Register, Operand),
    Move(Register, Operand),
    Rand(Register),
    Lerp(Register, Operand, Operand, Operand),

    Sin(Register, Operand),
    Cos(Register, Operand),
    Tan(Register, Operand),
    Asin(Register, Operand),
    Acos(Register, Operand),
    Atan(Register, Operand),
    Atan2(Register, Operand, Operand),

    And(Register, Operand, Operand),
    Or(Register, Operand, Operand),
    Xor(Register, Operand, Operand),
    Nor(Register, Operand, Operand),
    Not(Register, Operand),
    Sll(Register, Operand, Operand),
    Sla(Register, Operand, Operand),
    Srl(Register, Operand, Operand),
    Sra(Register, Operand, Operand),
    Ext {
        dest: Register,
        source: Operand,
        bit_offset: Operand,
        bit_length: Operand,
    },
    /// `ins dest field offset length` — insert bit field (max 53 bits).
    /// Note: stable IC10 (as of 2026-01-12) has a parameter-order bug — codegen must
    /// account for the actual stable order (`offset, length, field`) vs. the documented
    /// order (`field, offset, length`).
    Ins {
        dest: Register,
        field: Operand,
        bit_offset: Operand,
        bit_length: Operand,
    },

    Seq(Register, Operand, Operand),
    Seqz(Register, Operand),
    Sne(Register, Operand, Operand),
    Snez(Register, Operand),
    Sgt(Register, Operand, Operand),
    Sgtz(Register, Operand),
    Sge(Register, Operand, Operand),
    Sgez(Register, Operand),
    Slt(Register, Operand, Operand),
    Sltz(Register, Operand),
    Sle(Register, Operand, Operand),
    Slez(Register, Operand),
    Sap(Register, Operand, Operand, Operand),
    Sapz(Register, Operand, Operand),
    Sna(Register, Operand, Operand, Operand),
    Snaz(Register, Operand, Operand),
    Snan(Register, Operand),
    Snanz(Register, Operand),
    Sdse(Register, DevicePin),
    Sdns(Register, DevicePin),
    Select(Register, Operand, Operand, Operand),

    /// Internal pseudo-instruction: marks a label site. Stripped during label resolution.
    Label(String),
    Jump(JumpTarget),
    JumpRelative(Operand),
    JumpAndLink(JumpTarget),

    BranchEqual(Operand, Operand, JumpTarget),
    BranchEqualZero(Operand, JumpTarget),
    BranchNotEqual(Operand, Operand, JumpTarget),
    BranchNotEqualZero(Operand, JumpTarget),
    BranchGreaterThan(Operand, Operand, JumpTarget),
    BranchGreaterThanZero(Operand, JumpTarget),
    BranchGreaterEqual(Operand, Operand, JumpTarget),
    BranchGreaterEqualZero(Operand, JumpTarget),
    BranchLessThan(Operand, Operand, JumpTarget),
    BranchLessThanZero(Operand, JumpTarget),
    BranchLessEqual(Operand, Operand, JumpTarget),
    BranchLessEqualZero(Operand, JumpTarget),
    BranchApproximateEqual {
        left: Operand,
        right: Operand,
        epsilon: Operand,
        target: JumpTarget,
    },
    BranchApproximateZero {
        value: Operand,
        epsilon: Operand,
        target: JumpTarget,
    },
    BranchNotApproximateEqual {
        left: Operand,
        right: Operand,
        epsilon: Operand,
        target: JumpTarget,
    },
    BranchNotApproximateZero {
        value: Operand,
        epsilon: Operand,
        target: JumpTarget,
    },
    BranchNaN(Operand, JumpTarget),

    BranchEqualAndLink(Operand, Operand, JumpTarget),
    BranchEqualZeroAndLink(Operand, JumpTarget),
    BranchNotEqualAndLink(Operand, Operand, JumpTarget),
    BranchNotEqualZeroAndLink(Operand, JumpTarget),
    BranchGreaterThanAndLink(Operand, Operand, JumpTarget),
    BranchGreaterThanZeroAndLink(Operand, JumpTarget),
    BranchGreaterEqualAndLink(Operand, Operand, JumpTarget),
    BranchGreaterEqualZeroAndLink(Operand, JumpTarget),
    BranchLessThanAndLink(Operand, Operand, JumpTarget),
    BranchLessThanZeroAndLink(Operand, JumpTarget),
    BranchLessEqualAndLink(Operand, Operand, JumpTarget),
    BranchLessEqualZeroAndLink(Operand, JumpTarget),
    BranchApproximateEqualAndLink(Operand, Operand, Operand, JumpTarget),
    BranchApproximateZeroAndLink(Operand, Operand, JumpTarget),
    BranchNotApproximateEqualAndLink(Operand, Operand, Operand, JumpTarget),
    BranchNotApproximateZeroAndLink(Operand, Operand, JumpTarget),

    BranchEqualRelative(Operand, Operand, Operand),
    BranchEqualZeroRelative(Operand, Operand),
    BranchNotEqualRelative(Operand, Operand, Operand),
    BranchNotEqualZeroRelative(Operand, Operand),
    BranchGreaterThanRelative(Operand, Operand, Operand),
    BranchGreaterThanZeroRelative(Operand, Operand),
    BranchGreaterEqualRelative(Operand, Operand, Operand),
    BranchGreaterEqualZeroRelative(Operand, Operand),
    BranchLessThanRelative(Operand, Operand, Operand),
    BranchLessThanZeroRelative(Operand, Operand),
    BranchLessEqualRelative(Operand, Operand, Operand),
    BranchLessEqualZeroRelative(Operand, Operand),
    BranchApproximateEqualRelative(Operand, Operand, Operand, Operand),
    BranchApproximateZeroRelative(Operand, Operand, Operand),
    BranchNotApproximateEqualRelative(Operand, Operand, Operand, Operand),
    BranchNotApproximateZeroRelative(Operand, Operand, Operand),
    BranchNaNRelative(Operand, Operand),

    BranchDeviceSet(DevicePin, JumpTarget),
    BranchDeviceNotSet(DevicePin, JumpTarget),
    BranchDeviceSetAndLink(DevicePin, JumpTarget),
    BranchDeviceNotSetAndLink(DevicePin, JumpTarget),
    BranchDeviceSetRelative(DevicePin, Operand),
    BranchDeviceNotSetRelative(DevicePin, Operand),
    /// Branch if device does not support loading the given logic type.
    BranchDeviceNotValidLoad(DevicePin, String, JumpTarget),
    /// Branch if device does not support storing the given logic type.
    BranchDeviceNotValidStore(DevicePin, String, JumpTarget),

    Push(Operand),
    Pop(Register),
    Peek(Register),
    Poke(Operand, Operand),
    ClearStack(DevicePin),
    ClearStackById(Operand),
    Get(Register, DevicePin, Operand),
    GetById(Register, Operand, Operand),
    Put(DevicePin, Operand, Operand),
    PutById(Operand, Operand, Operand),

    Load(Register, DevicePin, String),
    Store(DevicePin, String, Operand),
    LoadSlot(Register, DevicePin, Operand, String),
    StoreSlot(DevicePin, Operand, String, Operand),
    /// `lr` — load reagent info from a device slot.
    LoadReagent(Register, DevicePin, Operand, Operand),
    /// `rmap` — map a reagent hash to the prefab hash the device expects.
    ReagentMap(Register, DevicePin, Operand),
    LoadById(Register, Operand, String),
    StoreById {
        reference_id: Operand,
        logic_type: String,
        source: Operand,
    },
    BatchLoad {
        dest: Register,
        device_hash: Operand,
        logic_type: String,
        batch_mode: BatchMode,
    },
    BatchStore {
        device_hash: Operand,
        logic_type: String,
        source: Operand,
    },
    BatchStoreByName {
        device_hash: Operand,
        name_hash: Operand,
        logic_type: String,
        source: Operand,
    },
    BatchLoadSlot {
        dest: Register,
        device_hash: Operand,
        slot: Operand,
        slot_logic_type: String,
        batch_mode: BatchMode,
    },
    BatchStoreSlot {
        device_hash: Operand,
        slot: Operand,
        slot_logic_type: String,
        source: Operand,
    },
    BatchLoadSlotByName {
        dest: Register,
        device_hash: Operand,
        name_hash: Operand,
        slot: Operand,
        slot_logic_type: String,
        batch_mode: BatchMode,
    },

    HaltAndCatchFire,
    Sleep(Operand),
    Yield,
}

impl IC10Instruction {
    /// Returns the general-purpose register (R0–R15) written by this instruction, if any.
    ///
    /// Used by clobber-set analysis to determine which registers a function may modify.
    /// Returns `None` for instructions that do not write a GP register (branches, stores,
    /// push, labels, control flow without link, etc.).  `JumpAndLink` and branch-and-link
    /// variants write `Ra` but that is handled separately by the prologue/epilogue.
    pub fn written_register(&self) -> Option<Register> {
        match self {
            IC10Instruction::Abs(dest, _)
            | IC10Instruction::Add(dest, _, _)
            | IC10Instruction::Sub(dest, _, _)
            | IC10Instruction::Mul(dest, _, _)
            | IC10Instruction::Div(dest, _, _)
            | IC10Instruction::Mod(dest, _, _)
            | IC10Instruction::Pow(dest, _, _)
            | IC10Instruction::Exp(dest, _)
            | IC10Instruction::Log(dest, _)
            | IC10Instruction::Sqrt(dest, _)
            | IC10Instruction::Max(dest, _, _)
            | IC10Instruction::Min(dest, _, _)
            | IC10Instruction::Ceil(dest, _)
            | IC10Instruction::Floor(dest, _)
            | IC10Instruction::Round(dest, _)
            | IC10Instruction::Trunc(dest, _)
            | IC10Instruction::Move(dest, _)
            | IC10Instruction::Rand(dest)
            | IC10Instruction::Lerp(dest, _, _, _)
            | IC10Instruction::Sin(dest, _)
            | IC10Instruction::Cos(dest, _)
            | IC10Instruction::Tan(dest, _)
            | IC10Instruction::Asin(dest, _)
            | IC10Instruction::Acos(dest, _)
            | IC10Instruction::Atan(dest, _)
            | IC10Instruction::Atan2(dest, _, _)
            | IC10Instruction::And(dest, _, _)
            | IC10Instruction::Or(dest, _, _)
            | IC10Instruction::Xor(dest, _, _)
            | IC10Instruction::Nor(dest, _, _)
            | IC10Instruction::Not(dest, _)
            | IC10Instruction::Sll(dest, _, _)
            | IC10Instruction::Sla(dest, _, _)
            | IC10Instruction::Srl(dest, _, _)
            | IC10Instruction::Sra(dest, _, _)
            | IC10Instruction::Seq(dest, _, _)
            | IC10Instruction::Seqz(dest, _)
            | IC10Instruction::Sne(dest, _, _)
            | IC10Instruction::Snez(dest, _)
            | IC10Instruction::Sgt(dest, _, _)
            | IC10Instruction::Sgtz(dest, _)
            | IC10Instruction::Sge(dest, _, _)
            | IC10Instruction::Sgez(dest, _)
            | IC10Instruction::Slt(dest, _, _)
            | IC10Instruction::Sltz(dest, _)
            | IC10Instruction::Sle(dest, _, _)
            | IC10Instruction::Slez(dest, _)
            | IC10Instruction::Sap(dest, _, _, _)
            | IC10Instruction::Sapz(dest, _, _)
            | IC10Instruction::Sna(dest, _, _, _)
            | IC10Instruction::Snaz(dest, _, _)
            | IC10Instruction::Snan(dest, _)
            | IC10Instruction::Snanz(dest, _)
            | IC10Instruction::Sdse(dest, _)
            | IC10Instruction::Sdns(dest, _)
            | IC10Instruction::Select(dest, _, _, _)
            | IC10Instruction::Pop(dest)
            | IC10Instruction::Peek(dest)
            | IC10Instruction::Get(dest, _, _)
            | IC10Instruction::GetById(dest, _, _)
            | IC10Instruction::Load(dest, _, _)
            | IC10Instruction::LoadSlot(dest, _, _, _)
            | IC10Instruction::LoadReagent(dest, _, _, _)
            | IC10Instruction::ReagentMap(dest, _, _)
            | IC10Instruction::LoadById(dest, _, _) => Some(*dest),

            IC10Instruction::Ext { dest, .. }
            | IC10Instruction::Ins { dest, .. }
            | IC10Instruction::BatchLoad { dest, .. }
            | IC10Instruction::BatchLoadSlot { dest, .. }
            | IC10Instruction::BatchLoadSlotByName { dest, .. } => Some(*dest),

            IC10Instruction::Label(_)
            | IC10Instruction::Jump(_)
            | IC10Instruction::JumpRelative(_)
            | IC10Instruction::JumpAndLink(_)
            | IC10Instruction::BranchEqual(..)
            | IC10Instruction::BranchEqualZero(..)
            | IC10Instruction::BranchNotEqual(..)
            | IC10Instruction::BranchNotEqualZero(..)
            | IC10Instruction::BranchGreaterThan(..)
            | IC10Instruction::BranchGreaterThanZero(..)
            | IC10Instruction::BranchGreaterEqual(..)
            | IC10Instruction::BranchGreaterEqualZero(..)
            | IC10Instruction::BranchLessThan(..)
            | IC10Instruction::BranchLessThanZero(..)
            | IC10Instruction::BranchLessEqual(..)
            | IC10Instruction::BranchLessEqualZero(..)
            | IC10Instruction::BranchApproximateEqual { .. }
            | IC10Instruction::BranchApproximateZero { .. }
            | IC10Instruction::BranchNotApproximateEqual { .. }
            | IC10Instruction::BranchNotApproximateZero { .. }
            | IC10Instruction::BranchNaN(..)
            | IC10Instruction::BranchEqualAndLink(..)
            | IC10Instruction::BranchEqualZeroAndLink(..)
            | IC10Instruction::BranchNotEqualAndLink(..)
            | IC10Instruction::BranchNotEqualZeroAndLink(..)
            | IC10Instruction::BranchGreaterThanAndLink(..)
            | IC10Instruction::BranchGreaterThanZeroAndLink(..)
            | IC10Instruction::BranchGreaterEqualAndLink(..)
            | IC10Instruction::BranchGreaterEqualZeroAndLink(..)
            | IC10Instruction::BranchLessThanAndLink(..)
            | IC10Instruction::BranchLessThanZeroAndLink(..)
            | IC10Instruction::BranchLessEqualAndLink(..)
            | IC10Instruction::BranchLessEqualZeroAndLink(..)
            | IC10Instruction::BranchApproximateEqualAndLink(..)
            | IC10Instruction::BranchApproximateZeroAndLink(..)
            | IC10Instruction::BranchNotApproximateEqualAndLink(..)
            | IC10Instruction::BranchNotApproximateZeroAndLink(..)
            | IC10Instruction::BranchEqualRelative(..)
            | IC10Instruction::BranchEqualZeroRelative(..)
            | IC10Instruction::BranchNotEqualRelative(..)
            | IC10Instruction::BranchNotEqualZeroRelative(..)
            | IC10Instruction::BranchGreaterThanRelative(..)
            | IC10Instruction::BranchGreaterThanZeroRelative(..)
            | IC10Instruction::BranchGreaterEqualRelative(..)
            | IC10Instruction::BranchGreaterEqualZeroRelative(..)
            | IC10Instruction::BranchLessThanRelative(..)
            | IC10Instruction::BranchLessThanZeroRelative(..)
            | IC10Instruction::BranchLessEqualRelative(..)
            | IC10Instruction::BranchLessEqualZeroRelative(..)
            | IC10Instruction::BranchApproximateEqualRelative(..)
            | IC10Instruction::BranchApproximateZeroRelative(..)
            | IC10Instruction::BranchNotApproximateEqualRelative(..)
            | IC10Instruction::BranchNotApproximateZeroRelative(..)
            | IC10Instruction::BranchNaNRelative(..)
            | IC10Instruction::BranchDeviceSet(..)
            | IC10Instruction::BranchDeviceNotSet(..)
            | IC10Instruction::BranchDeviceSetAndLink(..)
            | IC10Instruction::BranchDeviceNotSetAndLink(..)
            | IC10Instruction::BranchDeviceSetRelative(..)
            | IC10Instruction::BranchDeviceNotSetRelative(..)
            | IC10Instruction::BranchDeviceNotValidLoad(..)
            | IC10Instruction::BranchDeviceNotValidStore(..)
            | IC10Instruction::Push(_)
            | IC10Instruction::Poke(..)
            | IC10Instruction::ClearStack(_)
            | IC10Instruction::ClearStackById(_)
            | IC10Instruction::Put(..)
            | IC10Instruction::PutById(..)
            | IC10Instruction::Store(..)
            | IC10Instruction::StoreSlot(..)
            | IC10Instruction::StoreById { .. }
            | IC10Instruction::BatchStore { .. }
            | IC10Instruction::BatchStoreByName { .. }
            | IC10Instruction::BatchStoreSlot { .. }
            | IC10Instruction::HaltAndCatchFire
            | IC10Instruction::Sleep(_)
            | IC10Instruction::Yield => None,
        }
    }
}

/// A register-allocated function ready for code generation.
#[derive(Debug)]
pub struct IC10Function {
    pub name: String,
    pub instructions: Vec<IC10Instruction>,
    /// `true` for the `main` function — the program entry point.
    pub is_entry: bool,
}

/// The output of the register allocator: a complete, register-assigned IC10 program.
#[derive(Debug)]
pub struct IC10Program {
    pub functions: Vec<IC10Function>,
}
