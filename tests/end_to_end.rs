use ic20c::bind;
use ic20c::cfg;
use ic20c::codegen;
use ic20c::diagnostic::Severity;
use ic20c::opt::{self, OptLevel};
use ic20c::parser;
use ic20c::regalloc;
use ic20c::ssa;

fn compile(source: &str) -> Result<String, String> {
    let (ast, parse_diagnostics) = parser::parse(source);
    let errors: Vec<_> = parse_diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    if !errors.is_empty() {
        return Err(format!("parse errors: {errors:#?}"));
    }

    let (bound, _) =
        bind::bind(&ast).map_err(|diagnostics| format!("bind errors: {diagnostics:#?}"))?;

    let (mut program, cfg_diagnostics) = cfg::build(&bound);
    let cfg_errors: Vec<_> = cfg_diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    if !cfg_errors.is_empty() {
        return Err(format!("cfg errors: {cfg_errors:#?}"));
    }

    ssa::construct_program(&mut program);
    let opt_features = ic20c::opt::Features::from_opt_level(OptLevel::O2);
    opt::optimize_program(&mut program, OptLevel::O2, &opt_features);

    let ic10_program = regalloc::allocate_registers(&mut program, false, &opt_features)
        .map_err(|diagnostics| format!("regalloc errors: {diagnostics:#?}"))?;

    let (text, codegen_diagnostics) = codegen::generate(&ic10_program, false);
    let codegen_errors: Vec<_> = codegen_diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    if !codegen_errors.is_empty() {
        return Err(format!("codegen errors: {codegen_errors:#?}"));
    }

    Ok(text)
}

fn compile_file(path: &str) -> String {
    let source = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("cannot read '{path}': {error}"));
    compile(&source).unwrap_or_else(|error| panic!("compilation failed: {error}"))
}

#[test]
fn hello_compiles() {
    let output = compile_file("tests/examples/hello.ic20");
    assert!(
        output.lines().any(|l| l.starts_with("s d0 On")),
        "expected a line starting with 's d0 On': {output}"
    );
}

#[test]
fn empty_main_produces_hcf() {
    let output = compile("fn main() {}").unwrap();
    assert_eq!(output.trim(), "hcf");
}

#[test]
fn simple_variable() {
    let output =
        compile("device out: d0;\nfn main() { let x: i53 = 42; out.Setting = x; }").unwrap();
    assert!(
        output.lines().any(|l| l.contains("42")),
        "expected a line containing literal 42: {output}"
    );
    assert!(
        output.lines().any(|l| l.starts_with("s d0 Setting")),
        "expected a line starting with 's d0 Setting': {output}"
    );
}

#[test]
fn device_write_bool_value() {
    let output =
        compile("device light: d0;\nfn main() { let on: bool = true; light.On = on; }").unwrap();
    assert!(
        output.lines().any(|l| l.starts_with("s d0 On")),
        "expected a line starting with 's d0 On': {output}"
    );
}

#[test]
fn device_write_bool_literal() {
    let output = compile("device light: d0;\nfn main() { light.On = true; }").unwrap();
    assert!(
        output.lines().any(|l| l.starts_with("s d0 On")),
        "expected a line starting with 's d0 On': {output}"
    );
}

#[test]
fn device_read_bool_annotation() {
    let output = compile(
        r#"
        device sensor: d0;
        device out: d1;
        fn main() {
            let on: bool = sensor.On;
            out.Setting = on;
        }
        "#,
    )
    .unwrap();
    assert!(
        output
            .lines()
            .any(|l| l.starts_with("l r") && l.contains("d0 On")),
        "expected a line with 'l r… d0 On': {output}"
    );
    assert!(
        output.lines().any(|l| l.starts_with("s d1 Setting")),
        "expected a line starting with 's d1 Setting': {output}"
    );
}

#[test]
fn device_read_i53_annotation() {
    let output = compile(
        r#"
        device sensor: d0;
        device out: d1;
        fn main() {
            let count: i53 = sensor.Count;
            out.Setting = count;
        }
        "#,
    )
    .unwrap();
    assert!(
        output
            .lines()
            .any(|l| l.starts_with("l r") && l.contains("d0 Count")),
        "expected a line with 'l r… d0 Count': {output}"
    );
    assert!(
        output.lines().any(|l| l.starts_with("s d1 Setting")),
        "expected a line starting with 's d1 Setting': {output}"
    );
}

#[test]
fn schmitt_trigger_compiles() {
    let output = compile_file("tests/examples/schmitt_trigger.ic20");
    assert!(
        output
            .lines()
            .any(|l| l.starts_with("l r") && l.contains("d0 Temperature")),
        "expected a line with 'l r… d0 Temperature': {output}"
    );
    assert!(
        output.lines().any(|l| l.starts_with("s d1 On")),
        "expected a line starting with 's d1 On': {output}"
    );
    assert!(
        output.lines().any(|l| l == "yield"),
        "expected a yield line: {output}"
    );
}

#[test]
fn thermostat_compiles() {
    let output = compile_file("tests/examples/thermostat.ic20");
    assert!(
        output.lines().any(|l| l == "yield"),
        "expected a yield line: {output}"
    );
}

#[test]
fn const_folding() {
    let output =
        compile("device out: d0;\nconst X: i53 = 3 * 4 + 5;\nfn main() { out.Setting = X; }")
            .unwrap();
    assert!(
        output.lines().any(|l| l.starts_with("s d0 Setting 17")),
        "expected 's d0 Setting 17' (folded constant): {output}"
    );
}

#[test]
fn function_inlining_and_folding() {
    let output = compile(
        r#"
        device out: d0;
        fn double(x: i53) -> i53 { return x * 2; }
        fn main() { out.Setting = double(21); }
        "#,
    )
    .unwrap();
    assert!(
        output.lines().any(|l| l.starts_with("s d0 Setting 42")),
        "expected 's d0 Setting 42' (inlined + folded): {output}"
    );
}

#[test]
fn output_within_128_lines() {
    let output = compile_file("tests/examples/schmitt_trigger.ic20");
    let line_count = output.lines().count();
    assert!(
        line_count <= 128,
        "output exceeds 128 lines: {line_count} lines"
    );
}

#[test]
fn parse_error_is_reported() {
    let error = compile("fn main( {}").unwrap_err();
    assert!(
        error.starts_with("parse errors:"),
        "expected parse error, got: {error}"
    );
}

#[test]
fn type_error_is_reported() {
    let error = compile("fn main() { let x: i53 = true + 1; }").unwrap_err();
    assert!(
        error.starts_with("bind errors:"),
        "expected type/bind error, got: {error}"
    );
}

#[test]
fn undeclared_variable_is_reported() {
    let error = compile("fn main() { let x: i53 = y; }").unwrap_err();
    assert!(
        error.starts_with("bind errors:"),
        "expected bind error for undeclared variable, got: {error}"
    );
}

#[test]
fn missing_main_is_reported() {
    let error = compile("fn foo() {}").unwrap_err();
    assert!(
        error.starts_with("bind errors:") && error.contains("main"),
        "expected bind error about missing main, got: {error}"
    );
}

#[test]
fn while_loop() {
    let output = compile(
        r#"
        device out: d0;
        fn main() {
            let mut i: i53 = 0;
            while i < 5 {
                i = i + 1;
            }
            out.Setting = i;
        }
        "#,
    )
    .unwrap();
    assert_eq!(
        output.trim(),
        "move r1 0\n\
         add r0 r1 1\n\
         blt r0 5 5\n\
         s d0 Setting r0\n\
         hcf\n\
         move r1 r0\n\
         j 1",
    );
}

#[test]
fn for_loop() {
    let output = compile(
        r#"
        device out: d0;
        fn main() {
            let mut sum: i53 = 0;
            for i in 0..10 {
                sum = sum + i;
            }
            out.Setting = sum;
        }
        "#,
    )
    .unwrap();
    let lines: Vec<&str> = output.trim().lines().collect();
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("add ") && l.ends_with(" 1")),
        "ascending step-1 loop must increment by 1: {output}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("blt ") && l.contains(" 10 ")),
        "exclusive ascending loop must use blt with bound 10: {output}"
    );
    assert!(
        lines.last().unwrap().starts_with("j "),
        "bottom-tested loop must end with back-edge jump: {output}"
    );
    assert!(
        output.contains("s d0 Setting") && output.contains("hcf"),
        "must write result and halt: {output}"
    );
}

#[test]
fn for_loop_inclusive_range() {
    let output = compile(
        r#"
        device out: d0;
        fn main() {
            let mut sum: i53 = 0;
            for i in 0..=9 {
                sum = sum + i;
            }
            out.Setting = sum;
        }
        "#,
    )
    .unwrap();
    let lines: Vec<&str> = output.trim().lines().collect();
    assert!(
        lines.iter().any(|l| l.starts_with("ble ")),
        "inclusive range must use ble (not blt): {output}"
    );
    assert!(
        !lines.iter().any(|l| l.starts_with("blt ")),
        "inclusive range must not use blt: {output}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("ble ") && l.contains(" 9 ")),
        "inclusive back-edge must compare against upper bound 9: {output}"
    );
    assert!(
        lines.last().unwrap().starts_with("j "),
        "bottom-tested loop must end with back-edge jump: {output}"
    );
}

#[test]
fn for_loop_reverse() {
    let output = compile(
        r#"
        device out: d0;
        fn main() {
            let mut sum: i53 = 0;
            for i in (0..10).rev() {
                sum = sum + i;
            }
            out.Setting = sum;
        }
        "#,
    )
    .unwrap();
    let lines: Vec<&str> = output.trim().lines().collect();
    assert!(
        lines.iter().any(|l| l.starts_with("sub ")),
        "reverse loop must decrement with sub: {output}"
    );
    assert!(
        lines.iter().any(|l| l.starts_with("bge ")),
        "reverse loop must use bge for back-edge: {output}"
    );
    assert!(
        lines[0].contains(" 9") || lines[1].contains(" 9"),
        "exclusive reverse 0..10 must start iterator at 9 (upper - 1): {output}"
    );
    assert!(
        lines.last().unwrap().starts_with("j "),
        "bottom-tested loop must end with back-edge jump: {output}"
    );
}

#[test]
fn for_loop_inclusive_reverse() {
    let output = compile(
        r#"
        device out: d0;
        fn main() {
            let mut sum: i53 = 0;
            for i in (0..=9).rev() {
                sum = sum + i;
            }
            out.Setting = sum;
        }
        "#,
    )
    .unwrap();
    let lines: Vec<&str> = output.trim().lines().collect();
    assert!(
        lines.iter().any(|l| l.starts_with("sub ")),
        "reverse loop must decrement with sub: {output}"
    );
    assert!(
        lines.iter().any(|l| l.starts_with("bge ")),
        "reverse loop must use bge for back-edge: {output}"
    );
    assert!(
        lines[0].contains(" 9") || lines[1].contains(" 9"),
        "inclusive reverse 0..=9 must start iterator at 9: {output}"
    );
    assert!(
        lines.last().unwrap().starts_with("j "),
        "bottom-tested loop must end with back-edge jump: {output}"
    );
}

#[test]
fn for_loop_step_by() {
    let output = compile(
        r#"
        device out: d0;
        fn main() {
            let mut sum: i53 = 0;
            for i in (0..10).step_by(2) {
                sum = sum + i;
            }
            out.Setting = sum;
        }
        "#,
    )
    .unwrap();
    let lines: Vec<&str> = output.trim().lines().collect();
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("add ") && l.ends_with(" 2")),
        "step_by(2) must increment by 2: {output}"
    );
    assert!(
        lines.iter().any(|l| l.starts_with("blt ")),
        "ascending exclusive loop must use blt for back-edge: {output}"
    );
    assert!(
        lines.last().unwrap().starts_with("j "),
        "bottom-tested loop must end with back-edge jump: {output}"
    );
}

#[test]
fn for_loop_reverse_step_by() {
    let output = compile(
        r#"
        device out: d0;
        fn main() {
            let mut sum: i53 = 0;
            for i in (0..=10).rev().step_by(3) {
                sum = sum + i;
            }
            out.Setting = sum;
        }
        "#,
    )
    .unwrap();
    let lines: Vec<&str> = output.trim().lines().collect();
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("sub ") && l.ends_with(" 3")),
        "rev().step_by(3) must decrement by 3: {output}"
    );
    assert!(
        lines.iter().any(|l| l.starts_with("bge ")),
        "reverse loop must use bge for back-edge: {output}"
    );
    assert!(
        lines[0].contains(" 10") || lines[1].contains(" 10"),
        "inclusive reverse 0..=10 must start iterator at 10: {output}"
    );
    assert!(
        lines.last().unwrap().starts_with("j "),
        "bottom-tested loop must end with back-edge jump: {output}"
    );
}

#[test]
fn is_nan_compiles_to_snan() {
    let output = compile(
        r#"
        device sensor: d0;
        device out: d1;
        fn main() {
            let x: f64 = sensor.Value;
            let result: bool = is_nan(x);
            out.Setting = result;
        }
        "#,
    )
    .unwrap();
    assert!(
        output.lines().any(|l| l.starts_with("snan ")),
        "expected a line starting with 'snan ': {output}"
    );
}

#[test]
fn is_nan_branch_fuses_to_bnan() {
    let output = compile(
        r#"
        device sensor: d0;
        device out: d1;
        fn main() {
            let x: f64 = sensor.Value;
            if is_nan(x) {
                out.Setting = 1.0;
            } else {
                out.Setting = 0.0;
            }
        }
        "#,
    )
    .unwrap();
    assert!(
        output.contains("bnan"),
        "expected bnan instruction: {output}"
    );
    assert!(
        !output.contains("snan"),
        "snan should be fused away: {output}"
    );
}

#[test]
fn not_is_nan_branch_fuses_to_bnan_with_swapped_blocks() {
    let output = compile(
        r#"
        device sensor: d0;
        device out: d1;
        fn main() {
            let x: f64 = sensor.Value;
            if !is_nan(x) {
                out.Setting = 0.0;
            } else {
                out.Setting = 1.0;
            }
        }
        "#,
    )
    .unwrap();
    let expected = r#"l r0 d0 Value
bnan r0 4
s d1 Setting 0
j 5
s d1 Setting 1
hcf"#;
    assert_eq!(
        output, expected,
        "expected bnan with swapped blocks: {output}"
    );
}
