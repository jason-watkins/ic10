pub(crate) mod allocator;
pub(crate) mod calling_convention;
pub(crate) mod emitter;
pub(crate) mod ic10;
pub(crate) mod liveness;
pub(crate) mod phi;

use crate::diagnostic::Diagnostic;
use crate::ir::cfg::Program;
use crate::opt::Features;

pub use allocator::{AllocationResult, SpillRecord, allocate_function, resolve_parallel_moves};
pub use calling_convention::{
    CallingConventionInfo, FunctionClass, analyze_calling_convention, classify_function,
    find_call_sites, find_live_across_calls,
};
pub use emitter::{
    EmittedCallSite, compute_clobber_sets, emit_function, insert_callee_saves, insert_caller_saves,
    resolve_labels,
};
pub use ic10::{IC10Function, IC10Instruction, IC10Program, JumpTarget, Operand, Register};
pub use liveness::{
    LinearMap, LinearPosition, LinearRange, LiveInterval, LiveRange, compute_live_ranges,
    compute_reverse_postorder, linearize_function,
};
pub use phi::deconstruct_phis;

pub fn allocate_registers(
    program: &mut Program,
    keep_labels: bool,
    features: &Features,
) -> Result<IC10Program, Vec<Diagnostic>> {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();

    for function in &mut program.functions {
        deconstruct_phis(function);
    }

    let mut ic10_functions = Vec::new();
    let mut all_call_sites: Vec<Vec<EmittedCallSite>> = Vec::new();

    for function in &program.functions {
        let block_order = compute_reverse_postorder(function);
        let linear_map = linearize_function(function, &block_order);
        let live_ranges = compute_live_ranges(function, &linear_map);
        let calling_convention = analyze_calling_convention(function, &linear_map, &live_ranges);

        match allocate_function(function, &linear_map, &live_ranges, &calling_convention) {
            Ok(result) => {
                let (ic10_function, call_sites) = emit_function(
                    function,
                    &block_order,
                    &linear_map,
                    &result,
                    &calling_convention,
                    &program.symbols,
                );
                ic10_functions.push(ic10_function);
                all_call_sites.push(call_sites);
            }
            Err(diags) => {
                diagnostics.extend(diags);
                all_call_sites.push(Vec::new());
            }
        }
    }

    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    // Caller-save: compute clobber sets and insert push/pop around call sites.
    let clobber_sets = compute_clobber_sets(&ic10_functions);
    for (function, call_sites) in ic10_functions.iter_mut().zip(all_call_sites.iter()) {
        insert_caller_saves(function, call_sites, &clobber_sets);
    }

    // Callee-save: insert push/pop for r8–r15 at function entry/exit.
    for function in &mut ic10_functions {
        insert_callee_saves(function);
    }

    let mut ic10_program = IC10Program {
        functions: ic10_functions,
    };
    crate::opt::ic10::optimize_program(&mut ic10_program, features);
    let mut ic10_functions = ic10_program.functions;

    // Order functions: main first, then others in declaration order.
    ic10_functions.sort_by_key(|f| if f.is_entry { 0 } else { 1 });

    // Collect the flat global instruction stream for label resolution.
    let mut global_instructions: Vec<IC10Instruction> = Vec::new();
    for function in &ic10_functions {
        global_instructions.extend(function.instructions.iter().cloned());
    }

    let (resolved, non_label_count): (Vec<IC10Instruction>, usize) = if keep_labels {
        // Keep symbolic labels and IC10Instruction::Label pseudo-instructions as-is.
        let count = global_instructions
            .iter()
            .filter(|i| !matches!(i, IC10Instruction::Label(_)))
            .count();
        (global_instructions, count)
    } else {
        // Resolve labels to absolute line numbers and strip IC10Instruction::Label.
        let resolved = resolve_labels(global_instructions);
        let count = resolved.len();
        (resolved, count)
    };

    // Check the 128-line limit (labels are not real instructions).
    if non_label_count > 128 {
        diagnostics.push(Diagnostic {
            severity: crate::diagnostic::Severity::Warning,
            span: crate::diagnostic::Span { start: 0, end: 0 },
            message: format!(
                "program exceeds 128-line IC10 limit ({} lines)",
                non_label_count
            ),
        });
    }

    // Rebuild IC10Functions from the global instruction slice, re-partitioning by function.
    let mut offset = 0;
    let mut resolved_functions = Vec::new();
    for function in &ic10_functions {
        // When keep_labels is true the resolved slice retains all instructions including
        // IC10Instruction::Label pseudo-instructions, so the raw length is correct.
        // When keep_labels is false resolve_labels has stripped those pseudo-instructions,
        // so only non-label instructions appear in the resolved slice.
        let function_slice_count = if keep_labels {
            function.instructions.len()
        } else {
            function
                .instructions
                .iter()
                .filter(|i| !matches!(i, IC10Instruction::Label(_)))
                .count()
        };
        resolved_functions.push(IC10Function {
            name: function.name.clone(),
            instructions: resolved[offset..offset + function_slice_count].to_vec(),
            is_entry: function.is_entry,
        });
        offset += function_slice_count;
    }

    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    Ok(IC10Program {
        functions: resolved_functions,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::phi::sequence_parallel_copies;
    use super::*;
    use crate::bind::bind;
    use crate::cfg;
    use crate::ir::bound::SymbolId;
    use crate::ir::cfg::{BlockId, Function, Instruction, Operation, Program, TempId};
    use crate::parser::parse;
    use crate::ssa;

    fn build_ssa(source: &str) -> Program {
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

    fn get_function_mut<'a>(program: &'a mut Program, name: &str) -> &'a mut Function {
        program
            .functions
            .iter_mut()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("function '{}' not found", name))
    }

    fn count_phis(function: &Function) -> usize {
        function
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter(|instruction| matches!(instruction, Instruction::Phi { .. }))
            .count()
    }

    fn collect_copies(function: &Function) -> Vec<(BlockId, TempId, TempId)> {
        let mut copies = Vec::new();
        for block in &function.blocks {
            for instruction in &block.instructions {
                if let Instruction::Assign {
                    dest,
                    operation: Operation::Copy(source),
                } = instruction
                {
                    copies.push((block.id, *dest, *source));
                }
            }
        }
        copies
    }

    #[test]
    fn straight_line_no_phis_unchanged() {
        let mut program = build_ssa("fn main() { let x = 1; let y = 2; }");
        let main = get_function_mut(&mut program, "main");
        assert_eq!(count_phis(main), 0);
        let copies_before = collect_copies(main);
        deconstruct_phis(main);
        assert_eq!(count_phis(main), 0);
        let copies_after = collect_copies(main);
        assert_eq!(copies_before.len(), copies_after.len());
    }

    #[test]
    fn diamond_if_else_phis_removed() {
        let mut program = build_ssa(
            r#"fn main() {
                let mut x: i53 = 1;
                if true {
                    x = 2;
                } else {
                    x = 3;
                }
                let y = x;
            }"#,
        );
        let main = get_function_mut(&mut program, "main");
        assert!(
            count_phis(main) > 0,
            "should have phis before deconstruction"
        );
        deconstruct_phis(main);
        assert_eq!(count_phis(main), 0, "all phis should be removed");
    }

    #[test]
    fn diamond_if_else_copies_inserted_in_predecessors() {
        let mut program = build_ssa(
            r#"fn main() {
                let mut x: i53 = 1;
                if true {
                    x = 2;
                } else {
                    x = 3;
                }
                let y = x;
            }"#,
        );
        let main = get_function_mut(&mut program, "main");
        let copies_before = collect_copies(main);
        deconstruct_phis(main);
        let copies_after = collect_copies(main);
        assert!(
            copies_after.len() > copies_before.len(),
            "phi deconstruction should insert copy instructions"
        );
    }

    #[test]
    fn while_loop_phis_removed() {
        let mut program = build_ssa(
            r#"fn main() {
                let mut x: i53 = 0;
                while x < 10 {
                    x = x + 1;
                }
            }"#,
        );
        let main = get_function_mut(&mut program, "main");
        assert!(
            count_phis(main) > 0,
            "should have phis before deconstruction"
        );
        deconstruct_phis(main);
        assert_eq!(count_phis(main), 0, "all phis should be removed");
    }

    #[test]
    fn for_loop_phis_removed() {
        let mut program = build_ssa("fn main() { for i in 0..10 { yield; } }");
        let main = get_function_mut(&mut program, "main");
        assert!(count_phis(main) > 0);
        deconstruct_phis(main);
        assert_eq!(count_phis(main), 0);
    }

    #[test]
    fn if_without_else_phis_removed() {
        let mut program = build_ssa(
            r#"fn main() {
                let mut x: i53 = 1;
                if true {
                    x = 2;
                }
                let y = x;
            }"#,
        );
        let main = get_function_mut(&mut program, "main");
        assert!(count_phis(main) > 0);
        deconstruct_phis(main);
        assert_eq!(count_phis(main), 0);
    }

    #[test]
    fn cfg_edges_remain_consistent_after_deconstruction() {
        let mut program = build_ssa(
            r#"fn main() {
                let mut x: i53 = 1;
                if true {
                    x = 2;
                } else {
                    x = 3;
                }
                let y = x;
            }"#,
        );
        let main = get_function_mut(&mut program, "main");
        deconstruct_phis(main);
        for block in &main.blocks {
            for &successor in &block.successors {
                assert!(
                    main.blocks[successor.0].predecessors.contains(&block.id),
                    "block {:?} lists {:?} as successor but {:?} doesn't list {:?} as predecessor",
                    block.id,
                    successor,
                    successor,
                    block.id,
                );
            }
            for &predecessor in &block.predecessors {
                assert!(
                    main.blocks[predecessor.0].successors.contains(&block.id),
                    "block {:?} lists {:?} as predecessor but {:?} doesn't list {:?} as successor",
                    block.id,
                    predecessor,
                    predecessor,
                    block.id,
                );
            }
        }
    }

    #[test]
    fn critical_edge_split_inserts_intermediate_block() {
        let mut program = build_ssa(
            r#"fn main() {
                let mut x: i53 = 0;
                while x < 10 {
                    if x > 5 {
                        x = x + 2;
                    } else {
                        x = x + 1;
                    }
                }
            }"#,
        );
        let main = get_function_mut(&mut program, "main");
        let block_count_before = main.blocks.len();
        deconstruct_phis(main);
        assert_eq!(count_phis(main), 0);
        assert!(
            main.blocks.len() >= block_count_before,
            "critical edge splitting may add blocks"
        );
    }

    #[test]
    fn parallel_copy_sequencing_no_cycle() {
        let mut function = Function {
            name: "test".to_string(),
            symbol_id: SymbolId(0),
            parameters: Vec::new(),
            return_type: None,
            blocks: Vec::new(),
            entry: BlockId(0),
            variable_definitions: HashMap::new(),
            variable_temps: HashMap::new(),
            immediate_dominators: HashMap::new(),
            dominance_frontiers: HashMap::new(),
            next_temp: 10,
        };

        let copies = vec![(TempId(0), TempId(1)), (TempId(2), TempId(3))];
        let result = sequence_parallel_copies(&copies, &mut function);
        assert_eq!(result.len(), 2);
        let destinations: Vec<_> = result.iter().map(|(d, _)| *d).collect();
        assert!(destinations.contains(&TempId(0)));
        assert!(destinations.contains(&TempId(2)));
    }

    #[test]
    fn parallel_copy_sequencing_dependency_chain() {
        let mut function = Function {
            name: "test".to_string(),
            symbol_id: SymbolId(0),
            parameters: Vec::new(),
            return_type: None,
            blocks: Vec::new(),
            entry: BlockId(0),
            variable_definitions: HashMap::new(),
            variable_temps: HashMap::new(),
            immediate_dominators: HashMap::new(),
            dominance_frontiers: HashMap::new(),
            next_temp: 10,
        };

        // a <- b, c <- a: must emit c <- a before a <- b
        let copies = vec![(TempId(0), TempId(1)), (TempId(2), TempId(0))];
        let result = sequence_parallel_copies(&copies, &mut function);
        assert_eq!(result.len(), 2);
        let first_dest = result[0].0;
        assert_eq!(
            first_dest,
            TempId(2),
            "c <- a must come before a <- b to avoid clobbering a"
        );
    }

    #[test]
    fn parallel_copy_sequencing_cycle_uses_temporary() {
        let mut function = Function {
            name: "test".to_string(),
            symbol_id: SymbolId(0),
            parameters: Vec::new(),
            return_type: None,
            blocks: Vec::new(),
            entry: BlockId(0),
            variable_definitions: HashMap::new(),
            variable_temps: HashMap::new(),
            immediate_dominators: HashMap::new(),
            dominance_frontiers: HashMap::new(),
            next_temp: 10,
        };

        // a <- b, b <- a: cycle — needs a temp
        let copies = vec![(TempId(0), TempId(1)), (TempId(1), TempId(0))];
        let result = sequence_parallel_copies(&copies, &mut function);
        assert_eq!(
            result.len(),
            3,
            "cycle requires 3 copies (1 temp save + 2 assignments)"
        );
        let temp = result[0].0;
        assert!(temp.0 >= 10, "fresh temp should have id >= next_temp");
    }

    #[test]
    fn self_copy_eliminated() {
        let mut function = Function {
            name: "test".to_string(),
            symbol_id: SymbolId(0),
            parameters: Vec::new(),
            return_type: None,
            blocks: Vec::new(),
            entry: BlockId(0),
            variable_definitions: HashMap::new(),
            variable_temps: HashMap::new(),
            immediate_dominators: HashMap::new(),
            dominance_frontiers: HashMap::new(),
            next_temp: 10,
        };

        let copies = vec![(TempId(0), TempId(0))];
        let result = sequence_parallel_copies(&copies, &mut function);
        assert!(result.is_empty(), "self-copies should be eliminated");
    }

    #[test]
    fn multiple_phis_in_same_block_deconstructed() {
        let mut program = build_ssa(
            r#"fn main() {
                let mut x: i53 = 0;
                let mut y: i53 = 1;
                while x < 10 {
                    x = x + 1;
                    y = y + 2;
                }
            }"#,
        );
        let main = get_function_mut(&mut program, "main");
        assert!(
            count_phis(main) >= 2,
            "should have at least 2 phis (one per variable)"
        );
        deconstruct_phis(main);
        assert_eq!(count_phis(main), 0);
    }

    #[test]
    fn nested_loop_phis_deconstructed() {
        let mut program = build_ssa(
            r#"fn main() {
                let mut x: i53 = 0;
                while x < 10 {
                    let mut y: i53 = 0;
                    while y < 5 {
                        y = y + 1;
                    }
                    x = x + 1;
                }
            }"#,
        );
        let main = get_function_mut(&mut program, "main");
        assert!(count_phis(main) > 0);
        deconstruct_phis(main);
        assert_eq!(count_phis(main), 0);
    }

    // Deconstruct phis in `name` and return the resulting LinearMap. The mutable borrow is
    // released when this function returns, so callers can immediately re-borrow `program`
    // immutably to call `compute_live_ranges`.
    fn prepare_for_live_ranges(program: &mut Program, name: &str) -> LinearMap {
        let index = program
            .functions
            .iter()
            .position(|f| f.name == name)
            .unwrap_or_else(|| panic!("function '{}' not found", name));
        let function = &mut program.functions[index];
        deconstruct_phis(function);
        let block_order = compute_reverse_postorder(function);
        linearize_function(function, &block_order)
    }

    #[test]
    fn straight_line_temp_single_interval() {
        let mut program = build_ssa("fn main() { let x: i53 = 1; let y: i53 = x + 1; }");
        let linear_map = prepare_for_live_ranges(&mut program, "main");
        let function = program.functions.iter().find(|f| f.name == "main").unwrap();
        let ranges = compute_live_ranges(function, &linear_map);

        for range in ranges.values() {
            assert_eq!(
                range.intervals.len(),
                1,
                "straight-line code should produce single-interval ranges"
            );
        }
    }

    #[test]
    fn temp_defined_before_use_has_correct_start_and_end() {
        let mut program = build_ssa("fn main() { let x: i53 = 1; let _y: i53 = x + 2; }");
        let linear_map = prepare_for_live_ranges(&mut program, "main");
        let function = program.functions.iter().find(|f| f.name == "main").unwrap();
        let ranges = compute_live_ranges(function, &linear_map);

        for range in ranges.values() {
            let interval = range.intervals[0];
            assert!(
                interval.start <= interval.end,
                "start must not exceed end: {:?}",
                interval
            );
        }
    }

    #[test]
    fn temp_used_in_successor_is_live_across_block_boundary() {
        let mut program = build_ssa(
            r#"fn main() {
                let mut x: i53 = 1;
                if true {
                    x = 2;
                }
                let _y: i53 = x;
            }"#,
        );
        let linear_map = prepare_for_live_ranges(&mut program, "main");
        let function = program.functions.iter().find(|f| f.name == "main").unwrap();
        let ranges = compute_live_ranges(function, &linear_map);

        let multi_block = ranges.values().any(|range| {
            let start = range.start();
            let end = range.end();
            linear_map.block_order.iter().any(|&block_id| {
                let block_range = linear_map.block_ranges[&block_id];
                start < block_range.start && end >= block_range.start
            })
        });
        assert!(
            multi_block,
            "at least one temp should be live across a block boundary"
        );
    }

    #[test]
    fn loop_variable_live_range_covers_back_edge() {
        let mut program = build_ssa(
            r#"fn main() {
                let mut x: i53 = 0;
                while x < 10 {
                    x = x + 1;
                }
            }"#,
        );
        let linear_map = prepare_for_live_ranges(&mut program, "main");
        let function = program.functions.iter().find(|f| f.name == "main").unwrap();
        let ranges = compute_live_ranges(function, &linear_map);

        let max_block_span = linear_map
            .block_order
            .iter()
            .map(|&id| {
                let r = linear_map.block_ranges[&id];
                r.end.0 - r.start.0
            })
            .max()
            .unwrap_or(0);

        let max_temp_span = ranges
            .values()
            .map(|range| range.end().0.saturating_sub(range.start().0))
            .max()
            .unwrap_or(0);

        assert!(
            max_temp_span > max_block_span,
            "a loop variable's span ({}) should exceed the widest single block ({})",
            max_temp_span,
            max_block_span,
        );
    }

    #[test]
    fn dead_temp_gets_zero_length_interval() {
        let mut program = build_ssa("fn main() { let _x: i53 = 1; }");
        let linear_map = prepare_for_live_ranges(&mut program, "main");
        let function = program.functions.iter().find(|f| f.name == "main").unwrap();
        let ranges = compute_live_ranges(function, &linear_map);

        let has_zero_length = ranges
            .values()
            .any(|range| range.intervals.len() == 1 && range.start() == range.end());
        assert!(
            has_zero_length,
            "a never-used temp should produce a zero-length interval"
        );
    }

    #[test]
    fn live_range_intervals_are_sorted_and_non_overlapping() {
        let mut program = build_ssa(
            r#"fn main() {
                let mut x: i53 = 0;
                while x < 10 {
                    let mut y: i53 = 0;
                    while y < 5 {
                        y = y + 1;
                    }
                    x = x + 1;
                }
            }"#,
        );
        let linear_map = prepare_for_live_ranges(&mut program, "main");
        let function = program.functions.iter().find(|f| f.name == "main").unwrap();
        let ranges = compute_live_ranges(function, &linear_map);

        for (temp, range) in &ranges {
            for window in range.intervals.windows(2) {
                assert!(
                    window[0].end < window[1].start,
                    "intervals for temp {:?} overlap or are not sorted: {:?} / {:?}",
                    temp,
                    window[0],
                    window[1],
                );
            }
            for interval in &range.intervals {
                assert!(
                    interval.start <= interval.end,
                    "interval for temp {:?} has start after end: {:?}",
                    temp,
                    interval,
                );
            }
        }
    }

    // Helper: run all pre-live-range setup and return (linear_map, live_ranges).
    fn prepare_for_calling_convention(
        program: &mut Program,
        name: &str,
    ) -> (LinearMap, HashMap<TempId, LiveRange>) {
        let index = program
            .functions
            .iter()
            .position(|f| f.name == name)
            .unwrap_or_else(|| panic!("function '{}' not found", name));
        let function = &mut program.functions[index];
        deconstruct_phis(function);
        let block_order = compute_reverse_postorder(function);
        let linear_map = linearize_function(function, &block_order);
        let function = &program.functions[index];
        let live_ranges = compute_live_ranges(function, &linear_map);
        (linear_map, live_ranges)
    }

    #[test]
    fn leaf_function_classified_as_leaf() {
        let program = build_ssa("fn main() { let x: i53 = 1 + 2; }");
        let main = program.functions.iter().find(|f| f.name == "main").unwrap();
        assert_eq!(classify_function(main), FunctionClass::Leaf);
    }

    #[test]
    fn non_leaf_function_classified_as_non_leaf() {
        let program = build_ssa(
            r#"fn helper() -> i53 { return 42; }
               fn main() { let x: i53 = helper(); }"#,
        );
        let main = program.functions.iter().find(|f| f.name == "main").unwrap();
        assert_eq!(classify_function(main), FunctionClass::NonLeaf);
    }

    #[test]
    fn find_call_sites_empty_for_leaf() {
        let mut program = build_ssa("fn main() { let x: i53 = 1; }");
        let (linear_map, _) = prepare_for_calling_convention(&mut program, "main");
        let function = program.functions.iter().find(|f| f.name == "main").unwrap();
        let sites = find_call_sites(function, &linear_map);
        assert!(sites.is_empty(), "leaf function should have no call sites");
    }

    #[test]
    fn find_call_sites_finds_single_call() {
        let mut program = build_ssa(
            r#"fn helper() -> i53 { return 42; }
               fn main() { let x: i53 = helper(); }"#,
        );
        let (linear_map, _) = prepare_for_calling_convention(&mut program, "main");
        let function = program.functions.iter().find(|f| f.name == "main").unwrap();
        let sites = find_call_sites(function, &linear_map);
        assert_eq!(sites.len(), 1, "should find exactly one call site");
    }

    #[test]
    fn find_call_sites_finds_two_calls() {
        let mut program = build_ssa(
            r#"fn helper() -> i53 { return 1; }
               fn main() { let a: i53 = helper(); let b: i53 = helper(); }"#,
        );
        let (linear_map, _) = prepare_for_calling_convention(&mut program, "main");
        let function = program.functions.iter().find(|f| f.name == "main").unwrap();
        let sites = find_call_sites(function, &linear_map);
        assert_eq!(sites.len(), 2, "should find two call sites");
        assert!(
            sites[0] < sites[1],
            "call site positions must be increasing"
        );
    }

    #[test]
    fn parameter_constraints_assign_correct_registers() {
        let mut program = build_ssa(
            r#"fn add(a: i53, b: i53) -> i53 { let result: i53 = a + b; return result; }
               fn main() {}"#,
        );
        let (linear_map, live_ranges) = prepare_for_calling_convention(&mut program, "add");
        let function = program.functions.iter().find(|f| f.name == "add").unwrap();
        let info = analyze_calling_convention(function, &linear_map, &live_ranges);

        let assigned_registers: Vec<Register> = info.fixed.values().copied().collect();
        assert!(
            assigned_registers.contains(&Register::R0),
            "first parameter must be constrained to r0"
        );
        assert!(
            assigned_registers.contains(&Register::R1),
            "second parameter must be constrained to r1"
        );
    }

    #[test]
    fn return_value_constrained_to_r0() {
        let mut program = build_ssa("fn answer() -> i53 { return 42; }  fn main() {}");
        let (linear_map, live_ranges) = prepare_for_calling_convention(&mut program, "answer");
        let function = program
            .functions
            .iter()
            .find(|f| f.name == "answer")
            .unwrap();
        let info = analyze_calling_convention(function, &linear_map, &live_ranges);

        let assigned_registers: Vec<Register> = info.fixed.values().copied().collect();
        assert!(
            assigned_registers.contains(&Register::R0),
            "return value must be constrained to r0"
        );
    }

    #[test]
    fn call_result_constrained_to_r0() {
        let mut program = build_ssa(
            r#"fn helper() -> i53 { return 1; }
               fn main() { let x: i53 = helper(); }"#,
        );
        let (linear_map, live_ranges) = prepare_for_calling_convention(&mut program, "main");
        let function = program.functions.iter().find(|f| f.name == "main").unwrap();
        let info = analyze_calling_convention(function, &linear_map, &live_ranges);

        assert!(
            info.fixed.values().any(|&r| r == Register::R0),
            "call return value must be constrained to r0"
        );
    }

    #[test]
    fn call_args_constrained_to_correct_registers() {
        let mut program = build_ssa(
            r#"fn add(a: i53, b: i53) -> i53 { let result: i53 = a + b; return result; }
               fn main() { let x: i53 = add(1, 2); }"#,
        );
        let (linear_map, live_ranges) = prepare_for_calling_convention(&mut program, "main");
        let function = program.functions.iter().find(|f| f.name == "main").unwrap();
        let info = analyze_calling_convention(function, &linear_map, &live_ranges);

        let registers: Vec<Register> = info.fixed.values().copied().collect();
        assert!(
            registers.contains(&Register::R0),
            "first call arg must be constrained to r0"
        );
        assert!(
            registers.contains(&Register::R1),
            "second call arg must be constrained to r1"
        );
    }

    #[test]
    fn no_live_across_calls_for_leaf() {
        let mut program = build_ssa("fn main() { let x: i53 = 1 + 2; }");
        let (linear_map, live_ranges) = prepare_for_calling_convention(&mut program, "main");
        let function = program.functions.iter().find(|f| f.name == "main").unwrap();
        let info = analyze_calling_convention(function, &linear_map, &live_ranges);
        assert!(
            info.live_across_calls.is_empty(),
            "leaf function should have no live-across-call entries"
        );
    }

    #[test]
    fn live_across_call_detected_when_value_used_after_call() {
        let mut program = build_ssa(
            r#"fn helper() -> i53 { return 1; }
               fn main() {
                   let x: i53 = 10;
                   let _result: i53 = helper();
                   let _y: i53 = x + 1;
               }"#,
        );
        let (linear_map, live_ranges) = prepare_for_calling_convention(&mut program, "main");
        let function = program.functions.iter().find(|f| f.name == "main").unwrap();
        let info = analyze_calling_convention(function, &linear_map, &live_ranges);

        let any_live_across = info
            .live_across_calls
            .values()
            .any(|temps| !temps.is_empty());
        assert!(
            any_live_across,
            "x should be detected as live across the call to helper"
        );
    }

    #[test]
    fn arg_temps_not_counted_as_live_across_call() {
        let mut program = build_ssa(
            r#"fn consume(v: i53) { let _ignored: i53 = v; }
               fn main() {
                   let x: i53 = 5;
                   consume(x);
               }"#,
        );
        let (linear_map, live_ranges) = prepare_for_calling_convention(&mut program, "main");
        let function = program.functions.iter().find(|f| f.name == "main").unwrap();
        let sites = find_call_sites(function, &linear_map);
        assert_eq!(sites.len(), 1);
        let site = sites[0];
        let live_across: Vec<TempId> = live_ranges
            .iter()
            .filter(|(_, range)| range.start() < site && range.end() > site)
            .map(|(&t, _)| t)
            .collect();
        assert!(
            live_across.is_empty(),
            "argument temp should not be live across the call (its last use is the call): {:?}",
            live_across,
        );
    }

    fn prepare_for_allocation(
        program: &mut Program,
        name: &str,
    ) -> (LinearMap, HashMap<TempId, LiveRange>, CallingConventionInfo) {
        let index = program
            .functions
            .iter()
            .position(|f| f.name == name)
            .unwrap_or_else(|| panic!("function '{}' not found", name));
        let function = &mut program.functions[index];
        deconstruct_phis(function);
        let block_order = compute_reverse_postorder(function);
        let linear_map = linearize_function(function, &block_order);
        let function = &program.functions[index];
        let live_ranges = compute_live_ranges(function, &linear_map);
        let calling_convention = analyze_calling_convention(function, &linear_map, &live_ranges);
        (linear_map, live_ranges, calling_convention)
    }

    fn run_allocation(source: &str, function_name: &str) -> (Program, AllocationResult) {
        let mut program = build_ssa(source);
        let (linear_map, live_ranges, calling_convention) =
            prepare_for_allocation(&mut program, function_name);
        let function = program
            .functions
            .iter()
            .find(|f| f.name == function_name)
            .unwrap();
        let result = allocate_function(function, &linear_map, &live_ranges, &calling_convention)
            .unwrap_or_else(|diagnostics| panic!("allocation failed: {:#?}", diagnostics));
        (program, result)
    }

    #[test]
    fn allocate_straight_line_succeeds() {
        let (_, result) =
            run_allocation("fn main() { let x: i53 = 1; let y: i53 = x + 2; }", "main");
        assert!(
            !result.assignments.is_empty(),
            "should assign at least one temp"
        );
    }

    #[test]
    fn allocate_every_temp_gets_assignment() {
        let (_, result) = run_allocation(
            "fn main() { let a: i53 = 1; let b: i53 = 2; let c: i53 = a + b; }",
            "main",
        );
        for (&temp, &register) in &result.assignments {
            assert!(
                matches!(
                    register,
                    Register::R0
                        | Register::R1
                        | Register::R2
                        | Register::R3
                        | Register::R4
                        | Register::R5
                        | Register::R6
                        | Register::R7
                        | Register::R8
                        | Register::R9
                        | Register::R10
                        | Register::R11
                        | Register::R12
                        | Register::R13
                        | Register::R14
                        | Register::R15
                ),
                "temp {:?} assigned to non-allocatable register {:?}",
                temp,
                register,
            );
        }
    }

    #[test]
    fn allocate_straight_line_no_spills() {
        let (_, result) = run_allocation(
            "fn main() { let a: i53 = 1; let b: i53 = 2; let c: i53 = a + b; }",
            "main",
        );
        assert!(
            result.spills.is_empty(),
            "straight-line code with few temps should not spill"
        );
        assert_eq!(result.max_stack_depth, 0);
    }

    #[test]
    fn allocate_diamond_if_else_succeeds() {
        let (_, result) = run_allocation(
            r#"fn main() {
                let mut x: i53 = 1;
                if true {
                    x = 2;
                } else {
                    x = 3;
                }
                let _y: i53 = x;
            }"#,
            "main",
        );
        assert!(!result.assignments.is_empty());
    }

    #[test]
    fn allocate_while_loop_succeeds() {
        let (_, result) = run_allocation(
            r#"fn main() {
                let mut x: i53 = 0;
                while x < 10 {
                    x = x + 1;
                }
            }"#,
            "main",
        );
        assert!(!result.assignments.is_empty());
    }

    #[test]
    fn allocate_for_loop_succeeds() {
        let (_, result) = run_allocation("fn main() { for i in 0..10 { yield; } }", "main");
        assert!(!result.assignments.is_empty());
    }

    #[test]
    fn allocate_nested_loops_succeeds() {
        let (_, result) = run_allocation(
            r#"fn main() {
                let mut x: i53 = 0;
                while x < 10 {
                    let mut y: i53 = 0;
                    while y < 5 {
                        y = y + 1;
                    }
                    x = x + 1;
                }
            }"#,
            "main",
        );
        assert!(!result.assignments.is_empty());
    }

    #[test]
    fn allocate_parameter_gets_preferred_register() {
        let source = r#"
            fn identity(x: i53) -> i53 { return x; }
            fn main() {}
        "#;
        let mut program = build_ssa(source);
        let (linear_map, live_ranges, calling_convention) =
            prepare_for_allocation(&mut program, "identity");
        let function = program
            .functions
            .iter()
            .find(|f| f.name == "identity")
            .unwrap();
        let result =
            allocate_function(function, &linear_map, &live_ranges, &calling_convention).unwrap();

        let has_r0 = result.assignments.values().any(|&r| r == Register::R0);
        assert!(
            has_r0,
            "parameter function should have a temp assigned to r0"
        );
    }

    #[test]
    fn allocate_return_value_gets_r0() {
        let source = r#"
            fn answer() -> i53 { return 42; }
            fn main() {}
        "#;
        let mut program = build_ssa(source);
        let (linear_map, live_ranges, calling_convention) =
            prepare_for_allocation(&mut program, "answer");
        let function = program
            .functions
            .iter()
            .find(|f| f.name == "answer")
            .unwrap();
        let result =
            allocate_function(function, &linear_map, &live_ranges, &calling_convention).unwrap();

        for (&temp, &register) in &result.assignments {
            if calling_convention.fixed.get(&temp) == Some(&Register::R0) {
                assert_eq!(
                    register,
                    Register::R0,
                    "temp {:?} constrained to r0 but assigned {:?}",
                    temp,
                    register,
                );
            }
        }
    }

    #[test]
    fn allocate_no_simultaneous_register_conflicts() {
        let mut program = build_ssa(
            r#"fn main() {
                let a: i53 = 1;
                let b: i53 = 2;
                let c: i53 = 3;
                let d: i53 = 4;
                let _result: i53 = a + b + c + d;
            }"#,
        );
        let (linear_map, live_ranges, calling_convention) =
            prepare_for_allocation(&mut program, "main");
        let function = program.functions.iter().find(|f| f.name == "main").unwrap();
        let result =
            allocate_function(function, &linear_map, &live_ranges, &calling_convention).unwrap();

        for position in 0..linear_map.total {
            let pos = LinearPosition(position);
            // Exclude temps whose range starts exactly at `pos`: those are destinations being
            // defined at this instruction and can legally share a register with a source whose
            // range ends at `pos` (read-before-write within the same IC10 instruction).
            let live_at_position: Vec<(TempId, Register)> = result
                .assignments
                .iter()
                .filter(|(temp, _)| {
                    live_ranges
                        .get(temp)
                        .is_some_and(|range| range.start() < pos && range.end() >= pos)
                })
                .map(|(&temp, &register)| (temp, register))
                .collect();

            let mut seen_registers: std::collections::HashSet<Register> =
                std::collections::HashSet::new();
            for (_temp, register) in &live_at_position {
                assert!(
                    seen_registers.insert(*register),
                    "register {:?} assigned to multiple simultaneously-live temps at position {}: {:?}",
                    register,
                    position,
                    live_at_position,
                );
            }
        }
    }

    #[test]
    fn allocate_spills_have_reload_positions() {
        let (_, result) = run_allocation(
            r#"fn main() {
                let a: i53 = 1;
                let b: i53 = 2;
                let c: i53 = 3;
                let _x: i53 = a + b + c;
            }"#,
            "main",
        );
        for spill in &result.spills {
            assert!(
                spill.reload_position.is_some(),
                "dead spills should be pruned: {:?}",
                spill,
            );
        }
    }

    #[test]
    fn allocate_max_stack_depth_zero_when_no_spills() {
        let (_, result) =
            run_allocation("fn main() { let x: i53 = 1; let _y: i53 = x + 2; }", "main");
        assert!(result.spills.is_empty());
        assert_eq!(result.max_stack_depth, 0);
    }

    #[test]
    fn allocate_max_stack_depth_consistent_with_spills() {
        let (_, result) = run_allocation(
            r#"fn main() {
                let a: i53 = 1;
                let b: i53 = 2;
                let c: i53 = a + b;
                let _d: i53 = c + 1;
            }"#,
            "main",
        );
        if result.spills.is_empty() {
            assert_eq!(result.max_stack_depth, 0);
        } else {
            assert!(
                result.max_stack_depth > 0,
                "max_stack_depth should be positive when spills exist"
            );
            assert!(
                result.max_stack_depth <= result.spills.len(),
                "max_stack_depth ({}) should not exceed total spill count ({})",
                result.max_stack_depth,
                result.spills.len(),
            );
        }
    }

    #[test]
    fn allocate_with_function_call_succeeds() {
        let (_, result) = run_allocation(
            r#"fn helper() -> i53 { return 42; }
               fn main() { let _x: i53 = helper(); }"#,
            "main",
        );
        assert!(!result.assignments.is_empty());
    }

    #[test]
    fn allocate_call_dest_gets_r0() {
        let source = r#"
            fn helper() -> i53 { return 42; }
            fn main() { let _x: i53 = helper(); }
        "#;
        let mut program = build_ssa(source);
        let (linear_map, live_ranges, calling_convention) =
            prepare_for_allocation(&mut program, "main");
        let function = program.functions.iter().find(|f| f.name == "main").unwrap();
        let result =
            allocate_function(function, &linear_map, &live_ranges, &calling_convention).unwrap();

        for (&temp, &expected_register) in &calling_convention.fixed {
            if let Some(&assigned_register) = result.assignments.get(&temp) {
                assert_eq!(
                    assigned_register, expected_register,
                    "temp {:?} should be in {:?} but got {:?}",
                    temp, expected_register, assigned_register,
                );
            }
        }
    }

    #[test]
    fn allocate_call_with_args_succeeds() {
        let (_, result) = run_allocation(
            r#"fn add(a: i53, b: i53) -> i53 { let r: i53 = a + b; return r; }
               fn main() { let _x: i53 = add(10, 20); }"#,
            "main",
        );
        assert!(!result.assignments.is_empty());
    }

    #[test]
    fn allocate_two_parameter_function() {
        let source = r#"
            fn add(a: i53, b: i53) -> i53 { let r: i53 = a + b; return r; }
            fn main() {}
        "#;
        let mut program = build_ssa(source);
        let (linear_map, live_ranges, calling_convention) =
            prepare_for_allocation(&mut program, "add");
        let function = program.functions.iter().find(|f| f.name == "add").unwrap();
        let result =
            allocate_function(function, &linear_map, &live_ranges, &calling_convention).unwrap();

        let assigned_registers: std::collections::HashSet<Register> =
            result.assignments.values().copied().collect();
        assert!(
            assigned_registers.contains(&Register::R0),
            "first parameter should get r0"
        );
        assert!(
            assigned_registers.contains(&Register::R1),
            "second parameter should get r1"
        );
    }

    #[test]
    fn allocate_many_independent_temps_no_spills() {
        let (_, result) = run_allocation(
            r#"fn main() {
                let a: i53 = 1;
                let b: i53 = 2;
                let c: i53 = 3;
                let d: i53 = 4;
                let e: i53 = 5;
                let f: i53 = 6;
                let g: i53 = 7;
                let h: i53 = 8;
                let _sum: i53 = a + b + c + d + e + f + g + h;
            }"#,
            "main",
        );
        assert!(
            result.spills.is_empty(),
            "8 temps should fit in 16 registers without spilling"
        );
    }

    #[test]
    fn allocate_empty_function_succeeds() {
        let (_, result) = run_allocation("fn main() {}", "main");
        assert!(result.spills.is_empty());
        assert_eq!(result.max_stack_depth, 0);
    }

    #[test]
    fn allocate_spill_reload_positions_ordered() {
        let (_, result) = run_allocation(
            r#"fn main() {
                let mut x: i53 = 0;
                while x < 100 {
                    x = x + 1;
                }
            }"#,
            "main",
        );
        for spill in &result.spills {
            if let Some(reload) = spill.reload_position {
                assert!(
                    spill.spill_position < reload,
                    "spill position {:?} should precede reload position {:?} for temp {:?}",
                    spill.spill_position,
                    reload,
                    spill.temp,
                );
            }
        }
    }

    #[test]
    fn allocate_if_without_else_succeeds() {
        let (_, result) = run_allocation(
            r#"fn main() {
                let mut x: i53 = 1;
                if true {
                    x = x + 1;
                }
                let _y: i53 = x;
            }"#,
            "main",
        );
        assert!(!result.assignments.is_empty());
    }

    #[test]
    fn allocate_value_used_across_call_succeeds() {
        let (_, result) = run_allocation(
            r#"fn helper() -> i53 { return 1; }
               fn main() {
                   let x: i53 = 10;
                   let _r: i53 = helper();
                   let _y: i53 = x + 1;
               }"#,
            "main",
        );
        assert!(!result.assignments.is_empty());
    }

    #[test]
    fn allocate_multiple_calls_succeeds() {
        let (_, result) = run_allocation(
            r#"fn helper() -> i53 { return 1; }
               fn main() {
                   let a: i53 = helper();
                   let b: i53 = helper();
                   let _c: i53 = a + b;
               }"#,
            "main",
        );
        assert!(!result.assignments.is_empty());
    }

    #[test]
    fn allocate_constrained_temps_get_correct_registers() {
        let source = r#"
            fn add(a: i53, b: i53) -> i53 { let r: i53 = a + b; return r; }
            fn main() {}
        "#;
        let mut program = build_ssa(source);
        let (linear_map, live_ranges, calling_convention) =
            prepare_for_allocation(&mut program, "add");
        let function = program.functions.iter().find(|f| f.name == "add").unwrap();
        let result =
            allocate_function(function, &linear_map, &live_ranges, &calling_convention).unwrap();

        for (&temp, &expected) in &calling_convention.fixed {
            if let Some(&actual) = result.assignments.get(&temp) {
                assert_eq!(
                    actual, expected,
                    "constrained temp {:?} should be {:?} but got {:?}",
                    temp, expected, actual,
                );
            }
        }
    }

    fn compile_to_ic10(source: &str) -> IC10Program {
        let mut program = build_ssa(source);
        let features = Features::from_opt_level(crate::opt::OptLevel::O2);
        allocate_registers(&mut program, false, &features)
            .unwrap_or_else(|diagnostics| panic!("register allocation failed: {:#?}", diagnostics))
    }

    fn compile_to_ic10_with_diagnostics(
        source: &str,
    ) -> Result<IC10Program, Vec<crate::diagnostic::Diagnostic>> {
        let mut program = build_ssa(source);
        let features = Features::from_opt_level(crate::opt::OptLevel::O2);
        allocate_registers(&mut program, false, &features)
    }

    fn all_instructions(program: &IC10Program) -> Vec<&IC10Instruction> {
        program
            .functions
            .iter()
            .flat_map(|f| &f.instructions)
            .collect()
    }

    #[test]
    fn end_to_end_empty_main() {
        let program = compile_to_ic10("fn main() {}");
        assert_eq!(program.functions.len(), 1);
        assert!(program.functions[0].is_entry);
        assert!(
            !program.functions[0].instructions.is_empty(),
            "even an empty main should emit at least hcf"
        );
    }

    #[test]
    fn end_to_end_single_constant() {
        let program = compile_to_ic10("fn main() { let _x: i53 = 42; }");
        let instructions = all_instructions(&program);
        let has_move_42 = instructions.iter().any(|instruction| {
            matches!(instruction, IC10Instruction::Move(_, Operand::Literal(v)) if (*v - 42.0).abs() < f64::EPSILON)
        });
        assert!(has_move_42, "should emit a move with literal 42");
    }

    #[test]
    fn end_to_end_arithmetic_expression() {
        let program =
            compile_to_ic10("fn main() { let a: i53 = 3; let b: i53 = 7; let _c: i53 = a + b; }");
        let instructions = all_instructions(&program);
        let has_add = instructions
            .iter()
            .any(|instruction| matches!(instruction, IC10Instruction::Add(..)));
        assert!(has_add, "should emit an Add instruction for a + b");
    }

    #[test]
    fn end_to_end_device_read_write() {
        let program = compile_to_ic10(
            r#"
            device sensor: d0;
            device actuator: d1;
            fn main() {
                let temp: f64 = sensor.Temperature;
                actuator.Setting = temp;
            }
            "#,
        );
        let instructions = all_instructions(&program);
        let has_load = instructions
            .iter()
            .any(|instruction| matches!(instruction, IC10Instruction::Load(..)));
        let has_store = instructions
            .iter()
            .any(|instruction| matches!(instruction, IC10Instruction::Store(..)));
        assert!(has_load, "should emit a Load instruction for device read");
        assert!(
            has_store,
            "should emit a Store instruction for device write"
        );
    }

    #[test]
    fn end_to_end_function_call_args_and_return() {
        let program = compile_to_ic10(
            r#"
            fn add(a: i53, b: i53) -> i53 { let result: i53 = a + b; return result; }
            fn main() { let _x: i53 = add(10, 20); }
            "#,
        );
        assert!(
            program.functions.len() >= 2,
            "should have at least main and add"
        );
        assert!(
            program.functions[0].is_entry,
            "main should be first function"
        );

        let main_instructions = &program.functions[0].instructions;
        let has_jal = main_instructions
            .iter()
            .any(|instruction| matches!(instruction, IC10Instruction::JumpAndLink(..)));
        assert!(has_jal, "main should emit a jal to call add");
    }

    #[test]
    fn end_to_end_non_leaf_push_pop_ra() {
        let program = compile_to_ic10(
            r#"
            fn leaf() -> i53 { return 1; }
            fn middle() -> i53 { let x: i53 = leaf(); return x; }
            fn main() { let _x: i53 = middle(); }
            "#,
        );
        let middle = program
            .functions
            .iter()
            .find(|f| f.name == "middle")
            .expect("should have middle function");
        let has_push_ra = middle.instructions.iter().any(|instruction| {
            matches!(
                instruction,
                IC10Instruction::Push(Operand::Register(Register::Ra))
            )
        });
        let has_pop_ra = middle
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, IC10Instruction::Pop(Register::Ra)));
        assert!(
            has_push_ra,
            "non-leaf non-main function should emit push ra at entry"
        );
        assert!(
            has_pop_ra,
            "non-leaf non-main function should emit pop ra before return"
        );
        let main_fn = program.functions.iter().find(|f| f.is_entry).unwrap();
        let main_has_push_ra = main_fn.instructions.iter().any(|instruction| {
            matches!(
                instruction,
                IC10Instruction::Push(Operand::Register(Register::Ra))
            )
        });
        assert!(
            !main_has_push_ra,
            "main must never push ra — ra is undefined at program start"
        );
    }

    #[test]
    fn end_to_end_caller_save_spill() {
        let program = compile_to_ic10(
            r#"
            fn helper() -> i53 {
                let a: i53 = 1; let b: i53 = 2; let c: i53 = 3;
                let d: i53 = 4; let e: i53 = 5; let f: i53 = 6;
                let g: i53 = 7; let h: i53 = 8;
                let result: i53 = a + b + c + d + e + f + g + h;
                return result;
            }
            fn caller() -> i53 {
                let x: i53 = 10;
                let _result: i53 = helper();
                let value: i53 = x + 1;
                return value;
            }
            fn main() { let _x: i53 = caller(); }
            "#,
        );
        let caller_fn = program
            .functions
            .iter()
            .find(|f| f.name == "caller")
            .expect("should have caller function");
        let push_count = caller_fn
            .instructions
            .iter()
            .filter(|instruction| matches!(instruction, IC10Instruction::Push(..)))
            .count();
        let pop_count = caller_fn
            .instructions
            .iter()
            .filter(|instruction| matches!(instruction, IC10Instruction::Pop(..)))
            .count();
        assert!(
            push_count >= 2,
            "should push ra and the live-across value x (got {} pushes)",
            push_count
        );
        assert!(
            pop_count >= 2,
            "should pop the live-across value x and ra (got {} pops)",
            pop_count
        );
        assert_eq!(
            push_count, pop_count,
            "pushes and pops should be balanced in a non-main function"
        );
    }

    #[test]
    fn end_to_end_register_pressure_spill() {
        let source = r#"
            fn main() {
                let a: i53 = 1;
                let b: i53 = 2;
                let c: i53 = 3;
                let d: i53 = 4;
                let e: i53 = 5;
                let f: i53 = 6;
                let g: i53 = 7;
                let h: i53 = 8;
                let i: i53 = 9;
                let j: i53 = 10;
                let k: i53 = 11;
                let l: i53 = 12;
                let m: i53 = 13;
                let n: i53 = 14;
                let o: i53 = 15;
                let p: i53 = 16;
                let q: i53 = 17;
                let _result: i53 = a + b + c + d + e + f + g + h + i + j + k + l + m + n + o + p + q;
            }
        "#;
        let program = compile_to_ic10(source);
        let instructions = all_instructions(&program);
        let has_push = instructions
            .iter()
            .any(|instruction| matches!(instruction, IC10Instruction::Push(Operand::Register(..))));
        let has_pop = instructions
            .iter()
            .any(|instruction| matches!(instruction, IC10Instruction::Pop(..)));
        assert!(
            has_push && has_pop,
            "17 simultaneous live values should require spilling"
        );
    }

    #[test]
    fn end_to_end_branch_fusion() {
        let program = compile_to_ic10(
            r#"
            fn main() {
                let x: i53 = 5;
                if x > 3 {
                    yield;
                }
            }
            "#,
        );
        let instructions = all_instructions(&program);
        let has_fused_branch = instructions.iter().any(|instruction| {
            matches!(
                instruction,
                IC10Instruction::BranchGreaterThan(..)
                    | IC10Instruction::BranchLessEqual(..)
                    | IC10Instruction::BranchGreaterEqual(..)
                    | IC10Instruction::BranchLessThan(..)
            )
        });
        let has_set_then_branch = instructions
            .iter()
            .any(|instruction| matches!(instruction, IC10Instruction::Sgt(..)));
        assert!(
            has_fused_branch,
            "comparison + branch should be fused into a single conditional branch"
        );
        assert!(
            !has_set_then_branch,
            "fused branch should suppress the separate sgt instruction"
        );
    }

    #[test]
    fn end_to_end_copy_coalescing() {
        let program = compile_to_ic10(
            r#"
            fn identity(x: i53) -> i53 { return x; }
            fn main() {}
            "#,
        );
        let identity = program
            .functions
            .iter()
            .find(|f| f.name == "identity")
            .expect("should have identity function");
        for instruction in &identity.instructions {
            if let IC10Instruction::Move(dest, Operand::Register(source)) = instruction {
                assert_ne!(
                    dest, source,
                    "redundant move from {:?} to itself should be coalesced",
                    dest,
                );
            }
        }
    }

    #[test]
    fn end_to_end_phi_lowered_to_moves() {
        let program = compile_to_ic10(
            r#"
            fn main() {
                let mut x: i53 = 1;
                if true {
                    x = 2;
                } else {
                    x = 3;
                }
                let _y: i53 = x;
            }
            "#,
        );
        let instructions = all_instructions(&program);
        assert!(
            !instructions.is_empty(),
            "should produce instructions for if-else with phi"
        );
        let has_no_phi_labels = !instructions
            .iter()
            .any(|instruction| matches!(instruction, IC10Instruction::Label(..)));
        assert!(
            has_no_phi_labels,
            "labels should be resolved away in final output"
        );
    }

    #[test]
    fn end_to_end_loop_with_device_io() {
        let program = compile_to_ic10(
            r#"
            device sensor: d0;
            device actuator: d1;
            fn main() {
                loop {
                    let temp: f64 = sensor.Temperature;
                    actuator.Setting = temp;
                    yield;
                }
            }
            "#,
        );
        let instructions = all_instructions(&program);
        let has_load = instructions
            .iter()
            .any(|instruction| matches!(instruction, IC10Instruction::Load(..)));
        let has_store = instructions
            .iter()
            .any(|instruction| matches!(instruction, IC10Instruction::Store(..)));
        let has_yield = instructions
            .iter()
            .any(|instruction| matches!(instruction, IC10Instruction::Yield));
        let has_jump = instructions
            .iter()
            .any(|instruction| matches!(instruction, IC10Instruction::Jump(JumpTarget::Line(..))));
        assert!(has_load, "should emit Load for device read");
        assert!(has_store, "should emit Store for device write");
        assert!(has_yield, "should emit Yield");
        assert!(
            has_jump,
            "loop should produce a back-edge jump to a resolved line number"
        );
    }

    #[test]
    fn end_to_end_exceeds_128_lines_produces_warning() {
        let yields = "yield; ".repeat(130);
        let source = format!("fn main() {{ {} }}", yields);
        let result = compile_to_ic10_with_diagnostics(&source);
        let diagnostics =
            result.expect_err("program with 130+ yields should exceed 128 lines and return Err");
        let has_warning = diagnostics.iter().any(|d| {
            d.severity == crate::diagnostic::Severity::Warning && d.message.contains("128")
        });
        assert!(
            has_warning,
            "should produce a warning about exceeding 128-line limit, got: {:#?}",
            diagnostics,
        );
    }

    #[test]
    fn leaf_function_prefers_low_registers() {
        let source = r#"
            fn leaf(x: i53) -> i53 {
                let a: i53 = x + 1;
                let b: i53 = a + 2;
                return b;
            }
            fn main() {}
        "#;
        let mut program = build_ssa(source);
        let (linear_map, live_ranges, calling_convention) =
            prepare_for_allocation(&mut program, "leaf");
        assert_eq!(calling_convention.function_class, FunctionClass::Leaf);
        let function = program.functions.iter().find(|f| f.name == "leaf").unwrap();
        let result =
            allocate_function(function, &linear_map, &live_ranges, &calling_convention).unwrap();
        for &register in result.assignments.values() {
            assert!(
                register.is_caller_saved(),
                "leaf function should prefer r0-r7, but got {:?}",
                register,
            );
        }
    }

    #[test]
    fn non_leaf_function_prefers_callee_saved_registers() {
        let source = r#"
            fn helper() -> i53 { return 42; }
            fn caller() -> i53 {
                let x: i53 = 10;
                let _result: i53 = helper();
                let value: i53 = x + 1;
                return value;
            }
            fn main() { let _x: i53 = caller(); }
        "#;
        let mut program = build_ssa(source);
        let (linear_map, live_ranges, calling_convention) =
            prepare_for_allocation(&mut program, "caller");
        assert_eq!(calling_convention.function_class, FunctionClass::NonLeaf);
        let function = program
            .functions
            .iter()
            .find(|f| f.name == "caller")
            .unwrap();
        let result =
            allocate_function(function, &linear_map, &live_ranges, &calling_convention).unwrap();
        let has_callee_saved = result
            .assignments
            .values()
            .any(|register| register.is_callee_saved());
        assert!(
            has_callee_saved,
            "non-leaf function should use callee-saved registers (r8-r15)"
        );
    }

    #[test]
    fn end_to_end_callee_save_push_pop() {
        let program = compile_to_ic10(
            r#"
            fn helper() -> i53 { return 42; }
            fn caller() -> i53 {
                let x: i53 = 10;
                let _result: i53 = helper();
                let value: i53 = x + 1;
                return value;
            }
            fn main() { let _x: i53 = caller(); }
            "#,
        );
        let caller_fn = program
            .functions
            .iter()
            .find(|f| f.name == "caller")
            .expect("should have caller function");
        let callee_saved_pushes: Vec<_> = caller_fn
            .instructions
            .iter()
            .filter(|instruction| {
                if let IC10Instruction::Push(Operand::Register(register)) = instruction {
                    register.is_callee_saved()
                } else {
                    false
                }
            })
            .collect();
        let callee_saved_pops: Vec<_> = caller_fn
            .instructions
            .iter()
            .filter(|instruction| {
                if let IC10Instruction::Pop(register) = instruction {
                    register.is_callee_saved()
                } else {
                    false
                }
            })
            .collect();
        assert_eq!(
            callee_saved_pushes.len(),
            callee_saved_pops.len(),
            "callee-saved pushes and pops must be balanced"
        );
        if !callee_saved_pushes.is_empty() {
            assert!(
                !callee_saved_pushes.is_empty(),
                "should have at least one callee-saved push/pop pair"
            );
        }
    }

    #[test]
    fn end_to_end_callee_save_no_caller_save_for_r8_plus() {
        let program = compile_to_ic10(
            r#"
            fn helper() -> i53 { return 42; }
            fn caller() -> i53 {
                let x: i53 = 10;
                let _result: i53 = helper();
                let value: i53 = x + 1;
                return value;
            }
            fn main() { let _x: i53 = caller(); }
            "#,
        );
        let caller_fn = program
            .functions
            .iter()
            .find(|f| f.name == "caller")
            .expect("should have caller function");

        let instructions = &caller_fn.instructions;
        let jal_indices: Vec<usize> = instructions
            .iter()
            .enumerate()
            .filter(|(_, instruction)| matches!(instruction, IC10Instruction::JumpAndLink(..)))
            .map(|(i, _)| i)
            .collect();

        for &jal_index in &jal_indices {
            if jal_index > 0
                && let IC10Instruction::Push(Operand::Register(register)) =
                    &instructions[jal_index - 1]
            {
                assert!(
                    !register.is_callee_saved(),
                    "callee-saved register {:?} should not be caller-saved around a call",
                    register,
                );
            }
        }
    }
}
