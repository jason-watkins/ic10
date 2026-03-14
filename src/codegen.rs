use std::fmt;

use crate::ast::{BatchMode, DevicePin};
use crate::diagnostic::{Diagnostic, Span};
use crate::regalloc::{IC10Instruction, IC10Program, JumpTarget, Operand, Register};

impl fmt::Display for Register {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Register::R0 => write!(f, "r0"),
            Register::R1 => write!(f, "r1"),
            Register::R2 => write!(f, "r2"),
            Register::R3 => write!(f, "r3"),
            Register::R4 => write!(f, "r4"),
            Register::R5 => write!(f, "r5"),
            Register::R6 => write!(f, "r6"),
            Register::R7 => write!(f, "r7"),
            Register::R8 => write!(f, "r8"),
            Register::R9 => write!(f, "r9"),
            Register::R10 => write!(f, "r10"),
            Register::R11 => write!(f, "r11"),
            Register::R12 => write!(f, "r12"),
            Register::R13 => write!(f, "r13"),
            Register::R14 => write!(f, "r14"),
            Register::R15 => write!(f, "r15"),
            Register::Ra => write!(f, "ra"),
            Register::Sp => write!(f, "sp"),
        }
    }
}

impl fmt::Display for Operand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Operand::Register(register) => write!(f, "{register}"),
            Operand::Literal(value) => write!(f, "{}", format_float(*value)),
        }
    }
}

impl fmt::Display for JumpTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JumpTarget::Label(name) => write!(f, "{name}"),
            JumpTarget::Line(line) => write!(f, "{line}"),
            JumpTarget::Register(register) => write!(f, "{register}"),
        }
    }
}

impl fmt::Display for DevicePin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DevicePin::D0 => write!(f, "d0"),
            DevicePin::D1 => write!(f, "d1"),
            DevicePin::D2 => write!(f, "d2"),
            DevicePin::D3 => write!(f, "d3"),
            DevicePin::D4 => write!(f, "d4"),
            DevicePin::D5 => write!(f, "d5"),
            DevicePin::Db => write!(f, "db"),
        }
    }
}

impl fmt::Display for BatchMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BatchMode::Average => write!(f, "0"),
            BatchMode::Sum => write!(f, "1"),
            BatchMode::Minimum => write!(f, "2"),
            BatchMode::Maximum => write!(f, "3"),
            BatchMode::Contents => write!(f, "4"),
        }
    }
}

fn format_float(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_string();
    }
    if value.is_infinite() {
        return if value.is_sign_positive() {
            "pinf".to_string()
        } else {
            "ninf".to_string()
        };
    }
    if value == value.trunc() && value.abs() < 1e15 {
        // Whole number — omit the decimal point for cleaner IC10 output.
        let integer = value as i64;
        return integer.to_string();
    }
    format!("{value}")
}

impl fmt::Display for IC10Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IC10Instruction::Abs(d, a) => write!(f, "abs {d} {a}"),
            IC10Instruction::Add(d, a, b) => write!(f, "add {d} {a} {b}"),
            IC10Instruction::Sub(d, a, b) => write!(f, "sub {d} {a} {b}"),
            IC10Instruction::Mul(d, a, b) => write!(f, "mul {d} {a} {b}"),
            IC10Instruction::Div(d, a, b) => write!(f, "div {d} {a} {b}"),
            IC10Instruction::Mod(d, a, b) => write!(f, "mod {d} {a} {b}"),
            IC10Instruction::Pow(d, a, b) => write!(f, "pow {d} {a} {b}"),
            IC10Instruction::Exp(d, a) => write!(f, "exp {d} {a}"),
            IC10Instruction::Log(d, a) => write!(f, "log {d} {a}"),
            IC10Instruction::Sqrt(d, a) => write!(f, "sqrt {d} {a}"),
            IC10Instruction::Max(d, a, b) => write!(f, "max {d} {a} {b}"),
            IC10Instruction::Min(d, a, b) => write!(f, "min {d} {a} {b}"),
            IC10Instruction::Ceil(d, a) => write!(f, "ceil {d} {a}"),
            IC10Instruction::Floor(d, a) => write!(f, "floor {d} {a}"),
            IC10Instruction::Round(d, a) => write!(f, "round {d} {a}"),
            IC10Instruction::Trunc(d, a) => write!(f, "trunc {d} {a}"),
            IC10Instruction::Move(d, a) => write!(f, "move {d} {a}"),
            IC10Instruction::Rand(d) => write!(f, "rand {d}"),
            IC10Instruction::Lerp(d, a, b, c) => write!(f, "lerp {d} {a} {b} {c}"),

            IC10Instruction::Sin(d, a) => write!(f, "sin {d} {a}"),
            IC10Instruction::Cos(d, a) => write!(f, "cos {d} {a}"),
            IC10Instruction::Tan(d, a) => write!(f, "tan {d} {a}"),
            IC10Instruction::Asin(d, a) => write!(f, "asin {d} {a}"),
            IC10Instruction::Acos(d, a) => write!(f, "acos {d} {a}"),
            IC10Instruction::Atan(d, a) => write!(f, "atan {d} {a}"),
            IC10Instruction::Atan2(d, a, b) => write!(f, "atan2 {d} {a} {b}"),

            IC10Instruction::And(d, a, b) => write!(f, "and {d} {a} {b}"),
            IC10Instruction::Or(d, a, b) => write!(f, "or {d} {a} {b}"),
            IC10Instruction::Xor(d, a, b) => write!(f, "xor {d} {a} {b}"),
            IC10Instruction::Nor(d, a, b) => write!(f, "nor {d} {a} {b}"),
            IC10Instruction::Not(d, a) => write!(f, "not {d} {a}"),
            IC10Instruction::Sll(d, a, b) => write!(f, "sll {d} {a} {b}"),
            IC10Instruction::Sla(d, a, b) => write!(f, "sla {d} {a} {b}"),
            IC10Instruction::Srl(d, a, b) => write!(f, "srl {d} {a} {b}"),
            IC10Instruction::Sra(d, a, b) => write!(f, "sra {d} {a} {b}"),
            IC10Instruction::Ext {
                dest,
                source,
                bit_offset,
                bit_length,
            } => write!(f, "ext {dest} {source} {bit_offset} {bit_length}"),
            IC10Instruction::Ins {
                dest,
                field,
                bit_offset,
                bit_length,
            } => write!(f, "ins {dest} {bit_offset} {bit_length} {field}"),

            IC10Instruction::Seq(d, a, b) => write!(f, "seq {d} {a} {b}"),
            IC10Instruction::Seqz(d, a) => write!(f, "seqz {d} {a}"),
            IC10Instruction::Sne(d, a, b) => write!(f, "sne {d} {a} {b}"),
            IC10Instruction::Snez(d, a) => write!(f, "snez {d} {a}"),
            IC10Instruction::Sgt(d, a, b) => write!(f, "sgt {d} {a} {b}"),
            IC10Instruction::Sgtz(d, a) => write!(f, "sgtz {d} {a}"),
            IC10Instruction::Sge(d, a, b) => write!(f, "sge {d} {a} {b}"),
            IC10Instruction::Sgez(d, a) => write!(f, "sgez {d} {a}"),
            IC10Instruction::Slt(d, a, b) => write!(f, "slt {d} {a} {b}"),
            IC10Instruction::Sltz(d, a) => write!(f, "sltz {d} {a}"),
            IC10Instruction::Sle(d, a, b) => write!(f, "sle {d} {a} {b}"),
            IC10Instruction::Slez(d, a) => write!(f, "slez {d} {a}"),
            IC10Instruction::Sap(d, a, b, c) => write!(f, "sap {d} {a} {b} {c}"),
            IC10Instruction::Sapz(d, a, b) => write!(f, "sapz {d} {a} {b}"),
            IC10Instruction::Sna(d, a, b, c) => write!(f, "sna {d} {a} {b} {c}"),
            IC10Instruction::Snaz(d, a, b) => write!(f, "snaz {d} {a} {b}"),
            IC10Instruction::Snan(d, a) => write!(f, "snan {d} {a}"),
            IC10Instruction::Snanz(d, a) => write!(f, "snanz {d} {a}"),
            IC10Instruction::Sdse(d, pin) => write!(f, "sdse {d} {pin}"),
            IC10Instruction::Sdns(d, pin) => write!(f, "sdns {d} {pin}"),
            IC10Instruction::Select(d, a, b, c) => write!(f, "select {d} {a} {b} {c}"),

            IC10Instruction::Label(_) => Ok(()),
            IC10Instruction::Jump(target) => write!(f, "j {target}"),
            IC10Instruction::JumpRelative(offset) => write!(f, "jr {offset}"),
            IC10Instruction::JumpAndLink(target) => write!(f, "jal {target}"),

            IC10Instruction::BranchEqual(a, b, t) => write!(f, "beq {a} {b} {t}"),
            IC10Instruction::BranchEqualZero(a, t) => write!(f, "beqz {a} {t}"),
            IC10Instruction::BranchNotEqual(a, b, t) => write!(f, "bne {a} {b} {t}"),
            IC10Instruction::BranchNotEqualZero(a, t) => write!(f, "bnez {a} {t}"),
            IC10Instruction::BranchGreaterThan(a, b, t) => write!(f, "bgt {a} {b} {t}"),
            IC10Instruction::BranchGreaterThanZero(a, t) => write!(f, "bgtz {a} {t}"),
            IC10Instruction::BranchGreaterEqual(a, b, t) => write!(f, "bge {a} {b} {t}"),
            IC10Instruction::BranchGreaterEqualZero(a, t) => write!(f, "bgez {a} {t}"),
            IC10Instruction::BranchLessThan(a, b, t) => write!(f, "blt {a} {b} {t}"),
            IC10Instruction::BranchLessThanZero(a, t) => write!(f, "bltz {a} {t}"),
            IC10Instruction::BranchLessEqual(a, b, t) => write!(f, "ble {a} {b} {t}"),
            IC10Instruction::BranchLessEqualZero(a, t) => write!(f, "blez {a} {t}"),
            IC10Instruction::BranchApproximateEqual {
                left,
                right,
                epsilon,
                target,
            } => write!(f, "bap {left} {right} {epsilon} {target}"),
            IC10Instruction::BranchApproximateZero {
                value,
                epsilon,
                target,
            } => write!(f, "bapz {value} {epsilon} {target}"),
            IC10Instruction::BranchNotApproximateEqual {
                left,
                right,
                epsilon,
                target,
            } => write!(f, "bna {left} {right} {epsilon} {target}"),
            IC10Instruction::BranchNotApproximateZero {
                value,
                epsilon,
                target,
            } => write!(f, "bnaz {value} {epsilon} {target}"),
            IC10Instruction::BranchNaN(a, t) => write!(f, "bnan {a} {t}"),

            IC10Instruction::BranchEqualAndLink(a, b, t) => write!(f, "beqal {a} {b} {t}"),
            IC10Instruction::BranchEqualZeroAndLink(a, t) => write!(f, "beqzal {a} {t}"),
            IC10Instruction::BranchNotEqualAndLink(a, b, t) => write!(f, "bneal {a} {b} {t}"),
            IC10Instruction::BranchNotEqualZeroAndLink(a, t) => write!(f, "bnezal {a} {t}"),
            IC10Instruction::BranchGreaterThanAndLink(a, b, t) => write!(f, "bgtal {a} {b} {t}"),
            IC10Instruction::BranchGreaterThanZeroAndLink(a, t) => write!(f, "bgtzal {a} {t}"),
            IC10Instruction::BranchGreaterEqualAndLink(a, b, t) => write!(f, "bgeal {a} {b} {t}"),
            IC10Instruction::BranchGreaterEqualZeroAndLink(a, t) => write!(f, "bgezal {a} {t}"),
            IC10Instruction::BranchLessThanAndLink(a, b, t) => write!(f, "bltal {a} {b} {t}"),
            IC10Instruction::BranchLessThanZeroAndLink(a, t) => write!(f, "bltzal {a} {t}"),
            IC10Instruction::BranchLessEqualAndLink(a, b, t) => write!(f, "bleal {a} {b} {t}"),
            IC10Instruction::BranchLessEqualZeroAndLink(a, t) => write!(f, "blezal {a} {t}"),
            IC10Instruction::BranchApproximateEqualAndLink(a, b, c, t) => {
                write!(f, "bapal {a} {b} {c} {t}")
            }
            IC10Instruction::BranchApproximateZeroAndLink(a, b, t) => {
                write!(f, "bapzal {a} {b} {t}")
            }
            IC10Instruction::BranchNotApproximateEqualAndLink(a, b, c, t) => {
                write!(f, "bnaal {a} {b} {c} {t}")
            }
            IC10Instruction::BranchNotApproximateZeroAndLink(a, b, t) => {
                write!(f, "bnazal {a} {b} {t}")
            }

            IC10Instruction::BranchEqualRelative(a, b, off) => write!(f, "breq {a} {b} {off}"),
            IC10Instruction::BranchEqualZeroRelative(a, off) => write!(f, "breqz {a} {off}"),
            IC10Instruction::BranchNotEqualRelative(a, b, off) => write!(f, "brne {a} {b} {off}"),
            IC10Instruction::BranchNotEqualZeroRelative(a, off) => write!(f, "brnez {a} {off}"),
            IC10Instruction::BranchGreaterThanRelative(a, b, off) => {
                write!(f, "brgt {a} {b} {off}")
            }
            IC10Instruction::BranchGreaterThanZeroRelative(a, off) => {
                write!(f, "brgtz {a} {off}")
            }
            IC10Instruction::BranchGreaterEqualRelative(a, b, off) => {
                write!(f, "brge {a} {b} {off}")
            }
            IC10Instruction::BranchGreaterEqualZeroRelative(a, off) => {
                write!(f, "brgez {a} {off}")
            }
            IC10Instruction::BranchLessThanRelative(a, b, off) => {
                write!(f, "brlt {a} {b} {off}")
            }
            IC10Instruction::BranchLessThanZeroRelative(a, off) => write!(f, "brltz {a} {off}"),
            IC10Instruction::BranchLessEqualRelative(a, b, off) => {
                write!(f, "brle {a} {b} {off}")
            }
            IC10Instruction::BranchLessEqualZeroRelative(a, off) => write!(f, "brlez {a} {off}"),
            IC10Instruction::BranchApproximateEqualRelative(a, b, c, off) => {
                write!(f, "brap {a} {b} {c} {off}")
            }
            IC10Instruction::BranchApproximateZeroRelative(a, b, off) => {
                write!(f, "brapz {a} {b} {off}")
            }
            IC10Instruction::BranchNotApproximateEqualRelative(a, b, c, off) => {
                write!(f, "brna {a} {b} {c} {off}")
            }
            IC10Instruction::BranchNotApproximateZeroRelative(a, b, off) => {
                write!(f, "brnaz {a} {b} {off}")
            }
            IC10Instruction::BranchNaNRelative(a, off) => write!(f, "brnan {a} {off}"),

            IC10Instruction::BranchDeviceSet(pin, t) => write!(f, "bdse {pin} {t}"),
            IC10Instruction::BranchDeviceNotSet(pin, t) => write!(f, "bdns {pin} {t}"),
            IC10Instruction::BranchDeviceSetAndLink(pin, t) => write!(f, "bdseal {pin} {t}"),
            IC10Instruction::BranchDeviceNotSetAndLink(pin, t) => write!(f, "bdnsal {pin} {t}"),
            IC10Instruction::BranchDeviceSetRelative(pin, off) => write!(f, "brdse {pin} {off}"),
            IC10Instruction::BranchDeviceNotSetRelative(pin, off) => {
                write!(f, "brdns {pin} {off}")
            }
            IC10Instruction::BranchDeviceNotValidLoad(pin, logic_type, t) => {
                write!(f, "bdnvl {pin} {logic_type} {t}")
            }
            IC10Instruction::BranchDeviceNotValidStore(pin, logic_type, t) => {
                write!(f, "bdnvs {pin} {logic_type} {t}")
            }

            IC10Instruction::Push(a) => write!(f, "push {a}"),
            IC10Instruction::Pop(d) => write!(f, "pop {d}"),
            IC10Instruction::Peek(d) => write!(f, "peek {d}"),
            IC10Instruction::Poke(addr, val) => write!(f, "poke {addr} {val}"),
            IC10Instruction::ClearStack(pin) => write!(f, "clr {pin}"),
            IC10Instruction::ClearStackById(id) => write!(f, "clrd {id}"),
            IC10Instruction::Get(d, pin, addr) => write!(f, "get {d} {pin} {addr}"),
            IC10Instruction::GetById(d, id, addr) => write!(f, "getd {d} {id} {addr}"),
            IC10Instruction::Put(pin, addr, val) => write!(f, "put {pin} {addr} {val}"),
            IC10Instruction::PutById(id, addr, val) => write!(f, "putd {id} {addr} {val}"),

            IC10Instruction::Load(d, pin, logic_type) => write!(f, "l {d} {pin} {logic_type}"),
            IC10Instruction::Store(pin, logic_type, a) => write!(f, "s {pin} {logic_type} {a}"),
            IC10Instruction::LoadSlot(d, pin, slot, logic_type) => {
                write!(f, "ls {d} {pin} {slot} {logic_type}")
            }
            IC10Instruction::StoreSlot(pin, slot, logic_type, a) => {
                write!(f, "ss {pin} {slot} {logic_type} {a}")
            }
            IC10Instruction::LoadReagent(d, pin, reagent_mode, hash) => {
                write!(f, "lr {d} {pin} {reagent_mode} {hash}")
            }
            IC10Instruction::ReagentMap(d, pin, hash) => write!(f, "rmap {d} {pin} {hash}"),
            IC10Instruction::LoadById(d, id, logic_type) => {
                write!(f, "ld {d} {id} {logic_type}")
            }
            IC10Instruction::StoreById {
                reference_id,
                logic_type,
                source,
            } => write!(f, "sd {reference_id} {logic_type} {source}"),
            IC10Instruction::BatchLoad {
                dest,
                device_hash,
                logic_type,
                batch_mode,
            } => write!(f, "lb {dest} {device_hash} {logic_type} {batch_mode}"),
            IC10Instruction::BatchStore {
                device_hash,
                logic_type,
                source,
            } => write!(f, "sb {device_hash} {logic_type} {source}"),
            IC10Instruction::BatchStoreByName {
                device_hash,
                name_hash,
                logic_type,
                source,
            } => write!(f, "sbn {device_hash} {name_hash} {logic_type} {source}"),
            IC10Instruction::BatchLoadSlot {
                dest,
                device_hash,
                slot,
                slot_logic_type,
                batch_mode,
            } => write!(
                f,
                "lbs {dest} {device_hash} {slot} {slot_logic_type} {batch_mode}"
            ),
            IC10Instruction::BatchStoreSlot {
                device_hash,
                slot,
                slot_logic_type,
                source,
            } => write!(f, "sbs {device_hash} {slot} {slot_logic_type} {source}"),
            IC10Instruction::BatchLoadSlotByName {
                dest,
                device_hash,
                name_hash,
                slot,
                slot_logic_type,
                batch_mode,
            } => write!(
                f,
                "lbns {dest} {device_hash} {name_hash} {slot} {slot_logic_type} {batch_mode}"
            ),

            IC10Instruction::HaltAndCatchFire => write!(f, "hcf"),
            IC10Instruction::Sleep(a) => write!(f, "sleep {a}"),
            IC10Instruction::Yield => write!(f, "yield"),
        }
    }
}

const IC10_LINE_LIMIT: usize = 128;

/// Convert a register-allocated IC10 program into IC10 assembly text.
///
/// Returns the complete IC10 program as a newline-separated string and a
/// (possibly empty) list of diagnostics. If the output exceeds the 128-line
/// IC10 limit a warning diagnostic is included but the text is still returned.
///
/// When `keep_labels` is `true`, `IC10Instruction::Label` pseudo-instructions
/// are emitted as `"name:"` lines and jump targets remain as symbolic label
/// names rather than resolved line numbers.
pub fn generate(program: &IC10Program, keep_labels: bool) -> (String, Vec<Diagnostic>) {
    let mut lines: Vec<String> = Vec::new();
    for function in &program.functions {
        for instruction in &function.instructions {
            if let IC10Instruction::Label(name) = instruction {
                if keep_labels {
                    lines.push(format!("{name}:"));
                }
                continue;
            }
            lines.push(instruction.to_string());
        }
    }

    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    if lines.len() > IC10_LINE_LIMIT {
        diagnostics.push(Diagnostic::warning(
            Span::new(0, 0),
            format!(
                "program exceeds {IC10_LINE_LIMIT}-line IC10 limit ({} lines emitted)",
                lines.len()
            ),
        ));
    }

    (lines.join("\n"), diagnostics)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg;
    use crate::opt;
    use crate::parser::parse;
    use crate::regalloc;
    use crate::resolve::resolve;
    use crate::ssa;

    fn compile(source: &str) -> String {
        let (ast, parse_diagnostics) = parse(source);
        let errors: Vec<_> = parse_diagnostics
            .iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "parse errors: {errors:#?}");
        let (resolved, _) =
            resolve(&ast).unwrap_or_else(|diagnostics| panic!("resolve errors: {diagnostics:#?}"));
        let (mut program, _) = cfg::build(&resolved);
        ssa::construct_program(&mut program);
        opt::optimize_program(&mut program);
        let ic10_program = regalloc::allocate_registers(&mut program, false)
            .unwrap_or_else(|diagnostics| panic!("regalloc errors: {diagnostics:#?}"));
        let (text, diagnostics) = generate(&ic10_program, false);
        assert!(
            diagnostics
                .iter()
                .all(|d| d.severity != crate::diagnostic::Severity::Error),
            "codegen errors: {diagnostics:#?}"
        );
        text
    }

    fn compile_lines(source: &str) -> Vec<String> {
        compile(source).lines().map(String::from).collect()
    }

    #[test]
    fn empty_main_emits_hcf() {
        let output = compile("fn main() {}");
        assert_eq!(output.trim(), "hcf");
    }

    #[test]
    fn constant_assignment() {
        let lines = compile_lines(
            r#"
            device out: d0;
            fn main() { let x: i53 = 42; out.Setting = x; }
            "#,
        );
        assert!(
            lines.iter().any(|line| *line == "s d0 Setting 42"),
            "expected constant to be inlined directly into store: {lines:?}"
        );
    }

    #[test]
    fn arithmetic_expression() {
        let lines = compile_lines(
            r#"
            device io: d0;
            fn main() {
                let a: f64 = io.Setting;
                let b: f64 = io.Pressure;
                let c: f64 = a + b;
                io.Setting = c;
            }
            "#,
        );
        assert!(
            lines.iter().any(|line| line.starts_with("add ")),
            "expected an 'add' instruction: {lines:?}"
        );
    }

    #[test]
    fn device_read_write() {
        let lines = compile_lines(
            r#"
            device sensor: d0;
            device actuator: d1;
            fn main() {
                let temp: f64 = sensor.Temperature;
                actuator.Setting = temp;
            }
            "#,
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("l r") && line.contains("d0 Temperature")),
            "expected load from d0: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.starts_with("s d1 Setting")),
            "expected store to d1: {lines:?}"
        );
    }

    #[test]
    fn function_call_emits_jal() {
        let lines = compile_lines(
            r#"
            device io: d0;
            fn compute(a: f64, b: f64) -> f64 {
                let c = a + b;
                let d = c * a;
                let e = d - b;
                let f = e + c;
                return f * d;
            }
            fn main() {
                io.Setting = compute(1.0, 2.0);
                io.Setting = compute(3.0, 4.0);
            }
            "#,
        );
        assert!(
            lines.iter().any(|line| line.starts_with("jal ")),
            "expected a 'jal' instruction: {lines:?}"
        );
    }

    #[test]
    fn non_leaf_function_saves_ra() {
        let lines = compile_lines(
            r#"
            device io: d0;
            fn leaf(a: f64, b: f64) -> f64 {
                let c = a + b;
                let d = c * a;
                let e = d - b;
                let f = e + c;
                return f * d;
            }
            fn middle(x: f64) -> f64 {
                let a = leaf(x, x);
                let b = leaf(a, x);
                return a + b;
            }
            fn main() {
                io.Setting = middle(1.0);
                io.Setting = middle(2.0);
            }
            "#,
        );
        assert!(
            lines.iter().any(|line| line == "push ra"),
            "expected 'push ra': {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line == "pop ra"),
            "expected 'pop ra': {lines:?}"
        );
    }

    #[test]
    fn return_via_j_ra() {
        let lines = compile_lines(
            r#"
            device io: d0;
            fn helper(a: f64, b: f64) -> f64 {
                let c = a + b;
                let d = c * a;
                let e = d - b;
                let f = e + c;
                return f * d;
            }
            fn main() {
                io.Setting = helper(1.0, 2.0);
                io.Setting = helper(3.0, 4.0);
            }
            "#,
        );
        assert!(
            lines.iter().any(|line| line == "j ra"),
            "expected 'j ra' for function return: {lines:?}"
        );
    }

    #[test]
    fn branch_fusion_comparison() {
        let lines = compile_lines(
            r#"
            device io: d0;
            fn main() {
                let a: f64 = io.Setting;
                let b: f64 = io.Pressure;
                if a < b {
                    io.Setting = a;
                }
            }
            "#,
        );
        let has_fused = lines
            .iter()
            .any(|line| line.starts_with("blt ") || line.starts_with("bge "));
        assert!(has_fused, "expected a fused blt/bge branch: {lines:?}");
    }

    #[test]
    fn while_loop_emits_jump_back() {
        let lines = compile_lines(
            r#"
            fn main() {
                let mut i: i53 = 0;
                while i < 10 {
                    i = i + 1;
                }
            }
            "#,
        );
        let jump_count = lines.iter().filter(|line| line.starts_with("j ")).count();
        assert!(
            jump_count >= 1,
            "expected at least one jump for loop: {lines:?}"
        );
    }

    #[test]
    fn builtin_sqrt() {
        let lines = compile_lines(
            r#"
            device io: d0;
            fn main() {
                let v: f64 = io.Setting;
                let x: f64 = sqrt(v);
                io.Setting = x;
            }
            "#,
        );
        assert!(
            lines.iter().any(|line| line.starts_with("sqrt ")),
            "expected 'sqrt' instruction: {lines:?}"
        );
    }

    #[test]
    fn sleep_and_yield() {
        let lines = compile_lines(
            r#"
            fn main() {
                yield;
                sleep(1.0);
            }
            "#,
        );
        assert!(
            lines.iter().any(|line| line == "yield"),
            "expected 'yield': {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.starts_with("sleep ")),
            "expected 'sleep': {lines:?}"
        );
    }

    #[test]
    fn select_ternary() {
        let lines = compile_lines(
            r#"
            device io: d0;
            fn main() {
                let a: f64 = io.Setting;
                let b: f64 = io.Pressure;
                let cond: bool = a > b;
                let x: f64 = select(cond, a, b);
                io.Setting = x;
            }
            "#,
        );
        assert!(
            lines.iter().any(|line| line.starts_with("select ")),
            "expected 'select' instruction: {lines:?}"
        );
    }

    #[test]
    fn cast_to_i53_emits_trunc() {
        let lines = compile_lines(
            r#"
            device out: d0;
            fn main() {
                let y: f64 = out.Setting;
                let x: i53 = y as i53;
                out.Setting = x;
            }
            "#,
        );
        assert!(
            lines.iter().any(|line| line.starts_with("trunc ")),
            "expected 'trunc' for cast: {lines:?}"
        );
    }

    #[test]
    fn format_float_integers() {
        assert_eq!(format_float(0.0), "0");
        assert_eq!(format_float(42.0), "42");
        assert_eq!(format_float(-1.0), "-1");
        assert_eq!(format_float(128.0), "128");
    }

    #[test]
    fn format_float_fractions() {
        assert_eq!(format_float(2.03), "2.03");
        assert_eq!(format_float(-0.5), "-0.5");
    }

    #[test]
    fn format_float_special_values() {
        assert_eq!(format_float(f64::NAN), "nan");
        assert_eq!(format_float(f64::INFINITY), "pinf");
        assert_eq!(format_float(f64::NEG_INFINITY), "ninf");
    }

    #[test]
    fn register_display() {
        assert_eq!(Register::R0.to_string(), "r0");
        assert_eq!(Register::R15.to_string(), "r15");
        assert_eq!(Register::Ra.to_string(), "ra");
        assert_eq!(Register::Sp.to_string(), "sp");
    }

    #[test]
    fn operand_display() {
        assert_eq!(Operand::Register(Register::R5).to_string(), "r5");
        assert_eq!(Operand::Literal(42.0).to_string(), "42");
        assert_eq!(Operand::Literal(2.03).to_string(), "2.03");
    }

    #[test]
    fn jump_target_display() {
        assert_eq!(JumpTarget::Line(10).to_string(), "10");
        assert_eq!(JumpTarget::Register(Register::Ra).to_string(), "ra");
        assert_eq!(JumpTarget::Label("foo".to_string()).to_string(), "foo");
    }

    #[test]
    fn device_pin_display() {
        assert_eq!(DevicePin::D0.to_string(), "d0");
        assert_eq!(DevicePin::D5.to_string(), "d5");
        assert_eq!(DevicePin::Db.to_string(), "db");
    }

    #[test]
    fn batch_mode_display() {
        assert_eq!(BatchMode::Average.to_string(), "0");
        assert_eq!(BatchMode::Sum.to_string(), "1");
        assert_eq!(BatchMode::Minimum.to_string(), "2");
        assert_eq!(BatchMode::Maximum.to_string(), "3");
        assert_eq!(BatchMode::Contents.to_string(), "4");
    }

    #[test]
    fn instruction_display_math() {
        let instruction = IC10Instruction::Add(
            Register::R0,
            Operand::Register(Register::R1),
            Operand::Literal(5.0),
        );
        assert_eq!(instruction.to_string(), "add r0 r1 5");
    }

    #[test]
    fn instruction_display_branch() {
        let instruction = IC10Instruction::BranchGreaterThan(
            Operand::Register(Register::R0),
            Operand::Register(Register::R1),
            JumpTarget::Line(10),
        );
        assert_eq!(instruction.to_string(), "bgt r0 r1 10");
    }

    #[test]
    fn instruction_display_device_io() {
        let load = IC10Instruction::Load(Register::R0, DevicePin::D0, "Temperature".to_string());
        assert_eq!(load.to_string(), "l r0 d0 Temperature");
        let store = IC10Instruction::Store(
            DevicePin::D1,
            "Setting".to_string(),
            Operand::Register(Register::R0),
        );
        assert_eq!(store.to_string(), "s d1 Setting r0");
    }

    #[test]
    fn instruction_display_slot_io() {
        let load_slot = IC10Instruction::LoadSlot(
            Register::R0,
            DevicePin::D0,
            Operand::Literal(0.0),
            "Occupied".to_string(),
        );
        assert_eq!(load_slot.to_string(), "ls r0 d0 0 Occupied");
    }

    #[test]
    fn instruction_display_batch_load() {
        let batch = IC10Instruction::BatchLoad {
            dest: Register::R0,
            device_hash: Operand::Literal(12345.0),
            logic_type: "Temperature".to_string(),
            batch_mode: BatchMode::Average,
        };
        assert_eq!(batch.to_string(), "lb r0 12345 Temperature 0");
    }

    #[test]
    fn instruction_display_stack() {
        assert_eq!(
            IC10Instruction::Push(Operand::Register(Register::Ra)).to_string(),
            "push ra"
        );
        assert_eq!(IC10Instruction::Pop(Register::Ra).to_string(), "pop ra");
        assert_eq!(IC10Instruction::Peek(Register::R0).to_string(), "peek r0");
    }

    #[test]
    fn instruction_display_control() {
        assert_eq!(IC10Instruction::HaltAndCatchFire.to_string(), "hcf");
        assert_eq!(IC10Instruction::Yield.to_string(), "yield");
        assert_eq!(
            IC10Instruction::Sleep(Operand::Literal(1.0)).to_string(),
            "sleep 1"
        );
    }

    #[test]
    fn line_count_limit_exceeded() {
        let program = IC10Program {
            functions: vec![crate::regalloc::IC10Function {
                name: "main".to_string(),
                instructions: (0..129).map(|_| IC10Instruction::Yield).collect(),
                is_entry: true,
            }],
        };
        let (text, diagnostics) = generate(&program, false);
        assert_eq!(text.lines().count(), 129, "text should still be emitted");
        assert_eq!(
            diagnostics.len(),
            1,
            "should produce exactly one diagnostic"
        );
        assert_eq!(
            diagnostics[0].severity,
            crate::diagnostic::Severity::Warning
        );
        assert!(diagnostics[0].message.contains("128-line"));
    }

    #[test]
    fn line_count_exactly_128_ok() {
        let program = IC10Program {
            functions: vec![crate::regalloc::IC10Function {
                name: "main".to_string(),
                instructions: (0..128).map(|_| IC10Instruction::Yield).collect(),
                is_entry: true,
            }],
        };
        let (text, diagnostics) = generate(&program, false);
        assert!(
            diagnostics.is_empty(),
            "128 lines should produce no diagnostics"
        );
        assert_eq!(text.lines().count(), 128);
    }

    #[test]
    fn batch_read_end_to_end() {
        let lines = compile_lines(
            r#"
            device out: d0;
            fn main() {
                let avg: f64 = batch_read(hash("StructureGasSensor"), Temperature, Average);
                out.Setting = avg;
            }
            "#,
        );
        assert!(
            lines.iter().any(|line| line.starts_with("lb ")),
            "expected 'lb' for batch read: {lines:?}"
        );
    }
}
