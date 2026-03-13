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

/// A register-allocated function ready for code generation.
pub struct IC10Function {
    pub name: String,
    pub instructions: Vec<IC10Instruction>,
    /// `true` for the `main` function — the program entry point.
    pub is_entry: bool,
}

/// The output of the register allocator: a complete, register-assigned IC10 program.
pub struct IC10Program {
    pub functions: Vec<IC10Function>,
}
