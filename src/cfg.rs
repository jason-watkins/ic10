//! CFG builder — lowers the bound IR into a control-flow graph of basic blocks.

use std::collections::HashMap;

use crate::diagnostic::{Diagnostic, Span};
use crate::ir::bound::{
    self, AssignmentTarget, BatchWriteStatement, ElseClause, ExpressionKind,
    Program as BoundProgram, Statement, SymbolId, SymbolKind,
};
use crate::ir::cfg::{
    BasicBlock, BlockId, BlockRole, Function, Instruction, Operation, Program, TempId, Terminator,
};
use crate::ir::{BinaryOperator, Type};

/// Loop context for break/continue targeting.
///
/// Pushed onto `Builder::loop_stack` when entering a loop; popped on exit.
struct LoopContext {
    /// Optional user-defined label (`'name`).
    label: Option<String>,
    /// Block to jump to on `continue`.
    continue_target: BlockId,
    /// Block to jump to on `break`.
    break_target: BlockId,
}

/// Builds a CFG `Function` from a `bound::FunctionDeclaration`.
struct Builder<'a> {
    blocks: Vec<BasicBlock>,
    next_temp: usize,
    current_block: BlockId,
    loop_stack: Vec<LoopContext>,
    variable_temps: HashMap<SymbolId, TempId>,
    variable_definitions: HashMap<SymbolId, Vec<(TempId, BlockId)>>,
    symbols: &'a bound::SymbolTable,
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

impl<'a> Builder<'a> {
    /// Creates a new CFG builder for a single function.
    fn new(symbols: &'a bound::SymbolTable) -> Self {
        Self {
            blocks: Vec::new(),
            next_temp: 0,
            current_block: BlockId(0),
            loop_stack: Vec::new(),
            variable_temps: HashMap::new(),
            variable_definitions: HashMap::new(),
            symbols,
            diagnostics: Vec::new(),
            unreachable_after: None,
            next_if_index: 0,
            next_loop_index: 0,
        }
    }

    /// Allocate a fresh `TempId`.
    fn fresh_temp(&mut self) -> TempId {
        let id = TempId(self.next_temp);
        self.next_temp += 1;
        id
    }

    /// Create a new empty basic block and return its `BlockId`.
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

    /// Append an instruction to the current block.
    fn emit(&mut self, instruction: Instruction) {
        self.blocks[self.current_block.0]
            .instructions
            .push(instruction);
    }

    /// Set the terminator of the current block.
    fn set_terminator(&mut self, terminator: Terminator) {
        let block = &mut self.blocks[self.current_block.0];
        block.terminator = terminator;
    }

    /// Switch the insertion point to a different block.
    fn switch_to(&mut self, block: BlockId) {
        self.current_block = block;
    }

    /// Add a control-flow edge from `from` to `to`, updating both
    /// predecessor and successor lists.
    fn add_edge(&mut self, from: BlockId, to: BlockId) {
        self.blocks[from.0].successors.push(to);
        self.blocks[to.0].predecessors.push(from);
    }

    /// Set the current block's terminator to an unconditional jump and add the edge.
    fn terminate_and_jump(&mut self, target: BlockId) {
        self.set_terminator(Terminator::Jump(target));
        self.add_edge(self.current_block, target);
    }

    /// Set the current block's terminator to a conditional branch and add both edges.
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

    /// Record that `symbol_id` is now held in `temp`, for SSA rename tracking.
    fn record_variable_definition(&mut self, symbol_id: SymbolId, temp: TempId) {
        self.variable_temps.insert(symbol_id, temp);
        self.variable_definitions
            .entry(symbol_id)
            .or_default()
            .push((temp, self.current_block));
    }

    /// Lower a bound expression to a sequence of three-address instructions,
    /// returning the `TempId` that holds the result.
    fn lower_expression(&mut self, expression: &bound::Expression) -> TempId {
        match &expression.kind {
            ExpressionKind::Literal(value) => {
                let target = self.fresh_temp();
                self.emit(Instruction::Assign {
                    target,
                    operation: Operation::Constant(*value),
                });
                target
            }

            ExpressionKind::Variable(symbol_id) => {
                if let SymbolKind::Static(static_id) = self.symbols.get(*symbol_id).kind {
                    let target = self.fresh_temp();
                    self.emit(Instruction::LoadStatic { target, static_id });
                    target
                } else {
                    let source = self.variable_temps[symbol_id];
                    let target = self.fresh_temp();
                    self.emit(Instruction::Assign {
                        target,
                        operation: Operation::Copy(source),
                    });
                    target
                }
            }

            ExpressionKind::Binary(operator, left, right) => {
                let left_temp = self.lower_expression(left);
                let right_temp = self.lower_expression(right);
                let target = self.fresh_temp();
                self.emit(Instruction::Assign {
                    target,
                    operation: Operation::Binary {
                        operator: *operator,
                        left: left_temp,
                        right: right_temp,
                    },
                });
                target
            }

            ExpressionKind::Unary(operator, operand) => {
                let operand_temp = self.lower_expression(operand);
                let target = self.fresh_temp();
                self.emit(Instruction::Assign {
                    target,
                    operation: Operation::Unary {
                        operator: *operator,
                        operand: operand_temp,
                    },
                });
                target
            }

            ExpressionKind::Cast(inner, target_type) => {
                let operand_temp = self.lower_expression(inner);
                let target = self.fresh_temp();
                self.emit(Instruction::Assign {
                    target,
                    operation: Operation::Cast {
                        operand: operand_temp,
                        target_type: *target_type,
                        source_type: inner.ty,
                    },
                });
                target
            }

            ExpressionKind::Call(function_symbol, args) => {
                let arg_temps: Vec<TempId> =
                    args.iter().map(|a| self.lower_expression(a)).collect();
                let target = if expression.ty != Type::Unit {
                    Some(self.fresh_temp())
                } else {
                    None
                };
                self.emit(Instruction::Call {
                    target,
                    function: *function_symbol,
                    args: arg_temps,
                });
                // `lower_expression` is only called on a Call node when the call appears
                // as a sub-expression (argument, RHS, etc.). The binder rejects any use
                // of a unit-returning call as a value, so `target` is always `Some` here.
                target.expect(
                    "unit-returning call reached lower_expression; binder invariant violated",
                )
            }

            ExpressionKind::IntrinsicCall(function, args) => {
                let arg_temps: Vec<TempId> =
                    args.iter().map(|a| self.lower_expression(a)).collect();
                let target = self.fresh_temp();
                self.emit(Instruction::IntrinsicCall {
                    target,
                    function: *function,
                    args: arg_temps,
                });
                target
            }

            ExpressionKind::DeviceRead { pin, field } => {
                let target = self.fresh_temp();
                self.emit(Instruction::LoadDevice {
                    target,
                    pin: *pin,
                    field: field.clone(),
                });
                target
            }

            ExpressionKind::SlotRead { pin, slot, field } => {
                let slot_temp = self.lower_expression(slot);
                let target = self.fresh_temp();
                self.emit(Instruction::LoadSlot {
                    target,
                    pin: *pin,
                    slot: slot_temp,
                    field: field.clone(),
                });
                target
            }

            ExpressionKind::BatchRead {
                hash_expr,
                field,
                mode,
            } => {
                let hash_temp = self.lower_expression(hash_expr);
                let target = self.fresh_temp();
                self.emit(Instruction::BatchRead {
                    target,
                    hash: hash_temp,
                    field: field.clone(),
                    mode: *mode,
                });
                target
            }

            ExpressionKind::Select {
                condition,
                if_true,
                if_false,
            } => {
                let cond_temp = self.lower_expression(condition);
                let true_temp = self.lower_expression(if_true);
                let false_temp = self.lower_expression(if_false);
                let target = self.fresh_temp();
                self.emit(Instruction::Assign {
                    target,
                    operation: Operation::Select {
                        condition: cond_temp,
                        if_true: true_temp,
                        if_false: false_temp,
                    },
                });
                target
            }
        }
    }

    /// Lower a bound statement to CFG instructions and control-flow edges.
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
                let target = self.fresh_temp();
                self.emit(Instruction::Assign {
                    target,
                    operation: Operation::Copy(init_temp),
                });
                self.record_variable_definition(let_statement.symbol_id, target);
            }

            Statement::Assign(assign_statement) => match &assign_statement.target {
                AssignmentTarget::Variable { symbol_id, .. } => {
                    let value_temp = self.lower_expression(&assign_statement.value);
                    if let SymbolKind::Static(static_id) = self.symbols.get(*symbol_id).kind {
                        self.emit(Instruction::StoreStatic {
                            static_id,
                            source: value_temp,
                        });
                    } else {
                        let target = self.fresh_temp();
                        self.emit(Instruction::Assign {
                            target,
                            operation: Operation::Copy(value_temp),
                        });
                        self.record_variable_definition(*symbol_id, target);
                    }
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

    /// Lower a call expression used as a statement (discards any result for void calls).
    fn lower_expression_statement(&mut self, statement: &bound::ExpressionStatement) {
        match &statement.expression.kind {
            ExpressionKind::Call(function_symbol, args) => {
                let arg_temps: Vec<TempId> =
                    args.iter().map(|a| self.lower_expression(a)).collect();
                let target = if statement.expression.ty != Type::Unit {
                    Some(self.fresh_temp())
                } else {
                    None
                };
                self.emit(Instruction::Call {
                    target,
                    function: *function_symbol,
                    args: arg_temps,
                });
            }
            ExpressionKind::IntrinsicCall(function, args) => {
                let arg_temps: Vec<TempId> =
                    args.iter().map(|a| self.lower_expression(a)).collect();
                let target = self.fresh_temp();
                self.emit(Instruction::IntrinsicCall {
                    target,
                    function: *function,
                    args: arg_temps,
                });
            }
            _ => {
                self.lower_expression(&statement.expression);
            }
        }
    }

    /// Lower an `if` statement to conditional branches and merge blocks.
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
                target: t,
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
                    target: loop_var,
                    operation: Operation::Copy(upper_temp),
                });
                bound_temp = lower_temp;
            } else {
                // (a..b).rev(): start at b - step, count down to a
                let start = self.fresh_temp();
                self.emit(Instruction::Assign {
                    target: start,
                    operation: Operation::Binary {
                        operator: BinaryOperator::Sub,
                        left: upper_temp,
                        right: step_temp,
                    },
                });
                self.emit(Instruction::Assign {
                    target: loop_var,
                    operation: Operation::Copy(start),
                });
                bound_temp = lower_temp;
            }
        } else {
            // Ascending: start at lower, bound is upper
            self.emit(Instruction::Assign {
                target: loop_var,
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
            target: guard_cond,
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
            target: incremented,
            operation: Operation::Binary {
                operator: increment_operator,
                left: current_var,
                right: step_temp,
            },
        });
        self.record_variable_definition(for_statement.variable, incremented);

        let back_cond = self.fresh_temp();
        self.emit(Instruction::Assign {
            target: back_cond,
            operation: Operation::Binary {
                operator: back_edge_operator,
                left: incremented,
                right: bound_temp,
            },
        });
        self.terminate_and_branch(back_cond, body_block, exit_block);

        self.switch_to(exit_block);
    }

    /// Lower each statement in a block. Resets the unreachable tracker on exit.
    fn lower_block(&mut self, block: &bound::Block) {
        for statement in &block.statements {
            self.lower_statement(statement);
        }
        self.unreachable_after = None;
    }

    /// Returns `true` if the current block has no terminator set yet.
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
    bound_program: &BoundProgram,
    diagnostics: &mut Vec<Diagnostic>,
) -> Function {
    let mut builder = Builder::new(&bound_program.symbols);
    let entry = builder.new_block();
    builder.blocks[entry.0].role = BlockRole::Entry;
    builder.switch_to(entry);

    let mut parameter_ids = Vec::new();
    for (index, parameter) in function.parameters.iter().enumerate() {
        let temp = builder.fresh_temp();
        builder.emit(Instruction::Assign {
            target: temp,
            operation: Operation::Parameter { index },
        });
        builder.record_variable_definition(parameter.symbol_id, temp);
        parameter_ids.push(parameter.symbol_id);
    }

    if function.name == "main" {
        for initializer in &bound_program.static_initializers {
            let value_temp = builder.lower_expression(&initializer.expression);
            builder.emit(Instruction::StoreStatic {
                static_id: initializer.static_id,
                source: value_temp,
            });
        }
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
        .map(|f| lower_function(f, bound_program, &mut diagnostics))
        .collect();
    (
        Program {
            functions,
            statics: bound_program.statics.clone(),
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
            Instruction::Assign { target: TempId(0), operation: Operation::Constant(v) } if *v == 42.0
        ));
        assert!(matches!(
            &instructions[1],
            Instruction::Assign {
                target: TempId(1),
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
        assert_eq!(main.blocks.len(), 4);
        assert!(matches!(main.blocks[0].role, BlockRole::Entry));
        assert!(matches!(main.blocks[1].role, BlockRole::LoopStart(_)));
        assert!(matches!(main.blocks[2].role, BlockRole::LoopContinue(_)));
        assert!(matches!(main.blocks[3].role, BlockRole::LoopEnd(_)));

        let init = &main.blocks[0].instructions;
        assert!(
            matches!(&init[0], Instruction::Assign { operation: Operation::Constant(v), .. } if *v == 0.0)
        );
        assert!(
            matches!(&init[1], Instruction::Assign { operation: Operation::Constant(v), .. } if *v == 10.0)
        );
        assert!(
            matches!(&init[2], Instruction::Assign { operation: Operation::Constant(v), .. } if *v == 1.0)
        );

        assert!(
            main.blocks[1]
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::Yield))
        );

        let cont = &main.blocks[2].instructions;
        assert!(cont.iter().any(|i| matches!(
            i,
            Instruction::Assign {
                operation: Operation::Binary {
                    operator: BinaryOperator::Add,
                    ..
                },
                ..
            }
        )));
    }

    #[test]
    fn break_in_loop() {
        let program = build_cfg("fn main() { let x = true; loop { if x { break; } yield; } }");
        let main = get_function(&program, "main");

        let exit_block = main
            .blocks
            .iter()
            .find(|b| matches!(b.role, BlockRole::LoopEnd(_)))
            .expect("should have a LoopEnd block");

        let break_block = main
            .blocks
            .iter()
            .find(|b| matches!(b.role, BlockRole::IfTrue(_)))
            .expect("should have an IfTrue block (the break branch)");

        assert!(
            matches!(break_block.terminator, Terminator::Jump(target) if target == exit_block.id),
            "break should jump to the loop exit block"
        );
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
            Instruction::LoadDevice { target: TempId(0), pin: DevicePin::D0, field } if field == "Temperature"
        ));
        assert!(matches!(
            &instructions[1],
            Instruction::Assign {
                target: TempId(1),
                operation: Operation::Copy(TempId(0))
            }
        ));
        assert!(matches!(
            &instructions[2],
            Instruction::Assign { target: TempId(2), operation: Operation::Constant(v) } if *v == 1.0
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
            Instruction::Assign { target: TempId(0), operation: Operation::Constant(v) } if *v == 2.5
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
            Instruction::Assign { target: TempId(0), operation: Operation::Constant(v) } if *v == 1.0
        ));
        assert!(matches!(
            &instructions[1],
            Instruction::Assign { target: TempId(1), operation: Operation::Constant(v) } if *v == 1.0
        ));
        assert!(matches!(
            &instructions[2],
            Instruction::Assign { target: TempId(2), operation: Operation::Constant(v) } if *v == 2.0
        ));
        assert!(matches!(
            &instructions[3],
            Instruction::Assign {
                target: TempId(3),
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
                target: TempId(4),
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
            Instruction::Call { target: None, function, args }
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
            Instruction::Assign { target: TempId(0), operation: Operation::Constant(v) } if *v == 4.0
        ));
        assert!(matches!(
            &instructions[1],
            Instruction::IntrinsicCall { target: TempId(1), function: Intrinsic::Sqrt, args }
            if args == &[TempId(0)]
        ));
        assert!(matches!(
            &instructions[2],
            Instruction::Assign {
                target: TempId(2),
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
            Instruction::Assign { target: TempId(0), operation: Operation::Constant(v) } if *v == 1.0
        ));
        assert!(matches!(
            &instructions[1],
            Instruction::Assign { target: TempId(1), operation: Operation::Constant(v) } if *v == 2.0
        ));
        assert!(matches!(
            &instructions[2],
            Instruction::Assign {
                target: TempId(2),
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
                target: TempId(3),
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
            Instruction::Assign { target: TempId(0), operation: Operation::Constant(v) } if *v == 42.0
        ));
        assert!(matches!(
            &instructions[1],
            Instruction::Assign {
                target: TempId(1),
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
                target: TempId(2),
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

        let continue_block = main
            .blocks
            .iter()
            .find(|b| matches!(b.role, BlockRole::LoopContinue(_)))
            .expect("should have a LoopContinue block");

        let if_true_block = main
            .blocks
            .iter()
            .find(|b| matches!(b.role, BlockRole::IfTrue(_)))
            .expect("should have an IfTrue block (the continue branch)");

        assert!(
            matches!(if_true_block.terminator, Terminator::Jump(target) if target == continue_block.id),
            "continue should jump to the loop continue (increment) block"
        );
    }

    #[test]
    fn nested_loops() {
        let program =
            build_cfg("fn main() { loop { let mut x = true; while x { x = false; } yield; } }");
        let main = get_function(&program, "main");
        assert_eq!(main.blocks.len(), 7);

        let outer_loop_starts: Vec<_> = main
            .blocks
            .iter()
            .filter(|b| matches!(b.role, BlockRole::LoopStart(1)))
            .collect();
        let inner_loop_starts: Vec<_> = main
            .blocks
            .iter()
            .filter(|b| matches!(b.role, BlockRole::LoopStart(2)))
            .collect();
        assert_eq!(
            outer_loop_starts.len(),
            1,
            "should have one outer LoopStart"
        );
        assert_eq!(
            inner_loop_starts.len(),
            1,
            "should have one inner LoopStart"
        );

        let outer_continue = main
            .blocks
            .iter()
            .find(|b| matches!(b.role, BlockRole::LoopContinue(1)))
            .unwrap();
        assert!(
            matches!(outer_continue.terminator, Terminator::Jump(target) if target == outer_loop_starts[0].id),
            "outer continue should jump back to outer loop start"
        );

        let inner_end = main
            .blocks
            .iter()
            .find(|b| matches!(b.role, BlockRole::LoopEnd(2)))
            .unwrap();
        assert!(
            matches!(inner_end.terminator, Terminator::Jump(target) if target == outer_continue.id),
            "inner loop exit should flow to outer loop continue"
        );
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
        assert_eq!(main.blocks.len(), 7);

        assert!(matches!(main.blocks[0].role, BlockRole::Entry));
        assert!(matches!(
            main.blocks[0].terminator,
            Terminator::Branch { .. }
        ));

        let if_true_count = main
            .blocks
            .iter()
            .filter(|b| matches!(b.role, BlockRole::IfTrue(_)))
            .count();
        let if_false_count = main
            .blocks
            .iter()
            .filter(|b| matches!(b.role, BlockRole::IfFalse(_)))
            .count();
        assert_eq!(if_true_count, 2, "two then-branches (if and else-if)");
        assert_eq!(if_false_count, 2, "two else-branches (else-if and else)");

        let outer_else = main
            .blocks
            .iter()
            .find(|b| matches!(b.role, BlockRole::IfFalse(1)))
            .unwrap();
        assert!(
            matches!(outer_else.terminator, Terminator::Branch { .. }),
            "the else-if block should branch again"
        );
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
            Instruction::Assign { target: TempId(0), operation: Operation::Constant(v) } if *v == 0.0
        ));
        assert!(matches!(
            &instructions[1],
            Instruction::LoadSlot { target: TempId(1), pin: DevicePin::D0, slot: TempId(0), field }
            if field == "Occupied"
        ));
        assert!(matches!(
            &instructions[2],
            Instruction::Assign {
                target: TempId(2),
                operation: Operation::Copy(TempId(1))
            }
        ));
        assert!(matches!(
            &instructions[3],
            Instruction::Assign { target: TempId(3), operation: Operation::Constant(v) } if *v == 1.0
        ));
        assert!(matches!(
            &instructions[4],
            Instruction::Assign { target: TempId(4), operation: Operation::Constant(v) } if *v == 1.0
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
                target: TempId(0),
                operation: Operation::Constant(_)
            }
        ));
        assert!(matches!(
            &instructions[1],
            Instruction::BatchRead { target: TempId(1), hash: TempId(0), field, mode: BatchMode::Average }
            if field == "Ratio"
        ));
        assert!(matches!(
            &instructions[2],
            Instruction::Assign {
                target: TempId(2),
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
            Instruction::Assign { target: TempId(0), operation: Operation::Constant(v) } if *v == 42.0
        ));
        assert!(matches!(
            &instructions[1],
            Instruction::Assign {
                target: TempId(1),
                operation: Operation::Unary {
                    operator: UnaryOperator::Neg,
                    operand: TempId(0)
                },
            }
        ));
        assert!(matches!(
            &instructions[2],
            Instruction::Assign {
                target: TempId(2),
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
            Instruction::Assign { target: TempId(0), operation: Operation::Constant(v) } if *v == 1.0
        ));
        assert!(matches!(
            &instructions[1],
            Instruction::Assign {
                target: TempId(1),
                operation: Operation::Copy(TempId(0))
            }
        ));
        assert!(matches!(
            &instructions[2],
            Instruction::Assign { target: TempId(2), operation: Operation::Constant(v) } if *v == 2.0
        ));
        assert!(matches!(
            &instructions[3],
            Instruction::Assign {
                target: TempId(3),
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

        let outer_exit = main
            .blocks
            .iter()
            .find(|b| matches!(b.role, BlockRole::LoopEnd(1)))
            .expect("outer for-loop should have an exit block");

        let inner_body = main
            .blocks
            .iter()
            .find(|b| matches!(b.role, BlockRole::LoopBody(2) | BlockRole::LoopStart(2)))
            .expect("inner loop body should exist");

        match &inner_body.terminator {
            Terminator::Jump(target) => {
                assert_eq!(
                    target.0,
                    main.blocks
                        .iter()
                        .position(|b| std::ptr::eq(b, outer_exit))
                        .unwrap(),
                    "break 'outer should jump to outer for-loop exit"
                );
            }
            other => panic!("expected Jump terminator for break, got {:?}", other),
        }
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

    #[test]
    fn labeled_while_loop_break_targets_exit() {
        let program = build_cfg("fn main() { 'outer: while true { break 'outer; } }");
        let main = get_function(&program, "main");
        let loop_end = main
            .blocks
            .iter()
            .position(|b| matches!(b.role, BlockRole::LoopEnd(_)))
            .expect("should have a LoopEnd block");
        let loop_body = main
            .blocks
            .iter()
            .find(|b| matches!(b.role, BlockRole::LoopStart(_)))
            .expect("should have a LoopStart block");
        match &loop_body.terminator {
            Terminator::Jump(target) => assert_eq!(target.0, loop_end),
            other => panic!("expected Jump to loop exit, got {:?}", other),
        }
    }

    #[test]
    fn labeled_while_loop_continue_targets_header() {
        let program = build_cfg("fn main() { 'outer: while true { continue 'outer; } }");
        let main = get_function(&program, "main");
        let loop_start_idx = main
            .blocks
            .iter()
            .position(|b| matches!(b.role, BlockRole::LoopStart(_)))
            .expect("should have a LoopStart block");
        let loop_body = &main.blocks[loop_start_idx];
        match &loop_body.terminator {
            Terminator::Jump(target) => {
                let target_block = &main.blocks[target.0];
                assert!(
                    matches!(
                        target_block.role,
                        BlockRole::LoopStart(_) | BlockRole::LoopContinue(_)
                    ),
                    "continue should jump to loop header or continue block, got {:?}",
                    target_block.role,
                );
            }
            other => panic!("expected Jump for continue, got {:?}", other),
        }
    }

    #[test]
    fn batch_write_lowering() {
        let program =
            build_cfg(r#"fn main() { batch_write(hash("StructureWallType"), On, 1.0); }"#);
        let main = get_function(&program, "main");
        let has_batch_write = main.blocks.iter().any(|b| {
            b.instructions
                .iter()
                .any(|i| matches!(i, Instruction::BatchWrite { field, .. } if field == "On"))
        });
        assert!(
            has_batch_write,
            "should lower batch_write to BatchWrite instruction"
        );
    }

    #[test]
    fn logical_and_lowers_to_and_instruction() {
        let program = build_cfg(
            "device out: d0; fn main() { let a: bool = true; let b: bool = false; out.Setting = a && b; }",
        );
        let main = get_function(&program, "main");
        let has_and = main.blocks.iter().any(|b| {
            b.instructions.iter().any(|i| {
                matches!(
                    i,
                    Instruction::Assign {
                        operation: Operation::Binary {
                            operator: BinaryOperator::And,
                            ..
                        },
                        ..
                    }
                )
            })
        });
        assert!(has_and, "a && b should lower to BinaryOperator::And");
    }

    #[test]
    fn logical_or_lowers_to_or_instruction() {
        let program = build_cfg(
            "device out: d0; fn main() { let a: bool = true; let b: bool = false; out.Setting = a || b; }",
        );
        let main = get_function(&program, "main");
        let has_or = main.blocks.iter().any(|b| {
            b.instructions.iter().any(|i| {
                matches!(
                    i,
                    Instruction::Assign {
                        operation: Operation::Binary {
                            operator: BinaryOperator::Or,
                            ..
                        },
                        ..
                    }
                )
            })
        });
        assert!(has_or, "a || b should lower to BinaryOperator::Or");
    }

    #[test]
    fn is_nan_lowering() {
        let program = build_cfg(
            "device sensor: d0; device out: d1; fn main() { let x: f64 = sensor.Value; out.Setting = is_nan(x); }",
        );
        let main = get_function(&program, "main");
        let has_is_nan = main.blocks.iter().any(|b| {
            b.instructions.iter().any(|i| {
                matches!(
                    i,
                    Instruction::IntrinsicCall {
                        function: Intrinsic::IsNan,
                        ..
                    }
                )
            })
        });
        assert!(
            has_is_nan,
            "is_nan(x) should lower to IntrinsicCall with Intrinsic::IsNan"
        );
    }

    #[test]
    fn nested_labeled_loops_independent_targets() {
        let program = build_cfg(
            r#"
            fn main() {
                'a: loop {
                    'b: loop {
                        if true { break 'b; }
                        if false { break 'a; }
                    }
                }
            }
            "#,
        );
        let main = get_function(&program, "main");
        let loop_ends: Vec<usize> = main
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| matches!(b.role, BlockRole::LoopEnd(_)))
            .map(|(i, _)| i)
            .collect();
        assert!(
            loop_ends.len() >= 2,
            "should have at least two LoopEnd blocks, got {}",
            loop_ends.len()
        );
    }
}
