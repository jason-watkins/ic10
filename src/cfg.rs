use std::collections::HashMap;

use crate::diagnostic::{Diagnostic, Span};
use crate::ir::bound::{
    self, AssignmentTarget, BatchWriteStatement, ElseClause, ExpressionKind,
    Program as BoundProgram, Statement, SymbolId,
};
use crate::ir::cfg::{
    BasicBlock, BlockId, BlockRole, Function, Instruction, Operation, Program, TempId, Terminator,
};
use crate::ir::{BinaryOperator, Type};

/// Loop context for break/continue targeting.
struct LoopContext {
    label: Option<String>,
    continue_target: BlockId,
    break_target: BlockId,
}

/// Builds a CFG `Function` from a `bound::FunctionDeclaration`.
struct Builder {
    blocks: Vec<BasicBlock>,
    next_temp: usize,
    current_block: BlockId,
    loop_stack: Vec<LoopContext>,
    variable_temps: HashMap<SymbolId, TempId>,
    variable_definitions: HashMap<SymbolId, Vec<(TempId, BlockId)>>,
    diagnostics: Vec<Diagnostic>,
    /// Set to the span of a break/continue/return once we enter unreachable territory.
    /// The next statement lowered while this is `Some` triggers a warning, then further
    /// statements in the same unreachable region are silently skipped.
    unreachable_after: Option<Span>,
    /// Counter for assigning unique 1-based indices to if statements.
    next_if_index: usize,
    /// Counter for assigning unique 1-based indices to loops.
    next_loop_index: usize,
}

impl Builder {
    fn new() -> Self {
        Self {
            blocks: Vec::new(),
            next_temp: 0,
            current_block: BlockId(0),
            loop_stack: Vec::new(),
            variable_temps: HashMap::new(),
            variable_definitions: HashMap::new(),
            diagnostics: Vec::new(),
            unreachable_after: None,
            next_if_index: 0,
            next_loop_index: 0,
        }
    }

    fn fresh_temp(&mut self) -> TempId {
        let id = TempId(self.next_temp);
        self.next_temp += 1;
        id
    }

    fn new_block(&mut self) -> BlockId {
        let id = BlockId(self.blocks.len());
        self.blocks.push(BasicBlock {
            id,
            role: BlockRole::Generic,
            instructions: Vec::new(),
            terminator: Terminator::None,
            predecessors: Vec::new(),
            successors: Vec::new(),
        });
        id
    }

    fn emit(&mut self, instruction: Instruction) {
        self.blocks[self.current_block.0]
            .instructions
            .push(instruction);
    }

    fn set_terminator(&mut self, terminator: Terminator) {
        let block = &mut self.blocks[self.current_block.0];
        block.terminator = terminator;
    }

    fn switch_to(&mut self, block: BlockId) {
        self.current_block = block;
    }

    fn add_edge(&mut self, from: BlockId, to: BlockId) {
        self.blocks[from.0].successors.push(to);
        self.blocks[to.0].predecessors.push(from);
    }

    fn terminate_and_jump(&mut self, target: BlockId) {
        self.set_terminator(Terminator::Jump(target));
        self.add_edge(self.current_block, target);
    }

    fn terminate_and_branch(
        &mut self,
        condition: TempId,
        true_block: BlockId,
        false_block: BlockId,
    ) {
        self.set_terminator(Terminator::Branch {
            condition,
            true_block,
            false_block,
        });
        self.add_edge(self.current_block, true_block);
        self.add_edge(self.current_block, false_block);
    }

    fn record_variable_definition(&mut self, symbol_id: SymbolId, temp: TempId) {
        self.variable_temps.insert(symbol_id, temp);
        self.variable_definitions
            .entry(symbol_id)
            .or_default()
            .push((temp, self.current_block));
    }

    fn lower_expression(&mut self, expression: &bound::Expression) -> TempId {
        match &expression.kind {
            ExpressionKind::Literal(value) => {
                let dest = self.fresh_temp();
                self.emit(Instruction::Assign {
                    dest,
                    operation: Operation::Constant(*value),
                });
                dest
            }

            ExpressionKind::Variable(symbol_id) => {
                let source = self.variable_temps[symbol_id];
                let dest = self.fresh_temp();
                self.emit(Instruction::Assign {
                    dest,
                    operation: Operation::Copy(source),
                });
                dest
            }

            ExpressionKind::Binary(operator, left, right) => {
                let left_temp = self.lower_expression(left);
                let right_temp = self.lower_expression(right);
                let dest = self.fresh_temp();
                self.emit(Instruction::Assign {
                    dest,
                    operation: Operation::Binary {
                        operator: *operator,
                        left: left_temp,
                        right: right_temp,
                    },
                });
                dest
            }

            ExpressionKind::Unary(operator, operand) => {
                let operand_temp = self.lower_expression(operand);
                let dest = self.fresh_temp();
                self.emit(Instruction::Assign {
                    dest,
                    operation: Operation::Unary {
                        operator: *operator,
                        operand: operand_temp,
                    },
                });
                dest
            }

            ExpressionKind::Cast(inner, target_type) => {
                let operand_temp = self.lower_expression(inner);
                let dest = self.fresh_temp();
                self.emit(Instruction::Assign {
                    dest,
                    operation: Operation::Cast {
                        operand: operand_temp,
                        target_type: *target_type,
                        source_type: inner.ty,
                    },
                });
                dest
            }

            ExpressionKind::Call(function_symbol, args) => {
                let arg_temps: Vec<TempId> =
                    args.iter().map(|a| self.lower_expression(a)).collect();
                let dest = if expression.ty != Type::Unit {
                    Some(self.fresh_temp())
                } else {
                    None
                };
                self.emit(Instruction::Call {
                    dest,
                    function: *function_symbol,
                    args: arg_temps,
                });
                // `lower_expression` is only called on a Call node when the call appears
                // as a sub-expression (argument, RHS, etc.). The binder rejects any use
                // of a unit-returning call as a value, so `dest` is always `Some` here.
                dest.expect(
                    "unit-returning call reached lower_expression; binder invariant violated",
                )
            }

            ExpressionKind::IntrinsicCall(function, args) => {
                let arg_temps: Vec<TempId> =
                    args.iter().map(|a| self.lower_expression(a)).collect();
                let dest = self.fresh_temp();
                self.emit(Instruction::IntrinsicCall {
                    dest,
                    function: *function,
                    args: arg_temps,
                });
                dest
            }

            ExpressionKind::DeviceRead { pin, field } => {
                let dest = self.fresh_temp();
                self.emit(Instruction::LoadDevice {
                    dest,
                    pin: *pin,
                    field: field.clone(),
                });
                dest
            }

            ExpressionKind::SlotRead { pin, slot, field } => {
                let slot_temp = self.lower_expression(slot);
                let dest = self.fresh_temp();
                self.emit(Instruction::LoadSlot {
                    dest,
                    pin: *pin,
                    slot: slot_temp,
                    field: field.clone(),
                });
                dest
            }

            ExpressionKind::BatchRead {
                hash_expr,
                field,
                mode,
            } => {
                let hash_temp = self.lower_expression(hash_expr);
                let dest = self.fresh_temp();
                self.emit(Instruction::BatchRead {
                    dest,
                    hash: hash_temp,
                    field: field.clone(),
                    mode: *mode,
                });
                dest
            }

            ExpressionKind::Select {
                condition,
                if_true,
                if_false,
            } => {
                let cond_temp = self.lower_expression(condition);
                let true_temp = self.lower_expression(if_true);
                let false_temp = self.lower_expression(if_false);
                let dest = self.fresh_temp();
                self.emit(Instruction::Assign {
                    dest,
                    operation: Operation::Select {
                        condition: cond_temp,
                        if_true: true_temp,
                        if_false: false_temp,
                    },
                });
                dest
            }
        }
    }

    fn lower_statement(&mut self, statement: &Statement) {
        if let Some(cause_span) = self.unreachable_after {
            let _ = cause_span;
            self.diagnostics
                .push(Diagnostic::warning(statement.span(), "unreachable code"));
            self.unreachable_after = None;
            return;
        }

        match statement {
            Statement::Let(let_statement) => {
                let init_temp = self.lower_expression(&let_statement.init);
                let dest = self.fresh_temp();
                self.emit(Instruction::Assign {
                    dest,
                    operation: Operation::Copy(init_temp),
                });
                self.record_variable_definition(let_statement.symbol_id, dest);
            }

            Statement::Assign(assign_statement) => match &assign_statement.target {
                AssignmentTarget::Variable { symbol_id, .. } => {
                    let value_temp = self.lower_expression(&assign_statement.value);
                    let dest = self.fresh_temp();
                    self.emit(Instruction::Assign {
                        dest,
                        operation: Operation::Copy(value_temp),
                    });
                    self.record_variable_definition(*symbol_id, dest);
                }

                AssignmentTarget::DeviceField { pin, field, .. } => {
                    let value_temp = self.lower_expression(&assign_statement.value);
                    self.emit(Instruction::StoreDevice {
                        pin: *pin,
                        field: field.clone(),
                        source: value_temp,
                    });
                }

                AssignmentTarget::SlotField {
                    pin, slot, field, ..
                } => {
                    let slot_temp = self.lower_expression(slot);
                    let value_temp = self.lower_expression(&assign_statement.value);
                    self.emit(Instruction::StoreSlot {
                        pin: *pin,
                        slot: slot_temp,
                        field: field.clone(),
                        source: value_temp,
                    });
                }
            },

            Statement::Expression(expression_statement) => {
                self.lower_expression_statement(expression_statement);
            }

            Statement::If(if_statement) => {
                self.lower_if(if_statement);
            }

            Statement::While(while_statement) => {
                self.lower_while(while_statement);
            }

            Statement::For(for_statement) => {
                self.lower_for(for_statement);
            }

            Statement::Break(statement) => {
                let target = if let Some(ref label) = statement.label {
                    self.loop_stack
                        .iter()
                        .rev()
                        .find(|ctx| ctx.label.as_deref() == Some(label))
                        .map(|ctx| ctx.break_target)
                } else {
                    self.loop_stack.last().map(|ctx| ctx.break_target)
                };
                if let Some(target) = target {
                    self.terminate_and_jump(target);
                    let unreachable = self.new_block();
                    self.switch_to(unreachable);
                    self.unreachable_after = Some(statement.span);
                }
            }

            Statement::Continue(statement) => {
                let target = if let Some(ref label) = statement.label {
                    self.loop_stack
                        .iter()
                        .rev()
                        .find(|ctx| ctx.label.as_deref() == Some(label))
                        .map(|ctx| ctx.continue_target)
                } else {
                    self.loop_stack.last().map(|ctx| ctx.continue_target)
                };
                if let Some(target) = target {
                    self.terminate_and_jump(target);
                    let unreachable = self.new_block();
                    self.switch_to(unreachable);
                    self.unreachable_after = Some(statement.span);
                }
            }

            Statement::Return(return_statement) => {
                let value = return_statement
                    .value
                    .as_ref()
                    .map(|v| self.lower_expression(v));
                self.set_terminator(Terminator::Return(value));
                let unreachable = self.new_block();
                self.switch_to(unreachable);
                self.unreachable_after = Some(return_statement.span);
            }

            Statement::Yield(_) => {
                self.emit(Instruction::Yield);
            }

            Statement::Sleep(sleep_statement) => {
                let duration_temp = self.lower_expression(&sleep_statement.duration);
                self.emit(Instruction::Sleep {
                    duration: duration_temp,
                });
            }

            Statement::BatchWrite(BatchWriteStatement {
                hash_expr,
                field,
                value,
                ..
            }) => {
                let hash_temp = self.lower_expression(hash_expr);
                let value_temp = self.lower_expression(value);
                self.emit(Instruction::BatchWrite {
                    hash: hash_temp,
                    field: field.clone(),
                    value: value_temp,
                });
            }
        }
    }

    fn lower_expression_statement(&mut self, statement: &bound::ExpressionStatement) {
        match &statement.expression.kind {
            ExpressionKind::Call(function_symbol, args) => {
                let arg_temps: Vec<TempId> =
                    args.iter().map(|a| self.lower_expression(a)).collect();
                let dest = if statement.expression.ty != Type::Unit {
                    Some(self.fresh_temp())
                } else {
                    None
                };
                self.emit(Instruction::Call {
                    dest,
                    function: *function_symbol,
                    args: arg_temps,
                });
            }
            ExpressionKind::IntrinsicCall(function, args) => {
                let arg_temps: Vec<TempId> =
                    args.iter().map(|a| self.lower_expression(a)).collect();
                let dest = self.fresh_temp();
                self.emit(Instruction::IntrinsicCall {
                    dest,
                    function: *function,
                    args: arg_temps,
                });
            }
            _ => {
                self.lower_expression(&statement.expression);
            }
        }
    }

    fn lower_if(&mut self, if_statement: &bound::IfStatement) {
        let condition_temp = self.lower_expression(&if_statement.condition);

        self.next_if_index += 1;
        let if_index = self.next_if_index;

        let then_block = self.new_block();
        self.blocks[then_block.0].role = BlockRole::IfTrue(if_index);
        let merge_block = self.new_block();
        self.blocks[merge_block.0].role = BlockRole::IfEnd(if_index);

        match &if_statement.else_clause {
            None => {
                self.terminate_and_branch(condition_temp, then_block, merge_block);
            }
            Some(ElseClause::Block(else_block)) => {
                let else_block_id = self.new_block();
                self.blocks[else_block_id.0].role = BlockRole::IfFalse(if_index);
                self.terminate_and_branch(condition_temp, then_block, else_block_id);
                self.switch_to(else_block_id);
                self.lower_block(else_block);
                if self.current_block_needs_terminator() {
                    self.terminate_and_jump(merge_block);
                }
            }
            Some(ElseClause::If(nested_if)) => {
                let else_block_id = self.new_block();
                self.blocks[else_block_id.0].role = BlockRole::IfFalse(if_index);
                self.terminate_and_branch(condition_temp, then_block, else_block_id);
                self.switch_to(else_block_id);
                self.lower_if(nested_if);
                if self.current_block_needs_terminator() {
                    self.terminate_and_jump(merge_block);
                }
            }
        }

        self.switch_to(then_block);
        self.lower_block(&if_statement.then_block);
        if self.current_block_needs_terminator() {
            self.terminate_and_jump(merge_block);
        }

        self.switch_to(merge_block);
    }

    /// Lower `while cond { body }`.
    ///
    /// If the condition is a literal `true`, emit an unconditional
    /// back-edge instead of a conditional branch
    /// Lower `while cond { body }`.
    ///
    /// Uses a bottom-tested pattern: the condition is evaluated at the end of
    /// the loop as the back-edge test, saving one unconditional jump per
    /// iteration compared to a top-tested loop. An initial guard skips the
    /// loop entirely when the condition is false on entry.
    ///
    /// Infinite loops (`loop { … }`, desugared to `while true { … }`) skip
    /// the guard and use an unconditional back-edge.
    fn lower_while(&mut self, while_statement: &bound::WhileStatement) {
        let is_infinite = matches!(
            while_statement.condition.kind,
            ExpressionKind::Literal(v) if v == 1.0
        );

        self.next_loop_index += 1;
        let loop_index = self.next_loop_index;

        let body_block = self.new_block();
        self.blocks[body_block.0].role = BlockRole::LoopStart(loop_index);
        let check_block = self.new_block();
        self.blocks[check_block.0].role = BlockRole::LoopContinue(loop_index);
        let exit_block = self.new_block();
        self.blocks[exit_block.0].role = BlockRole::LoopEnd(loop_index);

        if is_infinite {
            self.terminate_and_jump(body_block);
        } else {
            let guard_cond = self.lower_expression(&while_statement.condition);
            self.terminate_and_branch(guard_cond, body_block, exit_block);
        }

        self.loop_stack.push(LoopContext {
            label: while_statement.label.clone(),
            continue_target: check_block,
            break_target: exit_block,
        });

        self.switch_to(body_block);
        self.lower_block(&while_statement.body);
        if self.current_block_needs_terminator() {
            self.terminate_and_jump(check_block);
        }

        self.loop_stack.pop();

        self.switch_to(check_block);
        if is_infinite {
            self.terminate_and_jump(body_block);
        } else {
            let back_cond = self.lower_expression(&while_statement.condition);
            self.terminate_and_branch(back_cond, body_block, exit_block);
        }

        self.switch_to(exit_block);
    }

    /// Lower `for var in lower..upper { body }` and its variants.
    ///
    /// Supports exclusive (`..`) and inclusive (`..=`) ranges, reverse
    /// iteration (`.rev()`), and custom step (`.step_by(n)`).
    ///
    /// Uses a bottom-tested pattern: the back-edge test is at the end of
    /// the loop, saving one unconditional jump per iteration. An initial
    /// guard branch skips the loop if the range is empty.
    ///
    /// Ascending exclusive `for i in a..b`:
    ///   r_i = a; r_upper = b
    ///   bge r_i r_upper for_end
    ///   for_body: <body>
    ///   for_continue: add r_i r_i step; blt r_i r_upper for_body
    ///   for_end:
    ///
    /// Ascending inclusive `for i in a..=b`:
    ///   r_i = a; r_upper = b
    ///   bgt r_i r_upper for_end
    ///   for_body: <body>
    ///   for_continue: add r_i r_i step; ble r_i r_upper for_body
    ///   for_end:
    ///
    /// Descending exclusive `(a..b).rev()`:
    ///   r_i = b - step; r_lower = a
    ///   blt r_i r_lower for_end
    ///   for_body: <body>
    ///   for_continue: sub r_i r_i step; bge r_i r_lower for_body
    ///   for_end:
    ///
    /// Descending inclusive `(a..=b).rev()`:
    ///   r_i = b; r_lower = a
    ///   blt r_i r_lower for_end
    ///   for_body: <body>
    ///   for_continue: sub r_i r_i step; bge r_i r_lower for_body
    ///   for_end:
    fn lower_for(&mut self, for_statement: &bound::ForStatement) {
        let lower_temp = self.lower_expression(&for_statement.lower);
        let upper_temp = self.lower_expression(&for_statement.upper);

        let step_temp = if let Some(step_expr) = &for_statement.step {
            self.lower_expression(step_expr)
        } else {
            let t = self.fresh_temp();
            self.emit(Instruction::Assign {
                dest: t,
                operation: Operation::Constant(1.0),
            });
            t
        };

        let reverse = for_statement.reverse;
        let inclusive = for_statement.inclusive;

        let loop_var = self.fresh_temp();
        let bound_temp;

        if reverse {
            if inclusive {
                // (a..=b).rev(): start at b, count down to a
                self.emit(Instruction::Assign {
                    dest: loop_var,
                    operation: Operation::Copy(upper_temp),
                });
                bound_temp = lower_temp;
            } else {
                // (a..b).rev(): start at b - step, count down to a
                let start = self.fresh_temp();
                self.emit(Instruction::Assign {
                    dest: start,
                    operation: Operation::Binary {
                        operator: BinaryOperator::Sub,
                        left: upper_temp,
                        right: step_temp,
                    },
                });
                self.emit(Instruction::Assign {
                    dest: loop_var,
                    operation: Operation::Copy(start),
                });
                bound_temp = lower_temp;
            }
        } else {
            // Ascending: start at lower, bound is upper
            self.emit(Instruction::Assign {
                dest: loop_var,
                operation: Operation::Copy(lower_temp),
            });
            bound_temp = upper_temp;
        }

        self.record_variable_definition(for_statement.variable, loop_var);

        self.next_loop_index += 1;
        let loop_index = self.next_loop_index;

        let body_block = self.new_block();
        self.blocks[body_block.0].role = BlockRole::LoopStart(loop_index);
        let continue_block = self.new_block();
        self.blocks[continue_block.0].role = BlockRole::LoopContinue(loop_index);
        let exit_block = self.new_block();
        self.blocks[exit_block.0].role = BlockRole::LoopEnd(loop_index);

        let guard_cond = self.fresh_temp();
        let guard_operator = if reverse {
            // Descending: skip if start < bound (empty range)
            BinaryOperator::Lt
        } else if inclusive {
            // Ascending inclusive: skip if start > bound (empty range)
            BinaryOperator::Gt
        } else {
            // Ascending exclusive: skip if start >= bound (empty range)
            BinaryOperator::Ge
        };
        let current_loop_var = self.variable_temps[&for_statement.variable];
        self.emit(Instruction::Assign {
            dest: guard_cond,
            operation: Operation::Binary {
                operator: guard_operator,
                left: current_loop_var,
                right: bound_temp,
            },
        });
        self.terminate_and_branch(guard_cond, exit_block, body_block);

        self.loop_stack.push(LoopContext {
            label: for_statement.label.clone(),
            continue_target: continue_block,
            break_target: exit_block,
        });

        self.switch_to(body_block);
        self.lower_block(&for_statement.body);
        if self.current_block_needs_terminator() {
            self.terminate_and_jump(continue_block);
        }

        self.loop_stack.pop();

        self.switch_to(continue_block);

        let (increment_operator, back_edge_operator) = if reverse {
            // Descending: subtract step, continue while i >= bound
            (BinaryOperator::Sub, BinaryOperator::Ge)
        } else if inclusive {
            // Ascending inclusive: add step, continue while i <= bound
            (BinaryOperator::Add, BinaryOperator::Le)
        } else {
            // Ascending exclusive: add step, continue while i < bound
            (BinaryOperator::Add, BinaryOperator::Lt)
        };

        let current_var = self.variable_temps[&for_statement.variable];
        let incremented = self.fresh_temp();
        self.emit(Instruction::Assign {
            dest: incremented,
            operation: Operation::Binary {
                operator: increment_operator,
                left: current_var,
                right: step_temp,
            },
        });
        self.record_variable_definition(for_statement.variable, incremented);

        let back_cond = self.fresh_temp();
        self.emit(Instruction::Assign {
            dest: back_cond,
            operation: Operation::Binary {
                operator: back_edge_operator,
                left: incremented,
                right: bound_temp,
            },
        });
        self.terminate_and_branch(back_cond, body_block, exit_block);

        self.switch_to(exit_block);
    }

    fn lower_block(&mut self, block: &bound::Block) {
        for statement in &block.statements {
            self.lower_statement(statement);
        }
        self.unreachable_after = None;
    }

    fn current_block_needs_terminator(&self) -> bool {
        matches!(
            self.blocks[self.current_block.0].terminator,
            Terminator::None
        )
    }
}

/// Lower a single `bound::FunctionDeclaration` into a `cfg::Function`.
fn lower_function(
    function: &bound::FunctionDeclaration,
    diagnostics: &mut Vec<Diagnostic>,
) -> Function {
    let mut builder = Builder::new();
    let entry = builder.new_block();
    builder.blocks[entry.0].role = BlockRole::Entry;
    builder.switch_to(entry);

    let mut parameter_ids = Vec::new();
    for (index, parameter) in function.parameters.iter().enumerate() {
        let temp = builder.fresh_temp();
        builder.emit(Instruction::Assign {
            dest: temp,
            operation: Operation::Parameter { index },
        });
        builder.record_variable_definition(parameter.symbol_id, temp);
        parameter_ids.push(parameter.symbol_id);
    }

    builder.lower_block(&function.body);

    if builder.current_block_needs_terminator() {
        builder.set_terminator(Terminator::Return(None));
    }

    diagnostics.append(&mut builder.diagnostics);

    let mut cfg_function = Function {
        name: function.name.clone(),
        symbol_id: function.symbol_id,
        parameters: parameter_ids,
        return_type: function.return_type,
        blocks: builder.blocks,
        entry,
        variable_definitions: builder.variable_definitions,
        variable_temps: builder.variable_temps,
        immediate_dominators: HashMap::new(),
        dominance_frontiers: HashMap::new(),
        next_temp: builder.next_temp,
    };

    compute_dominators(&mut cfg_function);
    compute_dominance_frontiers(&mut cfg_function);

    cfg_function
}

/// Cooper-Harvey-Kennedy "A Simple, Fast Dominance Algorithm".
///
/// Computes immediate dominators for all reachable blocks.
fn compute_dominators(function: &mut Function) {
    let block_count = function.blocks.len();
    if block_count == 0 {
        return;
    }

    let reverse_postorder = compute_reverse_postorder(function);

    let mut rpo_number = vec![usize::MAX; block_count];
    for (index, &block_id) in reverse_postorder.iter().enumerate() {
        rpo_number[block_id.0] = index;
    }

    // idom[rpo_index] = rpo_index of immediate dominator.
    let mut idom: Vec<Option<usize>> = vec![None; reverse_postorder.len()];
    idom[0] = Some(0);

    let intersect = |mut finger1: usize, mut finger2: usize, idom: &[Option<usize>]| -> usize {
        while finger1 != finger2 {
            while finger1 > finger2 {
                finger1 = idom[finger1].unwrap();
            }
            while finger2 > finger1 {
                finger2 = idom[finger2].unwrap();
            }
        }
        finger1
    };

    let mut changed = true;
    while changed {
        changed = false;
        for &block_id in reverse_postorder.iter().skip(1) {
            let rpo_idx = rpo_number[block_id.0];
            let predecessors = &function.blocks[block_id.0].predecessors;

            let mut new_idom = None;
            for &pred in predecessors {
                let pred_rpo = rpo_number[pred.0];
                if pred_rpo != usize::MAX && idom[pred_rpo].is_some() {
                    new_idom = Some(pred_rpo);
                    break;
                }
            }

            let Some(mut new_idom) = new_idom else {
                continue;
            };

            for &pred in predecessors {
                let pred_rpo = rpo_number[pred.0];
                if pred_rpo != usize::MAX && idom[pred_rpo].is_some() && pred_rpo != new_idom {
                    new_idom = intersect(pred_rpo, new_idom, &idom);
                }
            }

            if idom[rpo_idx] != Some(new_idom) {
                idom[rpo_idx] = Some(new_idom);
                changed = true;
            }
        }
    }

    function.immediate_dominators.clear();
    for (rpo_idx, &block_id) in reverse_postorder.iter().enumerate() {
        if let Some(dom_rpo) = idom[rpo_idx]
            && rpo_idx != dom_rpo
        {
            function
                .immediate_dominators
                .insert(block_id, reverse_postorder[dom_rpo]);
        }
    }
}

/// Compute dominance frontiers using the algorithm from Cytron et al.
///
/// For each join point (block with 2+ predecessors), walk up the dominator tree
/// from each predecessor until reaching the block's immediate dominator. Each
/// block visited (excluding the idom) has the join point in its dominance frontier.
fn compute_dominance_frontiers(function: &mut Function) {
    function.dominance_frontiers.clear();

    for block in &function.blocks {
        let block_id = block.id;
        if block.predecessors.len() >= 2 {
            let idom_of_block = function.immediate_dominators.get(&block_id).copied();

            for &predecessor in &block.predecessors {
                let mut runner = predecessor;
                loop {
                    if Some(runner) == idom_of_block {
                        break;
                    }
                    function
                        .dominance_frontiers
                        .entry(runner)
                        .or_default()
                        .insert(block_id);

                    match function.immediate_dominators.get(&runner) {
                        Some(&parent) => runner = parent,
                        None => break,
                    }
                }
            }
        }
    }
}

/// Compute reverse-postorder of reachable blocks via iterative DFS.
fn compute_reverse_postorder(function: &Function) -> Vec<BlockId> {
    let block_count = function.blocks.len();
    let mut visited = vec![false; block_count];
    let mut postorder = Vec::with_capacity(block_count);
    let mut stack: Vec<(BlockId, usize)> = vec![(function.entry, 0)];
    visited[function.entry.0] = true;

    while let Some((block_id, successor_index)) = stack.last_mut() {
        let successors = &function.blocks[block_id.0].successors;
        if *successor_index < successors.len() {
            let next = successors[*successor_index];
            *successor_index += 1;
            if !visited[next.0] {
                visited[next.0] = true;
                stack.push((next, 0));
            }
        } else {
            postorder.push(*block_id);
            stack.pop();
        }
    }

    postorder.reverse();
    postorder
}

/// Lower a complete bound program to a CFG program.
pub fn build(bound_program: &BoundProgram) -> (Program, Vec<Diagnostic>) {
    let mut diagnostics = Vec::new();
    let functions = bound_program
        .functions
        .iter()
        .map(|f| lower_function(f, &mut diagnostics))
        .collect();
    (
        Program {
            functions,
            symbols: bound_program.symbols.clone(),
        },
        diagnostics,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bind::bind;
    use crate::ir::cfg::{Function, Instruction, Operation, Program, TempId, Terminator};
    use crate::ir::{BatchMode, DevicePin, Intrinsic, UnaryOperator};
    use crate::parser::parse;

    fn build_cfg(source: &str) -> Program {
        let (ast, parse_diagnostics) = parse(source);
        let errors: Vec<_> = parse_diagnostics
            .iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "parse errors: {:#?}", errors);
        let (bound, _) = bind(&ast).unwrap_or_else(|diags| panic!("bind errors: {:#?}", diags));
        let (program, _) = build(&bound);
        program
    }

    fn build_cfg_with_diagnostics(source: &str) -> (Program, Vec<Diagnostic>) {
        let (ast, parse_diagnostics) = parse(source);
        let errors: Vec<_> = parse_diagnostics
            .iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "parse errors: {:#?}", errors);
        let (bound, _) = bind(&ast).unwrap_or_else(|diags| panic!("bind errors: {:#?}", diags));
        build(&bound)
    }

    fn get_function<'a>(program: &'a Program, name: &str) -> &'a Function {
        program
            .functions
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("function '{}' not found", name))
    }

    #[test]
    fn empty_main() {
        let program = build_cfg("fn main() {}");
        let main = get_function(&program, "main");
        assert_eq!(main.blocks.len(), 1);
        assert!(matches!(
            main.blocks[0].terminator,
            Terminator::Return(None)
        ));
    }

    #[test]
    fn simple_let_binding() {
        let program = build_cfg("fn main() { let x = 42; }");
        let main = get_function(&program, "main");
        assert_eq!(main.blocks.len(), 1);
        let instructions = &main.blocks[0].instructions;
        assert_eq!(instructions.len(), 2);
        assert!(matches!(
            &instructions[0],
            Instruction::Assign { dest: TempId(0), operation: Operation::Constant(v) } if *v == 42.0
        ));
        assert!(matches!(
            &instructions[1],
            Instruction::Assign {
                dest: TempId(1),
                operation: Operation::Copy(TempId(0))
            }
        ));
    }

    #[test]
    fn if_without_else_creates_three_blocks() {
        let program = build_cfg("fn main() { let x = true; if x { let y = 1; } }");
        let main = get_function(&program, "main");
        assert!(main.blocks.len() >= 3);
    }

    #[test]
    fn if_else_creates_four_blocks() {
        let program =
            build_cfg("fn main() { let x = true; if x { let y = 1; } else { let z = 2; } }");
        let main = get_function(&program, "main");
        assert!(main.blocks.len() >= 4);
    }

    #[test]
    fn while_loop_structure() {
        let program = build_cfg("fn main() { let mut x = true; while x { x = false; } }");
        let main = get_function(&program, "main");
        assert!(main.blocks.len() >= 4);
        let entry = &main.blocks[0];
        assert!(
            matches!(entry.terminator, Terminator::Branch { .. }),
            "expected guard Branch in entry block, got {:?}",
            entry.terminator
        );
    }

    #[test]
    fn infinite_loop_has_unconditional_jump() {
        let program = build_cfg("fn main() { loop { yield; } }");
        let main = get_function(&program, "main");
        assert!(main.blocks.len() >= 3);
        let header = &main.blocks[1];
        assert!(
            matches!(header.terminator, Terminator::Jump(_)),
            "expected Jump terminator for loop header, got {:?}",
            header.terminator
        );
    }

    #[test]
    fn for_loop_desugaring() {
        let program = build_cfg("fn main() { for i in 0..10 { yield; } }");
        let main = get_function(&program, "main");
        assert!(main.blocks.len() >= 4);
    }

    #[test]
    fn break_in_loop() {
        let program = build_cfg("fn main() { let x = true; loop { if x { break; } yield; } }");
        let main = get_function(&program, "main");
        let has_jump_terminator = main.blocks.iter().any(|b| {
            matches!(&b.terminator, Terminator::Jump(target) if {
                !main.blocks[target.0].predecessors.is_empty()
            })
        });
        assert!(has_jump_terminator);
    }

    #[test]
    fn return_statement_creates_return_terminator() {
        let program = build_cfg("fn add(a: f64, b: f64) -> f64 { return a + b; } fn main() {}");
        let add = get_function(&program, "add");
        let has_return = add
            .blocks
            .iter()
            .any(|b| matches!(&b.terminator, Terminator::Return(Some(_))));
        assert!(has_return);
    }

    #[test]
    fn device_io_lowers_to_load_store() {
        let program = build_cfg(
            "device sensor: d0; device heater: d1; fn main() { let t = sensor.Temperature; heater.On = 1.0; }",
        );
        let main = get_function(&program, "main");
        let instructions = &main.blocks[0].instructions;
        assert_eq!(instructions.len(), 4);
        assert!(matches!(
            &instructions[0],
            Instruction::LoadDevice { dest: TempId(0), pin: DevicePin::D0, field } if field == "Temperature"
        ));
        assert!(matches!(
            &instructions[1],
            Instruction::Assign {
                dest: TempId(1),
                operation: Operation::Copy(TempId(0))
            }
        ));
        assert!(matches!(
            &instructions[2],
            Instruction::Assign { dest: TempId(2), operation: Operation::Constant(v) } if *v == 1.0
        ));
        assert!(matches!(
            &instructions[3],
            Instruction::StoreDevice { pin: DevicePin::D1, field, source: TempId(2) } if field == "On"
        ));
    }

    #[test]
    fn yield_lowers_to_yield_instruction() {
        let program = build_cfg("fn main() { yield; }");
        let main = get_function(&program, "main");
        let instructions = &main.blocks[0].instructions;
        assert_eq!(instructions.len(), 1);
        assert!(matches!(&instructions[0], Instruction::Yield));
    }

    #[test]
    fn sleep_lowers_to_sleep_instruction() {
        let program = build_cfg("fn main() { sleep(2.5); }");
        let main = get_function(&program, "main");
        let instructions = &main.blocks[0].instructions;
        assert_eq!(instructions.len(), 2);
        assert!(matches!(
            &instructions[0],
            Instruction::Assign { dest: TempId(0), operation: Operation::Constant(v) } if *v == 2.5
        ));
        assert!(matches!(
            &instructions[1],
            Instruction::Sleep {
                duration: TempId(0)
            }
        ));
    }

    #[test]
    fn select_expression_lowers() {
        let program = build_cfg("fn main() { let x = select(true, 1.0, 2.0); }");
        let main = get_function(&program, "main");
        let instructions = &main.blocks[0].instructions;
        assert_eq!(instructions.len(), 5);
        assert!(matches!(
            &instructions[0],
            Instruction::Assign { dest: TempId(0), operation: Operation::Constant(v) } if *v == 1.0
        ));
        assert!(matches!(
            &instructions[1],
            Instruction::Assign { dest: TempId(1), operation: Operation::Constant(v) } if *v == 1.0
        ));
        assert!(matches!(
            &instructions[2],
            Instruction::Assign { dest: TempId(2), operation: Operation::Constant(v) } if *v == 2.0
        ));
        assert!(matches!(
            &instructions[3],
            Instruction::Assign {
                dest: TempId(3),
                operation: Operation::Select {
                    condition: TempId(0),
                    if_true: TempId(1),
                    if_false: TempId(2),
                },
            }
        ));
        assert!(matches!(
            &instructions[4],
            Instruction::Assign {
                dest: TempId(4),
                operation: Operation::Copy(TempId(3))
            }
        ));
    }

    #[test]
    fn dominator_tree_for_diamond() {
        let program = build_cfg(
            "fn main() { let x = true; if x { let y = 1; } else { let z = 2; } let w = 3; }",
        );
        let main = get_function(&program, "main");
        for block in &main.blocks {
            if block.id != main.entry {
                let mut dominated = block.id;
                let mut found_entry = false;
                for _ in 0..main.blocks.len() {
                    if dominated == main.entry {
                        found_entry = true;
                        break;
                    }
                    match main.immediate_dominators.get(&dominated) {
                        Some(&idom) => dominated = idom,
                        None => break,
                    }
                }
                assert!(found_entry, "block {:?} not dominated by entry", block.id);
            }
        }
    }

    #[test]
    fn dominance_frontiers_for_diamond() {
        let program = build_cfg(
            "fn main() { let x = true; if x { let y = 1; } else { let z = 2; } let w = 3; }",
        );
        let main = get_function(&program, "main");
        // In a diamond CFG the then and else blocks should have the merge block
        // in their dominance frontiers. Find the merge block: it's the one with 2 predecessors.
        let merge_block = main
            .blocks
            .iter()
            .find(|b| b.predecessors.len() == 2)
            .expect("expected a merge block with 2 predecessors");
        let has_frontier = main
            .dominance_frontiers
            .values()
            .any(|frontier| frontier.contains(&merge_block.id));
        assert!(
            has_frontier,
            "expected merge block in some dominance frontier"
        );
    }

    #[test]
    fn function_call_instruction() {
        let program = build_cfg("fn helper() {} fn main() { helper(); }");
        let helper = get_function(&program, "helper");
        let main = get_function(&program, "main");
        let instructions = &main.blocks[0].instructions;
        assert_eq!(instructions.len(), 1);
        assert!(matches!(
            &instructions[0],
            Instruction::Call { dest: None, function, args }
            if *function == helper.symbol_id && args.is_empty()
        ));
    }

    #[test]
    fn intrinsic_call_instruction() {
        let program = build_cfg("fn main() { let x = sqrt(4.0); }");
        let main = get_function(&program, "main");
        let instructions = &main.blocks[0].instructions;
        assert_eq!(instructions.len(), 3);
        assert!(matches!(
            &instructions[0],
            Instruction::Assign { dest: TempId(0), operation: Operation::Constant(v) } if *v == 4.0
        ));
        assert!(matches!(
            &instructions[1],
            Instruction::IntrinsicCall { dest: TempId(1), function: Intrinsic::Sqrt, args }
            if args == &[TempId(0)]
        ));
        assert!(matches!(
            &instructions[2],
            Instruction::Assign {
                dest: TempId(2),
                operation: Operation::Copy(TempId(1))
            }
        ));
    }

    #[test]
    fn binary_expression_lowering() {
        let program = build_cfg("fn main() { let x = 1 + 2; }");
        let main = get_function(&program, "main");
        let instructions = &main.blocks[0].instructions;
        assert_eq!(instructions.len(), 4);
        assert!(matches!(
            &instructions[0],
            Instruction::Assign { dest: TempId(0), operation: Operation::Constant(v) } if *v == 1.0
        ));
        assert!(matches!(
            &instructions[1],
            Instruction::Assign { dest: TempId(1), operation: Operation::Constant(v) } if *v == 2.0
        ));
        assert!(matches!(
            &instructions[2],
            Instruction::Assign {
                dest: TempId(2),
                operation: Operation::Binary {
                    operator: BinaryOperator::Add,
                    left: TempId(0),
                    right: TempId(1)
                },
            }
        ));
        assert!(matches!(
            &instructions[3],
            Instruction::Assign {
                dest: TempId(3),
                operation: Operation::Copy(TempId(2))
            }
        ));
    }

    #[test]
    fn cast_expression_lowering() {
        let program = build_cfg("fn main() { let x: f64 = 42 as f64; }");
        let main = get_function(&program, "main");
        let instructions = &main.blocks[0].instructions;
        assert_eq!(instructions.len(), 3);
        assert!(matches!(
            &instructions[0],
            Instruction::Assign { dest: TempId(0), operation: Operation::Constant(v) } if *v == 42.0
        ));
        assert!(matches!(
            &instructions[1],
            Instruction::Assign {
                dest: TempId(1),
                operation: Operation::Cast {
                    operand: TempId(0),
                    target_type: Type::F64,
                    source_type: Type::I53
                },
            }
        ));
        assert!(matches!(
            &instructions[2],
            Instruction::Assign {
                dest: TempId(2),
                operation: Operation::Copy(TempId(1))
            }
        ));
    }

    #[test]
    fn predecessor_successor_edges() {
        let program = build_cfg("fn main() { let x = true; if x { let y = 1; } }");
        let main = get_function(&program, "main");
        for block in &main.blocks {
            for &successor in &block.successors {
                assert!(
                    main.blocks[successor.0].predecessors.contains(&block.id),
                    "block {:?} successor {:?} doesn't have it as predecessor",
                    block.id,
                    successor
                );
            }
            for &predecessor in &block.predecessors {
                assert!(
                    main.blocks[predecessor.0].successors.contains(&block.id),
                    "block {:?} predecessor {:?} doesn't have it as successor",
                    block.id,
                    predecessor
                );
            }
        }
    }

    #[test]
    fn continue_in_for_loop() {
        let program = build_cfg("fn main() { for i in 0..5 { if i == 2 { continue; } yield; } }");
        let main = get_function(&program, "main");
        assert!(main.blocks.len() >= 5);
    }

    #[test]
    fn nested_loops() {
        let program =
            build_cfg("fn main() { loop { let mut x = true; while x { x = false; } yield; } }");
        let main = get_function(&program, "main");
        assert!(main.blocks.len() >= 5);
    }

    #[test]
    fn else_if_chain() {
        let program = build_cfg(
            r#"fn main() {
                let x: i53 = 1;
                if x == 1 {
                    yield;
                } else if x == 2 {
                    yield;
                } else {
                    yield;
                }
            }"#,
        );
        let main = get_function(&program, "main");
        assert!(main.blocks.len() >= 5);
    }

    #[test]
    fn slot_read_and_write() {
        let program = build_cfg(
            "device fab: d0; fn main() { let x = fab.slot(0).Occupied; fab.slot(1).Lock = 1.0; }",
        );
        let main = get_function(&program, "main");
        let instructions = &main.blocks[0].instructions;
        assert_eq!(instructions.len(), 6);
        assert!(matches!(
            &instructions[0],
            Instruction::Assign { dest: TempId(0), operation: Operation::Constant(v) } if *v == 0.0
        ));
        assert!(matches!(
            &instructions[1],
            Instruction::LoadSlot { dest: TempId(1), pin: DevicePin::D0, slot: TempId(0), field }
            if field == "Occupied"
        ));
        assert!(matches!(
            &instructions[2],
            Instruction::Assign {
                dest: TempId(2),
                operation: Operation::Copy(TempId(1))
            }
        ));
        assert!(matches!(
            &instructions[3],
            Instruction::Assign { dest: TempId(3), operation: Operation::Constant(v) } if *v == 1.0
        ));
        assert!(matches!(
            &instructions[4],
            Instruction::Assign { dest: TempId(4), operation: Operation::Constant(v) } if *v == 1.0
        ));
        assert!(matches!(
            &instructions[5],
            Instruction::StoreSlot { pin: DevicePin::D0, slot: TempId(3), field, source: TempId(4) }
            if field == "Lock"
        ));
    }

    #[test]
    fn batch_operations() {
        let program = build_cfg(
            r#"const BATTERY: f64 = hash("StructureBattery");
               fn main() { let avg = batch_read(BATTERY, Ratio, Average); }"#,
        );
        let main = get_function(&program, "main");
        let instructions = &main.blocks[0].instructions;
        assert_eq!(instructions.len(), 3);
        assert!(matches!(
            &instructions[0],
            Instruction::Assign {
                dest: TempId(0),
                operation: Operation::Constant(_)
            }
        ));
        assert!(matches!(
            &instructions[1],
            Instruction::BatchRead { dest: TempId(1), hash: TempId(0), field, mode: BatchMode::Average }
            if field == "Ratio"
        ));
        assert!(matches!(
            &instructions[2],
            Instruction::Assign {
                dest: TempId(2),
                operation: Operation::Copy(TempId(1))
            }
        ));
    }

    #[test]
    fn multiple_functions() {
        let program =
            build_cfg("fn helper(x: f64) -> f64 { return x; } fn main() { let y = helper(1.0); }");
        assert_eq!(program.functions.len(), 2);
        let helper = get_function(&program, "helper");
        assert!(
            helper
                .blocks
                .iter()
                .any(|b| matches!(&b.terminator, Terminator::Return(Some(_))))
        );
    }

    #[test]
    fn unary_expression_lowering() {
        let program = build_cfg("fn main() { let x: i53 = -42; }");
        let main = get_function(&program, "main");
        let instructions = &main.blocks[0].instructions;
        assert_eq!(instructions.len(), 3);
        assert!(matches!(
            &instructions[0],
            Instruction::Assign { dest: TempId(0), operation: Operation::Constant(v) } if *v == 42.0
        ));
        assert!(matches!(
            &instructions[1],
            Instruction::Assign {
                dest: TempId(1),
                operation: Operation::Unary {
                    operator: UnaryOperator::Neg,
                    operand: TempId(0)
                },
            }
        ));
        assert!(matches!(
            &instructions[2],
            Instruction::Assign {
                dest: TempId(2),
                operation: Operation::Copy(TempId(1))
            }
        ));
    }

    #[test]
    fn variable_reassignment_creates_new_temp() {
        let program = build_cfg("fn main() { let mut x = 1; x = 2; }");
        let main = get_function(&program, "main");
        let instructions = &main.blocks[0].instructions;
        assert_eq!(instructions.len(), 4);
        assert!(matches!(
            &instructions[0],
            Instruction::Assign { dest: TempId(0), operation: Operation::Constant(v) } if *v == 1.0
        ));
        assert!(matches!(
            &instructions[1],
            Instruction::Assign {
                dest: TempId(1),
                operation: Operation::Copy(TempId(0))
            }
        ));
        assert!(matches!(
            &instructions[2],
            Instruction::Assign { dest: TempId(2), operation: Operation::Constant(v) } if *v == 2.0
        ));
        assert!(matches!(
            &instructions[3],
            Instruction::Assign {
                dest: TempId(3),
                operation: Operation::Copy(TempId(2))
            }
        ));
    }

    #[test]
    fn unreachable_code_after_break() {
        let (_, diagnostics) = build_cfg_with_diagnostics("fn main() { loop { break; yield; } }");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message, "unreachable code");
        assert_eq!(
            diagnostics[0].severity,
            crate::diagnostic::Severity::Warning
        );
    }

    #[test]
    fn unreachable_code_after_continue() {
        let (_, diagnostics) =
            build_cfg_with_diagnostics("fn main() { loop { continue; yield; } }");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message, "unreachable code");
    }

    #[test]
    fn unreachable_code_after_return() {
        let (_, diagnostics) = build_cfg_with_diagnostics("fn main() { return; yield; }");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message, "unreachable code");
    }

    #[test]
    fn no_warning_when_break_is_last() {
        let (_, diagnostics) = build_cfg_with_diagnostics("fn main() { loop { yield; break; } }");
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn no_warning_when_return_is_last() {
        let (_, diagnostics) =
            build_cfg_with_diagnostics("fn foo() -> f64 { return 1.0; } fn main() {}");
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn unreachable_multiple_statements_single_warning() {
        let (_, diagnostics) =
            build_cfg_with_diagnostics("fn main() { loop { break; yield; yield; } }");
        assert_eq!(
            diagnostics.len(),
            1,
            "only one warning for the first unreachable statement"
        );
    }

    #[test]
    fn labeled_break_targets_outer_loop() {
        let program = build_cfg("fn main() { 'outer: loop { loop { break 'outer; } } }");
        let main = get_function(&program, "main");
        let outer_exit = main
            .blocks
            .iter()
            .find(|b| matches!(b.role, BlockRole::LoopEnd(1)))
            .expect("outer loop should have an exit block");
        let inner_body = main
            .blocks
            .iter()
            .find(|b| matches!(b.role, BlockRole::LoopStart(2)))
            .expect("inner loop body should exist");
        match &inner_body.terminator {
            Terminator::Jump(target) => {
                assert_eq!(
                    target.0,
                    main.blocks
                        .iter()
                        .position(|b| std::ptr::eq(b, outer_exit))
                        .unwrap(),
                    "break 'outer should jump to outer loop exit"
                );
            }
            other => panic!("expected Jump terminator, got {:?}", other),
        }
    }

    #[test]
    fn labeled_continue_targets_outer_loop() {
        let program = build_cfg("fn main() { 'outer: loop { loop { continue 'outer; } yield; } }");
        let main = get_function(&program, "main");
        let outer_continue = main
            .blocks
            .iter()
            .find(|b| matches!(b.role, BlockRole::LoopContinue(1)))
            .expect("outer loop should have a continue block");
        let inner_body = main
            .blocks
            .iter()
            .find(|b| matches!(b.role, BlockRole::LoopStart(2)))
            .expect("inner loop body should exist");
        match &inner_body.terminator {
            Terminator::Jump(target) => {
                assert_eq!(
                    target.0,
                    main.blocks
                        .iter()
                        .position(|b| std::ptr::eq(b, outer_continue))
                        .unwrap(),
                    "continue 'outer should jump to outer loop continue"
                );
            }
            other => panic!("expected Jump terminator, got {:?}", other),
        }
    }

    #[test]
    fn labeled_break_in_for_loop() {
        let program =
            build_cfg("fn main() { 'outer: for i in 0..3 { for j in 0..3 { break 'outer; } } }");
        let main = get_function(&program, "main");
        assert!(
            main.blocks
                .iter()
                .any(|b| matches!(b.role, BlockRole::LoopEnd(1))),
            "outer for-loop should have an exit block"
        );
    }

    #[test]
    fn unlabeled_break_still_targets_inner_loop() {
        let program = build_cfg("fn main() { 'outer: loop { loop { break; } yield; } }");
        let main = get_function(&program, "main");
        let inner_exit = main
            .blocks
            .iter()
            .find(|b| matches!(b.role, BlockRole::LoopEnd(2)))
            .expect("inner loop should have an exit block");
        let inner_body = main
            .blocks
            .iter()
            .find(|b| matches!(b.role, BlockRole::LoopStart(2)))
            .expect("inner loop body should exist");
        match &inner_body.terminator {
            Terminator::Jump(target) => {
                assert_eq!(
                    target.0,
                    main.blocks
                        .iter()
                        .position(|b| std::ptr::eq(b, inner_exit))
                        .unwrap(),
                    "unlabeled break should jump to inner loop exit, not outer"
                );
            }
            other => panic!("expected Jump terminator, got {:?}", other),
        }
    }
}
