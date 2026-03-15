use ic20::bind;
use ic20::cfg;
use ic20::codegen;
use ic20::diagnostic::Severity;
use ic20::opt::{self, OptLevel};
use ic20::parser;
use ic20::regalloc;
use ic20::ssa;

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
    let opt_features = ic20::opt::Features::from_opt_level(OptLevel::O2);
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
        output.contains("s d0 On"),
        "expected device write: {output}"
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
        output.contains("42"),
        "expected literal 42 in output: {output}"
    );
    assert!(
        output.contains("s d0 Setting"),
        "expected device write: {output}"
    );
}

#[test]
fn schmitt_trigger_compiles() {
    let output = compile_file("tests/examples/schmitt_trigger.ic20");
    assert!(
        output.contains("l r") && output.contains("d0 Temperature"),
        "expected device read: {output}"
    );
    assert!(
        output.contains("s d1 On"),
        "expected device write: {output}"
    );
    assert!(output.contains("yield"), "expected yield: {output}");
}

#[test]
fn thermostat_compiles() {
    let output = compile_file("tests/examples/thermostat.ic20");
    assert!(output.contains("yield"), "expected yield in loop: {output}");
}

#[test]
fn const_folding() {
    let output =
        compile("device out: d0;\nconst X: i53 = 3 * 4 + 5;\nfn main() { out.Setting = X; }")
            .unwrap();
    assert!(
        output.contains("17"),
        "expected folded constant 17: {output}"
    );
}

#[test]
fn function_call() {
    let output = compile(
        r#"
        device out: d0;
        fn double(x: i53) -> i53 { return x * 2; }
        fn main() { out.Setting = double(21); }
        "#,
    )
    .unwrap();
    assert!(
        output.contains("42"),
        "expected inlined+folded constant 42: {output}"
    );
    assert!(
        output.contains("s d0 Setting"),
        "expected device write: {output}"
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
    let result = compile("fn main( {}");
    assert!(result.is_err(), "expected parse error");
}

#[test]
fn type_error_is_reported() {
    let result = compile("fn main() { let x: i53 = true + 1; }");
    assert!(result.is_err(), "expected type error");
}

#[test]
fn undeclared_variable_is_reported() {
    let result = compile("fn main() { let x: i53 = y; }");
    assert!(result.is_err(), "expected undeclared variable error");
}

#[test]
fn missing_main_is_reported() {
    let result = compile("fn foo() {}");
    assert!(result.is_err(), "expected missing main error");
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
    assert!(!output.is_empty(), "expected non-empty output");
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
    assert!(!output.is_empty(), "expected non-empty output");
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
        output.contains("snan"),
        "expected snan instruction: {output}"
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
