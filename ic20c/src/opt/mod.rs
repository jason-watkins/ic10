//! Optimization passes for the IC20 compiler.
//!
//! CFG-level passes operate on `ir::cfg::Function` in SSA form. IC10-level
//! passes operate on the post-regalloc `IC10Program`.

/// CFG-level data-flow optimization passes.
pub mod cfg;
/// Post-regalloc IC10 instruction-level optimization passes.
pub mod ic10;

pub use cfg::optimize_program;

/// Controls which optimization passes run and whether they iterate to fixpoint.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OptLevel {
    /// Block simplifications only: unreachable-block removal, block coalescing,
    /// and empty-block merging. No data-flow analysis.
    O0,
    /// Single pass of all optimizations. No inlining. Debug-friendly.
    Og,
    /// Single pass of all optimizations including function inlining.
    O1,
    /// Full fixpoint loop of all optimizations including function inlining.
    O2,
}

/// Controls which individual optimization passes are enabled.
///
/// Defaults are set by [`OptLevel`] via [`Features::from_opt_level`] and can be
/// overridden with `-f`/`--feature` flags on the command line.
pub struct Features {
    /// Fold constants and propagate known values.
    pub constant_propagation: bool,
    /// Apply algebraic identities (e.g. `x + 0 → x`, `x * 1 → x`).
    pub algebraic_simplification: bool,
    /// Replace uses of copy-assigned temps with their source.
    pub copy_propagation: bool,
    /// Eliminate redundant computations via hash-based value numbering.
    pub global_value_numbering: bool,
    /// Remove instructions whose results are never used.
    pub dead_code_elimination: bool,
    /// Coalesce, merge, and remove unreachable basic blocks.
    pub block_simplification: bool,
    /// Merge structurally identical blocks.
    pub block_deduplication: bool,
    /// Optimize loads/stores of static variables.
    pub static_access: bool,
    /// Hoist invariant instructions out of loops.
    pub loop_invariant_code_motion: bool,
    /// Inline small, non-recursive function calls.
    pub inline: bool,
    /// Fuse comparison + branch into a single conditional branch instruction.
    pub branch_fusion: bool,
    /// Simplify IC10 instruction patterns post-regalloc.
    pub ic10_simplification: bool,
    /// Use symbolic labels instead of numeric line offsets.
    pub symbolic_labels: bool,
    /// Sparse conditional constant propagation.
    pub sccp: bool,
}

impl Features {
    /// Returns the default feature set for the given optimization level.
    pub fn from_opt_level(level: OptLevel) -> Self {
        match level {
            OptLevel::O0 => Features {
                constant_propagation: false,
                algebraic_simplification: false,
                copy_propagation: false,
                global_value_numbering: false,
                dead_code_elimination: false,
                block_simplification: true,
                block_deduplication: false,
                static_access: false,
                loop_invariant_code_motion: false,
                inline: false,
                branch_fusion: false,
                ic10_simplification: false,
                symbolic_labels: true,
                sccp: false,
            },
            OptLevel::Og => Features {
                constant_propagation: true,
                algebraic_simplification: true,
                copy_propagation: true,
                global_value_numbering: true,
                dead_code_elimination: true,
                block_simplification: true,
                block_deduplication: true,
                static_access: true,
                loop_invariant_code_motion: true,
                inline: false,
                branch_fusion: true,
                ic10_simplification: true,
                symbolic_labels: true,
                sccp: false,
            },
            OptLevel::O1 | OptLevel::O2 => Features {
                constant_propagation: true,
                algebraic_simplification: true,
                copy_propagation: true,
                global_value_numbering: true,
                dead_code_elimination: true,
                block_simplification: true,
                block_deduplication: true,
                static_access: true,
                loop_invariant_code_motion: true,
                inline: true,
                branch_fusion: true,
                ic10_simplification: true,
                symbolic_labels: false,
                sccp: true,
            },
        }
    }
}
