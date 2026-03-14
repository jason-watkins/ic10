use std::path::PathBuf;
use std::process;

use clap::{Parser, ValueEnum};

use ic20::cfg;
use ic20::codegen;
use ic20::diagnostic::{Diagnostic, Severity};
use ic20::opt;
use ic20::parser;
use ic20::regalloc;
use ic20::resolve;
use ic20::ssa;

/// Optimization level controlling which compiler passes run and whether the output
/// uses symbolic labels or resolved line numbers in jump targets.
#[derive(Clone, Copy, Default, ValueEnum)]
enum OptimizationLevel {
    /// Disable all SSA optimization passes. Jump targets use symbolic labels.
    #[value(name = "0")]
    O0,
    /// Standard optimizations (constant propagation, DCE, GVN, copy propagation).
    /// Jump targets are resolved to absolute line numbers. This is the default.
    #[default]
    #[value(name = "1")]
    O1,
    /// Debug-friendly: run all optimizations but keep symbolic labels in the output.
    #[value(name = "g")]
    Og,
}

impl OptimizationLevel {
    fn keep_labels(self) -> bool {
        matches!(self, OptimizationLevel::O0 | OptimizationLevel::Og)
    }

    fn run_optimizations(self) -> bool {
        matches!(self, OptimizationLevel::O1 | OptimizationLevel::Og)
    }
}

#[derive(Parser)]
#[command(
    name = "ic20c",
    about = "IC20 compiler — compiles .ic20 to IC10 assembly"
)]
struct Arguments {
    /// Path to the .ic20 source file
    source: PathBuf,

    /// Write output to this file instead of stdout
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Dump the AST after parsing and exit
    #[arg(long)]
    dump_ast: bool,

    /// Dump the resolved IR after name resolution and exit
    #[arg(long)]
    dump_resolved: bool,

    /// Dump the CFG after construction (before SSA) and exit
    #[arg(long)]
    dump_cfg: bool,

    /// Dump the SSA form (after SSA construction, before optimization) and exit
    #[arg(long)]
    dump_ssa: bool,

    /// Dump the register allocation map and exit
    #[arg(long)]
    dump_regmap: bool,

    /// Optimization level: 1 (default, full optimizations), g (debug-friendly labels),
    /// 0 (no optimizations, symbolic labels)
    #[arg(short = 'O', value_name = "LEVEL", default_value = "1")]
    optimization_level: OptimizationLevel,
}

fn main() {
    let arguments = Arguments::parse();

    let filename = arguments.source.display().to_string();
    let source = match std::fs::read_to_string(&arguments.source) {
        Ok(s) => s,
        Err(error) => {
            eprintln!("error: cannot read '{}': {}", filename, error);
            process::exit(1);
        }
    };

    let mut all_warnings: Vec<Diagnostic> = Vec::new();

    let (ast, parse_diagnostics) = parser::parse(&source);
    if emit_diagnostics_and_check_errors(&parse_diagnostics, &source, &filename) {
        process::exit(1);
    }
    collect_warnings(&parse_diagnostics, &mut all_warnings);

    if arguments.dump_ast {
        println!("{:#?}", ast);
        process::exit(0);
    }

    let (resolved, resolve_diagnostics) = match resolve::resolve(&ast) {
        Ok(result) => result,
        Err(diagnostics) => {
            emit_diagnostics(&diagnostics, &source, &filename);
            process::exit(1);
        }
    };
    collect_warnings(&resolve_diagnostics, &mut all_warnings);

    if arguments.dump_resolved {
        println!("{:#?}", resolved);
        process::exit(0);
    }

    let (mut program, cfg_diagnostics) = cfg::build(&resolved);
    if emit_diagnostics_and_check_errors(&cfg_diagnostics, &source, &filename) {
        process::exit(1);
    }
    collect_warnings(&cfg_diagnostics, &mut all_warnings);

    if arguments.dump_cfg {
        println!("{:#?}", program);
        process::exit(0);
    }

    ssa::construct_program(&mut program);

    if arguments.dump_ssa {
        println!("{:#?}", program);
        process::exit(0);
    }

    if arguments.optimization_level.run_optimizations() {
        opt::optimize_program(&mut program);
    }

    let keep_labels = arguments.optimization_level.keep_labels();
    let ic10_program = match regalloc::allocate_registers(&mut program, keep_labels) {
        Ok(result) => result,
        Err(diagnostics) => {
            emit_diagnostics(&diagnostics, &source, &filename);
            process::exit(1);
        }
    };

    if arguments.dump_regmap {
        println!("{:#?}", ic10_program);
        process::exit(0);
    }

    let (ic10_text, codegen_diagnostics) = codegen::generate(&ic10_program, keep_labels);
    if emit_diagnostics_and_check_errors(&codegen_diagnostics, &source, &filename) {
        process::exit(1);
    }
    collect_warnings(&codegen_diagnostics, &mut all_warnings);

    emit_diagnostics(&all_warnings, &source, &filename);

    match arguments.output {
        Some(path) => {
            if let Err(error) = std::fs::write(&path, &ic10_text) {
                eprintln!("error: cannot write '{}': {}", path.display(), error);
                process::exit(1);
            }
        }
        None => {
            print!("{}", ic10_text);
        }
    }
}

fn emit_diagnostics(diagnostics: &[Diagnostic], source: &str, filename: &str) {
    for diagnostic in diagnostics {
        eprintln!("{}", diagnostic.display(source, filename));
    }
}

fn emit_diagnostics_and_check_errors(
    diagnostics: &[Diagnostic],
    source: &str,
    filename: &str,
) -> bool {
    let mut has_errors = false;
    for diagnostic in diagnostics {
        eprintln!("{}", diagnostic.display(source, filename));
        if diagnostic.severity == Severity::Error {
            has_errors = true;
        }
    }
    has_errors
}

fn collect_warnings(diagnostics: &[Diagnostic], warnings: &mut Vec<Diagnostic>) {
    for diagnostic in diagnostics {
        if diagnostic.severity == Severity::Warning {
            warnings.push(diagnostic.clone());
        }
    }
}
