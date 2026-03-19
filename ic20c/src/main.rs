use std::path::PathBuf;
use std::process;
use std::str::FromStr;

use clap::{Parser, ValueEnum};

use ic20c::bind;
use ic20c::cfg;
use ic20c::codegen;
use ic20c::diagnostic::{Diagnostic, Severity};
use ic20c::opt::{self, Features, OptLevel};
use ic20c::parser;
use ic20c::regalloc;
use ic20c::ssa;

/// An individual optimization pass that can be enabled or disabled with `-f`/`--feature`.
#[derive(Clone, Copy, ValueEnum)]
enum Feature {
    #[value(name = "constant-propagation")]
    ConstantPropagation,
    #[value(name = "algebraic-simplification")]
    AlgebraicSimplification,
    #[value(name = "copy-propagation")]
    CopyPropagation,
    #[value(name = "global-value-numbering")]
    GlobalValueNumbering,
    #[value(name = "dead-code-elimination")]
    DeadCodeElimination,
    #[value(name = "block-simplification")]
    BlockSimplification,
    #[value(name = "block-deduplication")]
    BlockDeduplication,
    #[value(name = "inline")]
    Inline,
    #[value(name = "branch-fusion")]
    BranchFusion,
    #[value(name = "ic10-simplification")]
    Ic10Simplification,
    #[value(name = "static-access")]
    StaticAccess,
    #[value(name = "loop-invariant-code-motion")]
    LoopInvariantCodeMotion,
    #[value(name = "symbolic-labels")]
    SymbolicLabels,
    #[value(name = "sccp")]
    Sccp,
}

/// A feature toggle parsed from `-f <name>` (enable) or `-f no-<name>` (disable).
#[derive(Clone)]
struct FeatureToggle {
    enable: bool,
    feature: Feature,
}

impl FromStr for FeatureToggle {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (enable, name) = if let Some(rest) = s.strip_prefix("no-") {
            (false, rest)
        } else {
            (true, s)
        };
        let feature = <Feature as ValueEnum>::from_str(name, true).map_err(|_| {
            format!(
                "unknown feature '{}'; valid features are: \
                 constant-propagation, algebraic-simplification, copy-propagation, global-value-numbering, \
                 dead-code-elimination, block-simplification, block-deduplication, static-access, \
                 loop-invariant-code-motion, inline, \
                 branch-fusion, ic10-simplification, symbolic-labels, sccp",
                name
            )
        })?;
        Ok(FeatureToggle { enable, feature })
    }
}

/// Applies CLI feature toggles (`-f <name>` / `-f no-<name>`) to the features set.
fn apply_feature_toggles(features: &mut Features, toggles: &[FeatureToggle]) {
    for toggle in toggles {
        match toggle.feature {
            Feature::ConstantPropagation => features.constant_propagation = toggle.enable,
            Feature::AlgebraicSimplification => features.algebraic_simplification = toggle.enable,
            Feature::CopyPropagation => features.copy_propagation = toggle.enable,
            Feature::GlobalValueNumbering => features.global_value_numbering = toggle.enable,
            Feature::DeadCodeElimination => features.dead_code_elimination = toggle.enable,
            Feature::BlockSimplification => features.block_simplification = toggle.enable,
            Feature::BlockDeduplication => features.block_deduplication = toggle.enable,
            Feature::StaticAccess => features.static_access = toggle.enable,
            Feature::LoopInvariantCodeMotion => features.loop_invariant_code_motion = toggle.enable,
            Feature::Inline => features.inline = toggle.enable,
            Feature::BranchFusion => features.branch_fusion = toggle.enable,
            Feature::Ic10Simplification => features.ic10_simplification = toggle.enable,
            Feature::SymbolicLabels => features.symbolic_labels = toggle.enable,
            Feature::Sccp => features.sccp = toggle.enable,
        }
    }
}

/// Optimization level controlling which compiler passes run and whether the output
/// uses symbolic labels or resolved line numbers in jump targets.
#[derive(Clone, Copy, Default, ValueEnum)]
enum OptimizationLevel {
    /// Block simplifications only (unreachable block removal, block coalescing,
    /// empty-block merging). Jump targets use symbolic labels.
    #[value(name = "0")]
    O0,
    /// Debug-friendly: single pass of all optimizations, no inlining.
    /// Jump targets use symbolic labels.
    #[value(name = "g")]
    Og,
    /// Single pass of all optimizations including inlining.
    /// Jump targets are resolved to absolute line numbers.
    #[value(name = "1")]
    O1,
    /// Full optimizing: fixpoint loop with inlining until convergence.
    /// Jump targets are resolved to absolute line numbers. This is the default.
    #[default]
    #[value(name = "2")]
    O2,
}

impl OptimizationLevel {
    fn keep_labels(self) -> bool {
        matches!(self, OptimizationLevel::O0 | OptimizationLevel::Og)
    }
}

impl From<OptimizationLevel> for OptLevel {
    fn from(level: OptimizationLevel) -> OptLevel {
        match level {
            OptimizationLevel::O0 => OptLevel::O0,
            OptimizationLevel::Og => OptLevel::Og,
            OptimizationLevel::O1 => OptLevel::O1,
            OptimizationLevel::O2 => OptLevel::O2,
        }
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

    /// Optimization level: 0 (block simplifications only), g (debug-friendly, single pass,
    /// no inlining), 1 (single pass with inlining), 2 (full fixpoint, default)
    #[arg(short = 'O', value_name = "LEVEL", default_value = "2")]
    optimization_level: OptimizationLevel,

    /// Enable or disable an individual optimization pass or code generation feature.
    /// May be passed multiple times. Use `no-<name>` to disable (e.g. `-f no-inline`).
    /// Valid features: constant-propagation, algebraic-simplification, copy-propagation,
    /// global-value-numbering, dead-code-elimination, block-simplification, inline,
    /// branch-fusion, ic10-simplification, symbolic-labels.
    #[arg(short = 'f', long = "feature", value_name = "FEATURE")]
    features: Vec<FeatureToggle>,
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

    let (bound, bind_diagnostics) = match bind::bind(&ast) {
        Ok(result) => result,
        Err(diagnostics) => {
            emit_diagnostics(&diagnostics, &source, &filename);
            process::exit(1);
        }
    };
    collect_warnings(&bind_diagnostics, &mut all_warnings);

    if arguments.dump_resolved {
        println!("{:#?}", bound);
        process::exit(0);
    }

    let (mut program, cfg_diagnostics) = cfg::build(&bound);
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

    let opt_level: OptLevel = arguments.optimization_level.into();
    let mut opt_features = Features::from_opt_level(opt_level);
    opt_features.symbolic_labels = arguments.optimization_level.keep_labels();
    apply_feature_toggles(&mut opt_features, &arguments.features);
    opt::optimize_program(&mut program, opt_level, &opt_features);

    let keep_labels = opt_features.symbolic_labels;
    let ic10_program = match regalloc::allocate_registers(&mut program, keep_labels, &opt_features)
    {
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
            println!("{}", ic10_text);
        }
    }
}

/// Prints all diagnostics to stderr.
fn emit_diagnostics(diagnostics: &[Diagnostic], source: &str, filename: &str) {
    for diagnostic in diagnostics {
        eprintln!("{}", diagnostic.display(source, filename));
    }
}

/// Prints all diagnostics to stderr and returns `true` if any are errors.
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

/// Filters diagnostics for warnings and appends them to `warnings`.
fn collect_warnings(diagnostics: &[Diagnostic], warnings: &mut Vec<Diagnostic>) {
    for diagnostic in diagnostics {
        if diagnostic.severity == Severity::Warning {
            warnings.push(diagnostic.clone());
        }
    }
}
