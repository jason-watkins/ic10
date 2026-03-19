mod branch_fusion;
mod simplify_instructions;

use branch_fusion::fuse_branches;
use simplify_instructions::simplify_instructions;

use crate::regalloc::ic10::IC10Program;

use super::Features;

/// Run IC10 instruction-level optimizations on the program.
///
/// Called after register allocation and spill insertion but before label resolution.
/// Each optimization is a local rewrite pattern operating on the flat instruction stream.
pub fn optimize_program(program: &mut IC10Program, features: &Features) {
    if features.branch_fusion {
        for function in &mut program.functions {
            fuse_branches(&mut function.instructions);
        }
    }
    if features.ic10_simplification {
        for function in &mut program.functions {
            simplify_instructions(&mut function.instructions);
        }
    }
}
