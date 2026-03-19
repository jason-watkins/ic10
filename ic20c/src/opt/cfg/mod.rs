//! CFG-level optimization passes.
//!
//! Each pass is a standalone function taking `&mut Function`. The driver
//! composes them via `optimize_single_pass` and optionally iterates to
//! fixpoint at `-O2`.

mod algebraic_simplification;
mod block_deduplication;
mod block_simplification;
mod constant_propagation;
mod copy_propagation;
mod dead_code_elimination;
mod global_value_numbering;
mod inline;
mod loop_invariant_code_motion;
mod sccp;
mod static_access;
mod utilities;

use algebraic_simplification::algebraic_simplification;
use block_deduplication::deduplicate_blocks;
use block_simplification::{
    coalesce_blocks, invert_negated_branches, merge_empty_blocks, remove_unreachable_blocks,
};
use constant_propagation::constant_propagation;
use copy_propagation::copy_propagation;
use dead_code_elimination::dead_code_elimination;
use global_value_numbering::global_value_numbering;
use inline::inline_functions;
use loop_invariant_code_motion::loop_invariant_code_motion;
use sccp::sccp;
use static_access::optimize_static_access;

use crate::ir::cfg::{Function, Program};

use super::{Features, OptLevel};

/// Optimize all functions in a CFG program at the requested level, respecting
/// the per-pass `features` overrides.
pub fn optimize_program(program: &mut Program, level: OptLevel, features: &Features) {
    if features.inline {
        inline_functions(program);
    }

    for function in &mut program.functions {
        if level == OptLevel::O2 {
            optimize_to_fixpoint(function, features);
        } else {
            optimize_single_pass(function, features);
        }
    }
}

/// Run a single iteration of all enabled optimization passes on `function`.
/// Returns `true` if any pass modified the function.
fn optimize_single_pass(function: &mut Function, features: &Features) -> bool {
    let mut changed = false;
    if features.sccp {
        changed |= sccp(function);
    } else if features.constant_propagation {
        changed |= constant_propagation(function);
    }
    if features.algebraic_simplification {
        changed |= algebraic_simplification(function);
    }
    if features.copy_propagation {
        changed |= copy_propagation(function);
    }
    if features.global_value_numbering {
        changed |= global_value_numbering(function);
    }
    if features.static_access {
        changed |= optimize_static_access(function);
    }
    if features.loop_invariant_code_motion {
        changed |= loop_invariant_code_motion(function);
    }
    if features.dead_code_elimination {
        changed |= dead_code_elimination(function);
    }
    if features.block_simplification {
        changed |= remove_unreachable_blocks(function);
        changed |= coalesce_blocks(function);
        changed |= merge_empty_blocks(function);
        changed |= invert_negated_branches(function);
    }
    if features.block_deduplication {
        changed |= deduplicate_blocks(function);
    }
    changed
}

/// Repeatedly run `optimize_single_pass` until no pass reports a change,
/// or 100 iterations are exceeded (which triggers a panic).
fn optimize_to_fixpoint(function: &mut Function, features: &Features) {
    let mut iterations = 0;
    loop {
        let changed = optimize_single_pass(function, features);
        if !changed {
            break;
        }
        iterations += 1;
        assert!(
            iterations <= 100,
            "optimization loop failed to converge after {} iterations for function '{}'",
            iterations,
            function.name
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bind::bind;
    use crate::cfg;
    use crate::ir::cfg::{BlockRole, Function, Instruction, Operation, Program};
    use crate::parser::parse;
    use crate::ssa;

    fn build_optimized(source: &str) -> Program {
        let mut program = build_ssa_unoptimized(source);
        let features = Features::from_opt_level(OptLevel::O2);
        optimize_program(&mut program, OptLevel::O2, &features);
        program
    }

    fn build_ssa_unoptimized(source: &str) -> Program {
        let (ast, parse_diagnostics) = parse(source);
        let errors: Vec<_> = parse_diagnostics
            .iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "parse errors: {:#?}", errors);
        let (bound, _) =
            bind(&ast).unwrap_or_else(|diagnostics| panic!("bind errors: {:#?}", diagnostics));
        let (mut program, _) = cfg::build(&bound);
        ssa::construct_program(&mut program);
        program
    }

    fn get_function<'a>(program: &'a Program, name: &str) -> &'a Function {
        program
            .functions
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("function '{}' not found", name))
    }

    fn count_instructions(function: &Function) -> usize {
        function
            .blocks
            .iter()
            .map(|block| block.instructions.len())
            .sum()
    }

    fn has_binary_instruction(function: &Function) -> bool {
        function.blocks.iter().any(|block| {
            block.instructions.iter().any(|instruction| {
                matches!(
                    instruction,
                    Instruction::Assign {
                        operation: Operation::Binary { .. },
                        ..
                    }
                )
            })
        })
    }

    fn has_phi_instruction(function: &Function) -> bool {
        function.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|instruction| matches!(instruction, Instruction::Phi { .. }))
        })
    }

    fn count_constants(function: &Function) -> usize {
        function
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter())
            .filter(|instruction| {
                matches!(
                    instruction,
                    Instruction::Assign {
                        operation: Operation::Constant(_),
                        ..
                    }
                )
            })
            .count()
    }

    #[test]
    fn constant_folding_arithmetic() {
        let program = build_optimized("fn main() { let x: i53 = 3 + 4; }");
        let main = get_function(&program, "main");
        assert!(
            !has_binary_instruction(main),
            "binary instruction should be folded away"
        );
    }

    #[test]
    fn constant_folding_nested_arithmetic() {
        let program = build_optimized("fn main() { let x: i53 = (2 + 3) * (4 - 1); }");
        let main = get_function(&program, "main");
        assert!(
            !has_binary_instruction(main),
            "all arithmetic should be folded"
        );
    }

    #[test]
    fn dead_code_elimination_unused_variable() {
        let before = build_ssa_unoptimized("fn main() { let x: i53 = 5; }");
        let after = build_optimized("fn main() { let x: i53 = 5; }");
        let before_count = count_instructions(get_function(&before, "main"));
        let after_count = count_instructions(get_function(&after, "main"));
        assert!(
            after_count < before_count,
            "DCE should reduce instruction count: before={}, after={}",
            before_count,
            after_count
        );
    }

    #[test]
    fn dead_code_elimination_preserves_side_effects() {
        let program = build_optimized(
            r#"
            device sensor: d0;
            fn main() {
                sensor.Setting = 1;
            }
            "#,
        );
        let main = get_function(&program, "main");
        let has_store = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::StoreDevice { .. }))
        });
        assert!(has_store, "device store must be preserved");
    }

    #[test]
    fn copy_propagation_eliminates_copies() {
        let src = r#"fn main() {
            let x: i53 = 1;
            let y: i53 = x;
            let z: i53 = y;
        }"#;
        let before = build_ssa_unoptimized(src);
        let after = build_optimized(src);
        let before_count = count_instructions(get_function(&before, "main"));
        let after_count = count_instructions(get_function(&after, "main"));
        assert!(
            after_count < before_count,
            "copy propagation + DCE should reduce instruction count: before={}, after={}",
            before_count,
            after_count
        );
    }

    #[test]
    fn constant_branch_simplification() {
        let program = build_optimized(
            r#"
            device sensor: d0;
            fn main() {
                if true {
                    sensor.Setting = 1;
                } else {
                    sensor.Setting = 2;
                }
            }
            "#,
        );
        let main = get_function(&program, "main");
        let store_count: usize = main
            .blocks
            .iter()
            .flat_map(|b| b.instructions.iter())
            .filter(|i| matches!(i, Instruction::StoreDevice { .. }))
            .count();
        assert_eq!(
            store_count, 1,
            "dead branch should be eliminated, leaving only one store"
        );
    }

    #[test]
    fn phi_with_same_constant_folded() {
        let program = build_optimized(
            r#"fn main() {
                let mut x: i53 = 1;
                if true {
                    x = 1;
                }
                let y = x;
            }"#,
        );
        let main = get_function(&program, "main");
        assert!(
            !has_phi_instruction(main),
            "phi with identical constant arguments should be eliminated"
        );
    }

    #[test]
    fn gvn_eliminates_duplicate_constants() {
        let before = build_ssa_unoptimized(
            r#"
            device sensor: d0;
            fn main() {
                sensor.Setting = 42;
                sensor.Mode = 42;
            }
            "#,
        );
        let after = build_optimized(
            r#"
            device sensor: d0;
            fn main() {
                sensor.Setting = 42;
                sensor.Mode = 42;
            }
            "#,
        );
        let before_constants = count_constants(get_function(&before, "main"));
        let after_constants = count_constants(get_function(&after, "main"));
        assert!(
            after_constants < before_constants,
            "GVN should deduplicate identical constants: before={}, after={}",
            before_constants,
            after_constants
        );
    }

    #[test]
    fn pipeline_reduces_complex_program() {
        let before = build_ssa_unoptimized(
            r#"
            device sensor: d0;
            fn main() {
                let x: i53 = 2 + 3;
                let y: i53 = x * 2;
                let unused: i53 = 99;
                sensor.Setting = y;
            }
            "#,
        );
        let after = build_optimized(
            r#"
            device sensor: d0;
            fn main() {
                let x: i53 = 2 + 3;
                let y: i53 = x * 2;
                let unused: i53 = 99;
                sensor.Setting = y;
            }
            "#,
        );
        let before_count = count_instructions(get_function(&before, "main"));
        let after_count = count_instructions(get_function(&after, "main"));
        assert!(
            after_count < before_count,
            "optimization pipeline should reduce total instructions: before={}, after={}",
            before_count,
            after_count
        );
    }

    #[test]
    fn yield_preserved_through_optimization() {
        let program = build_optimized(
            r#"fn main() {
                loop {
                    yield;
                }
            }"#,
        );
        let main = get_function(&program, "main");
        let has_yield = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::Yield))
        });
        assert!(has_yield, "yield must be preserved");
    }

    #[test]
    fn sleep_preserved_through_optimization() {
        let program = build_optimized(
            r#"fn main() {
                sleep(1.0);
            }"#,
        );
        let main = get_function(&program, "main");
        let has_sleep = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::Sleep { .. }))
        });
        assert!(has_sleep, "sleep must be preserved");
    }

    #[test]
    fn intrinsic_constant_folding() {
        let program = build_optimized("fn main() { let x: f64 = sqrt(4.0); }");
        let main = get_function(&program, "main");
        let has_intrinsic = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::IntrinsicCall { .. }))
        });
        assert!(
            !has_intrinsic,
            "intrinsic call with constant args should be folded"
        );
    }

    #[test]
    fn loop_with_device_io_preserved() {
        let program = build_optimized(
            r#"
            device sensor: d0;
            device light: d1;
            fn main() {
                loop {
                    let temp = sensor.Temperature;
                    light.Setting = temp;
                    yield;
                }
            }
            "#,
        );
        let main = get_function(&program, "main");
        let has_load = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::LoadDevice { .. }))
        });
        let has_store = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::StoreDevice { .. }))
        });
        assert!(has_load, "device load in loop must be preserved");
        assert!(has_store, "device store in loop must be preserved");
    }

    #[test]
    fn unary_constant_folding() {
        let program = build_optimized("fn main() { let x: i53 = -5; }");
        let main = get_function(&program, "main");
        let has_unary = main.blocks.iter().any(|block| {
            block.instructions.iter().any(|i| {
                matches!(
                    i,
                    Instruction::Assign {
                        operation: Operation::Unary { .. },
                        ..
                    }
                )
            })
        });
        assert!(!has_unary, "unary negation of constant should be folded");
    }

    #[test]
    fn comparison_constant_folding() {
        let program = build_optimized("fn main() { let x: bool = 3 < 5; }");
        let main = get_function(&program, "main");
        assert!(
            !has_binary_instruction(main),
            "constant comparison should be folded"
        );
    }

    #[test]
    fn inline_small_function_called_once() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn helper(x: i53) -> i53 { return x + 1; }
            fn main() { out.Setting = helper(5); }
            "#,
        );
        assert!(
            program.functions.len() == 1,
            "helper should be inlined and removed, leaving only main; found: {:?}",
            program
                .functions
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
        let main = get_function(&program, "main");
        let has_call = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::Call { .. }))
        });
        assert!(!has_call, "call should be inlined away");
    }

    #[test]
    fn inline_constant_propagates_through_inlined_body() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn double(x: i53) -> i53 { return x * 2; }
            fn main() { out.Setting = double(21); }
            "#,
        );
        let main = get_function(&program, "main");
        let has_constant_42 = main.blocks.iter().any(|block| {
            block.instructions.iter().any(|i| {
                matches!(
                    i,
                    Instruction::Assign {
                        operation: Operation::Constant(v),
                        ..
                    } if *v == 42.0
                )
            })
        });
        assert!(
            has_constant_42,
            "double(21) should inline and fold to constant 42"
        );
    }

    #[test]
    fn inline_does_not_inline_recursive_function() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn recurse(x: i53) -> i53 {
                if x < 1 { return 0; }
                return recurse(x - 1) + 1;
            }
            fn main() { out.Setting = recurse(5); }
            "#,
        );
        assert!(
            program.functions.len() == 2,
            "recursive function should not be inlined; found: {:?}",
            program
                .functions
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn inline_preserves_side_effects() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn write_device(x: i53) {
                out.Setting = x;
            }
            fn main() { write_device(42); }
            "#,
        );
        let main = get_function(&program, "main");
        let has_store = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::StoreDevice { .. }))
        });
        assert!(
            has_store,
            "device store from inlined function must be preserved"
        );
    }

    #[test]
    fn inline_function_with_control_flow() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn abs_val(x: f64) -> f64 {
                if x < 0.0 { return -x; }
                return x;
            }
            fn main() { out.Setting = abs_val(-5.0); }
            "#,
        );
        let main = get_function(&program, "main");
        let has_call = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::Call { .. }))
        });
        assert!(!has_call, "abs_val should be inlined");
        let has_constant_5 = main.blocks.iter().any(|block| {
            block.instructions.iter().any(|i| {
                matches!(
                    i,
                    Instruction::Assign {
                        operation: Operation::Constant(v),
                        ..
                    } if *v == 5.0
                )
            })
        });
        assert!(has_constant_5, "abs_val(-5.0) should fold to constant 5.0");
    }

    #[test]
    fn inline_does_not_inline_large_function_called_many_times() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn big(a: f64, b: f64) -> f64 {
                let c = a + b;
                let d = c * a;
                let e = d - b;
                let f = e + c;
                return f * d;
            }
            fn main() {
                out.Setting = big(1.0, 2.0);
                out.Setting = big(3.0, 4.0);
                out.Setting = big(5.0, 6.0);
            }
            "#,
        );
        assert!(
            program.functions.len() == 2,
            "large function called 3 times should not be inlined; found: {:?}",
            program
                .functions
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn inline_void_function() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn set_output(val: f64) {
                out.Setting = val;
            }
            fn main() {
                set_output(10.0);
            }
            "#,
        );
        let main = get_function(&program, "main");
        let has_call = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::Call { .. }))
        });
        assert!(!has_call, "void function called once should be inlined");
        let has_store = main.blocks.iter().any(|block| {
            block
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::StoreDevice { .. }))
        });
        assert!(
            has_store,
            "device store from inlined void function must be preserved"
        );
    }

    #[test]
    fn inline_multiple_small_functions() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn add_one(x: i53) -> i53 { return x + 1; }
            fn double(x: i53) -> i53 { return x * 2; }
            fn main() { out.Setting = double(add_one(5)); }
            "#,
        );
        assert!(
            program.functions.len() == 1,
            "both small functions should be inlined; found: {:?}",
            program
                .functions
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
        let main = get_function(&program, "main");
        let has_constant_12 = main.blocks.iter().any(|block| {
            block.instructions.iter().any(|i| {
                matches!(
                    i,
                    Instruction::Assign {
                        operation: Operation::Constant(v),
                        ..
                    } if *v == 12.0
                )
            })
        });
        assert!(
            has_constant_12,
            "double(add_one(5)) = double(6) = 12 should fold to constant"
        );
    }

    #[test]
    fn fixpoint_convergence_multiple_iterations() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn main() {
                let a: i53 = 2;
                let b: i53 = 3;
                let c: i53 = a + b;
                let d: i53 = c;
                let e: i53 = d * 1;
                out.Setting = e;
            }
            "#,
        );
        let main = get_function(&program, "main");
        assert!(
            !has_binary_instruction(main),
            "all arithmetic should be folded after multiple iterations \
             (copy prop reveals constants for subsequent folding)"
        );
        let has_constant_5 = main.blocks.iter().any(|block| {
            block.instructions.iter().any(|i| {
                matches!(
                    i,
                    Instruction::Assign {
                        operation: Operation::Constant(v),
                        ..
                    } if *v == 5.0
                )
            })
        });
        assert!(
            has_constant_5,
            "2+3=5, copy d=c, d*1=5 should all fold to constant 5"
        );
    }

    #[test]
    fn branch_inversion() {
        use crate::ir::UnaryOperator;
        use crate::ir::cfg::Terminator;

        let mut program = build_ssa_unoptimized(
            r#"
            device io: d0;
            fn main() {
                let x: f64 = io.Setting;
                let cond: bool = x > 0.0;
                if !cond {
                    io.Setting = 1;
                } else {
                    io.Setting = 2;
                }
            }
            "#,
        );
        let function = program
            .functions
            .iter_mut()
            .find(|f| f.name == "main")
            .unwrap();

        let has_not_before = function.blocks.iter().any(|block| {
            block.instructions.iter().any(|i| {
                matches!(
                    i,
                    Instruction::Assign {
                        operation: Operation::Unary {
                            operator: UnaryOperator::Not,
                            ..
                        },
                        ..
                    }
                )
            })
        });
        assert!(
            has_not_before,
            "unoptimized program should contain a Not operation for !cond"
        );

        block_simplification::invert_negated_branches(function);

        let branch_conditions_use_not = function.blocks.iter().any(|block| {
            if let Terminator::Branch { condition, .. } = &block.terminator {
                function.blocks.iter().any(|b| {
                    b.instructions.iter().any(|i| {
                        matches!(
                            i,
                            Instruction::Assign {
                                target,
                                operation: Operation::Unary {
                                    operator: UnaryOperator::Not,
                                    ..
                                },
                            } if *target == *condition
                        )
                    })
                })
            } else {
                false
            }
        });
        assert!(
            !branch_conditions_use_not,
            "after inversion, no branch should use a Not-defined condition"
        );
    }

    #[test]
    fn block_deduplication() {
        use crate::ir::cfg::Terminator;

        let mut program = build_ssa_unoptimized(
            r#"
            device io: d0;
            fn main() {
                let x: f64 = io.Setting;
                if x > 0.0 {
                    io.Mode = 1;
                } else {
                    io.Mode = 1;
                }
            }
            "#,
        );
        let function = program
            .functions
            .iter_mut()
            .find(|f| f.name == "main")
            .unwrap();
        let blocks_before = function
            .blocks
            .iter()
            .filter(|b| !matches!(b.terminator, Terminator::None) || !b.instructions.is_empty())
            .count();
        let changed = block_deduplication::deduplicate_blocks(function);
        assert!(changed, "deduplication should find identical branches");
        let blocks_after = function
            .blocks
            .iter()
            .filter(|b| !matches!(b.terminator, Terminator::None) || !b.instructions.is_empty())
            .count();
        assert!(
            blocks_after < blocks_before,
            "deduplication should reduce block count: before={}, after={}",
            blocks_before,
            blocks_after,
        );
    }

    #[test]
    fn unused_intrinsic_result_eliminated() {
        let before =
            build_ssa_unoptimized("fn main() { let x: f64 = 2.0; let _y: f64 = sqrt(x); }");
        let after = build_optimized("fn main() { let x: f64 = 2.0; let _y: f64 = sqrt(x); }");
        let before_count = count_instructions(get_function(&before, "main"));
        let after_count = count_instructions(get_function(&after, "main"));
        assert!(
            after_count < before_count,
            "DCE should eliminate unused intrinsic: before={}, after={}",
            before_count,
            after_count,
        );
    }

    #[test]
    fn algebraic_add_zero_eliminated() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn main() {
                let x: f64 = out.Setting;
                let y: f64 = x + 0.0;
                out.Mode = y;
            }
            "#,
        );
        let main = get_function(&program, "main");
        assert!(
            !has_binary_instruction(main),
            "x + 0 should be simplified away by algebraic simplification"
        );
    }

    #[test]
    fn algebraic_mul_one_eliminated() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn main() {
                let x: f64 = out.Setting;
                let y: f64 = x * 1.0;
                out.Mode = y;
            }
            "#,
        );
        let main = get_function(&program, "main");
        assert!(
            !has_binary_instruction(main),
            "x * 1 should be simplified away by algebraic simplification"
        );
    }

    #[test]
    fn algebraic_mul_zero_folded() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn main() {
                let x: f64 = out.Setting;
                let y: f64 = x * 0.0;
                out.Mode = y;
            }
            "#,
        );
        let main = get_function(&program, "main");
        assert!(
            !has_binary_instruction(main),
            "x * 0 should be folded to constant 0"
        );
    }

    #[test]
    fn algebraic_sub_zero_eliminated() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn main() {
                let x: f64 = out.Setting;
                let y: f64 = x - 0.0;
                out.Mode = y;
            }
            "#,
        );
        let main = get_function(&program, "main");
        assert!(
            !has_binary_instruction(main),
            "x - 0 should be simplified away by algebraic simplification"
        );
    }

    #[test]
    fn algebraic_div_one_eliminated() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn main() {
                let x: f64 = out.Setting;
                let y: f64 = x / 1.0;
                out.Mode = y;
            }
            "#,
        );
        let main = get_function(&program, "main");
        assert!(
            !has_binary_instruction(main),
            "x / 1 should be simplified away by algebraic simplification"
        );
    }

    #[test]
    fn algebraic_and_false_folded() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn main() {
                let x: bool = out.Setting > 0.0;
                let y: bool = x && false;
                if y { out.Mode = 1; }
            }
            "#,
        );
        let main = get_function(&program, "main");
        let has_and = main.blocks.iter().any(|block| {
            block.instructions.iter().any(|i| {
                matches!(
                    i,
                    Instruction::Assign {
                        operation: Operation::Binary {
                            operator: crate::ir::BinaryOperator::And,
                            ..
                        },
                        ..
                    }
                )
            })
        });
        assert!(!has_and, "x && false should be folded to constant 0");
    }

    #[test]
    fn algebraic_or_true_folded() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn main() {
                let x: bool = out.Setting > 0.0;
                let y: bool = x || true;
                if y { out.Mode = 1; }
            }
            "#,
        );
        let main = get_function(&program, "main");
        let has_or = main.blocks.iter().any(|block| {
            block.instructions.iter().any(|i| {
                matches!(
                    i,
                    Instruction::Assign {
                        operation: Operation::Binary {
                            operator: crate::ir::BinaryOperator::Or,
                            ..
                        },
                        ..
                    }
                )
            })
        });
        assert!(!has_or, "x || true should be folded to constant 1");
    }

    #[test]
    fn gvn_cross_block_deduplication() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn main() {
                let a: f64 = out.Setting;
                let b: f64 = out.Mode;
                let c: f64 = a + b;
                if c > 0.0 {
                    let d: f64 = a + b;
                    out.Setting = d;
                }
            }
            "#,
        );
        let main = get_function(&program, "main");
        let add_count: usize = main
            .blocks
            .iter()
            .flat_map(|b| b.instructions.iter())
            .filter(|i| {
                matches!(
                    i,
                    Instruction::Assign {
                        operation: Operation::Binary {
                            operator: crate::ir::BinaryOperator::Add,
                            ..
                        },
                        ..
                    }
                )
            })
            .count();
        assert!(
            add_count <= 1,
            "dominator-based GVN should deduplicate a+b across blocks; found {} adds",
            add_count,
        );
    }

    #[test]
    fn licm_hoists_invariant_computation() {
        let before = build_ssa_unoptimized(
            r#"
            device out: d0;
            fn main() {
                let a: f64 = 3.0;
                let b: f64 = 4.0;
                while a > 0.0 {
                    let c: f64 = a + b;
                    out.Setting = c;
                    yield;
                }
            }
            "#,
        );
        let after = build_optimized(
            r#"
            device out: d0;
            fn main() {
                let a: f64 = 3.0;
                let b: f64 = 4.0;
                while a > 0.0 {
                    let c: f64 = a + b;
                    out.Setting = c;
                    yield;
                }
            }
            "#,
        );
        let before_loop_adds: usize = get_function(&before, "main")
            .blocks
            .iter()
            .filter(|b| {
                matches!(
                    b.role,
                    BlockRole::LoopStart(_) | BlockRole::LoopBody(_) | BlockRole::LoopContinue(_)
                )
            })
            .flat_map(|b| b.instructions.iter())
            .filter(|i| {
                matches!(
                    i,
                    Instruction::Assign {
                        operation: Operation::Binary { .. },
                        ..
                    }
                )
            })
            .count();
        let after_loop_adds: usize = get_function(&after, "main")
            .blocks
            .iter()
            .filter(|b| {
                matches!(
                    b.role,
                    BlockRole::LoopStart(_) | BlockRole::LoopBody(_) | BlockRole::LoopContinue(_)
                )
            })
            .flat_map(|b| b.instructions.iter())
            .filter(|i| {
                matches!(
                    i,
                    Instruction::Assign {
                        operation: Operation::Binary { .. },
                        ..
                    }
                )
            })
            .count();
        assert!(
            after_loop_adds < before_loop_adds,
            "LICM should hoist invariant a+b out of the loop: before={}, after={}",
            before_loop_adds,
            after_loop_adds,
        );
    }

    #[test]
    fn licm_does_not_hoist_device_load() {
        let program = build_optimized(
            r#"
            device sensor: d0;
            device out: d1;
            fn main() {
                loop {
                    let temp: f64 = sensor.Temperature;
                    out.Setting = temp;
                    yield;
                }
            }
            "#,
        );
        let main = get_function(&program, "main");
        let loop_has_load = main
            .blocks
            .iter()
            .filter(|b| {
                matches!(
                    b.role,
                    BlockRole::LoopStart(_) | BlockRole::LoopBody(_) | BlockRole::LoopContinue(_)
                )
            })
            .any(|b| {
                b.instructions
                    .iter()
                    .any(|i| matches!(i, Instruction::LoadDevice { .. }))
            });
        assert!(
            loop_has_load,
            "device loads must stay inside the loop (value changes each iteration)"
        );
    }

    #[test]
    fn licm_does_not_hoist_call() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn compute(x: f64) -> f64 { return x + 1.0; }
            fn main() {
                let mut i: f64 = 0.0;
                while i < 10.0 {
                    out.Setting = compute(i);
                    i = i + 1.0;
                }
            }
            "#,
        );
        let main = get_function(&program, "main");
        let any_loop_block_has_add = main
            .blocks
            .iter()
            .filter(|b| {
                matches!(
                    b.role,
                    BlockRole::LoopStart(_) | BlockRole::LoopBody(_) | BlockRole::LoopContinue(_)
                )
            })
            .any(|b| {
                b.instructions.iter().any(|i| {
                    matches!(
                        i,
                        Instruction::Assign {
                            operation: Operation::Binary { .. },
                            ..
                        }
                    )
                })
            });
        assert!(
            any_loop_block_has_add,
            "computation depending on loop variable should remain in the loop"
        );
    }

    #[test]
    fn licm_hoists_pure_intrinsic() {
        let before = build_ssa_unoptimized(
            r#"
            device out: d0;
            fn main() {
                let x: f64 = 2.0;
                while x > 0.0 {
                    let y: f64 = sqrt(x);
                    out.Setting = y;
                    yield;
                }
            }
            "#,
        );
        let after = build_optimized(
            r#"
            device out: d0;
            fn main() {
                let x: f64 = 2.0;
                while x > 0.0 {
                    let y: f64 = sqrt(x);
                    out.Setting = y;
                    yield;
                }
            }
            "#,
        );
        let before_loop_intrinsics: usize = get_function(&before, "main")
            .blocks
            .iter()
            .filter(|b| {
                matches!(
                    b.role,
                    BlockRole::LoopStart(_) | BlockRole::LoopBody(_) | BlockRole::LoopContinue(_)
                )
            })
            .flat_map(|b| b.instructions.iter())
            .filter(|i| matches!(i, Instruction::IntrinsicCall { .. }))
            .count();
        let after_loop_intrinsics: usize = get_function(&after, "main")
            .blocks
            .iter()
            .filter(|b| {
                matches!(
                    b.role,
                    BlockRole::LoopStart(_) | BlockRole::LoopBody(_) | BlockRole::LoopContinue(_)
                )
            })
            .flat_map(|b| b.instructions.iter())
            .filter(|i| matches!(i, Instruction::IntrinsicCall { .. }))
            .count();
        assert!(
            after_loop_intrinsics < before_loop_intrinsics,
            "LICM should hoist sqrt(x) when x is loop-invariant: before={}, after={}",
            before_loop_intrinsics,
            after_loop_intrinsics,
        );
    }

    #[test]
    fn licm_hoists_static_load_without_store() {
        let before = build_ssa_unoptimized(
            r#"
            device out: d0;
            static THRESHOLD: f64 = 10.0;
            fn main() {
                loop {
                    let t: f64 = THRESHOLD;
                    out.Setting = t;
                    yield;
                }
            }
            "#,
        );
        let after = build_optimized(
            r#"
            device out: d0;
            static THRESHOLD: f64 = 10.0;
            fn main() {
                loop {
                    let t: f64 = THRESHOLD;
                    out.Setting = t;
                    yield;
                }
            }
            "#,
        );
        let before_loop_loads: usize = get_function(&before, "main")
            .blocks
            .iter()
            .filter(|b| {
                matches!(
                    b.role,
                    BlockRole::LoopStart(_) | BlockRole::LoopBody(_) | BlockRole::LoopContinue(_)
                )
            })
            .flat_map(|b| b.instructions.iter())
            .filter(|i| matches!(i, Instruction::LoadStatic { .. }))
            .count();
        let after_loop_loads: usize = get_function(&after, "main")
            .blocks
            .iter()
            .filter(|b| {
                matches!(
                    b.role,
                    BlockRole::LoopStart(_) | BlockRole::LoopBody(_) | BlockRole::LoopContinue(_)
                )
            })
            .flat_map(|b| b.instructions.iter())
            .filter(|i| matches!(i, Instruction::LoadStatic { .. }))
            .count();
        assert!(
            after_loop_loads < before_loop_loads,
            "LICM should hoist LoadStatic when no StoreStatic in loop: before={}, after={}",
            before_loop_loads,
            after_loop_loads,
        );
    }

    #[test]
    fn licm_does_not_hoist_static_load_with_store() {
        let program = build_optimized(
            r#"
            device out: d0;
            device sensor: d1;
            static mut counter: f64 = 0.0;
            fn main() {
                loop {
                    counter = counter + 1.0;
                    out.Setting = counter;
                    yield;
                }
            }
            "#,
        );
        let main = get_function(&program, "main");
        let loop_has_store_static = main
            .blocks
            .iter()
            .filter(|b| {
                matches!(
                    b.role,
                    BlockRole::LoopStart(_) | BlockRole::LoopBody(_) | BlockRole::LoopContinue(_)
                )
            })
            .any(|b| {
                b.instructions
                    .iter()
                    .any(|i| matches!(i, Instruction::StoreStatic { .. }))
            });
        assert!(
            loop_has_store_static,
            "StoreStatic must remain inside the loop"
        );
    }

    #[test]
    fn sccp_correlated_equality() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn check(x: i53) {
                if x == 5 {
                    out.Setting = x;
                }
            }
            fn main() { check(5); }
            "#,
        );
        let main = get_function(&program, "main");
        let has_constant_5 = main.blocks.iter().any(|block| {
            block.instructions.iter().any(|i| {
                matches!(
                    i,
                    Instruction::Assign {
                        operation: Operation::Constant(v),
                        ..
                    } if *v == 5.0
                )
            })
        });
        assert!(
            has_constant_5,
            "SCCP should propagate x=5 into the true branch of x == 5"
        );
    }

    #[test]
    fn sccp_unreachable_branch_eliminated() {
        let program = build_optimized(
            r#"
            device out: d0;
            fn main() {
                let x: i53 = 3;
                if x == 5 {
                    out.Setting = 1;
                } else {
                    out.Setting = 2;
                }
            }
            "#,
        );
        let main = get_function(&program, "main");
        let store_count: usize = main
            .blocks
            .iter()
            .flat_map(|b| b.instructions.iter())
            .filter(|i| matches!(i, Instruction::StoreDevice { .. }))
            .count();
        assert_eq!(
            store_count, 1,
            "SCCP should eliminate the unreachable branch, leaving one store"
        );
    }
}
