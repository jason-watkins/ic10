use std::collections::{HashMap, HashSet};

use super::bound::{StaticId, StaticVariable, SymbolId, SymbolTable};
use super::shared::{BatchMode, BinaryOperator, DevicePin, Intrinsic, Type, UnaryOperator};

/// An opaque identifier for a temporary (three-address) value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TempId(pub usize);

/// An opaque identifier for a basic block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(pub usize);

/// A complete CFG program — one `Function` per bound function declaration.
#[derive(Debug)]
pub struct Program {
    pub functions: Vec<Function>,
    pub statics: Vec<StaticVariable>,
    pub symbols: SymbolTable,
}

/// A single function lowered to a control-flow graph.
#[derive(Debug)]
pub struct Function {
    pub name: String,
    pub symbol_id: SymbolId,
    pub parameters: Vec<SymbolId>,
    pub return_type: Option<Type>,
    pub blocks: Vec<BasicBlock>,
    pub entry: BlockId,
    /// `variable_definitions[symbol_id]` is the set of `(TempId, BlockId)` pairs
    /// that define that variable. Used by SSA construction.
    pub variable_definitions: HashMap<SymbolId, Vec<(TempId, BlockId)>>,
    /// Maps each `SymbolId` (parameter or local) to the most recent `TempId` that holds it.
    pub variable_temps: HashMap<SymbolId, TempId>,
    /// Immediate dominator for each block. Entry block has no entry.
    pub immediate_dominators: HashMap<BlockId, BlockId>,
    /// Dominance frontier sets.
    pub dominance_frontiers: HashMap<BlockId, HashSet<BlockId>>,
    /// Counter for allocating fresh TempIds beyond those already emitted.
    pub next_temp: usize,
}

impl Function {
    /// Allocate a fresh `TempId`, post-incrementing the internal counter.
    pub fn fresh_temp(&mut self) -> TempId {
        let id = TempId(self.next_temp);
        self.next_temp += 1;
        id
    }
}

/// The structural role a basic block plays in the program's control flow.
///
/// Used to generate descriptive, underscore-free camelCase labels for IC10 output.
#[derive(Debug, Clone)]
pub enum BlockRole {
    /// Function entry point — label is the function name itself.
    Entry,
    /// Loop header (while condition check, or for-loop check block).
    LoopStart(usize),
    /// Loop body.
    LoopBody(usize),
    /// For-loop increment step (continue target between body and header).
    LoopContinue(usize),
    /// Block after the loop exits.
    LoopEnd(usize),
    /// Then-branch of an if statement.
    IfTrue(usize),
    /// Else-branch of an if statement.
    IfFalse(usize),
    /// Merge point after both branches of an if statement.
    IfEnd(usize),
    /// Generic block with no special structural role.
    Generic,
    /// A block that was inlined from another function. Retains the original role and the
    /// callee's name so that generated labels reflect the inlined function's structure.
    Inlined {
        callee_name: String,
        original_role: Box<BlockRole>,
    },
}

/// A basic block: a linear sequence of instructions ending with a terminator.
#[derive(Debug)]
pub struct BasicBlock {
    pub id: BlockId,
    pub role: BlockRole,
    pub instructions: Vec<Instruction>,
    pub terminator: Terminator,
    pub predecessors: Vec<BlockId>,
    pub successors: Vec<BlockId>,
}

/// A three-address instruction in the CFG.
#[derive(Debug, Clone)]
pub enum Instruction {
    /// `dest = operation`
    Assign { dest: TempId, operation: Operation },
    /// Phi function — inserted by SSA construction, not by CFG builder.
    Phi {
        dest: TempId,
        args: Vec<(TempId, BlockId)>,
    },
    /// `dest = load device.field`
    LoadDevice {
        dest: TempId,
        pin: DevicePin,
        field: String,
    },
    /// `store device.field = src`
    StoreDevice {
        pin: DevicePin,
        field: String,
        source: TempId,
    },
    /// `dest = load device.slot(slot_index).field`
    LoadSlot {
        dest: TempId,
        pin: DevicePin,
        slot: TempId,
        field: String,
    },
    /// `store device.slot(slot_index).field = src`
    StoreSlot {
        pin: DevicePin,
        slot: TempId,
        field: String,
        source: TempId,
    },
    /// `dest = batch_read(hash, field, mode)`
    BatchRead {
        dest: TempId,
        hash: TempId,
        field: String,
        mode: BatchMode,
    },
    /// `batch_write(hash, field, value)`
    BatchWrite {
        hash: TempId,
        field: String,
        value: TempId,
    },
    /// `dest = call function(args)`
    Call {
        dest: Option<TempId>,
        function: SymbolId,
        args: Vec<TempId>,
    },
    /// `dest = intrinsic(args)`
    IntrinsicCall {
        dest: TempId,
        function: Intrinsic,
        args: Vec<TempId>,
    },
    /// `sleep(duration)`
    Sleep { duration: TempId },
    /// `yield`
    Yield,
    /// `dest = get db <static_address>` — load a static variable from its home location.
    LoadStatic {
        dest: TempId,
        static_id: StaticId,
    },
    /// `poke <static_address> source` — store a value to a static variable's home location.
    StoreStatic {
        static_id: StaticId,
        source: TempId,
    },
}

/// A pure operation that produces a value.
#[derive(Debug, Clone)]
pub enum Operation {
    /// Copy of another temp.
    Copy(TempId),
    /// A literal constant.
    Constant(f64),
    /// A function parameter. The value is deposited in the correct register by the
    /// caller before `jal`; the callee emits no instruction for this operation.
    /// The `index` is the 0-based position in the parameter list.
    Parameter { index: usize },
    /// Binary arithmetic/logic/comparison: `dest = lhs op rhs`
    Binary {
        operator: BinaryOperator,
        left: TempId,
        right: TempId,
    },
    /// Unary operator: `dest = op operand`
    Unary {
        operator: UnaryOperator,
        operand: TempId,
    },
    /// Type cast: `dest = operand as type`
    Cast {
        operand: TempId,
        target_type: Type,
        source_type: Type,
    },
    /// Select: `dest = select(cond, if_true, if_false)`
    Select {
        condition: TempId,
        if_true: TempId,
        if_false: TempId,
    },
}

/// The terminator of a basic block — determines control flow.
#[derive(Debug, Clone)]
pub enum Terminator {
    /// Unconditional jump to a block.
    Jump(BlockId),
    /// Conditional branch: if `condition` is true, go to `true_block`; else `false_block`.
    Branch {
        condition: TempId,
        true_block: BlockId,
        false_block: BlockId,
    },
    /// Return from the function, optionally with a value.
    Return(Option<TempId>),
    /// Placeholder — replaced before construction is complete.
    None,
}
