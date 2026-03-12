use crate::ast::{
    AssignStatement, AssignmentTarget, BatchMode, BinaryOperator, Block, BuiltinFunction,
    CallExpression, ConstDeclaration, DeviceDeclaration, DevicePin, ElseClause, Expression,
    ExpressionKind, ExpressionStatement, ForStatement, FunctionDeclaration, IfStatement, Item,
    LetStatement, LiteralKind, Parameter, Program, ReturnStatement, SleepStatement, Statement,
    Type, UnaryOperator, WhileStatement,
};
use crate::diagnostic::{Diagnostic, Span};
use crate::lexer::{Keyword, Literal, Operator, Punctuator, Token, TokenKind};

/// Recursive-descent / Pratt parser.
///
/// Consumes a flat token list produced by the lexer and builds a `Program` AST,
/// accumulating all parse errors rather than short-circuiting on the first one.
pub struct Parser {
    tokens: Vec<Token>,
    /// Index of the current (unconsumed) token.
    pos: usize,
    diagnostics: Vec<Diagnostic>,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            diagnostics: Vec::new(),
        }
    }

    /// Parse a complete program and return the AST together with any diagnostics.
    pub fn parse(mut self) -> (Program, Vec<Diagnostic>) {
        let program = self.parse_program();
        (program, self.diagnostics)
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    /// Advance past the current token and return it.
    fn advance(&mut self) -> &Token {
        let tok = &self.tokens[self.pos];
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    /// Return the span of the current token without consuming it.
    fn current_span(&self) -> Span {
        self.tokens[self.pos].span
    }

    /// Consume the current token if it matches `kind`; return it on success.
    fn accept(&mut self, kind: &TokenKind) -> Option<&Token> {
        if self.peek_kind() == kind {
            Some(self.advance())
        } else {
            None
        }
    }

    /// Consume the current token if it matches `kind`; emit an error and
    /// return `Err(())` on failure (error-recovery: do not consume).
    fn expect(&mut self, kind: &TokenKind) -> Result<Span, ()> {
        if self.peek_kind() == kind {
            Ok(self.advance().span)
        } else {
            let span = self.current_span();
            self.diagnostics.push(Diagnostic::error(
                span,
                format!(
                    "expected `{}`, found `{}`",
                    token_kind_name(kind),
                    token_kind_name(self.peek_kind())
                ),
            ));
            Err(())
        }
    }

    /// Consume the current token if it is an identifier; return the name on success.
    fn accept_identifier(&mut self) -> Option<(String, Span)> {
        if let TokenKind::Identifier(name) = self.peek_kind() {
            let name = name.clone();
            let span = self.advance().span;
            Some((name, span))
        } else {
            None
        }
    }

    /// Require an identifier; emit an error and return `Err(())` on failure.
    fn expect_identifier(&mut self) -> Result<(String, Span), ()> {
        if let Some(result) = self.accept_identifier() {
            Ok(result)
        } else {
            let span = self.current_span();
            self.diagnostics.push(Diagnostic::error(
                span,
                format!(
                    "expected identifier, found `{}`",
                    token_kind_name(self.peek_kind())
                ),
            ));
            Err(())
        }
    }

    /// Synchronize the token stream for error recovery. Advances past tokens
    /// until a synchronization point (`;` or `}`) is found, or EOF is reached.
    fn synchronize(&mut self) {
        loop {
            match self.peek_kind() {
                TokenKind::Eof => break,
                TokenKind::Punctuator(Punctuator::Semi) => {
                    self.advance();
                    break;
                }
                TokenKind::Punctuator(Punctuator::RBrace) => break,
                _ => {
                    self.advance();
                }
            }
        }
    }

    /// Parse the entire program: a sequence of items followed by EOF (§1.2).
    fn parse_program(&mut self) -> Program {
        let start = self.current_span();
        let mut items = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::Eof) {
            if let Some(item) = self.parse_item() {
                items.push(item);
            }
        }
        let end = self.current_span();
        Program {
            items,
            span: Span::new(start.start, end.end),
        }
    }

    /// Parse one top-level item (`const`, `device`, or `fn`).
    ///
    /// Acts as a synchronization boundary: on any parse error inside an item,
    /// the error is caught here, the stream is synchronized, and `None` is returned.
    fn parse_item(&mut self) -> Option<Item> {
        let result = match self.peek_kind() {
            TokenKind::Keyword(Keyword::Const) => self.parse_const_declaration().map(Item::Const),
            TokenKind::Keyword(Keyword::Device) => {
                self.parse_device_declaration().map(Item::Device)
            }
            TokenKind::Keyword(Keyword::Fn) => self.parse_function_declaration().map(Item::Fn),
            _ => {
                let span = self.current_span();
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!(
                        "expected `const`, `device`, or `fn`, found `{}`",
                        token_kind_name(self.peek_kind())
                    ),
                ));
                self.advance();
                Err(())
            }
        };
        match result {
            Ok(item) => Some(item),
            Err(()) => {
                self.synchronize();
                None
            }
        }
    }

    /// Parse a `const` declaration (§4.3).
    fn parse_const_declaration(&mut self) -> Result<ConstDeclaration, ()> {
        let start = self.expect(&TokenKind::Keyword(Keyword::Const))?;
        let (name, _) = self.expect_identifier()?;
        self.expect(&TokenKind::Punctuator(Punctuator::Colon))?;
        let ty = self.parse_type()?;
        self.expect(&TokenKind::Operator(Operator::Eq))?;
        let value = self.parse_expression()?;
        let end = self.expect(&TokenKind::Punctuator(Punctuator::Semi))?;
        Ok(ConstDeclaration {
            name,
            ty,
            value,
            span: Span::new(start.start, end.end),
        })
    }

    /// Parse a `device` declaration (§8.1).
    fn parse_device_declaration(&mut self) -> Result<DeviceDeclaration, ()> {
        let start = self.expect(&TokenKind::Keyword(Keyword::Device))?;
        let (name, _) = self.expect_identifier()?;
        self.expect(&TokenKind::Punctuator(Punctuator::Colon))?;
        let pin = self.parse_device_pin()?;
        let end = self.expect(&TokenKind::Punctuator(Punctuator::Semi))?;
        Ok(DeviceDeclaration {
            name,
            pin,
            span: Span::new(start.start, end.end),
        })
    }

    /// Parse a device pin identifier: `d0`–`d5` or `db` (§8.1).
    fn parse_device_pin(&mut self) -> Result<DevicePin, ()> {
        let pin = match self.peek_kind() {
            TokenKind::Identifier(name) => match name.as_str() {
                "d0" => DevicePin::D0,
                "d1" => DevicePin::D1,
                "d2" => DevicePin::D2,
                "d3" => DevicePin::D3,
                "d4" => DevicePin::D4,
                "d5" => DevicePin::D5,
                "db" => DevicePin::Db,
                _ => {
                    let span = self.current_span();
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        format!(
                            "expected device pin (d0-d5 or db), found `{}`",
                            token_kind_name(self.peek_kind())
                        ),
                    ));
                    return Err(());
                }
            },
            _ => {
                let span = self.current_span();
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!(
                        "expected device pin (d0-d5 or db), found `{}`",
                        token_kind_name(self.peek_kind())
                    ),
                ));
                return Err(());
            }
        };
        self.advance();
        Ok(pin)
    }

    /// Parse a `fn` declaration (§7.1).
    fn parse_function_declaration(&mut self) -> Result<FunctionDeclaration, ()> {
        let start = self.expect(&TokenKind::Keyword(Keyword::Fn))?;
        let (name, _) = self.expect_identifier()?;
        let params = self.parse_parameter_list()?;
        let return_type = self.parse_return_type()?;
        let body = self.parse_block()?;
        let span = Span::new(start.start, body.span.end);
        Ok(FunctionDeclaration {
            name,
            params,
            return_type,
            body,
            span,
        })
    }

    /// Parse a parameter list: `( parameter (, parameter)* )` (§7.2).
    fn parse_parameter_list(&mut self) -> Result<Vec<Parameter>, ()> {
        self.expect(&TokenKind::Punctuator(Punctuator::LParen))?;
        let mut params = Vec::new();
        while !matches!(
            self.peek_kind(),
            TokenKind::Punctuator(Punctuator::RParen) | TokenKind::Eof
        ) {
            params.push(self.parse_parameter()?);
            if self
                .accept(&TokenKind::Punctuator(Punctuator::Comma))
                .is_none()
            {
                break;
            }
        }
        self.expect(&TokenKind::Punctuator(Punctuator::RParen))?;
        Ok(params)
    }

    /// Parse a single parameter: `name : type` (§7.2).
    fn parse_parameter(&mut self) -> Result<Parameter, ()> {
        let (name, name_span) = self.expect_identifier()?;
        self.expect(&TokenKind::Punctuator(Punctuator::Colon))?;
        let ty = self.parse_type()?;
        let end = self.current_span();
        Ok(Parameter {
            name,
            ty,
            span: Span::new(name_span.start, end.start),
        })
    }

    /// Parse an optional return-type annotation: `-> type` (§7.3).
    fn parse_return_type(&mut self) -> Result<Option<Type>, ()> {
        if self
            .accept(&TokenKind::Punctuator(Punctuator::Arrow))
            .is_none()
        {
            return Ok(None);
        }
        Ok(Some(self.parse_type()?))
    }

    /// Parse a type keyword (`bool`, `i53`, `f64`) (§3).
    fn parse_type(&mut self) -> Result<Type, ()> {
        let ty = match self.peek_kind() {
            TokenKind::Keyword(Keyword::Bool) => Type::Bool,
            TokenKind::Keyword(Keyword::I53) => Type::I53,
            TokenKind::Keyword(Keyword::F64) => Type::F64,
            _ => {
                let span = self.current_span();
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!(
                        "expected type, found `{}`",
                        token_kind_name(self.peek_kind())
                    ),
                ));
                return Err(());
            }
        };
        self.advance();
        Ok(ty)
    }

    /// Parse a block: `{ statement* }` (§6.4).
    fn parse_block(&mut self) -> Result<Block, ()> {
        let start = self.expect(&TokenKind::Punctuator(Punctuator::LBrace))?;
        let mut stmts = Vec::new();
        while !matches!(
            self.peek_kind(),
            TokenKind::Punctuator(Punctuator::RBrace) | TokenKind::Eof
        ) {
            if let Some(stmt) = self.parse_statement() {
                stmts.push(stmt);
            }
        }
        let end = self.expect(&TokenKind::Punctuator(Punctuator::RBrace))?;
        Ok(Block {
            stmts,
            span: Span::new(start.start, end.end),
        })
    }

    /// Parse a single statement (§6).
    ///
    /// Acts as a synchronization boundary: on any parse error inside a statement,
    /// the error is caught here, the stream is synchronized, and `None` is returned.
    fn parse_statement(&mut self) -> Option<Statement> {
        let result = match self.peek_kind() {
            TokenKind::Keyword(Keyword::Let) => self.parse_let_statement().map(Statement::Let),
            TokenKind::Keyword(Keyword::If) => self.parse_if_statement().map(Statement::If),
            TokenKind::Keyword(Keyword::Loop) => self.parse_loop_statement().map(Statement::While),
            TokenKind::Keyword(Keyword::While) => {
                self.parse_while_statement().map(Statement::While)
            }
            TokenKind::Keyword(Keyword::For) => self.parse_for_statement().map(Statement::For),
            TokenKind::Keyword(Keyword::Return) => {
                self.parse_return_statement().map(Statement::Return)
            }
            TokenKind::Keyword(Keyword::Sleep) => {
                self.parse_sleep_statement().map(Statement::Sleep)
            }
            TokenKind::Keyword(Keyword::Break) => {
                let span = self.advance().span;
                self.expect(&TokenKind::Punctuator(Punctuator::Semi))
                    .map(|end| Statement::Break(Span::new(span.start, end.end)))
            }
            TokenKind::Keyword(Keyword::Continue) => {
                let span = self.advance().span;
                self.expect(&TokenKind::Punctuator(Punctuator::Semi))
                    .map(|end| Statement::Continue(Span::new(span.start, end.end)))
            }
            TokenKind::Keyword(Keyword::Yield) => {
                let span = self.advance().span;
                self.expect(&TokenKind::Punctuator(Punctuator::Semi))
                    .map(|end| Statement::Yield(Span::new(span.start, end.end)))
            }
            TokenKind::Identifier(_) => self.parse_assign_or_expression_statement(),
            _ => {
                let span = self.current_span();
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!(
                        "expected statement, found `{}`",
                        token_kind_name(self.peek_kind())
                    ),
                ));
                Err(())
            }
        };
        match result {
            Ok(stmt) => Some(stmt),
            Err(()) => {
                self.synchronize();
                None
            }
        }
    }

    /// Parse a `let` declaration statement (§6.1).
    fn parse_let_statement(&mut self) -> Result<LetStatement, ()> {
        let start = self.expect(&TokenKind::Keyword(Keyword::Let))?;
        let mutable = self.accept(&TokenKind::Keyword(Keyword::Mut)).is_some();
        let (name, _) = self.expect_identifier()?;
        let ty = if self
            .accept(&TokenKind::Punctuator(Punctuator::Colon))
            .is_some()
        {
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(&TokenKind::Operator(Operator::Eq))?;
        let init = self.parse_expression()?;
        let end = self.expect(&TokenKind::Punctuator(Punctuator::Semi))?;
        Ok(LetStatement {
            mutable,
            name,
            ty,
            init,
            span: Span::new(start.start, end.end),
        })
    }

    /// Parse an assignment statement or expression statement (§6.2–§6.3).
    ///
    /// Both begin with an identifier, so this function peeks ahead to
    /// determine which form is present.
    fn parse_assign_or_expression_statement(&mut self) -> Result<Statement, ()> {
        let start = self.current_span();
        let (name, name_span) = self.expect_identifier()?;
        if matches!(self.peek_kind(), TokenKind::Punctuator(Punctuator::LParen)) {
            let expr = self.parse_call(name, name_span)?;
            let end = self.expect(&TokenKind::Punctuator(Punctuator::Semi))?;
            let span = Span::new(start.start, end.end);
            return Ok(Statement::Expression(ExpressionStatement { expr, span }));
        }
        let lhs = self.parse_assignment_target(name, name_span)?;
        self.expect(&TokenKind::Operator(Operator::Eq))?;
        let rhs = self.parse_expression()?;
        let end = self.expect(&TokenKind::Punctuator(Punctuator::Semi))?;
        let span = Span::new(start.start, end.end);
        Ok(Statement::Assign(AssignStatement { lhs, rhs, span }))
    }

    /// Parse the left-hand side of an assignment (§6.2).
    fn parse_assignment_target(
        &mut self,
        name: String,
        name_span: Span,
    ) -> Result<AssignmentTarget, ()> {
        if self
            .accept(&TokenKind::Punctuator(Punctuator::Dot))
            .is_none()
        {
            return Ok(AssignmentTarget::Var {
                name,
                span: name_span,
            });
        }
        let (accessor, _) = self.expect_identifier()?;
        if matches!(self.peek_kind(), TokenKind::Punctuator(Punctuator::LParen)) {
            self.advance();
            let slot = self.parse_expression()?;
            self.expect(&TokenKind::Punctuator(Punctuator::RParen))?;
            self.expect(&TokenKind::Punctuator(Punctuator::Dot))?;
            let (field, _) = self.expect_identifier()?;
            let span = Span::new(name_span.start, self.current_span().start);
            Ok(AssignmentTarget::SlotField {
                device: name,
                slot,
                field,
                span,
            })
        } else {
            let span = Span::new(name_span.start, self.current_span().start);
            Ok(AssignmentTarget::DeviceField {
                device: name,
                field: accessor,
                span,
            })
        }
    }

    /// Parse an `if` statement (§6.5).
    fn parse_if_statement(&mut self) -> Result<IfStatement, ()> {
        let start = self.expect(&TokenKind::Keyword(Keyword::If))?;
        let cond = self.parse_expression()?;
        let then_block = self.parse_block()?;
        let else_clause = if matches!(self.peek_kind(), TokenKind::Keyword(Keyword::Else)) {
            Some(self.parse_else_clause()?)
        } else {
            None
        };
        let end = match &else_clause {
            Some(ElseClause::Block(block)) => block.span.end,
            Some(ElseClause::If(if_stmt)) => if_stmt.span.end,
            None => then_block.span.end,
        };
        Ok(IfStatement {
            cond,
            then_block,
            else_clause,
            span: Span::new(start.start, end),
        })
    }

    /// Parse an `else` clause (block or chained `else if`) (§6.5).
    fn parse_else_clause(&mut self) -> Result<ElseClause, ()> {
        self.expect(&TokenKind::Keyword(Keyword::Else))?;
        if matches!(self.peek_kind(), TokenKind::Keyword(Keyword::If)) {
            Ok(ElseClause::If(Box::new(self.parse_if_statement()?)))
        } else {
            Ok(ElseClause::Block(self.parse_block()?))
        }
    }

    /// Parse a `loop` statement (§6.6) and desugar it to `while true { … }`.
    fn parse_loop_statement(&mut self) -> Result<WhileStatement, ()> {
        let start = self.expect(&TokenKind::Keyword(Keyword::Loop))?;
        let body = self.parse_block()?;
        let span = Span::new(start.start, body.span.end);
        let cond = Expression {
            kind: ExpressionKind::Literal(LiteralKind::Bool(true)),
            span: Span::new(start.start, start.end),
        };
        Ok(WhileStatement { cond, body, span })
    }

    /// Parse a `while` statement (§6.7).
    fn parse_while_statement(&mut self) -> Result<WhileStatement, ()> {
        let start = self.expect(&TokenKind::Keyword(Keyword::While))?;
        let cond = self.parse_expression()?;
        let body = self.parse_block()?;
        let span = Span::new(start.start, body.span.end);
        Ok(WhileStatement { cond, body, span })
    }

    /// Parse a `for` statement (§6.8).
    fn parse_for_statement(&mut self) -> Result<ForStatement, ()> {
        let start = self.expect(&TokenKind::Keyword(Keyword::For))?;
        let (var, _) = self.expect_identifier()?;
        self.expect(&TokenKind::Keyword(Keyword::In))?;
        let lower = self.parse_expression()?;
        self.expect(&TokenKind::Punctuator(Punctuator::DotDot))?;
        let upper = self.parse_expression()?;
        let body = self.parse_block()?;
        let span = Span::new(start.start, body.span.end);
        Ok(ForStatement {
            var,
            lower,
            upper,
            body,
            span,
        })
    }

    /// Parse a `return` statement (§6.11).
    fn parse_return_statement(&mut self) -> Result<ReturnStatement, ()> {
        let start = self.expect(&TokenKind::Keyword(Keyword::Return))?;
        let value = if matches!(self.peek_kind(), TokenKind::Punctuator(Punctuator::Semi)) {
            None
        } else {
            Some(self.parse_expression()?)
        };
        let end = self.expect(&TokenKind::Punctuator(Punctuator::Semi))?;
        Ok(ReturnStatement {
            value,
            span: Span::new(start.start, end.end),
        })
    }

    /// Parse a `sleep` statement (§6.13).
    fn parse_sleep_statement(&mut self) -> Result<SleepStatement, ()> {
        let start = self.expect(&TokenKind::Keyword(Keyword::Sleep))?;
        self.expect(&TokenKind::Punctuator(Punctuator::LParen))?;
        let duration = self.parse_expression()?;
        self.expect(&TokenKind::Punctuator(Punctuator::RParen))?;
        let end = self.expect(&TokenKind::Punctuator(Punctuator::Semi))?;
        Ok(SleepStatement {
            duration,
            span: Span::new(start.start, end.end),
        })
    }

    /// Entry point for the expression parser; parses at the lowest precedence.
    fn parse_expression(&mut self) -> Result<Expression, ()> {
        self.parse_expression_binding_power(0)
    }

    /// Parse a binary expression using Pratt (top-down operator precedence).
    ///
    /// `minimum_binding_power` restricts which operators are consumed at this level.
    fn parse_expression_binding_power(
        &mut self,
        minimum_binding_power: u8,
    ) -> Result<Expression, ()> {
        let mut lhs = self.parse_unary()?;
        loop {
            // `as` cast: left binding power 19 (above all binary operators).
            if matches!(self.peek_kind(), TokenKind::Keyword(Keyword::As)) {
                if 19 < minimum_binding_power {
                    break;
                }
                self.advance();
                let ty = self.parse_type()?;
                let span = Span::new(lhs.span.start, self.current_span().start);
                lhs = Expression {
                    kind: ExpressionKind::Cast(Box::new(lhs), ty),
                    span,
                };
                continue;
            }
            let (left_bp, right_bp, op) = match Self::infix_operator(self.peek_kind()) {
                Some(entry) => entry,
                None => break,
            };
            if left_bp < minimum_binding_power {
                break;
            }
            self.advance();
            let rhs = self.parse_expression_binding_power(right_bp)?;
            let span = Span::new(lhs.span.start, rhs.span.end);
            lhs = Expression {
                kind: ExpressionKind::Binary(op, Box::new(lhs), Box::new(rhs)),
                span,
            };
        }
        Ok(lhs)
    }

    /// Parse a unary or postfix expression (§5.7).
    fn parse_unary(&mut self) -> Result<Expression, ()> {
        let start = self.current_span();
        let op = match self.peek_kind() {
            TokenKind::Operator(Operator::Minus) => Some(UnaryOperator::Neg),
            TokenKind::Operator(Operator::Bang) => Some(UnaryOperator::Not),
            TokenKind::Operator(Operator::Tilde) => Some(UnaryOperator::BitNot),
            _ => None,
        };
        if let Some(op) = op {
            self.advance();
            let operand = self.parse_unary()?;
            let span = Span::new(start.start, operand.span.end);
            return Ok(Expression {
                kind: ExpressionKind::Unary(op, Box::new(operand)),
                span,
            });
        }
        self.parse_primary()
    }

    /// Parse a primary expression: literal, identifier, grouped `(expr)`,
    /// `select(…)`, or `hash(…)` (§5.7).
    fn parse_primary(&mut self) -> Result<Expression, ()> {
        let start = self.current_span();
        match self.peek_kind().clone() {
            TokenKind::Literal(Literal::I53(val)) => {
                self.advance();
                Ok(Expression {
                    kind: ExpressionKind::Literal(LiteralKind::I53(val)),
                    span: start,
                })
            }
            TokenKind::Literal(Literal::F64(val)) => {
                self.advance();
                Ok(Expression {
                    kind: ExpressionKind::Literal(LiteralKind::F64(val)),
                    span: start,
                })
            }
            TokenKind::Keyword(Keyword::True) => {
                self.advance();
                Ok(Expression {
                    kind: ExpressionKind::Literal(LiteralKind::Bool(true)),
                    span: start,
                })
            }
            TokenKind::Keyword(Keyword::False) => {
                self.advance();
                Ok(Expression {
                    kind: ExpressionKind::Literal(LiteralKind::Bool(false)),
                    span: start,
                })
            }
            TokenKind::Keyword(Keyword::Nan) => {
                self.advance();
                Ok(Expression {
                    kind: ExpressionKind::Literal(LiteralKind::F64(f64::NAN)),
                    span: start,
                })
            }
            TokenKind::Keyword(Keyword::Inf) => {
                self.advance();
                Ok(Expression {
                    kind: ExpressionKind::Literal(LiteralKind::F64(f64::INFINITY)),
                    span: start,
                })
            }
            TokenKind::Punctuator(Punctuator::LParen) => {
                self.advance();
                let expr = self.parse_expression()?;
                self.expect(&TokenKind::Punctuator(Punctuator::RParen))?;
                Ok(expr)
            }
            TokenKind::Identifier(name) => {
                self.advance();
                match name.as_str() {
                    "batch_read" => self.parse_batch_read(start),
                    "select" => self.parse_select(start),
                    "hash" => self.parse_hash(start),
                    _ => {
                        if matches!(self.peek_kind(), TokenKind::Punctuator(Punctuator::LParen)) {
                            self.parse_call(name, start)
                        } else if matches!(self.peek_kind(), TokenKind::Punctuator(Punctuator::Dot))
                        {
                            self.parse_postfix_dot(name, start)
                        } else {
                            Ok(Expression {
                                kind: ExpressionKind::Variable(name),
                                span: start,
                            })
                        }
                    }
                }
            }
            _ => {
                self.diagnostics.push(Diagnostic::error(
                    start,
                    format!(
                        "expected expression, found `{}`",
                        token_kind_name(self.peek_kind())
                    ),
                ));
                Err(())
            }
        }
    }

    /// Parse a function-call or built-in-call expression (§5.9, §5.11–§5.13).
    ///
    /// Called after an identifier has been consumed; `name` and `name_span`
    /// identify the callee.
    fn parse_call(&mut self, name: String, name_span: Span) -> Result<Expression, ()> {
        let (args, close_span) = self.parse_argument_list()?;
        let span = Span::new(name_span.start, close_span.end);
        if let Some(builtin) = name_to_builtin(&name) {
            return Ok(Expression {
                kind: ExpressionKind::BuiltinCall(builtin, args),
                span,
            });
        }
        Ok(Expression {
            kind: ExpressionKind::Call(CallExpression { name, args, span }),
            span,
        })
    }

    /// Parse an argument list for a call: `( expr (, expr)* )`.
    fn parse_argument_list(&mut self) -> Result<(Vec<Expression>, Span), ()> {
        self.expect(&TokenKind::Punctuator(Punctuator::LParen))?;
        let mut args = Vec::new();
        while !matches!(
            self.peek_kind(),
            TokenKind::Punctuator(Punctuator::RParen) | TokenKind::Eof
        ) {
            args.push(self.parse_expression()?);
            if self
                .accept(&TokenKind::Punctuator(Punctuator::Comma))
                .is_none()
            {
                break;
            }
        }
        let close = self.expect(&TokenKind::Punctuator(Punctuator::RParen))?;
        Ok((args, close))
    }

    /// Parse a device field or slot access after a `.` has been seen (§5.10).
    ///
    /// `device` is the already-consumed device (or variable) name.
    fn parse_postfix_dot(&mut self, device: String, lhs_span: Span) -> Result<Expression, ()> {
        self.expect(&TokenKind::Punctuator(Punctuator::Dot))?;
        let (accessor, _) = self.expect_identifier()?;
        if matches!(self.peek_kind(), TokenKind::Punctuator(Punctuator::LParen)) {
            self.advance();
            let slot = self.parse_expression()?;
            self.expect(&TokenKind::Punctuator(Punctuator::RParen))?;
            self.expect(&TokenKind::Punctuator(Punctuator::Dot))?;
            let (field, _) = self.expect_identifier()?;
            let span = Span::new(lhs_span.start, self.current_span().start);
            Ok(Expression {
                kind: ExpressionKind::SlotRead {
                    device,
                    slot: Box::new(slot),
                    field,
                },
                span,
            })
        } else {
            let span = Span::new(lhs_span.start, self.current_span().start);
            Ok(Expression {
                kind: ExpressionKind::DeviceRead {
                    device,
                    field: accessor,
                },
                span,
            })
        }
    }

    /// Parse a `batch_read(hash_expr, field, mode)` expression (§8.5.1).
    fn parse_batch_read(&mut self, start: Span) -> Result<Expression, ()> {
        self.expect(&TokenKind::Punctuator(Punctuator::LParen))?;
        let hash_expr = self.parse_expression()?;
        self.expect(&TokenKind::Punctuator(Punctuator::Comma))?;
        let (field, _) = self.expect_identifier()?;
        self.expect(&TokenKind::Punctuator(Punctuator::Comma))?;
        let mode = self.parse_batch_mode()?;
        let close = self.expect(&TokenKind::Punctuator(Punctuator::RParen))?;
        let span = Span::new(start.start, close.end);
        Ok(Expression {
            kind: ExpressionKind::BatchRead {
                hash_expr: Box::new(hash_expr),
                field,
                mode,
            },
            span,
        })
    }

    /// Parse a `select(cond, if_true, if_false)` expression (§5.12).
    fn parse_select(&mut self, start: Span) -> Result<Expression, ()> {
        self.expect(&TokenKind::Punctuator(Punctuator::LParen))?;
        let cond = self.parse_expression()?;
        self.expect(&TokenKind::Punctuator(Punctuator::Comma))?;
        let if_true = self.parse_expression()?;
        self.expect(&TokenKind::Punctuator(Punctuator::Comma))?;
        let if_false = self.parse_expression()?;
        let close = self.expect(&TokenKind::Punctuator(Punctuator::RParen))?;
        let span = Span::new(start.start, close.end);
        Ok(Expression {
            kind: ExpressionKind::Select {
                cond: Box::new(cond),
                if_true: Box::new(if_true),
                if_false: Box::new(if_false),
            },
            span,
        })
    }

    /// Parse a `hash("string")` expression (§5.13).
    fn parse_hash(&mut self, start: Span) -> Result<Expression, ()> {
        self.expect(&TokenKind::Punctuator(Punctuator::LParen))?;
        let string_val = match self.peek_kind().clone() {
            TokenKind::Literal(Literal::String(s)) => {
                self.advance();
                s
            }
            _ => {
                let span = self.current_span();
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!(
                        "expected string literal, found `{}`",
                        token_kind_name(self.peek_kind())
                    ),
                ));
                return Err(());
            }
        };
        let close = self.expect(&TokenKind::Punctuator(Punctuator::RParen))?;
        let span = Span::new(start.start, close.end);
        Ok(Expression {
            kind: ExpressionKind::Hash(string_val),
            span,
        })
    }

    /// Parse a batch operation mode identifier (`Average`, `Sum`, etc.) (§8.5.3).
    fn parse_batch_mode(&mut self) -> Result<BatchMode, ()> {
        let mode = match self.peek_kind() {
            TokenKind::Identifier(name) => match name.as_str() {
                "Average" => BatchMode::Average,
                "Sum" => BatchMode::Sum,
                "Minimum" => BatchMode::Minimum,
                "Maximum" => BatchMode::Maximum,
                "Contents" => BatchMode::Contents,
                _ => {
                    let span = self.current_span();
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        format!(
                            "expected batch mode (Average, Sum, Minimum, Maximum, or Contents), found `{}`",
                            token_kind_name(self.peek_kind())
                        ),
                    ));
                    return Err(());
                }
            },
            _ => {
                let span = self.current_span();
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!(
                        "expected batch mode (Average, Sum, Minimum, Maximum, or Contents), found `{}`",
                        token_kind_name(self.peek_kind())
                    ),
                ));
                return Err(());
            }
        };
        self.advance();
        Ok(mode)
    }

    /// Return the Pratt table entry `(left_binding_power, right_binding_power, operator)` for
    /// a binary operator token, or `None` if the token is not a binary operator.
    ///
    /// Binding powers encode the precedence table from §5.1. Left-associative operators
    /// use (N, N+1); comparisons are also left-associative at the parse level — the type
    /// checker rejects chained comparisons because `bool` is not comparable with `<` etc.
    fn infix_operator(kind: &TokenKind) -> Option<(u8, u8, BinaryOperator)> {
        let entry = match kind {
            TokenKind::Operator(Operator::PipePipe) => (1, 2, BinaryOperator::Or),
            TokenKind::Operator(Operator::AmpAmp) => (3, 4, BinaryOperator::And),
            TokenKind::Operator(Operator::EqEq) => (5, 6, BinaryOperator::Eq),
            TokenKind::Operator(Operator::BangEq) => (5, 6, BinaryOperator::Ne),
            TokenKind::Operator(Operator::Lt) => (5, 6, BinaryOperator::Lt),
            TokenKind::Operator(Operator::Gt) => (5, 6, BinaryOperator::Gt),
            TokenKind::Operator(Operator::LtEq) => (5, 6, BinaryOperator::Le),
            TokenKind::Operator(Operator::GtEq) => (5, 6, BinaryOperator::Ge),
            TokenKind::Operator(Operator::Pipe) => (7, 8, BinaryOperator::BitOr),
            TokenKind::Operator(Operator::Caret) => (9, 10, BinaryOperator::BitXor),
            TokenKind::Operator(Operator::Amp) => (11, 12, BinaryOperator::BitAnd),
            TokenKind::Operator(Operator::Shl) => (13, 14, BinaryOperator::Shl),
            TokenKind::Operator(Operator::Shr) => (13, 14, BinaryOperator::Shr),
            TokenKind::Operator(Operator::Plus) => (15, 16, BinaryOperator::Add),
            TokenKind::Operator(Operator::Minus) => (15, 16, BinaryOperator::Sub),
            TokenKind::Operator(Operator::Star) => (17, 18, BinaryOperator::Mul),
            TokenKind::Operator(Operator::Slash) => (17, 18, BinaryOperator::Div),
            TokenKind::Operator(Operator::Percent) => (17, 18, BinaryOperator::Rem),
            _ => return None,
        };
        Some(entry)
    }
}

fn token_kind_name(kind: &TokenKind) -> &'static str {
    match kind {
        TokenKind::Literal(Literal::I53(_)) => "integer literal",
        TokenKind::Literal(Literal::F64(_)) => "float literal",
        TokenKind::Literal(Literal::String(_)) => "string literal",
        TokenKind::Identifier(_) => "identifier",
        TokenKind::Keyword(Keyword::Let) => "`let`",
        TokenKind::Keyword(Keyword::Const) => "`const`",
        TokenKind::Keyword(Keyword::Fn) => "`fn`",
        TokenKind::Keyword(Keyword::If) => "`if`",
        TokenKind::Keyword(Keyword::Else) => "`else`",
        TokenKind::Keyword(Keyword::Loop) => "`loop`",
        TokenKind::Keyword(Keyword::While) => "`while`",
        TokenKind::Keyword(Keyword::For) => "`for`",
        TokenKind::Keyword(Keyword::In) => "`in`",
        TokenKind::Keyword(Keyword::Break) => "`break`",
        TokenKind::Keyword(Keyword::Continue) => "`continue`",
        TokenKind::Keyword(Keyword::Return) => "`return`",
        TokenKind::Keyword(Keyword::Yield) => "`yield`",
        TokenKind::Keyword(Keyword::Sleep) => "`sleep`",
        TokenKind::Keyword(Keyword::Device) => "`device`",
        TokenKind::Keyword(Keyword::As) => "`as`",
        TokenKind::Keyword(Keyword::Mut) => "`mut`",
        TokenKind::Keyword(Keyword::Bool) => "`bool`",
        TokenKind::Keyword(Keyword::I53) => "`i53`",
        TokenKind::Keyword(Keyword::F64) => "`f64`",
        TokenKind::Keyword(Keyword::True) => "`true`",
        TokenKind::Keyword(Keyword::False) => "`false`",
        TokenKind::Keyword(Keyword::Nan) => "`nan`",
        TokenKind::Keyword(Keyword::Inf) => "`inf`",
        TokenKind::Reserved(_) => "reserved keyword",
        TokenKind::Operator(Operator::Plus) => "`+`",
        TokenKind::Operator(Operator::Minus) => "`-`",
        TokenKind::Operator(Operator::Star) => "`*`",
        TokenKind::Operator(Operator::Slash) => "`/`",
        TokenKind::Operator(Operator::Percent) => "`%`",
        TokenKind::Operator(Operator::Amp) => "`&`",
        TokenKind::Operator(Operator::Pipe) => "`|`",
        TokenKind::Operator(Operator::Caret) => "`^`",
        TokenKind::Operator(Operator::Tilde) => "`~`",
        TokenKind::Operator(Operator::Shl) => "`<<`",
        TokenKind::Operator(Operator::Shr) => "`>>`",
        TokenKind::Operator(Operator::EqEq) => "`==`",
        TokenKind::Operator(Operator::BangEq) => "`!=`",
        TokenKind::Operator(Operator::Lt) => "`<`",
        TokenKind::Operator(Operator::Gt) => "`>`",
        TokenKind::Operator(Operator::LtEq) => "`<=`",
        TokenKind::Operator(Operator::GtEq) => "`>=`",
        TokenKind::Operator(Operator::AmpAmp) => "`&&`",
        TokenKind::Operator(Operator::PipePipe) => "`||`",
        TokenKind::Operator(Operator::Bang) => "`!`",
        TokenKind::Operator(Operator::Eq) => "`=`",
        TokenKind::Punctuator(Punctuator::LParen) => "`(`",
        TokenKind::Punctuator(Punctuator::RParen) => "`)`",
        TokenKind::Punctuator(Punctuator::LBrace) => "`{`",
        TokenKind::Punctuator(Punctuator::RBrace) => "`}`",
        TokenKind::Punctuator(Punctuator::Semi) => "`;`",
        TokenKind::Punctuator(Punctuator::Colon) => "`:`",
        TokenKind::Punctuator(Punctuator::Comma) => "`,`",
        TokenKind::Punctuator(Punctuator::Dot) => "`.`",
        TokenKind::Punctuator(Punctuator::Arrow) => "`->`",
        TokenKind::Punctuator(Punctuator::DotDot) => "`..`",
        TokenKind::Eof => "end of file",
    }
}

fn name_to_builtin(name: &str) -> Option<BuiltinFunction> {
    match name {
        "abs" => Some(BuiltinFunction::Abs),
        "ceil" => Some(BuiltinFunction::Ceil),
        "floor" => Some(BuiltinFunction::Floor),
        "round" => Some(BuiltinFunction::Round),
        "trunc" => Some(BuiltinFunction::Trunc),
        "sqrt" => Some(BuiltinFunction::Sqrt),
        "exp" => Some(BuiltinFunction::Exp),
        "log" => Some(BuiltinFunction::Log),
        "sin" => Some(BuiltinFunction::Sin),
        "cos" => Some(BuiltinFunction::Cos),
        "tan" => Some(BuiltinFunction::Tan),
        "asin" => Some(BuiltinFunction::Asin),
        "acos" => Some(BuiltinFunction::Acos),
        "atan" => Some(BuiltinFunction::Atan),
        "atan2" => Some(BuiltinFunction::Atan2),
        "pow" => Some(BuiltinFunction::Pow),
        "min" => Some(BuiltinFunction::Min),
        "max" => Some(BuiltinFunction::Max),
        "lerp" => Some(BuiltinFunction::Lerp),
        "clamp" => Some(BuiltinFunction::Clamp),
        "rand" => Some(BuiltinFunction::Rand),
        _ => None,
    }
}

/// Parse IC20 source text into a `Program` AST.
///
/// Returns the (possibly partial) AST and all diagnostics collected. When the
/// diagnostic list contains errors the caller should not proceed to later
/// compiler stages.
pub fn parse(source: &str) -> (Program, Vec<Diagnostic>) {
    use crate::lexer::Lexer;
    let (tokens, mut lex_diags) = Lexer::new(source).tokenize();
    let (program, mut parse_diags) = Parser::new(tokens).parse();
    lex_diags.append(&mut parse_diags);
    (program, lex_diags)
}

#[cfg(test)]
mod tests {
    use core::f64;

    use super::parse;
    use crate::ast::{
        AssignmentTarget, BatchMode, BinaryOperator, BuiltinFunction, ConstDeclaration,
        DeviceDeclaration, DevicePin, ElseClause, ExpressionKind, FunctionDeclaration, Item,
        LetStatement, LiteralKind, Program, Statement, Type, UnaryOperator,
    };
    use crate::diagnostic::{Diagnostic, Severity};

    fn parse_ok(source: &str) -> Program {
        let (program, diagnostics) = parse(source);
        let errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "unexpected errors: {:#?}", errors);
        program
    }

    fn parse_errors(source: &str) -> Vec<Diagnostic> {
        let (_, diagnostics) = parse(source);
        diagnostics
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .collect()
    }

    fn single_const(source: &str) -> ConstDeclaration {
        let program = parse_ok(source);
        assert_eq!(program.items.len(), 1);
        match program.items.into_iter().next().unwrap() {
            Item::Const(declaration) => declaration,
            other => panic!("expected const item, got {:?}", other),
        }
    }

    fn single_device(source: &str) -> DeviceDeclaration {
        let program = parse_ok(source);
        assert_eq!(program.items.len(), 1);
        match program.items.into_iter().next().unwrap() {
            Item::Device(declaration) => declaration,
            other => panic!("expected device item, got {:?}", other),
        }
    }

    fn single_fn(source: &str) -> FunctionDeclaration {
        let program = parse_ok(source);
        assert_eq!(program.items.len(), 1);
        match program.items.into_iter().next().unwrap() {
            Item::Fn(declaration) => declaration,
            other => panic!("expected fn item, got {:?}", other),
        }
    }

    fn fn_statements(source: &str) -> Vec<Statement> {
        single_fn(source).body.stmts
    }

    fn single_let(source: &str) -> LetStatement {
        let statements = fn_statements(source);
        assert_eq!(statements.len(), 1);
        match statements.into_iter().next().unwrap() {
            Statement::Let(s) => s,
            other => panic!("expected let statement, got {:?}", other),
        }
    }

    #[test]
    fn const_i53_literal() {
        let declaration = single_const("const MAX: i53 = 42;");
        assert_eq!(declaration.name, "MAX");
        assert_eq!(declaration.ty, Type::I53);
        assert!(matches!(
            declaration.value.kind,
            ExpressionKind::Literal(LiteralKind::I53(42))
        ));
    }

    #[test]
    fn const_f64_literal() {
        let declaration = single_const("const PI: f64 = 3.141592653589793;");
        assert_eq!(declaration.name, "PI");
        assert_eq!(declaration.ty, Type::F64);
        if let ExpressionKind::Literal(LiteralKind::F64(value)) = declaration.value.kind {
            assert!((value - f64::consts::PI).abs() < 1e-10);
        } else {
            panic!("expected f64 literal");
        }
    }

    #[test]
    fn const_bool_true() {
        let declaration = single_const("const ENABLED: bool = true;");
        assert_eq!(declaration.name, "ENABLED");
        assert_eq!(declaration.ty, Type::Bool);
        assert!(matches!(
            declaration.value.kind,
            ExpressionKind::Literal(LiteralKind::Bool(true))
        ));
    }

    #[test]
    fn const_bool_false() {
        let declaration = single_const("const DISABLED: bool = false;");
        assert!(matches!(
            declaration.value.kind,
            ExpressionKind::Literal(LiteralKind::Bool(false))
        ));
    }

    #[test]
    fn const_span_covers_full_source() {
        let source = "const X: i53 = 0;";
        let declaration = single_const(source);
        assert_eq!(declaration.span.start, 0);
        assert_eq!(declaration.span.end, source.len());
    }

    #[test]
    fn device_pin_d0() {
        let declaration = single_device("device pump: d0;");
        assert_eq!(declaration.name, "pump");
        assert_eq!(declaration.pin, DevicePin::D0);
    }

    #[test]
    fn device_pin_db() {
        let declaration = single_device("device self_ic: db;");
        assert_eq!(declaration.name, "self_ic");
        assert_eq!(declaration.pin, DevicePin::Db);
    }

    #[test]
    fn device_all_pins_recognized() {
        let cases = [
            ("d0", DevicePin::D0),
            ("d1", DevicePin::D1),
            ("d2", DevicePin::D2),
            ("d3", DevicePin::D3),
            ("d4", DevicePin::D4),
            ("d5", DevicePin::D5),
            ("db", DevicePin::Db),
        ];
        for (pin_name, expected) in cases {
            let source = format!("device x: {};", pin_name);
            let declaration = single_device(&source);
            assert_eq!(declaration.pin, expected, "failed for pin {}", pin_name);
        }
    }

    #[test]
    fn fn_no_params_no_return_type() {
        let declaration = single_fn("fn main() { }");
        assert_eq!(declaration.name, "main");
        assert!(declaration.params.is_empty());
        assert_eq!(declaration.return_type, None);
        assert!(declaration.body.stmts.is_empty());
    }

    #[test]
    fn fn_single_param() {
        let declaration = single_fn("fn negate(x: f64) -> f64 { }");
        assert_eq!(declaration.params.len(), 1);
        assert_eq!(declaration.params[0].name, "x");
        assert_eq!(declaration.params[0].ty, Type::F64);
        assert_eq!(declaration.return_type, Some(Type::F64));
    }

    #[test]
    fn fn_multiple_params() {
        let declaration = single_fn("fn add(a: i53, b: i53) -> i53 { }");
        assert_eq!(declaration.name, "add");
        assert_eq!(declaration.params.len(), 2);
        assert_eq!(declaration.params[0].name, "a");
        assert_eq!(declaration.params[0].ty, Type::I53);
        assert_eq!(declaration.params[1].name, "b");
        assert_eq!(declaration.params[1].ty, Type::I53);
        assert_eq!(declaration.return_type, Some(Type::I53));
    }

    #[test]
    fn fn_return_type_bool() {
        let declaration = single_fn("fn check() -> bool { }");
        assert_eq!(declaration.return_type, Some(Type::Bool));
    }

    #[test]
    fn fn_three_params_same_type() {
        let declaration = single_fn("fn lerp(a: f64, b: f64, t: f64) -> f64 { }");
        assert_eq!(declaration.params.len(), 3);
        for param in &declaration.params {
            assert_eq!(param.ty, Type::F64);
        }
    }

    #[test]
    fn let_immutable_with_type() {
        let statement = single_let("fn f() { let x: i53 = 0; }");
        assert!(!statement.mutable);
        assert_eq!(statement.name, "x");
        assert_eq!(statement.ty, Some(Type::I53));
        assert!(matches!(
            statement.init.kind,
            ExpressionKind::Literal(LiteralKind::I53(0))
        ));
    }

    #[test]
    fn let_mutable() {
        let statement = single_let("fn f() { let mut counter: i53 = 0; }");
        assert!(statement.mutable);
        assert_eq!(statement.name, "counter");
    }

    #[test]
    fn let_without_type_annotation() {
        let statement = single_let("fn f() { let x = 1; }");
        assert_eq!(statement.ty, None);
        assert_eq!(statement.name, "x");
        assert!(matches!(
            statement.init.kind,
            ExpressionKind::Literal(LiteralKind::I53(_))
        ));
    }

    #[test]
    fn let_mutable_without_type() {
        let statement = single_let("fn f() { let mut v = 3.14; }");
        assert!(statement.mutable);
        assert_eq!(statement.ty, None);
        assert!(matches!(
            statement.init.kind,
            ExpressionKind::Literal(LiteralKind::F64(_))
        ));
    }

    #[test]
    fn assign_variable() {
        let statements = fn_statements("fn f() { x = 5; }");
        if let Statement::Assign(statement) = &statements[0] {
            assert!(matches!(
                &statement.lhs,
                AssignmentTarget::Var { name, .. } if name == "x"
            ));
            assert!(matches!(
                statement.rhs.kind,
                ExpressionKind::Literal(LiteralKind::I53(5))
            ));
        } else {
            panic!("expected assign statement");
        }
    }

    #[test]
    fn assign_device_field() {
        let statements = fn_statements("fn f() { pump.Setting = 100; }");
        if let Statement::Assign(statement) = &statements[0] {
            if let AssignmentTarget::DeviceField { device, field, .. } = &statement.lhs {
                assert_eq!(device, "pump");
                assert_eq!(field, "Setting");
            } else {
                panic!("expected device field assignment target");
            }
        } else {
            panic!("expected assign statement");
        }
    }

    #[test]
    fn assign_slot_field() {
        let statements = fn_statements("fn f() { rack.slot(0).Activate = 1; }");
        if let Statement::Assign(statement) = &statements[0] {
            if let AssignmentTarget::SlotField {
                device,
                slot,
                field,
                ..
            } = &statement.lhs
            {
                assert_eq!(device, "rack");
                assert_eq!(field, "Activate");
                assert!(matches!(
                    slot.kind,
                    ExpressionKind::Literal(LiteralKind::I53(0))
                ));
            } else {
                panic!("expected slot field assignment target");
            }
        } else {
            panic!("expected assign statement");
        }
    }

    #[test]
    fn expression_statement_call_no_args() {
        let statements = fn_statements("fn f() { run(); }");
        if let Statement::Expression(statement) = &statements[0] {
            if let ExpressionKind::Call(call) = &statement.expr.kind {
                assert_eq!(call.name, "run");
                assert!(call.args.is_empty());
            } else {
                panic!("expected call expression in expression statement");
            }
        } else {
            panic!("expected expression statement");
        }
    }

    #[test]
    fn expression_statement_call_with_args() {
        let statements = fn_statements("fn f() { emit(x, y); }");
        if let Statement::Expression(statement) = &statements[0] {
            if let ExpressionKind::Call(call) = &statement.expr.kind {
                assert_eq!(call.name, "emit");
                assert_eq!(call.args.len(), 2);
            } else {
                panic!("expected call expression");
            }
        } else {
            panic!("expected expression statement");
        }
    }

    #[test]
    fn if_with_no_else() {
        let statements = fn_statements("fn f() { if x { y = 1; } }");
        if let Statement::If(statement) = &statements[0] {
            assert!(statement.else_clause.is_none());
            assert_eq!(statement.then_block.stmts.len(), 1);
        } else {
            panic!("expected if statement");
        }
    }

    #[test]
    fn if_with_else_block() {
        let statements = fn_statements("fn f() { if x { a = 1; } else { a = 2; } }");
        if let Statement::If(statement) = &statements[0] {
            assert!(matches!(statement.else_clause, Some(ElseClause::Block(_))));
        } else {
            panic!("expected if statement");
        }
    }

    #[test]
    fn if_else_if_chain() {
        let statements = fn_statements("fn f() { if a { } else if b { } else { } }");
        if let Statement::If(statement) = &statements[0] {
            if let Some(ElseClause::If(chained)) = &statement.else_clause {
                assert!(matches!(chained.else_clause, Some(ElseClause::Block(_))));
            } else {
                panic!("expected else-if clause");
            }
        } else {
            panic!("expected if statement");
        }
    }

    #[test]
    fn loop_statement_with_break() {
        let statements = fn_statements("fn f() { loop { break; } }");
        if let Statement::While(statement) = &statements[0] {
            assert!(matches!(
                statement.cond.kind,
                ExpressionKind::Literal(LiteralKind::Bool(true))
            ));
            assert_eq!(statement.body.stmts.len(), 1);
            assert!(matches!(statement.body.stmts[0], Statement::Break(_)));
        } else {
            panic!("expected while statement (desugared loop)");
        }
    }

    #[test]
    fn loop_statement_with_continue() {
        let statements = fn_statements("fn f() { loop { continue; } }");
        if let Statement::While(statement) = &statements[0] {
            assert!(matches!(
                statement.cond.kind,
                ExpressionKind::Literal(LiteralKind::Bool(true))
            ));
            assert!(matches!(statement.body.stmts[0], Statement::Continue(_)));
        } else {
            panic!("expected while statement (desugared loop)");
        }
    }

    #[test]
    fn while_statement_true_condition() {
        let statements = fn_statements("fn f() { while true { } }");
        if let Statement::While(statement) = &statements[0] {
            assert!(matches!(
                statement.cond.kind,
                ExpressionKind::Literal(LiteralKind::Bool(true))
            ));
            assert!(statement.body.stmts.is_empty());
        } else {
            panic!("expected while statement");
        }
    }

    #[test]
    fn while_statement_with_body() {
        let statements = fn_statements("fn f() { while running { x = x + 1; } }");
        if let Statement::While(statement) = &statements[0] {
            assert_eq!(statement.body.stmts.len(), 1);
        } else {
            panic!("expected while statement");
        }
    }

    #[test]
    fn for_statement_integer_range() {
        let statements = fn_statements("fn f() { for i in 0..10 { } }");
        if let Statement::For(statement) = &statements[0] {
            assert_eq!(statement.var, "i");
            assert!(matches!(
                statement.lower.kind,
                ExpressionKind::Literal(LiteralKind::I53(0))
            ));
            assert!(matches!(
                statement.upper.kind,
                ExpressionKind::Literal(LiteralKind::I53(10))
            ));
            assert!(statement.body.stmts.is_empty());
        } else {
            panic!("expected for statement");
        }
    }

    #[test]
    fn for_statement_variable_bounds() {
        let statements = fn_statements("fn f() { for i in lo..hi { } }");
        if let Statement::For(statement) = &statements[0] {
            assert!(matches!(
                &statement.lower.kind,
                ExpressionKind::Variable(name) if name == "lo"
            ));
            assert!(matches!(
                &statement.upper.kind,
                ExpressionKind::Variable(name) if name == "hi"
            ));
        } else {
            panic!("expected for statement");
        }
    }

    #[test]
    fn break_statement() {
        let statements = fn_statements("fn f() { loop { break; } }");
        if let Statement::While(statement) = &statements[0] {
            assert!(matches!(statement.body.stmts[0], Statement::Break(_)));
        } else {
            panic!("expected while statement (desugared loop)");
        }
    }

    #[test]
    fn continue_statement() {
        let statements = fn_statements("fn f() { loop { continue; } }");
        if let Statement::While(statement) = &statements[0] {
            assert!(matches!(statement.body.stmts[0], Statement::Continue(_)));
        } else {
            panic!("expected while statement (desugared loop)");
        }
    }

    #[test]
    fn yield_statement() {
        let statements = fn_statements("fn f() { yield; }");
        assert!(matches!(statements[0], Statement::Yield(_)));
    }

    #[test]
    fn return_without_value() {
        let statements = fn_statements("fn f() { return; }");
        if let Statement::Return(statement) = &statements[0] {
            assert!(statement.value.is_none());
        } else {
            panic!("expected return statement");
        }
    }

    #[test]
    fn return_with_value() {
        let statements = fn_statements("fn f() -> i53 { return 42; }");
        if let Statement::Return(statement) = &statements[0] {
            let value = statement
                .value
                .as_ref()
                .expect("return should have a value");
            assert!(matches!(
                value.kind,
                ExpressionKind::Literal(LiteralKind::I53(42))
            ));
        } else {
            panic!("expected return statement");
        }
    }

    #[test]
    fn sleep_statement_float_duration() {
        let statements = fn_statements("fn f() { sleep(0.5); }");
        if let Statement::Sleep(statement) = &statements[0] {
            if let ExpressionKind::Literal(LiteralKind::F64(value)) = statement.duration.kind {
                assert!((value - 0.5).abs() < 1e-10);
            } else {
                panic!("expected f64 duration in sleep");
            }
        } else {
            panic!("expected sleep statement");
        }
    }

    #[test]
    fn sleep_statement_variable_duration() {
        let statements = fn_statements("fn f() { sleep(interval); }");
        if let Statement::Sleep(statement) = &statements[0] {
            assert!(matches!(
                &statement.duration.kind,
                ExpressionKind::Variable(name) if name == "interval"
            ));
        } else {
            panic!("expected sleep statement");
        }
    }

    #[test]
    fn literal_integer() {
        let statement = single_let("fn f() { let x = 123; }");
        assert!(matches!(
            statement.init.kind,
            ExpressionKind::Literal(LiteralKind::I53(123))
        ));
    }

    #[test]
    fn literal_negative_integer() {
        let statement = single_let("fn f() { let x = -7; }");
        if let ExpressionKind::Unary(UnaryOperator::Neg, inner) = &statement.init.kind {
            assert!(matches!(
                inner.kind,
                ExpressionKind::Literal(LiteralKind::I53(7))
            ));
        } else {
            panic!("expected unary neg wrapping integer literal");
        }
    }

    #[test]
    fn literal_float() {
        let statement = single_let("fn f() { let x = 2.718281828459045; }");
        if let ExpressionKind::Literal(LiteralKind::F64(value)) = statement.init.kind {
            assert!((value - f64::consts::E).abs() < 1e-10);
        } else {
            panic!("expected f64 literal");
        }
    }

    #[test]
    fn literal_nan() {
        let statement = single_let("fn f() { let x = nan; }");
        if let ExpressionKind::Literal(LiteralKind::F64(value)) = statement.init.kind {
            assert!(value.is_nan());
        } else {
            panic!("expected nan literal");
        }
    }

    #[test]
    fn literal_infinity() {
        let statement = single_let("fn f() { let x = inf; }");
        if let ExpressionKind::Literal(LiteralKind::F64(value)) = statement.init.kind {
            assert!(value.is_infinite() && value > 0.0);
        } else {
            panic!("expected inf literal");
        }
    }

    #[test]
    fn literal_true() {
        let statement = single_let("fn f() { let x = true; }");
        assert!(matches!(
            statement.init.kind,
            ExpressionKind::Literal(LiteralKind::Bool(true))
        ));
    }

    #[test]
    fn literal_false() {
        let statement = single_let("fn f() { let x = false; }");
        assert!(matches!(
            statement.init.kind,
            ExpressionKind::Literal(LiteralKind::Bool(false))
        ));
    }

    #[test]
    fn variable_reference() {
        let statement = single_let("fn f() { let y = x; }");
        assert!(matches!(
            &statement.init.kind,
            ExpressionKind::Variable(name) if name == "x"
        ));
    }

    #[test]
    fn unary_negation() {
        let statement = single_let("fn f() { let x = -value; }");
        if let ExpressionKind::Unary(operator, operand) = &statement.init.kind {
            assert_eq!(*operator, UnaryOperator::Neg);
            assert!(matches!(
                &operand.kind,
                ExpressionKind::Variable(name) if name == "value"
            ));
        } else {
            panic!("expected unary negation");
        }
    }

    #[test]
    fn unary_logical_not() {
        let statement = single_let("fn f() { let x = !flag; }");
        if let ExpressionKind::Unary(operator, operand) = &statement.init.kind {
            assert_eq!(*operator, UnaryOperator::Not);
            assert!(matches!(
                &operand.kind,
                ExpressionKind::Variable(name) if name == "flag"
            ));
        } else {
            panic!("expected unary logical not");
        }
    }

    #[test]
    fn unary_bitwise_not() {
        let statement = single_let("fn f() { let x = ~mask; }");
        if let ExpressionKind::Unary(operator, _) = &statement.init.kind {
            assert_eq!(*operator, UnaryOperator::BitNot);
        } else {
            panic!("expected unary bitwise not");
        }
    }

    #[test]
    fn unary_double_negation() {
        let statement = single_let("fn f() { let x = -~v; }");
        if let ExpressionKind::Unary(UnaryOperator::Neg, inner) = &statement.init.kind {
            assert!(matches!(
                inner.kind,
                ExpressionKind::Unary(UnaryOperator::BitNot, _)
            ));
        } else {
            panic!("expected nested unary");
        }
    }

    #[test]
    fn binary_addition() {
        let statement = single_let("fn f() { let x = a + b; }");
        assert!(matches!(
            statement.init.kind,
            ExpressionKind::Binary(BinaryOperator::Add, _, _)
        ));
    }

    #[test]
    fn binary_subtraction() {
        let statement = single_let("fn f() { let x = a - b; }");
        assert!(matches!(
            statement.init.kind,
            ExpressionKind::Binary(BinaryOperator::Sub, _, _)
        ));
    }

    #[test]
    fn binary_multiplication() {
        let statement = single_let("fn f() { let x = a * b; }");
        assert!(matches!(
            statement.init.kind,
            ExpressionKind::Binary(BinaryOperator::Mul, _, _)
        ));
    }

    #[test]
    fn binary_division() {
        let statement = single_let("fn f() { let x = a / b; }");
        assert!(matches!(
            statement.init.kind,
            ExpressionKind::Binary(BinaryOperator::Div, _, _)
        ));
    }

    #[test]
    fn binary_remainder() {
        let statement = single_let("fn f() { let x = a % b; }");
        assert!(matches!(
            statement.init.kind,
            ExpressionKind::Binary(BinaryOperator::Rem, _, _)
        ));
    }

    #[test]
    fn binary_logical_and() {
        let statement = single_let("fn f() { let x = a && b; }");
        assert!(matches!(
            statement.init.kind,
            ExpressionKind::Binary(BinaryOperator::And, _, _)
        ));
    }

    #[test]
    fn binary_logical_or() {
        let statement = single_let("fn f() { let x = a || b; }");
        assert!(matches!(
            statement.init.kind,
            ExpressionKind::Binary(BinaryOperator::Or, _, _)
        ));
    }

    #[test]
    fn binary_all_comparison_operators() {
        let cases = [
            ("==", BinaryOperator::Eq),
            ("!=", BinaryOperator::Ne),
            ("<", BinaryOperator::Lt),
            (">", BinaryOperator::Gt),
            ("<=", BinaryOperator::Le),
            (">=", BinaryOperator::Ge),
        ];
        for (operator_text, expected) in cases {
            let source = format!("fn f() {{ let x = a {} b; }}", operator_text);
            let statement = single_let(&source);
            if let ExpressionKind::Binary(operator, _, _) = statement.init.kind {
                assert_eq!(operator, expected, "wrong operator for `{}`", operator_text);
            } else {
                panic!("expected binary expression for `{}`", operator_text);
            }
        }
    }

    #[test]
    fn binary_all_bitwise_operators() {
        let cases = [
            ("&", BinaryOperator::BitAnd),
            ("|", BinaryOperator::BitOr),
            ("^", BinaryOperator::BitXor),
            ("<<", BinaryOperator::Shl),
            (">>", BinaryOperator::Shr),
        ];
        for (operator_text, expected) in cases {
            let source = format!("fn f() {{ let x = a {} b; }}", operator_text);
            let statement = single_let(&source);
            if let ExpressionKind::Binary(operator, _, _) = statement.init.kind {
                assert_eq!(operator, expected, "wrong operator for `{}`", operator_text);
            } else {
                panic!("expected binary expression for `{}`", operator_text);
            }
        }
    }

    #[test]
    fn precedence_mul_binds_tighter_than_add() {
        // a + b * c  =>  a + (b * c)
        let statement = single_let("fn f() { let x = a + b * c; }");
        if let ExpressionKind::Binary(BinaryOperator::Add, _lhs, rhs) = &statement.init.kind {
            assert!(matches!(
                rhs.kind,
                ExpressionKind::Binary(BinaryOperator::Mul, _, _)
            ));
        } else {
            panic!("expected Add at top with Mul nested on the right");
        }
    }

    #[test]
    fn precedence_add_left_associative() {
        // a - b - c  =>  (a - b) - c
        let statement = single_let("fn f() { let x = a - b - c; }");
        if let ExpressionKind::Binary(BinaryOperator::Sub, lhs, _rhs) = &statement.init.kind {
            assert!(matches!(
                lhs.kind,
                ExpressionKind::Binary(BinaryOperator::Sub, _, _)
            ));
        } else {
            panic!("expected Sub at top with Sub nested on the left");
        }
    }

    #[test]
    fn precedence_comparison_lower_than_add() {
        // a + b == c  =>  (a + b) == c
        let statement = single_let("fn f() { let x = a + b == c; }");
        if let ExpressionKind::Binary(BinaryOperator::Eq, lhs, _rhs) = &statement.init.kind {
            assert!(matches!(
                lhs.kind,
                ExpressionKind::Binary(BinaryOperator::Add, _, _)
            ));
        } else {
            panic!("expected Eq at top with Add nested on the left");
        }
    }

    #[test]
    fn precedence_logical_or_lowest() {
        // a && b || c && d  =>  (a && b) || (c && d)
        let statement = single_let("fn f() { let x = a && b || c && d; }");
        if let ExpressionKind::Binary(BinaryOperator::Or, lhs, rhs) = &statement.init.kind {
            assert!(matches!(
                lhs.kind,
                ExpressionKind::Binary(BinaryOperator::And, _, _)
            ));
            assert!(matches!(
                rhs.kind,
                ExpressionKind::Binary(BinaryOperator::And, _, _)
            ));
        } else {
            panic!("expected Or at top with And on both sides");
        }
    }

    #[test]
    fn precedence_parentheses_override() {
        // (a + b) * c  =>  Mul((a + b), c)
        let statement = single_let("fn f() { let x = (a + b) * c; }");
        if let ExpressionKind::Binary(BinaryOperator::Mul, lhs, _rhs) = &statement.init.kind {
            assert!(matches!(
                lhs.kind,
                ExpressionKind::Binary(BinaryOperator::Add, _, _)
            ));
        } else {
            panic!("expected Mul at top with Add on the left after grouping");
        }
    }

    // ── type casts ───────────────────────────────────────────────────────────

    #[test]
    fn cast_to_f64() {
        let statement = single_let("fn f() { let x = val as f64; }");
        if let ExpressionKind::Cast(_, target_type) = statement.init.kind {
            assert_eq!(target_type, Type::F64);
        } else {
            panic!("expected cast expression");
        }
    }

    #[test]
    fn cast_to_i53() {
        let statement = single_let("fn f() { let x = val as i53; }");
        if let ExpressionKind::Cast(_, target_type) = statement.init.kind {
            assert_eq!(target_type, Type::I53);
        } else {
            panic!("expected cast expression");
        }
    }

    #[test]
    fn cast_to_bool() {
        let statement = single_let("fn f() { let x = val as bool; }");
        if let ExpressionKind::Cast(_, target_type) = statement.init.kind {
            assert_eq!(target_type, Type::Bool);
        } else {
            panic!("expected cast expression");
        }
    }

    #[test]
    fn cast_binds_tighter_than_add() {
        // a + b as f64  =>  a + (b as f64)
        let statement = single_let("fn f() { let x = a + b as f64; }");
        if let ExpressionKind::Binary(BinaryOperator::Add, _lhs, rhs) = &statement.init.kind {
            assert!(matches!(rhs.kind, ExpressionKind::Cast(_, Type::F64)));
        } else {
            panic!("expected Add at the top with Cast on the right");
        }
    }

    #[test]
    fn call_no_args() {
        let statement = single_let("fn f() { let x = compute(); }");
        if let ExpressionKind::Call(call) = &statement.init.kind {
            assert_eq!(call.name, "compute");
            assert!(call.args.is_empty());
        } else {
            panic!("expected call expression");
        }
    }

    #[test]
    fn call_multiple_args() {
        let statement = single_let("fn f() { let x = add(a, b); }");
        if let ExpressionKind::Call(call) = &statement.init.kind {
            assert_eq!(call.name, "add");
            assert_eq!(call.args.len(), 2);
            assert!(matches!(
                &call.args[0].kind,
                ExpressionKind::Variable(name) if name == "a"
            ));
            assert!(matches!(
                &call.args[1].kind,
                ExpressionKind::Variable(name) if name == "b"
            ));
        } else {
            panic!("expected call expression");
        }
    }

    #[test]
    fn builtin_abs() {
        let statement = single_let("fn f() { let x = abs(v); }");
        if let ExpressionKind::BuiltinCall(function, arguments) = &statement.init.kind {
            assert_eq!(*function, BuiltinFunction::Abs);
            assert_eq!(arguments.len(), 1);
        } else {
            panic!("expected builtin call");
        }
    }

    #[test]
    fn builtin_atan2() {
        let statement = single_let("fn f() { let x = atan2(y, x); }");
        if let ExpressionKind::BuiltinCall(function, arguments) = &statement.init.kind {
            assert_eq!(*function, BuiltinFunction::Atan2);
            assert_eq!(arguments.len(), 2);
        } else {
            panic!("expected builtin call");
        }
    }

    #[test]
    fn builtin_clamp() {
        let statement = single_let("fn f() { let x = clamp(v, 0, 100); }");
        if let ExpressionKind::BuiltinCall(function, arguments) = &statement.init.kind {
            assert_eq!(*function, BuiltinFunction::Clamp);
            assert_eq!(arguments.len(), 3);
        } else {
            panic!("expected builtin call");
        }
    }

    #[test]
    fn builtin_lerp() {
        let statement = single_let("fn f() { let x = lerp(a, b, t); }");
        if let ExpressionKind::BuiltinCall(function, _) = &statement.init.kind {
            assert_eq!(*function, BuiltinFunction::Lerp);
        } else {
            panic!("expected builtin call");
        }
    }

    #[test]
    fn all_single_argument_builtins_recognized() {
        let cases = [
            ("abs", BuiltinFunction::Abs),
            ("ceil", BuiltinFunction::Ceil),
            ("floor", BuiltinFunction::Floor),
            ("round", BuiltinFunction::Round),
            ("trunc", BuiltinFunction::Trunc),
            ("sqrt", BuiltinFunction::Sqrt),
            ("exp", BuiltinFunction::Exp),
            ("log", BuiltinFunction::Log),
            ("sin", BuiltinFunction::Sin),
            ("cos", BuiltinFunction::Cos),
            ("tan", BuiltinFunction::Tan),
            ("asin", BuiltinFunction::Asin),
            ("acos", BuiltinFunction::Acos),
            ("atan", BuiltinFunction::Atan),
        ];
        for (name, expected) in cases {
            let source = format!("fn f() {{ let x = {}(v); }}", name);
            let statement = single_let(&source);
            if let ExpressionKind::BuiltinCall(function, _) = &statement.init.kind {
                assert_eq!(*function, expected, "wrong builtin for `{}`", name);
            } else {
                panic!("expected builtin call for `{}`", name);
            }
        }
    }

    #[test]
    fn builtin_rand_no_args() {
        let statement = single_let("fn f() { let x = rand(); }");
        if let ExpressionKind::BuiltinCall(function, arguments) = &statement.init.kind {
            assert_eq!(*function, BuiltinFunction::Rand);
            assert!(arguments.is_empty());
        } else {
            panic!("expected builtin call");
        }
    }

    #[test]
    fn device_field_read() {
        let statement = single_let("fn f() { let x = pump.Temperature; }");
        if let ExpressionKind::DeviceRead { device, field } = &statement.init.kind {
            assert_eq!(device, "pump");
            assert_eq!(field, "Temperature");
        } else {
            panic!("expected device field read");
        }
    }

    #[test]
    fn device_slot_field_read() {
        let statement = single_let("fn f() { let x = rack.slot(2).Quantity; }");
        if let ExpressionKind::SlotRead {
            device,
            slot,
            field,
        } = &statement.init.kind
        {
            assert_eq!(device, "rack");
            assert_eq!(field, "Quantity");
            assert!(matches!(
                slot.kind,
                ExpressionKind::Literal(LiteralKind::I53(2))
            ));
        } else {
            panic!("expected slot field read");
        }
    }

    #[test]
    fn device_slot_field_read_variable_index() {
        let statement = single_let("fn f() { let x = rack.slot(idx).Quantity; }");
        if let ExpressionKind::SlotRead {
            device,
            slot,
            field,
        } = &statement.init.kind
        {
            assert_eq!(device, "rack");
            assert_eq!(field, "Quantity");
            assert!(matches!(
                &slot.kind,
                ExpressionKind::Variable(name) if name == "idx"
            ));
        } else {
            panic!("expected slot field read");
        }
    }

    #[test]
    fn batch_read_average_mode() {
        let statement =
            single_let(r#"fn f() { let x = batch_read(hash("MyDevice"), Temperature, Average); }"#);
        if let ExpressionKind::BatchRead {
            hash_expr,
            field,
            mode,
        } = &statement.init.kind
        {
            assert_eq!(field, "Temperature");
            assert_eq!(*mode, BatchMode::Average);
            assert!(matches!(
                &hash_expr.kind,
                ExpressionKind::Hash(s) if s == "MyDevice"
            ));
        } else {
            panic!("expected batch read expression");
        }
    }

    #[test]
    fn batch_read_all_modes_recognized() {
        let cases = [
            ("Average", BatchMode::Average),
            ("Sum", BatchMode::Sum),
            ("Minimum", BatchMode::Minimum),
            ("Maximum", BatchMode::Maximum),
            ("Contents", BatchMode::Contents),
        ];
        for (mode_name, expected) in cases {
            let source = format!("fn f() {{ let x = batch_read(h, Field, {}); }}", mode_name);
            let statement = single_let(&source);
            if let ExpressionKind::BatchRead { mode, .. } = &statement.init.kind {
                assert_eq!(*mode, expected, "wrong mode for `{}`", mode_name);
            } else {
                panic!("expected batch read for mode `{}`", mode_name);
            }
        }
    }

    #[test]
    fn select_expression() {
        let statement = single_let("fn f() { let x = select(cond, a, b); }");
        if let ExpressionKind::Select {
            cond,
            if_true,
            if_false,
        } = &statement.init.kind
        {
            assert!(matches!(
                &cond.kind,
                ExpressionKind::Variable(name) if name == "cond"
            ));
            assert!(matches!(
                &if_true.kind,
                ExpressionKind::Variable(name) if name == "a"
            ));
            assert!(matches!(
                &if_false.kind,
                ExpressionKind::Variable(name) if name == "b"
            ));
        } else {
            panic!("expected select expression");
        }
    }

    #[test]
    fn hash_expression() {
        let statement = single_let(r#"fn f() { let x = hash("MyCircuitBoard"); }"#);
        if let ExpressionKind::Hash(string_value) = &statement.init.kind {
            assert_eq!(string_value, "MyCircuitBoard");
        } else {
            panic!("expected hash expression");
        }
    }

    #[test]
    fn multiple_top_level_items() {
        let program = parse_ok("const A: i53 = 1;\ndevice pump: d0;\nfn main() { }");
        assert_eq!(program.items.len(), 3);
        assert!(matches!(program.items[0], Item::Const(_)));
        assert!(matches!(program.items[1], Item::Device(_)));
        assert!(matches!(program.items[2], Item::Fn(_)));
    }

    #[test]
    fn multiple_functions() {
        let program = parse_ok("fn a() { }\nfn b() { }\nfn c() { }");
        assert_eq!(program.items.len(), 3);
        for item in &program.items {
            assert!(matches!(item, Item::Fn(_)));
        }
    }

    #[test]
    fn function_multiple_statements() {
        let statements =
            fn_statements("fn f() { let x: i53 = 0; let mut y: f64 = 1.0; x = 5; return; }");
        assert_eq!(statements.len(), 4);
        assert!(matches!(statements[0], Statement::Let(_)));
        assert!(matches!(statements[1], Statement::Let(_)));
        assert!(matches!(statements[2], Statement::Assign(_)));
        assert!(matches!(statements[3], Statement::Return(_)));
    }

    #[test]
    fn nested_if_inside_loop() {
        let statements = fn_statements("fn f() { loop { if x { break; } } }");
        if let Statement::While(loop_statement) = &statements[0] {
            assert_eq!(loop_statement.body.stmts.len(), 1);
            assert!(matches!(loop_statement.body.stmts[0], Statement::If(_)));
        } else {
            panic!("expected while statement (desugared loop)");
        }
    }

    #[test]
    fn program_span_starts_at_zero() {
        let program = parse_ok("fn f() { }");
        assert_eq!(program.span.start, 0);
    }

    #[test]
    fn function_span_covers_full_declaration() {
        let source = "fn f() { }";
        let declaration = single_fn(source);
        assert_eq!(declaration.span.start, 0);
        assert_eq!(declaration.span.end, source.len());
    }

    #[test]
    fn error_on_unexpected_top_level_token() {
        let errors = parse_errors("42;");
        assert!(!errors.is_empty());
    }

    #[test]
    fn error_on_bad_device_pin() {
        let errors = parse_errors("device x: d9;");
        assert!(!errors.is_empty());
    }

    #[test]
    fn error_on_missing_colon_in_const() {
        let errors = parse_errors("const X i53 = 42;");
        assert!(!errors.is_empty());
    }

    #[test]
    fn error_on_missing_type_in_const() {
        let errors = parse_errors("const X: = 42;");
        assert!(!errors.is_empty());
    }

    #[test]
    fn error_on_missing_fn_body() {
        let errors = parse_errors("fn f()");
        assert!(!errors.is_empty());
    }

    #[test]
    fn error_on_missing_type_after_return_arrow() {
        let errors = parse_errors("fn f() -> { }");
        assert!(!errors.is_empty());
    }

    #[test]
    fn error_on_unclosed_block() {
        let errors = parse_errors("fn f() { let x = 1;");
        assert!(!errors.is_empty());
    }

    #[test]
    fn error_on_non_string_hash_argument() {
        let errors = parse_errors("fn f() { let x = hash(42); }");
        assert!(!errors.is_empty());
    }

    #[test]
    fn error_on_invalid_batch_mode() {
        let errors = parse_errors("fn f() { let x = batch_read(h, Field, Unknown); }");
        assert!(!errors.is_empty());
    }

    #[test]
    fn error_recovery_continues_after_bad_item() {
        // A is missing its value expression (bare `;` token) — an error is recorded
        // and synchronize() consumes the `;`, leaving B to parse cleanly.
        let (program, diagnostics) = parse("const A: i53 = ;\nconst B: i53 = 2;");
        let errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(!errors.is_empty(), "expected at least one error");
        let parsed_b = program
            .items
            .iter()
            .any(|item| matches!(item, Item::Const(declaration) if declaration.name == "B"));
        assert!(
            parsed_b,
            "B should have been parsed despite the earlier error"
        );
    }

    #[test]
    fn error_recovery_multiple_bad_items() {
        // Two broken items followed by a valid one.
        let (program, diagnostics) = parse("42;\n99;\nfn ok() { }");
        let errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(!errors.is_empty());
        let has_ok = program
            .items
            .iter()
            .any(|item| matches!(item, Item::Fn(declaration) if declaration.name == "ok"));
        assert!(has_ok, "valid fn after errors should be parsed");
    }
}
