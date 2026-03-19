use std::collections::HashMap;
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum OperandKind {
    Register,
    RegisterOrNumber,
    Device,
    DeviceOrIndirectDevice,
    LogicType,
    LogicSlotType,
    BatchMode,
    ReagentMode,
    AliasTarget,
    DefineName,
    DefineValue,
    AliasName,
    Target,
}

#[derive(Debug, Clone)]
pub struct InstructionSignature {
    pub name: &'static str,
    pub operands: &'static [OperandKind],
    pub description: &'static str,
}

use OperandKind::*;

static INSTRUCTIONS: &[InstructionSignature] = &[
    // Utility
    InstructionSignature {
        name: "alias",
        operands: &[AliasName, AliasTarget],
        description: "Name a register or device",
    },
    InstructionSignature {
        name: "define",
        operands: &[DefineName, DefineValue],
        description: "Compile-time constant",
    },
    InstructionSignature {
        name: "hcf",
        operands: &[],
        description: "Halt and catch fire",
    },
    InstructionSignature {
        name: "sleep",
        operands: &[RegisterOrNumber],
        description: "Pause for a seconds",
    },
    InstructionSignature {
        name: "yield",
        operands: &[],
        description: "Pause for 1 tick",
    },
    // Math — r? = f(a) or f(a, b) or f(a, b, c)
    InstructionSignature {
        name: "abs",
        operands: &[Register, RegisterOrNumber],
        description: "r? = |a|",
    },
    InstructionSignature {
        name: "add",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "r? = a + b",
    },
    InstructionSignature {
        name: "sub",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "r? = a - b",
    },
    InstructionSignature {
        name: "mul",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "r? = a * b",
    },
    InstructionSignature {
        name: "div",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "r? = a / b",
    },
    InstructionSignature {
        name: "mod",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "r? = a mod b",
    },
    InstructionSignature {
        name: "pow",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "r? = a^b",
    },
    InstructionSignature {
        name: "exp",
        operands: &[Register, RegisterOrNumber],
        description: "r? = e^a",
    },
    InstructionSignature {
        name: "log",
        operands: &[Register, RegisterOrNumber],
        description: "r? = ln(a)",
    },
    InstructionSignature {
        name: "sqrt",
        operands: &[Register, RegisterOrNumber],
        description: "r? = sqrt(a)",
    },
    InstructionSignature {
        name: "max",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "r? = max(a, b)",
    },
    InstructionSignature {
        name: "min",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "r? = min(a, b)",
    },
    InstructionSignature {
        name: "ceil",
        operands: &[Register, RegisterOrNumber],
        description: "r? = ceil(a)",
    },
    InstructionSignature {
        name: "floor",
        operands: &[Register, RegisterOrNumber],
        description: "r? = floor(a)",
    },
    InstructionSignature {
        name: "round",
        operands: &[Register, RegisterOrNumber],
        description: "r? = round(a)",
    },
    InstructionSignature {
        name: "trunc",
        operands: &[Register, RegisterOrNumber],
        description: "r? = trunc(a)",
    },
    InstructionSignature {
        name: "move",
        operands: &[Register, RegisterOrNumber],
        description: "r? = a",
    },
    InstructionSignature {
        name: "rand",
        operands: &[Register],
        description: "r? = random in [0, 1)",
    },
    InstructionSignature {
        name: "lerp",
        operands: &[
            Register,
            RegisterOrNumber,
            RegisterOrNumber,
            RegisterOrNumber,
        ],
        description: "r? = lerp(a, b, clamp(c, 0, 1))",
    },
    // Trig
    InstructionSignature {
        name: "sin",
        operands: &[Register, RegisterOrNumber],
        description: "r? = sin(a) (radians)",
    },
    InstructionSignature {
        name: "cos",
        operands: &[Register, RegisterOrNumber],
        description: "r? = cos(a) (radians)",
    },
    InstructionSignature {
        name: "tan",
        operands: &[Register, RegisterOrNumber],
        description: "r? = tan(a) (radians)",
    },
    InstructionSignature {
        name: "asin",
        operands: &[Register, RegisterOrNumber],
        description: "r? = asin(a)",
    },
    InstructionSignature {
        name: "acos",
        operands: &[Register, RegisterOrNumber],
        description: "r? = acos(a)",
    },
    InstructionSignature {
        name: "atan",
        operands: &[Register, RegisterOrNumber],
        description: "r? = atan(a)",
    },
    InstructionSignature {
        name: "atan2",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "r? = atan2(y, x)",
    },
    // Bitwise
    InstructionSignature {
        name: "and",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "Bitwise AND",
    },
    InstructionSignature {
        name: "or",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "Bitwise OR",
    },
    InstructionSignature {
        name: "xor",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "Bitwise XOR",
    },
    InstructionSignature {
        name: "nor",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "Bitwise NOR",
    },
    InstructionSignature {
        name: "not",
        operands: &[Register, RegisterOrNumber],
        description: "Bitwise NOT",
    },
    InstructionSignature {
        name: "sll",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "Logical left shift",
    },
    InstructionSignature {
        name: "sla",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "Arithmetic left shift",
    },
    InstructionSignature {
        name: "srl",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "Logical right shift",
    },
    InstructionSignature {
        name: "sra",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "Arithmetic right shift",
    },
    InstructionSignature {
        name: "ext",
        operands: &[
            Register,
            RegisterOrNumber,
            RegisterOrNumber,
            RegisterOrNumber,
        ],
        description: "Extract bit field",
    },
    InstructionSignature {
        name: "ins",
        operands: &[
            Register,
            RegisterOrNumber,
            RegisterOrNumber,
            RegisterOrNumber,
        ],
        description: "Insert bit field",
    },
    // Comparison / Set
    InstructionSignature {
        name: "seq",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "r? = (a == b)",
    },
    InstructionSignature {
        name: "seqz",
        operands: &[Register, RegisterOrNumber],
        description: "r? = (a == 0)",
    },
    InstructionSignature {
        name: "sne",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "r? = (a != b)",
    },
    InstructionSignature {
        name: "snez",
        operands: &[Register, RegisterOrNumber],
        description: "r? = (a != 0)",
    },
    InstructionSignature {
        name: "sgt",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "r? = (a > b)",
    },
    InstructionSignature {
        name: "sgtz",
        operands: &[Register, RegisterOrNumber],
        description: "r? = (a > 0)",
    },
    InstructionSignature {
        name: "sge",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "r? = (a >= b)",
    },
    InstructionSignature {
        name: "sgez",
        operands: &[Register, RegisterOrNumber],
        description: "r? = (a >= 0)",
    },
    InstructionSignature {
        name: "slt",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "r? = (a < b)",
    },
    InstructionSignature {
        name: "sltz",
        operands: &[Register, RegisterOrNumber],
        description: "r? = (a < 0)",
    },
    InstructionSignature {
        name: "sle",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "r? = (a <= b)",
    },
    InstructionSignature {
        name: "slez",
        operands: &[Register, RegisterOrNumber],
        description: "r? = (a <= 0)",
    },
    InstructionSignature {
        name: "sap",
        operands: &[
            Register,
            RegisterOrNumber,
            RegisterOrNumber,
            RegisterOrNumber,
        ],
        description: "r? = approximately equal",
    },
    InstructionSignature {
        name: "sapz",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "r? = approximately zero",
    },
    InstructionSignature {
        name: "sna",
        operands: &[
            Register,
            RegisterOrNumber,
            RegisterOrNumber,
            RegisterOrNumber,
        ],
        description: "r? = not approximately equal",
    },
    InstructionSignature {
        name: "snaz",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "r? = not approximately zero",
    },
    InstructionSignature {
        name: "snan",
        operands: &[Register, RegisterOrNumber],
        description: "r? = (a is NaN)",
    },
    InstructionSignature {
        name: "snanz",
        operands: &[Register, RegisterOrNumber],
        description: "r? = (a is not NaN)",
    },
    InstructionSignature {
        name: "sdse",
        operands: &[Register, DeviceOrIndirectDevice],
        description: "r? = device is set",
    },
    InstructionSignature {
        name: "sdns",
        operands: &[Register, DeviceOrIndirectDevice],
        description: "r? = device is not set",
    },
    InstructionSignature {
        name: "select",
        operands: &[
            Register,
            RegisterOrNumber,
            RegisterOrNumber,
            RegisterOrNumber,
        ],
        description: "r? = a ? b : c",
    },
    // Branching — unconditional
    InstructionSignature {
        name: "j",
        operands: &[Target],
        description: "Jump to target",
    },
    InstructionSignature {
        name: "jr",
        operands: &[RegisterOrNumber],
        description: "Relative jump",
    },
    InstructionSignature {
        name: "jal",
        operands: &[Target],
        description: "Jump and link",
    },
    // Branching — two-operand conditional (a b target)
    InstructionSignature {
        name: "beq",
        operands: &[RegisterOrNumber, RegisterOrNumber, Target],
        description: "Branch if a == b",
    },
    InstructionSignature {
        name: "bne",
        operands: &[RegisterOrNumber, RegisterOrNumber, Target],
        description: "Branch if a != b",
    },
    InstructionSignature {
        name: "bgt",
        operands: &[RegisterOrNumber, RegisterOrNumber, Target],
        description: "Branch if a > b",
    },
    InstructionSignature {
        name: "bge",
        operands: &[RegisterOrNumber, RegisterOrNumber, Target],
        description: "Branch if a >= b",
    },
    InstructionSignature {
        name: "blt",
        operands: &[RegisterOrNumber, RegisterOrNumber, Target],
        description: "Branch if a < b",
    },
    InstructionSignature {
        name: "ble",
        operands: &[RegisterOrNumber, RegisterOrNumber, Target],
        description: "Branch if a <= b",
    },
    InstructionSignature {
        name: "bap",
        operands: &[RegisterOrNumber, RegisterOrNumber, RegisterOrNumber, Target],
        description: "Branch if approximately equal",
    },
    InstructionSignature {
        name: "bna",
        operands: &[RegisterOrNumber, RegisterOrNumber, RegisterOrNumber, Target],
        description: "Branch if not approximately equal",
    },
    // Branching — one-operand conditional (a target)
    InstructionSignature {
        name: "beqz",
        operands: &[RegisterOrNumber, Target],
        description: "Branch if a == 0",
    },
    InstructionSignature {
        name: "bnez",
        operands: &[RegisterOrNumber, Target],
        description: "Branch if a != 0",
    },
    InstructionSignature {
        name: "bgtz",
        operands: &[RegisterOrNumber, Target],
        description: "Branch if a > 0",
    },
    InstructionSignature {
        name: "bgez",
        operands: &[RegisterOrNumber, Target],
        description: "Branch if a >= 0",
    },
    InstructionSignature {
        name: "bltz",
        operands: &[RegisterOrNumber, Target],
        description: "Branch if a < 0",
    },
    InstructionSignature {
        name: "blez",
        operands: &[RegisterOrNumber, Target],
        description: "Branch if a <= 0",
    },
    InstructionSignature {
        name: "bapz",
        operands: &[RegisterOrNumber, RegisterOrNumber, Target],
        description: "Branch if approximately zero",
    },
    InstructionSignature {
        name: "bnaz",
        operands: &[RegisterOrNumber, RegisterOrNumber, Target],
        description: "Branch if not approximately zero",
    },
    InstructionSignature {
        name: "bnan",
        operands: &[RegisterOrNumber, Target],
        description: "Branch if NaN",
    },
    // Branching — two-operand conditional + link
    InstructionSignature {
        name: "beqal",
        operands: &[RegisterOrNumber, RegisterOrNumber, Target],
        description: "Branch+link if a == b",
    },
    InstructionSignature {
        name: "bneal",
        operands: &[RegisterOrNumber, RegisterOrNumber, Target],
        description: "Branch+link if a != b",
    },
    InstructionSignature {
        name: "bgtal",
        operands: &[RegisterOrNumber, RegisterOrNumber, Target],
        description: "Branch+link if a > b",
    },
    InstructionSignature {
        name: "bgeal",
        operands: &[RegisterOrNumber, RegisterOrNumber, Target],
        description: "Branch+link if a >= b",
    },
    InstructionSignature {
        name: "bltal",
        operands: &[RegisterOrNumber, RegisterOrNumber, Target],
        description: "Branch+link if a < b",
    },
    InstructionSignature {
        name: "bleal",
        operands: &[RegisterOrNumber, RegisterOrNumber, Target],
        description: "Branch+link if a <= b",
    },
    InstructionSignature {
        name: "bapal",
        operands: &[RegisterOrNumber, RegisterOrNumber, RegisterOrNumber, Target],
        description: "Branch+link if approximately equal",
    },
    InstructionSignature {
        name: "bnaal",
        operands: &[RegisterOrNumber, RegisterOrNumber, RegisterOrNumber, Target],
        description: "Branch+link if not approximately equal",
    },
    // Branching — one-operand conditional + link
    InstructionSignature {
        name: "beqzal",
        operands: &[RegisterOrNumber, Target],
        description: "Branch+link if a == 0",
    },
    InstructionSignature {
        name: "bnezal",
        operands: &[RegisterOrNumber, Target],
        description: "Branch+link if a != 0",
    },
    InstructionSignature {
        name: "bgtzal",
        operands: &[RegisterOrNumber, Target],
        description: "Branch+link if a > 0",
    },
    InstructionSignature {
        name: "bgezal",
        operands: &[RegisterOrNumber, Target],
        description: "Branch+link if a >= 0",
    },
    InstructionSignature {
        name: "bltzal",
        operands: &[RegisterOrNumber, Target],
        description: "Branch+link if a < 0",
    },
    InstructionSignature {
        name: "blezal",
        operands: &[RegisterOrNumber, Target],
        description: "Branch+link if a <= 0",
    },
    InstructionSignature {
        name: "bapzal",
        operands: &[RegisterOrNumber, RegisterOrNumber, Target],
        description: "Branch+link if approximately zero",
    },
    InstructionSignature {
        name: "bnazal",
        operands: &[RegisterOrNumber, RegisterOrNumber, Target],
        description: "Branch+link if not approximately zero",
    },
    // Branching — relative two-operand
    InstructionSignature {
        name: "breq",
        operands: &[RegisterOrNumber, RegisterOrNumber, RegisterOrNumber],
        description: "Relative branch if a == b",
    },
    InstructionSignature {
        name: "brne",
        operands: &[RegisterOrNumber, RegisterOrNumber, RegisterOrNumber],
        description: "Relative branch if a != b",
    },
    InstructionSignature {
        name: "brgt",
        operands: &[RegisterOrNumber, RegisterOrNumber, RegisterOrNumber],
        description: "Relative branch if a > b",
    },
    InstructionSignature {
        name: "brge",
        operands: &[RegisterOrNumber, RegisterOrNumber, RegisterOrNumber],
        description: "Relative branch if a >= b",
    },
    InstructionSignature {
        name: "brlt",
        operands: &[RegisterOrNumber, RegisterOrNumber, RegisterOrNumber],
        description: "Relative branch if a < b",
    },
    InstructionSignature {
        name: "brle",
        operands: &[RegisterOrNumber, RegisterOrNumber, RegisterOrNumber],
        description: "Relative branch if a <= b",
    },
    InstructionSignature {
        name: "brap",
        operands: &[
            RegisterOrNumber,
            RegisterOrNumber,
            RegisterOrNumber,
            RegisterOrNumber,
        ],
        description: "Relative branch if approximately equal",
    },
    InstructionSignature {
        name: "brna",
        operands: &[
            RegisterOrNumber,
            RegisterOrNumber,
            RegisterOrNumber,
            RegisterOrNumber,
        ],
        description: "Relative branch if not approximately equal",
    },
    // Branching — relative one-operand
    InstructionSignature {
        name: "breqz",
        operands: &[RegisterOrNumber, RegisterOrNumber],
        description: "Relative branch if a == 0",
    },
    InstructionSignature {
        name: "brnez",
        operands: &[RegisterOrNumber, RegisterOrNumber],
        description: "Relative branch if a != 0",
    },
    InstructionSignature {
        name: "brgtz",
        operands: &[RegisterOrNumber, RegisterOrNumber],
        description: "Relative branch if a > 0",
    },
    InstructionSignature {
        name: "brgez",
        operands: &[RegisterOrNumber, RegisterOrNumber],
        description: "Relative branch if a >= 0",
    },
    InstructionSignature {
        name: "brltz",
        operands: &[RegisterOrNumber, RegisterOrNumber],
        description: "Relative branch if a < 0",
    },
    InstructionSignature {
        name: "brlez",
        operands: &[RegisterOrNumber, RegisterOrNumber],
        description: "Relative branch if a <= 0",
    },
    InstructionSignature {
        name: "brapz",
        operands: &[RegisterOrNumber, RegisterOrNumber, RegisterOrNumber],
        description: "Relative branch if approximately zero",
    },
    InstructionSignature {
        name: "brnaz",
        operands: &[RegisterOrNumber, RegisterOrNumber, RegisterOrNumber],
        description: "Relative branch if not approximately zero",
    },
    InstructionSignature {
        name: "brnan",
        operands: &[RegisterOrNumber, RegisterOrNumber],
        description: "Relative branch if NaN",
    },
    // Device branching
    InstructionSignature {
        name: "bdse",
        operands: &[DeviceOrIndirectDevice, Target],
        description: "Branch if device set",
    },
    InstructionSignature {
        name: "bdns",
        operands: &[DeviceOrIndirectDevice, Target],
        description: "Branch if device not set",
    },
    InstructionSignature {
        name: "bdseal",
        operands: &[DeviceOrIndirectDevice, Target],
        description: "Branch+link if device set",
    },
    InstructionSignature {
        name: "bdnsal",
        operands: &[DeviceOrIndirectDevice, Target],
        description: "Branch+link if device not set",
    },
    InstructionSignature {
        name: "brdse",
        operands: &[DeviceOrIndirectDevice, RegisterOrNumber],
        description: "Relative branch if device set",
    },
    InstructionSignature {
        name: "brdns",
        operands: &[DeviceOrIndirectDevice, RegisterOrNumber],
        description: "Relative branch if device not set",
    },
    InstructionSignature {
        name: "bdnvl",
        operands: &[DeviceOrIndirectDevice, LogicType, Target],
        description: "Branch if device invalid for load",
    },
    InstructionSignature {
        name: "bdnvs",
        operands: &[DeviceOrIndirectDevice, LogicType, Target],
        description: "Branch if device invalid for store",
    },
    // Device I/O — direct
    InstructionSignature {
        name: "l",
        operands: &[Register, DeviceOrIndirectDevice, LogicType],
        description: "Load LogicType from device",
    },
    InstructionSignature {
        name: "s",
        operands: &[DeviceOrIndirectDevice, LogicType, RegisterOrNumber],
        description: "Store to device LogicType",
    },
    InstructionSignature {
        name: "ls",
        operands: &[
            Register,
            DeviceOrIndirectDevice,
            RegisterOrNumber,
            LogicSlotType,
        ],
        description: "Load slot property",
    },
    InstructionSignature {
        name: "ss",
        operands: &[
            DeviceOrIndirectDevice,
            RegisterOrNumber,
            LogicSlotType,
            RegisterOrNumber,
        ],
        description: "Store to slot",
    },
    InstructionSignature {
        name: "lr",
        operands: &[
            Register,
            DeviceOrIndirectDevice,
            ReagentMode,
            RegisterOrNumber,
        ],
        description: "Load reagent info",
    },
    InstructionSignature {
        name: "rmap",
        operands: &[Register, DeviceOrIndirectDevice, RegisterOrNumber],
        description: "Map reagent hash",
    },
    // Device I/O — by ReferenceId
    InstructionSignature {
        name: "ld",
        operands: &[Register, RegisterOrNumber, LogicType],
        description: "Load by ReferenceId",
    },
    InstructionSignature {
        name: "sd",
        operands: &[RegisterOrNumber, LogicType, RegisterOrNumber],
        description: "Store by ReferenceId",
    },
    // Batch
    InstructionSignature {
        name: "lb",
        operands: &[Register, RegisterOrNumber, LogicType, BatchMode],
        description: "Batch load",
    },
    InstructionSignature {
        name: "sb",
        operands: &[RegisterOrNumber, LogicType, RegisterOrNumber],
        description: "Batch store",
    },
    InstructionSignature {
        name: "lbn",
        operands: &[
            Register,
            RegisterOrNumber,
            RegisterOrNumber,
            LogicType,
            BatchMode,
        ],
        description: "Batch load by name",
    },
    InstructionSignature {
        name: "sbn",
        operands: &[
            RegisterOrNumber,
            RegisterOrNumber,
            LogicType,
            RegisterOrNumber,
        ],
        description: "Batch store by name",
    },
    InstructionSignature {
        name: "lbs",
        operands: &[
            Register,
            RegisterOrNumber,
            RegisterOrNumber,
            LogicSlotType,
            BatchMode,
        ],
        description: "Batch load slot",
    },
    InstructionSignature {
        name: "sbs",
        operands: &[
            RegisterOrNumber,
            RegisterOrNumber,
            LogicSlotType,
            RegisterOrNumber,
        ],
        description: "Batch store slot",
    },
    InstructionSignature {
        name: "lbns",
        operands: &[
            Register,
            RegisterOrNumber,
            RegisterOrNumber,
            RegisterOrNumber,
            LogicSlotType,
            BatchMode,
        ],
        description: "Batch load slot by name",
    },
    // Stack — self
    InstructionSignature {
        name: "push",
        operands: &[RegisterOrNumber],
        description: "Push to stack",
    },
    InstructionSignature {
        name: "pop",
        operands: &[Register],
        description: "Pop from stack",
    },
    InstructionSignature {
        name: "peek",
        operands: &[Register],
        description: "Peek top of stack",
    },
    InstructionSignature {
        name: "poke",
        operands: &[RegisterOrNumber, RegisterOrNumber],
        description: "Write to stack address",
    },
    // Stack — device
    InstructionSignature {
        name: "get",
        operands: &[Register, DeviceOrIndirectDevice, RegisterOrNumber],
        description: "Read from device stack",
    },
    InstructionSignature {
        name: "getd",
        operands: &[Register, RegisterOrNumber, RegisterOrNumber],
        description: "Read device stack by ReferenceId",
    },
    InstructionSignature {
        name: "put",
        operands: &[DeviceOrIndirectDevice, RegisterOrNumber, RegisterOrNumber],
        description: "Write to device stack",
    },
    InstructionSignature {
        name: "putd",
        operands: &[RegisterOrNumber, RegisterOrNumber, RegisterOrNumber],
        description: "Write device stack by ReferenceId",
    },
    InstructionSignature {
        name: "clr",
        operands: &[DeviceOrIndirectDevice],
        description: "Clear device stack",
    },
    InstructionSignature {
        name: "clrd",
        operands: &[RegisterOrNumber],
        description: "Clear device stack by ReferenceId",
    },
];

pub static INSTRUCTION_MAP: LazyLock<HashMap<&'static str, &'static InstructionSignature>> =
    LazyLock::new(|| {
        let mut map = HashMap::with_capacity(INSTRUCTIONS.len());
        for instruction in INSTRUCTIONS {
            map.insert(instruction.name, instruction);
        }
        map
    });
