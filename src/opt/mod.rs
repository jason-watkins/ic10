pub mod cfg;
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
    pub constant_propagation: bool,
    pub copy_propagation: bool,
    pub global_value_numbering: bool,
    pub dead_code_elimination: bool,
    pub block_simplification: bool,
    pub block_deduplication: bool,
    pub static_access: bool,
    pub inline: bool,
    pub branch_fusion: bool,
    pub symbolic_labels: bool,
}

impl Features {
    pub fn from_opt_level(level: OptLevel) -> Self {
        match level {
            OptLevel::O0 => Features {
                constant_propagation: false,
                copy_propagation: false,
                global_value_numbering: false,
                dead_code_elimination: false,
                block_simplification: true,
                block_deduplication: false,
                static_access: false,
                inline: false,
                branch_fusion: false,
                symbolic_labels: true,
            },
            OptLevel::Og => Features {
                constant_propagation: true,
                copy_propagation: true,
                global_value_numbering: true,
                dead_code_elimination: true,
                block_simplification: true,
                block_deduplication: true,
                static_access: true,
                inline: false,
                branch_fusion: true,
                symbolic_labels: true,
            },
            OptLevel::O1 | OptLevel::O2 => Features {
                constant_propagation: true,
                copy_propagation: true,
                global_value_numbering: true,
                dead_code_elimination: true,
                block_simplification: true,
                block_deduplication: true,
                static_access: true,
                inline: true,
                branch_fusion: true,
                symbolic_labels: false,
            },
        }
    }
}

