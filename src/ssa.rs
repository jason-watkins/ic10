use std::collections::{HashMap, HashSet};

use crate::ir::bound::SymbolId;
use crate::ir::cfg::{
    BasicBlock, BlockId, Function, Instruction, Operation, Program, TempId, Terminator,
};

/// Convert all functions in a CFG program to pruned SSA form.
pub fn construct_program(program: &mut Program) {
    for function in &mut program.functions {
        construct(function);
    }
}

/// Convert a single CFG function into pruned SSA form.
///
/// Inserts phi functions at dominance-frontier merge points where a variable
/// is live-in, then renames all variable references so that every use sees
/// the correct reaching definition. After this pass, every `TempId` that
/// represents a variable definition is defined exactly once.
pub fn construct(function: &mut Function) {
    let definition_map = build_definition_map(function);
    if definition_map.is_empty() {
        return;
    }

    let variables_needing_phis = find_variables_needing_phis(function, &definition_map);
    if variables_needing_phis.is_empty() {
        return;
    }

    let liveness =
        compute_liveness_for_variables(function, &variables_needing_phis, &definition_map);

    let phi_defs = place_phi_functions(function, &variables_needing_phis, &liveness);

    rename_variables(function, &definition_map, &phi_defs);
}

/// Build an inverse mapping from definition TempId to SymbolId.
///
/// The CFG builder records `variable_definitions[symbol] = vec![(temp, block), ...]`.
/// This inverts that to `temp → symbol` so that when we encounter a TempId as an
/// operand, we can determine which variable it represents.
fn build_definition_map(function: &Function) -> HashMap<TempId, SymbolId> {
    let mut map = HashMap::new();
    for (symbol_id, definitions) in &function.variable_definitions {
        for &(temp_id, _block_id) in definitions {
            map.insert(temp_id, *symbol_id);
        }
    }
    map
}

/// Identify variables that need phi functions: those with definitions in 2+ distinct blocks.
fn find_variables_needing_phis(
    function: &Function,
    definition_map: &HashMap<TempId, SymbolId>,
) -> HashSet<SymbolId> {
    let _ = definition_map;
    let mut result = HashSet::new();
    for (symbol_id, definitions) in &function.variable_definitions {
        let distinct_blocks: HashSet<BlockId> =
            definitions.iter().map(|&(_temp, block)| block).collect();
        if distinct_blocks.len() >= 2 {
            result.insert(*symbol_id);
        }
    }
    result
}

/// Compute per-variable liveness using backward dataflow analysis.
///
/// Returns a map from SymbolId to the set of blocks where that variable is live-in.
/// A variable is live-in at block B if:
/// - B contains a use of the variable before any redefinition, OR
/// - The variable is live-out at B (live-in at some successor) and B does not define it.
fn compute_liveness_for_variables(
    function: &Function,
    variables: &HashSet<SymbolId>,
    definition_map: &HashMap<TempId, SymbolId>,
) -> HashMap<SymbolId, HashSet<BlockId>> {
    let mut liveness: HashMap<SymbolId, HashSet<BlockId>> = HashMap::new();

    for &variable in variables {
        let mut live_in: HashSet<BlockId> = HashSet::new();

        let mut changed = true;
        while changed {
            changed = false;
            for block in &function.blocks {
                if live_in.contains(&block.id) {
                    continue;
                }
                let uses_before_def =
                    block_uses_variable_before_def(block, variable, definition_map);
                let defines = block_defines_variable(block, variable, definition_map);

                let live_out = block
                    .successors
                    .iter()
                    .any(|successor| live_in.contains(successor));

                if uses_before_def || (live_out && !defines) {
                    live_in.insert(block.id);
                    changed = true;
                }
            }
        }

        liveness.insert(variable, live_in);
    }

    liveness
}

/// Check whether `block` uses `variable` (via a def-temp operand) before redefining it.
fn block_uses_variable_before_def(
    block: &BasicBlock,
    variable: SymbolId,
    definition_map: &HashMap<TempId, SymbolId>,
) -> bool {
    for instruction in &block.instructions {
        let uses = instruction_uses(instruction);
        for used_temp in &uses {
            if definition_map.get(used_temp) == Some(&variable) {
                return true;
            }
        }
        if let Some(dest) = instruction_dest(instruction)
            && definition_map.get(&dest) == Some(&variable)
        {
            return false;
        }
    }
    for used_temp in terminator_uses(&block.terminator) {
        if definition_map.get(&used_temp) == Some(&variable) {
            return true;
        }
    }
    false
}

/// Check whether `block` defines `variable`.
fn block_defines_variable(
    block: &BasicBlock,
    variable: SymbolId,
    definition_map: &HashMap<TempId, SymbolId>,
) -> bool {
    for instruction in &block.instructions {
        if let Some(dest) = instruction_dest(instruction)
            && definition_map.get(&dest) == Some(&variable)
        {
            return true;
        }
    }
    false
}

/// Collect all TempId operands used (read) by an instruction.
fn instruction_uses(instruction: &Instruction) -> Vec<TempId> {
    match instruction {
        Instruction::Assign { operation, .. } => match operation {
            Operation::Copy(source) => vec![*source],
            Operation::Constant(_) | Operation::Parameter { .. } => vec![],
            Operation::Binary { left, right, .. } => vec![*left, *right],
            Operation::Unary { operand, .. } => vec![*operand],
            Operation::Cast { operand, .. } => vec![*operand],
            Operation::Select {
                condition,
                if_true,
                if_false,
            } => vec![*condition, *if_true, *if_false],
        },
        Instruction::Phi { args, .. } => args.iter().map(|&(temp, _)| temp).collect(),
        Instruction::LoadDevice { .. } => vec![],
        Instruction::StoreDevice { source, .. } => vec![*source],
        Instruction::LoadSlot { slot, .. } => vec![*slot],
        Instruction::StoreSlot { slot, source, .. } => vec![*slot, *source],
        Instruction::BatchRead { hash, .. } => vec![*hash],
        Instruction::BatchWrite { hash, value, .. } => vec![*hash, *value],
        Instruction::Call { args, .. } => args.clone(),
        Instruction::IntrinsicCall { args, .. } => args.clone(),
        Instruction::Sleep { duration } => vec![*duration],
        Instruction::Yield => vec![],
        Instruction::LoadStatic { .. } => vec![],
        Instruction::StoreStatic { source, .. } => vec![*source],
    }
}

/// Collect all TempId operands used (read) by a terminator.
fn terminator_uses(terminator: &Terminator) -> Vec<TempId> {
    match terminator {
        Terminator::Jump(_) => vec![],
        Terminator::Branch { condition, .. } => vec![*condition],
        Terminator::Return(Some(value)) => vec![*value],
        Terminator::Return(None) => vec![],
        Terminator::None => vec![],
    }
}

/// Get the TempId defined (written) by an instruction, if any.
fn instruction_dest(instruction: &Instruction) -> Option<TempId> {
    match instruction {
        Instruction::Assign { dest, .. }
        | Instruction::Phi { dest, .. }
        | Instruction::LoadDevice { dest, .. }
        | Instruction::LoadSlot { dest, .. }
        | Instruction::BatchRead { dest, .. }
        | Instruction::IntrinsicCall { dest, .. } => Some(*dest),
        Instruction::Call { dest, .. } => *dest,
        Instruction::LoadStatic { dest, .. } => Some(*dest),
        Instruction::StoreDevice { .. }
        | Instruction::StoreSlot { .. }
        | Instruction::StoreStatic { .. }
        | Instruction::BatchWrite { .. }
        | Instruction::Sleep { .. }
        | Instruction::Yield => None,
    }
}

/// Insert phi functions at the pruned iterated dominance frontier.
///
/// For each variable with definitions in multiple blocks, compute the IDF
/// and insert a `Phi` instruction (with empty args) at each frontier block
/// where the variable is live-in. Returns a mapping from (BlockId, SymbolId)
/// to the phi's destination TempId, used during renaming to fill in args.
fn place_phi_functions(
    function: &mut Function,
    variables: &HashSet<SymbolId>,
    liveness: &HashMap<SymbolId, HashSet<BlockId>>,
) -> HashMap<(BlockId, SymbolId), TempId> {
    let mut phi_defs: HashMap<(BlockId, SymbolId), TempId> = HashMap::new();

    for &variable in variables {
        let live_in = match liveness.get(&variable) {
            Some(live) => live,
            None => continue,
        };

        let def_blocks: HashSet<BlockId> = function
            .variable_definitions
            .get(&variable)
            .map(|defs| defs.iter().map(|&(_temp, block)| block).collect())
            .unwrap_or_default();

        let mut has_phi: HashSet<BlockId> = HashSet::new();
        let mut ever_on_worklist: HashSet<BlockId> = HashSet::new();
        let mut worklist: Vec<BlockId> = Vec::new();

        for &block in &def_blocks {
            ever_on_worklist.insert(block);
            worklist.push(block);
        }

        while let Some(block) = worklist.pop() {
            let frontiers = function
                .dominance_frontiers
                .get(&block)
                .cloned()
                .unwrap_or_default();

            for frontier_block in frontiers {
                if has_phi.contains(&frontier_block) {
                    continue;
                }
                if !live_in.contains(&frontier_block) {
                    continue;
                }

                let phi_dest = function.fresh_temp();

                function.blocks[frontier_block.0].instructions.insert(
                    0,
                    Instruction::Phi {
                        dest: phi_dest,
                        args: Vec::new(),
                    },
                );

                phi_defs.insert((frontier_block, variable), phi_dest);
                has_phi.insert(frontier_block);

                if !ever_on_worklist.contains(&frontier_block) {
                    ever_on_worklist.insert(frontier_block);
                    worklist.push(frontier_block);
                }
            }
        }
    }

    phi_defs
}

/// Rename variable uses by walking the dominator tree in pre-order.
///
/// For each variable, maintains a stack of TempIds representing the current
/// reaching definition. At each definition (including phi destinations),
/// pushes the TempId. At each use of a variable's def-temp, replaces it
/// with the stack's top. At successor-block phis, fills in the argument
/// for the current predecessor block.
fn rename_variables(
    function: &mut Function,
    definition_map: &HashMap<TempId, SymbolId>,
    phi_defs: &HashMap<(BlockId, SymbolId), TempId>,
) {
    let children = function.dominator_tree_children();

    // Build an extended definition map that includes phi destinations.
    let mut extended_definition_map = definition_map.clone();
    for (&(_block, variable), &phi_temp) in phi_defs {
        extended_definition_map.insert(phi_temp, variable);
    }

    // Initialize stacks: each variable's stack starts empty; we push definitions
    // as we encounter them during the dominator-tree walk.
    let all_variables: HashSet<SymbolId> = extended_definition_map.values().copied().collect();
    let mut stacks: HashMap<SymbolId, Vec<TempId>> = HashMap::new();
    for variable in &all_variables {
        stacks.insert(*variable, Vec::new());
    }

    // Iterative dominator-tree pre-order walk using an explicit stack.
    // Each frame records (block_id, definition_count_per_variable) so we can
    // pop the correct number of definitions when backtracking.
    let mut work_stack: Vec<RenameFrame> = vec![RenameFrame {
        block_id: function.entry,
        child_index: 0,
        definition_counts: HashMap::new(),
        phase: RenamePhase::Enter,
    }];

    while let Some(frame) = work_stack.last_mut() {
        match frame.phase {
            RenamePhase::Enter => {
                let block_id = frame.block_id;
                let mut def_counts: HashMap<SymbolId, usize> = HashMap::new();

                // 1. Process phi instructions: each phi defines a new version.
                for instruction in &function.blocks[block_id.0].instructions {
                    if let Instruction::Phi { dest, .. } = instruction
                        && let Some(&variable) = extended_definition_map.get(dest)
                    {
                        stacks.get_mut(&variable).unwrap().push(*dest);
                        *def_counts.entry(variable).or_default() += 1;
                    }
                }

                // 2. Process non-phi instructions: rename uses, then push defs.
                let block = &mut function.blocks[block_id.0];
                for instruction in &mut block.instructions {
                    if matches!(instruction, Instruction::Phi { .. }) {
                        continue;
                    }
                    rename_operands(instruction, &extended_definition_map, &stacks);

                    if let Some(dest) = instruction_dest(instruction)
                        && let Some(&variable) = extended_definition_map.get(&dest)
                    {
                        stacks.get_mut(&variable).unwrap().push(dest);
                        *def_counts.entry(variable).or_default() += 1;
                    }
                }

                // 3. Rename terminator operands.
                rename_terminator_operands(
                    &mut block.terminator,
                    &extended_definition_map,
                    &stacks,
                );

                // 4. Fill in phi args in successor blocks.
                let successors: Vec<BlockId> = block.successors.clone();
                for &successor in &successors {
                    for instruction in &mut function.blocks[successor.0].instructions {
                        if let Instruction::Phi { dest, args } = instruction
                            && let Some(&variable) = extended_definition_map.get(dest)
                            && let Some(stack) = stacks.get(&variable)
                            && let Some(&current) = stack.last()
                        {
                            args.push((current, block_id));
                        }
                    }
                }

                frame.definition_counts = def_counts;
                frame.phase = RenamePhase::Children;
                frame.child_index = 0;
            }

            RenamePhase::Children => {
                let block_id = frame.block_id;
                let child_index = frame.child_index;
                let child_list = children.get(&block_id);

                if let Some(kids) = child_list
                    && child_index < kids.len()
                {
                    let child = kids[child_index];
                    frame.child_index += 1;
                    work_stack.push(RenameFrame {
                        block_id: child,
                        child_index: 0,
                        definition_counts: HashMap::new(),
                        phase: RenamePhase::Enter,
                    });
                    continue;
                }

                // All children processed — pop definitions.
                let def_counts = &frame.definition_counts;
                for (&variable, &count) in def_counts {
                    let stack = stacks.get_mut(&variable).unwrap();
                    for _ in 0..count {
                        stack.pop();
                    }
                }
                work_stack.pop();
            }
        }
    }

    // Update variable_definitions to include phi defs and reflect any changes.
    for (&(block_id, variable), &phi_temp) in phi_defs {
        function
            .variable_definitions
            .entry(variable)
            .or_default()
            .push((phi_temp, block_id));
    }
}

#[derive(Clone, Copy)]
enum RenamePhase {
    Enter,
    Children,
}

struct RenameFrame {
    block_id: BlockId,
    child_index: usize,
    definition_counts: HashMap<SymbolId, usize>,
    phase: RenamePhase,
}



/// Replace all variable-definition-temp operands in an instruction with the
/// current reaching definition from the per-variable stacks.
fn rename_operands(
    instruction: &mut Instruction,
    definition_map: &HashMap<TempId, SymbolId>,
    stacks: &HashMap<SymbolId, Vec<TempId>>,
) {
    match instruction {
        Instruction::Assign { operation, .. } => match operation {
            Operation::Copy(source) => {
                rename_temp(source, definition_map, stacks);
            }
            Operation::Constant(_) | Operation::Parameter { .. } => {}
            Operation::Binary { left, right, .. } => {
                rename_temp(left, definition_map, stacks);
                rename_temp(right, definition_map, stacks);
            }
            Operation::Unary { operand, .. } => {
                rename_temp(operand, definition_map, stacks);
            }
            Operation::Cast { operand, .. } => {
                rename_temp(operand, definition_map, stacks);
            }
            Operation::Select {
                condition,
                if_true,
                if_false,
            } => {
                rename_temp(condition, definition_map, stacks);
                rename_temp(if_true, definition_map, stacks);
                rename_temp(if_false, definition_map, stacks);
            }
        },
        Instruction::StoreDevice { source, .. } => {
            rename_temp(source, definition_map, stacks);
        }
        Instruction::LoadSlot { slot, .. } => {
            rename_temp(slot, definition_map, stacks);
        }
        Instruction::StoreSlot { slot, source, .. } => {
            rename_temp(slot, definition_map, stacks);
            rename_temp(source, definition_map, stacks);
        }
        Instruction::BatchRead { hash, .. } => {
            rename_temp(hash, definition_map, stacks);
        }
        Instruction::BatchWrite { hash, value, .. } => {
            rename_temp(hash, definition_map, stacks);
            rename_temp(value, definition_map, stacks);
        }
        Instruction::Call { args, .. } => {
            for arg in args.iter_mut() {
                rename_temp(arg, definition_map, stacks);
            }
        }
        Instruction::IntrinsicCall { args, .. } => {
            for arg in args.iter_mut() {
                rename_temp(arg, definition_map, stacks);
            }
        }
        Instruction::Sleep { duration } => {
            rename_temp(duration, definition_map, stacks);
        }
        Instruction::StoreStatic { source, .. } => {
            rename_temp(source, definition_map, stacks);
        }
        Instruction::Phi { .. }
        | Instruction::LoadDevice { .. }
        | Instruction::LoadStatic { .. }
        | Instruction::Yield => {}
    }
}

/// Replace a single TempId if it is a known variable definition temp.
fn rename_temp(
    temp: &mut TempId,
    definition_map: &HashMap<TempId, SymbolId>,
    stacks: &HashMap<SymbolId, Vec<TempId>>,
) {
    if let Some(&variable) = definition_map.get(temp)
        && let Some(stack) = stacks.get(&variable)
        && let Some(&current) = stack.last()
    {
        *temp = current;
    }
}

/// Rename operands in a terminator.
fn rename_terminator_operands(
    terminator: &mut Terminator,
    definition_map: &HashMap<TempId, SymbolId>,
    stacks: &HashMap<SymbolId, Vec<TempId>>,
) {
    match terminator {
        Terminator::Branch { condition, .. } => {
            rename_temp(condition, definition_map, stacks);
        }
        Terminator::Return(Some(value)) => {
            rename_temp(value, definition_map, stacks);
        }
        Terminator::Jump(_) | Terminator::Return(None) | Terminator::None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bind::bind;
    use crate::cfg;
    use crate::ir::cfg::{BlockId, Function, Instruction, Program, TempId};
    use crate::parser::parse;

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
        construct_program(&mut program);
        program
    }

    fn get_function<'a>(program: &'a Program, name: &str) -> &'a Function {
        program
            .functions
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("function '{}' not found", name))
    }

    struct PhiRecord {
        block: BlockId,
        dest: TempId,
        args: Vec<(TempId, BlockId)>,
    }

    fn collect_phis(function: &Function) -> Vec<PhiRecord> {
        let mut phis = Vec::new();
        for block in &function.blocks {
            for instruction in &block.instructions {
                if let Instruction::Phi { dest, args } = instruction {
                    phis.push(PhiRecord {
                        block: block.id,
                        dest: *dest,
                        args: args.clone(),
                    });
                }
            }
        }
        phis
    }

    #[test]
    fn straight_line_code_no_phis() {
        let program = build_ssa("fn main() { let x = 1; let y = 2; }");
        let main = get_function(&program, "main");
        let phis = collect_phis(main);
        assert!(phis.is_empty(), "no phis expected for straight-line code");
    }

    #[test]
    fn diamond_if_else_inserts_phi() {
        let program = build_ssa(
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
        let main = get_function(&program, "main");
        let phis = collect_phis(main);
        assert!(
            !phis.is_empty(),
            "expected phi at merge point after if/else, got none"
        );
        // The phi should have exactly 2 arguments (one from each branch).
        let merge_phi = &phis[0];
        assert_eq!(
            merge_phi.args.len(),
            2,
            "phi should have 2 args, got {:?}",
            merge_phi.args
        );
    }

    #[test]
    fn while_loop_inserts_phi() {
        let program = build_ssa(
            r#"fn main() {
                let mut x: i53 = 0;
                while x < 10 {
                    x = x + 1;
                }
            }"#,
        );
        let main = get_function(&program, "main");
        let phis = collect_phis(main);
        assert!(
            !phis.is_empty(),
            "expected phi at loop header for while-loop variable"
        );
    }

    #[test]
    fn for_loop_inserts_phi_for_loop_variable() {
        let program = build_ssa("fn main() { for i in 0..10 { yield; } }");
        let main = get_function(&program, "main");
        let phis = collect_phis(main);
        assert!(
            !phis.is_empty(),
            "expected phi for loop variable i at check block"
        );
        // The phi should have 2 args: initial value from entry, incremented from continue.
        let loop_phi = &phis[0];
        assert_eq!(
            loop_phi.args.len(),
            2,
            "loop variable phi should have 2 args, got {:?}",
            loop_phi.args
        );
    }

    #[test]
    fn if_without_else_inserts_phi() {
        let program = build_ssa(
            r#"fn main() {
                let mut x: i53 = 1;
                if true {
                    x = 2;
                }
                let y = x;
            }"#,
        );
        let main = get_function(&program, "main");
        let phis = collect_phis(main);
        assert!(
            !phis.is_empty(),
            "expected phi at merge point for variable modified in only one branch"
        );
    }

    #[test]
    fn no_phi_for_immutable_variable() {
        let program = build_ssa(
            r#"fn main() {
                let x: i53 = 1;
                if true {
                    let y = x;
                } else {
                    let z = x;
                }
            }"#,
        );
        let main = get_function(&program, "main");
        let phis = collect_phis(main);
        assert!(
            phis.is_empty(),
            "no phis expected for immutable variable used in both branches"
        );
    }

    #[test]
    fn phi_args_reference_valid_predecessor_blocks() {
        let program = build_ssa(
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
        let main = get_function(&program, "main");
        let phis = collect_phis(main);
        for record in &phis {
            let predecessors = &main.blocks[record.block.0].predecessors;
            for (_, arg_block) in &record.args {
                assert!(
                    predecessors.contains(arg_block),
                    "phi arg block {:?} is not a predecessor of phi block {:?}",
                    arg_block,
                    record.block
                );
            }
        }
    }

    #[test]
    fn use_in_merge_block_references_phi_dest() {
        let program = build_ssa(
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
        let main = get_function(&program, "main");
        let phis = collect_phis(main);
        assert!(!phis.is_empty());
        let first_phi = &phis[0];
        // Find a Copy instruction in the merge block that uses the phi dest.
        let merge_block = &main.blocks[first_phi.block.0];
        let uses_phi = merge_block.instructions.iter().any(|instruction| {
            let uses = instruction_uses(instruction);
            uses.contains(&first_phi.dest)
        });
        assert!(
            uses_phi,
            "expected some instruction in merge block to use phi dest {:?}",
            first_phi.dest
        );
    }

    #[test]
    fn multiple_variables_get_separate_phis() {
        let program = build_ssa(
            r#"fn main() {
                let mut x: i53 = 1;
                let mut y: i53 = 10;
                if true {
                    x = 2;
                    y = 20;
                } else {
                    x = 3;
                    y = 30;
                }
                let a = x + y;
            }"#,
        );
        let main = get_function(&program, "main");
        let phis = collect_phis(main);
        assert_eq!(
            phis.len(),
            2,
            "expected 2 phis (one per variable), got {}",
            phis.len()
        );
    }

    #[test]
    fn nested_if_else() {
        let program = build_ssa(
            r#"fn main() {
                let mut x: i53 = 0;
                if true {
                    if true {
                        x = 1;
                    } else {
                        x = 2;
                    }
                } else {
                    x = 3;
                }
                let y = x;
            }"#,
        );
        let main = get_function(&program, "main");
        let phis = collect_phis(main);
        assert!(
            phis.len() >= 2,
            "expected at least 2 phis for nested if/else, got {}",
            phis.len()
        );
    }

    #[test]
    fn infinite_loop_with_break_and_variable() {
        let program = build_ssa(
            r#"fn main() {
                let mut x: i53 = 0;
                loop {
                    x = x + 1;
                    if x > 10 {
                        break;
                    }
                }
            }"#,
        );
        let main = get_function(&program, "main");
        let phis = collect_phis(main);
        assert!(!phis.is_empty(), "expected phi for x at loop header");
    }

    #[test]
    fn each_temp_defined_at_most_once() {
        let program = build_ssa(
            r#"fn main() {
                let mut x: i53 = 0;
                while x < 5 {
                    x = x + 1;
                }
            }"#,
        );
        let main = get_function(&program, "main");
        let mut defs: HashMap<TempId, usize> = HashMap::new();
        for block in &main.blocks {
            for instruction in &block.instructions {
                if let Some(dest) = instruction_dest(instruction) {
                    *defs.entry(dest).or_default() += 1;
                }
            }
        }
        for (temp, count) in &defs {
            assert_eq!(
                *count, 1,
                "TempId {:?} defined {} times (expected exactly 1)",
                temp, count
            );
        }
    }

    #[test]
    fn empty_function_no_crash() {
        let program = build_ssa("fn main() {}");
        let main = get_function(&program, "main");
        let phis = collect_phis(main);
        assert!(phis.is_empty());
    }

    #[test]
    fn parameter_used_across_branches() {
        let program = build_ssa(
            r#"fn foo(x: i53) -> i53 {
                let mut result: i53 = x;
                if x > 0 {
                    result = x + 1;
                }
                return result;
            }
            fn main() {}"#,
        );
        let foo = get_function(&program, "foo");
        let phis = collect_phis(foo);
        assert!(
            !phis.is_empty(),
            "expected phi for result in function with parameter-dependent branch"
        );
    }

    #[test]
    fn parameter_reassigned_in_loop_gets_phi() {
        let program = build_ssa(
            r#"device out: d0;
            fn foo(n: i53) -> i53 {
                let mut x: i53 = n;
                while x < 100 {
                    x = x + 1;
                }
                return x;
            }
            fn main() { out.Setting = foo(0); }"#,
        );
        let foo = get_function(&program, "foo");
        let phis = collect_phis(foo);
        assert!(
            !phis.is_empty(),
            "parameter modified inside loop should produce a phi at the loop header"
        );
    }
}
