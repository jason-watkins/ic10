//! Lexer — tokenizes IC20 source text into a flat token stream.

use crate::diagnostic::{Diagnostic, Span};

/// A parsed literal value.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    /// A 53-bit signed integer literal (decimal, hex, octal, or binary).
    I53(i64),
    /// A 64-bit floating-point literal.
    F64(f64),
    /// A double-quoted string literal (used only by `hash("...")`).
    String(String),
}

/// An active keyword recognized by the lexer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Keyword {
    /// `let` — local variable binding.
    Let,
    /// `const` — compile-time constant declaration.
    Const,
    /// `fn` — function declaration.
    Fn,
    /// `if` — conditional branch.
    If,
    /// `else` — alternative branch.
    Else,
    /// `loop` — infinite loop.
    Loop,
    /// `while` — condition-tested loop.
    While,
    /// `for` — range-based iteration.
    For,
    /// `in` — range delimiter in `for` loops.
    In,
    /// `break` — exit a loop.
    Break,
    /// `continue` — skip to the next loop iteration.
    Continue,
    /// `return` — exit a function.
    Return,
    /// `yield` — suspend execution for one IC10 tick.
    Yield,
    /// `sleep` — suspend execution for a duration.
    Sleep,
    /// `device` — hardware pin binding.
    Device,
    /// `static` — top-level persistent variable.
    Static,
    /// `as` — type cast operator.
    As,
    /// `mut` — mutable qualifier.
    Mut,
    /// The `bool` type keyword.
    Bool,
    /// The `i53` type keyword.
    I53,
    /// The `f64` type keyword.
    F64,
    /// Boolean literal `true`.
    True,
    /// Boolean literal `false`.
    False,
    /// The `nan` floating-point constant.
    Nan,
    /// The `inf` floating-point constant.
    Inf,
}

/// A reserved keyword that is not yet implemented but is reserved for future use.
/// Using one of these as an identifier produces a diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reserved {
    Enum,
    Extern,
    Match,
    Pub,
    Ref,
    Struct,
    Super,
    Trait,
    Type,
    Unsafe,
    Where,
    Pin,
    BatchReadNamed,
    BatchWriteNamed,
    BatchReadSlot,
    BatchWriteSlot,
    BatchReadSlotNamed,
    BatchWriteSlotNamed,
    BitExtract,
    BitInsert,
}

/// A binary or unary operator token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operator {
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `/`
    Slash,
    /// `%`
    Percent,
    /// `&`
    Amp,
    /// `|`
    Pipe,
    /// `^`
    Caret,
    /// `~`
    Tilde,
    /// `<<`
    Shl,
    /// `>>`
    Shr,
    /// `==`
    EqEq,
    /// `!=`
    BangEq,
    /// `<`
    Lt,
    /// `>`
    Gt,
    /// `<=`
    LtEq,
    /// `>=`
    GtEq,
    /// `&&`
    AmpAmp,
    /// `||`
    PipePipe,
    /// `!`
    Bang,
    /// `=`
    Eq,
}

/// A punctuation token (delimiters, separators, range operators).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Punctuator {
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `{`
    LBrace,
    /// `}`
    RBrace,
    /// `;`
    Semi,
    /// `:`
    Colon,
    /// `,`
    Comma,
    /// `.`
    Dot,
    /// `->`
    Arrow,
    /// `..` (exclusive range)
    DotDot,
    /// `..=` (inclusive range)
    DotDotEq,
}

/// Every distinct token the lexer can produce.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    /// A numeric or string literal.
    Literal(Literal),
    /// A user-defined name.
    Identifier(String),
    /// A label: `'name` (used for labeled loops, break, and continue).
    Label(String),
    /// A language keyword.
    Keyword(Keyword),
    /// A reserved word (not yet implemented).
    Reserved(Reserved),
    /// A binary or unary operator.
    Operator(Operator),
    /// A delimiter or separator.
    Punctuator(Punctuator),
    /// End of input.
    Eof,
}

/// A token with its kind and source span.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    /// Creates a new token with the given kind and span.
    pub fn new(kind: TokenKind, span: Span) -> Self {
        Self { kind, span }
    }
}

fn keyword(s: &str) -> Option<TokenKind> {
    match s {
        // Active keywords
        "let" => Some(TokenKind::Keyword(Keyword::Let)),
        "const" => Some(TokenKind::Keyword(Keyword::Const)),
        "fn" => Some(TokenKind::Keyword(Keyword::Fn)),
        "if" => Some(TokenKind::Keyword(Keyword::If)),
        "else" => Some(TokenKind::Keyword(Keyword::Else)),
        "loop" => Some(TokenKind::Keyword(Keyword::Loop)),
        "while" => Some(TokenKind::Keyword(Keyword::While)),
        "for" => Some(TokenKind::Keyword(Keyword::For)),
        "in" => Some(TokenKind::Keyword(Keyword::In)),
        "break" => Some(TokenKind::Keyword(Keyword::Break)),
        "continue" => Some(TokenKind::Keyword(Keyword::Continue)),
        "return" => Some(TokenKind::Keyword(Keyword::Return)),
        "yield" => Some(TokenKind::Keyword(Keyword::Yield)),
        "sleep" => Some(TokenKind::Keyword(Keyword::Sleep)),
        "device" => Some(TokenKind::Keyword(Keyword::Device)),
        "as" => Some(TokenKind::Keyword(Keyword::As)),
        "mut" => Some(TokenKind::Keyword(Keyword::Mut)),
        "bool" => Some(TokenKind::Keyword(Keyword::Bool)),
        "i53" => Some(TokenKind::Keyword(Keyword::I53)),
        "f64" => Some(TokenKind::Keyword(Keyword::F64)),
        "true" => Some(TokenKind::Keyword(Keyword::True)),
        "false" => Some(TokenKind::Keyword(Keyword::False)),
        "nan" => Some(TokenKind::Keyword(Keyword::Nan)),
        "inf" => Some(TokenKind::Keyword(Keyword::Inf)),
        // Reserved keywords
        "enum" => Some(TokenKind::Reserved(Reserved::Enum)),
        "extern" => Some(TokenKind::Reserved(Reserved::Extern)),
        "match" => Some(TokenKind::Reserved(Reserved::Match)),
        "pub" => Some(TokenKind::Reserved(Reserved::Pub)),
        "ref" => Some(TokenKind::Reserved(Reserved::Ref)),
        "static" => Some(TokenKind::Keyword(Keyword::Static)),
        "struct" => Some(TokenKind::Reserved(Reserved::Struct)),
        "super" => Some(TokenKind::Reserved(Reserved::Super)),
        "trait" => Some(TokenKind::Reserved(Reserved::Trait)),
        "type" => Some(TokenKind::Reserved(Reserved::Type)),
        "unsafe" => Some(TokenKind::Reserved(Reserved::Unsafe)),
        "where" => Some(TokenKind::Reserved(Reserved::Where)),
        "pin" => Some(TokenKind::Reserved(Reserved::Pin)),
        "batch_read_named" => Some(TokenKind::Reserved(Reserved::BatchReadNamed)),
        "batch_write_named" => Some(TokenKind::Reserved(Reserved::BatchWriteNamed)),
        "batch_read_slot" => Some(TokenKind::Reserved(Reserved::BatchReadSlot)),
        "batch_write_slot" => Some(TokenKind::Reserved(Reserved::BatchWriteSlot)),
        "batch_read_slot_named" => Some(TokenKind::Reserved(Reserved::BatchReadSlotNamed)),
        "batch_write_slot_named" => Some(TokenKind::Reserved(Reserved::BatchWriteSlotNamed)),
        "bit_extract" => Some(TokenKind::Reserved(Reserved::BitExtract)),
        "bit_insert" => Some(TokenKind::Reserved(Reserved::BitInsert)),
        _ => None,
    }
}

/// Converts IC20 source text into a flat `Vec<Token>`.
pub struct Lexer<'src> {
    /// The complete source text being lexed.
    source: &'src str,
    /// Raw bytes of `source` for fast single-byte lookahead.
    bytes: &'src [u8],
    /// Current byte position in the source.
    pos: usize,
    /// Accumulated diagnostics (errors and warnings).
    diagnostics: Vec<Diagnostic>,
}

impl<'src> Lexer<'src> {
    /// Creates a new lexer for the given source text, starting at position 0.
    pub fn new(source: &'src str) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            pos: 0,
            diagnostics: Vec::new(),
        }
    }

    /// Tokenize the entire source, returning tokens and any diagnostics.
    /// The returned token list always ends with an `Eof` token.
    pub fn tokenize(mut self) -> (Vec<Token>, Vec<Diagnostic>) {
        let mut tokens = Vec::new();
        loop {
            self.skip_whitespace_and_comments();
            if self.at_end() {
                tokens.push(Token::new(TokenKind::Eof, Span::new(self.pos, self.pos)));
                break;
            }
            match self.next_token() {
                Some(tok) => tokens.push(tok),
                None => {
                    // Error recovery: skip the unknown character.
                    let start = self.pos;
                    let ch = self.advance_char();
                    self.diagnostics.push(Diagnostic::error(
                        Span::new(start, self.pos),
                        format!("unexpected character '{}'", ch),
                    ));
                }
            }
        }
        (tokens, self.diagnostics)
    }

    fn at_end(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    fn peek(&self) -> u8 {
        if self.at_end() {
            0
        } else {
            self.bytes[self.pos]
        }
    }

    fn peek_at(&self, offset: usize) -> u8 {
        let i = self.pos + offset;
        if i >= self.bytes.len() {
            0
        } else {
            self.bytes[i]
        }
    }

    fn advance(&mut self) -> u8 {
        let b = self.bytes[self.pos];
        self.pos += 1;
        b
    }

    /// Advance one full UTF-8 character and return it.
    fn advance_char(&mut self) -> char {
        let start = self.pos;
        let ch = self.source[start..].chars().next().unwrap_or('\0');
        self.pos += ch.len_utf8();
        ch
    }

    fn matches(&mut self, expected: u8) -> bool {
        if self.peek() == expected {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            // Skip whitespace
            while !self.at_end() && matches!(self.peek(), b' ' | b'\t' | b'\r' | b'\n') {
                self.pos += 1;
            }
            if self.at_end() {
                return;
            }
            // Single-line comment
            if self.peek() == b'/' && self.peek_at(1) == b'/' {
                self.pos += 2;
                while !self.at_end() && self.peek() != b'\n' {
                    self.pos += 1;
                }
                continue;
            }
            // Block comment
            if self.peek() == b'/' && self.peek_at(1) == b'*' {
                let start = self.pos;
                self.pos += 2;
                let mut closed = false;
                while !self.at_end() {
                    if self.peek() == b'*' && self.peek_at(1) == b'/' {
                        self.pos += 2;
                        closed = true;
                        break;
                    }
                    self.pos += 1;
                }
                if !closed {
                    self.diagnostics.push(Diagnostic::error(
                        Span::new(start, self.pos),
                        "unterminated block comment",
                    ));
                }
                continue;
            }
            break;
        }
    }

    fn next_token(&mut self) -> Option<Token> {
        let start = self.pos;
        let b = self.peek();

        // Identifier or keyword
        if b.is_ascii_alphabetic() || b == b'_' {
            return Some(self.lex_ident_or_keyword(start));
        }

        // Numeric literal (decimal, hex, octal, binary, float)
        if b.is_ascii_digit() {
            return Some(self.lex_number(start));
        }

        // String literal
        if b == b'"' {
            return Some(self.lex_string(start));
        }

        // Label: 'identifier
        if b == b'\'' {
            return Some(self.lex_label(start));
        }

        // Operators and punctuators
        self.lex_operator(start)
    }

    fn lex_label(&mut self, start: usize) -> Token {
        self.pos += 1; // skip the leading '
        let name_start = self.pos;
        while !self.at_end() && (self.peek().is_ascii_alphanumeric() || self.peek() == b'_') {
            self.pos += 1;
        }
        let name = self.source[name_start..self.pos].to_string();
        if name.is_empty() {
            self.diagnostics.push(Diagnostic::error(
                Span::new(start, self.pos),
                "expected identifier after `'`",
            ));
        }
        Token::new(TokenKind::Label(name), Span::new(start, self.pos))
    }

    fn lex_ident_or_keyword(&mut self, start: usize) -> Token {
        while !self.at_end() && (self.peek().is_ascii_alphanumeric() || self.peek() == b'_') {
            self.pos += 1;
        }
        let text = &self.source[start..self.pos];
        let span = Span::new(start, self.pos);
        let kind = keyword(text).unwrap_or_else(|| TokenKind::Identifier(text.to_string()));
        Token::new(kind, span)
    }

    fn lex_number(&mut self, start: usize) -> Token {
        // Check for 0x, 0o, 0b prefixes
        if self.peek() == b'0' {
            match self.peek_at(1) {
                b'x' | b'X' => return self.lex_hex(start),
                b'o' | b'O' => return self.lex_octal(start),
                b'b' | b'B' => return self.lex_binary(start),
                _ => {}
            }
        }

        // Decimal integer or float
        self.eat_decimal_digits();

        // Check for float: decimal point with digit after, or exponent
        if self.peek() == b'.' && self.peek_at(1).is_ascii_digit() {
            self.pos += 1; // consume '.'
            self.eat_decimal_digits();
            self.eat_exponent();
            return self.make_float_token(start);
        }

        if self.peek() == b'e' || self.peek() == b'E' {
            self.eat_exponent();
            return self.make_float_token(start);
        }

        // Plain integer
        self.make_int_token(start, 10)
    }

    fn lex_hex(&mut self, start: usize) -> Token {
        self.pos += 2; // skip 0x
        let digit_start = self.pos;
        while !self.at_end() && (self.peek().is_ascii_hexdigit() || self.peek() == b'_') {
            self.pos += 1;
        }
        if self.pos == digit_start {
            self.diagnostics.push(Diagnostic::error(
                Span::new(start, self.pos),
                "expected hex digits after '0x'",
            ));
            return Token::new(
                TokenKind::Literal(Literal::I53(0)),
                Span::new(start, self.pos),
            );
        }
        self.make_int_token(start, 16)
    }

    fn lex_octal(&mut self, start: usize) -> Token {
        self.pos += 2; // skip 0o
        let digit_start = self.pos;
        while !self.at_end()
            && ((self.peek() >= b'0' && self.peek() <= b'7') || self.peek() == b'_')
        {
            self.pos += 1;
        }
        if self.pos == digit_start {
            self.diagnostics.push(Diagnostic::error(
                Span::new(start, self.pos),
                "expected octal digits after '0o'",
            ));
            return Token::new(
                TokenKind::Literal(Literal::I53(0)),
                Span::new(start, self.pos),
            );
        }
        self.make_int_token(start, 8)
    }

    fn lex_binary(&mut self, start: usize) -> Token {
        self.pos += 2; // skip 0b
        let digit_start = self.pos;
        while !self.at_end() && (self.peek() == b'0' || self.peek() == b'1' || self.peek() == b'_')
        {
            self.pos += 1;
        }
        if self.pos == digit_start {
            self.diagnostics.push(Diagnostic::error(
                Span::new(start, self.pos),
                "expected binary digits after '0b'",
            ));
            return Token::new(
                TokenKind::Literal(Literal::I53(0)),
                Span::new(start, self.pos),
            );
        }
        self.make_int_token(start, 2)
    }

    fn eat_decimal_digits(&mut self) {
        while !self.at_end() && (self.peek().is_ascii_digit() || self.peek() == b'_') {
            self.pos += 1;
        }
    }

    fn eat_exponent(&mut self) {
        if self.peek() == b'e' || self.peek() == b'E' {
            self.pos += 1;
            if self.peek() == b'+' || self.peek() == b'-' {
                self.pos += 1;
            }
            self.eat_decimal_digits();
        }
    }

    fn make_int_token(&mut self, start: usize, radix: u32) -> Token {
        let span = Span::new(start, self.pos);
        let raw = &self.source[start..self.pos];

        let digits: String = if radix == 10 {
            raw.replace('_', "")
        } else {
            raw[2..].replace('_', "")
        };

        match i64::from_str_radix(&digits, radix) {
            Ok(v) => {
                // Check i53 range: −2^53 to 2^53
                const MAX_I53: i64 = 1 << 53;
                if !(-MAX_I53..=MAX_I53).contains(&v) {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        format!("integer literal {} is out of i53 range", v),
                    ));
                }
                Token::new(TokenKind::Literal(Literal::I53(v)), span)
            }
            Err(_) => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!("invalid integer literal '{}'", raw),
                ));
                Token::new(TokenKind::Literal(Literal::I53(0)), span)
            }
        }
    }

    fn make_float_token(&mut self, start: usize) -> Token {
        let span = Span::new(start, self.pos);
        let raw = &self.source[start..self.pos];
        let clean: String = raw.replace('_', "");
        match clean.parse::<f64>() {
            Ok(v) => Token::new(TokenKind::Literal(Literal::F64(v)), span),
            Err(_) => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!("invalid float literal '{}'", raw),
                ));
                Token::new(TokenKind::Literal(Literal::F64(0.0)), span)
            }
        }
    }

    fn lex_string(&mut self, start: usize) -> Token {
        self.pos += 1; // skip opening '"'
        let content_start = self.pos;
        loop {
            if self.at_end() || self.peek() == b'\n' || self.peek() == b'\r' {
                self.diagnostics.push(Diagnostic::error(
                    Span::new(start, self.pos),
                    "unterminated string literal",
                ));
                let content = self.source[content_start..self.pos].to_string();
                return Token::new(
                    TokenKind::Literal(Literal::String(content)),
                    Span::new(start, self.pos),
                );
            }
            if self.peek() == b'"' {
                let content = self.source[content_start..self.pos].to_string();
                self.pos += 1; // skip closing '"'
                return Token::new(
                    TokenKind::Literal(Literal::String(content)),
                    Span::new(start, self.pos),
                );
            }
            self.pos += 1;
        }
    }

    fn lex_operator(&mut self, start: usize) -> Option<Token> {
        let b = self.advance();
        let kind = match b {
            b'+' => TokenKind::Operator(Operator::Plus),
            b'*' => TokenKind::Operator(Operator::Star),
            b'%' => TokenKind::Operator(Operator::Percent),
            b'^' => TokenKind::Operator(Operator::Caret),
            b'~' => TokenKind::Operator(Operator::Tilde),
            b'(' => TokenKind::Punctuator(Punctuator::LParen),
            b')' => TokenKind::Punctuator(Punctuator::RParen),
            b'{' => TokenKind::Punctuator(Punctuator::LBrace),
            b'}' => TokenKind::Punctuator(Punctuator::RBrace),
            b';' => TokenKind::Punctuator(Punctuator::Semi),
            b':' => TokenKind::Punctuator(Punctuator::Colon),
            b',' => TokenKind::Punctuator(Punctuator::Comma),
            b'-' => {
                if self.matches(b'>') {
                    TokenKind::Punctuator(Punctuator::Arrow)
                } else {
                    TokenKind::Operator(Operator::Minus)
                }
            }
            b'/' => TokenKind::Operator(Operator::Slash),
            b'&' => {
                if self.matches(b'&') {
                    TokenKind::Operator(Operator::AmpAmp)
                } else {
                    TokenKind::Operator(Operator::Amp)
                }
            }
            b'|' => {
                if self.matches(b'|') {
                    TokenKind::Operator(Operator::PipePipe)
                } else {
                    TokenKind::Operator(Operator::Pipe)
                }
            }
            b'<' => {
                if self.matches(b'<') {
                    TokenKind::Operator(Operator::Shl)
                } else if self.matches(b'=') {
                    TokenKind::Operator(Operator::LtEq)
                } else {
                    TokenKind::Operator(Operator::Lt)
                }
            }
            b'>' => {
                if self.matches(b'>') {
                    TokenKind::Operator(Operator::Shr)
                } else if self.matches(b'=') {
                    TokenKind::Operator(Operator::GtEq)
                } else {
                    TokenKind::Operator(Operator::Gt)
                }
            }
            b'=' => {
                if self.matches(b'=') {
                    TokenKind::Operator(Operator::EqEq)
                } else {
                    TokenKind::Operator(Operator::Eq)
                }
            }
            b'!' => {
                if self.matches(b'=') {
                    TokenKind::Operator(Operator::BangEq)
                } else {
                    TokenKind::Operator(Operator::Bang)
                }
            }
            b'.' => {
                if self.matches(b'.') {
                    if self.matches(b'=') {
                        TokenKind::Punctuator(Punctuator::DotDotEq)
                    } else {
                        TokenKind::Punctuator(Punctuator::DotDot)
                    }
                } else {
                    TokenKind::Punctuator(Punctuator::Dot)
                }
            }
            _ => {
                // Put pos back — the unknown char will be handled by the caller.
                self.pos = start;
                return None;
            }
        };
        Some(Token::new(kind, Span::new(start, self.pos)))
    }
}

/// Tokenize an IC20 source string. Returns the token stream and any diagnostics.
pub fn tokenize(source: &str) -> (Vec<Token>, Vec<Diagnostic>) {
    Lexer::new(source).tokenize()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: lex a source string and return just the token kinds (dropping Eof).
    fn kinds(source: &str) -> Vec<TokenKind> {
        let (tokens, diags) = tokenize(source);
        assert!(diags.is_empty(), "unexpected diagnostics: {:?}", diags);
        tokens
            .into_iter()
            .map(|t| t.kind)
            .filter(|k| *k != TokenKind::Eof)
            .collect()
    }

    /// Helper: lex and return diagnostics.
    fn diags(source: &str) -> Vec<Diagnostic> {
        let (_, diags) = tokenize(source);
        diags
    }

    // ── Token-sequence tests ─────────────────────────────────────────

    #[test]
    fn all_keywords() {
        assert_eq!(
            kinds(
                "let const fn if else loop while for in break continue return yield sleep device as mut bool i53 f64 true false nan inf"
            ),
            vec![
                TokenKind::Keyword(Keyword::Let),
                TokenKind::Keyword(Keyword::Const),
                TokenKind::Keyword(Keyword::Fn),
                TokenKind::Keyword(Keyword::If),
                TokenKind::Keyword(Keyword::Else),
                TokenKind::Keyword(Keyword::Loop),
                TokenKind::Keyword(Keyword::While),
                TokenKind::Keyword(Keyword::For),
                TokenKind::Keyword(Keyword::In),
                TokenKind::Keyword(Keyword::Break),
                TokenKind::Keyword(Keyword::Continue),
                TokenKind::Keyword(Keyword::Return),
                TokenKind::Keyword(Keyword::Yield),
                TokenKind::Keyword(Keyword::Sleep),
                TokenKind::Keyword(Keyword::Device),
                TokenKind::Keyword(Keyword::As),
                TokenKind::Keyword(Keyword::Mut),
                TokenKind::Keyword(Keyword::Bool),
                TokenKind::Keyword(Keyword::I53),
                TokenKind::Keyword(Keyword::F64),
                TokenKind::Keyword(Keyword::True),
                TokenKind::Keyword(Keyword::False),
                TokenKind::Keyword(Keyword::Nan),
                TokenKind::Keyword(Keyword::Inf),
            ],
        );
    }

    #[test]
    fn all_reserved_keywords() {
        assert_eq!(
            kinds("enum extern match pub ref struct super trait type unsafe where pin"),
            vec![
                TokenKind::Reserved(Reserved::Enum),
                TokenKind::Reserved(Reserved::Extern),
                TokenKind::Reserved(Reserved::Match),
                TokenKind::Reserved(Reserved::Pub),
                TokenKind::Reserved(Reserved::Ref),
                TokenKind::Reserved(Reserved::Struct),
                TokenKind::Reserved(Reserved::Super),
                TokenKind::Reserved(Reserved::Trait),
                TokenKind::Reserved(Reserved::Type),
                TokenKind::Reserved(Reserved::Unsafe),
                TokenKind::Reserved(Reserved::Where),
                TokenKind::Reserved(Reserved::Pin),
            ],
        );
    }

    #[test]
    fn is_nan_tokenizes_as_identifier() {
        assert_eq!(
            kinds("is_nan"),
            vec![TokenKind::Identifier("is_nan".to_string())],
        );
    }

    #[test]
    fn all_operators_and_punctuators() {
        assert_eq!(
            kinds("+ - * / % & | ^ ~ << >> == != < > <= >= && || ! = ( ) { } ; : , . -> .."),
            vec![
                TokenKind::Operator(Operator::Plus),
                TokenKind::Operator(Operator::Minus),
                TokenKind::Operator(Operator::Star),
                TokenKind::Operator(Operator::Slash),
                TokenKind::Operator(Operator::Percent),
                TokenKind::Operator(Operator::Amp),
                TokenKind::Operator(Operator::Pipe),
                TokenKind::Operator(Operator::Caret),
                TokenKind::Operator(Operator::Tilde),
                TokenKind::Operator(Operator::Shl),
                TokenKind::Operator(Operator::Shr),
                TokenKind::Operator(Operator::EqEq),
                TokenKind::Operator(Operator::BangEq),
                TokenKind::Operator(Operator::Lt),
                TokenKind::Operator(Operator::Gt),
                TokenKind::Operator(Operator::LtEq),
                TokenKind::Operator(Operator::GtEq),
                TokenKind::Operator(Operator::AmpAmp),
                TokenKind::Operator(Operator::PipePipe),
                TokenKind::Operator(Operator::Bang),
                TokenKind::Operator(Operator::Eq),
                TokenKind::Punctuator(Punctuator::LParen),
                TokenKind::Punctuator(Punctuator::RParen),
                TokenKind::Punctuator(Punctuator::LBrace),
                TokenKind::Punctuator(Punctuator::RBrace),
                TokenKind::Punctuator(Punctuator::Semi),
                TokenKind::Punctuator(Punctuator::Colon),
                TokenKind::Punctuator(Punctuator::Comma),
                TokenKind::Punctuator(Punctuator::Dot),
                TokenKind::Punctuator(Punctuator::Arrow),
                TokenKind::Punctuator(Punctuator::DotDot),
            ],
        );
    }

    #[test]
    fn all_integer_literal_bases() {
        assert_eq!(
            kinds("0 42 1_000_000 0xDEAD_BEEF 0o755 0b1010_0011"),
            vec![
                TokenKind::Literal(Literal::I53(0)),
                TokenKind::Literal(Literal::I53(42)),
                TokenKind::Literal(Literal::I53(1_000_000)),
                TokenKind::Literal(Literal::I53(0xDEAD_BEEF)),
                TokenKind::Literal(Literal::I53(0o755)),
                TokenKind::Literal(Literal::I53(0b1010_0011)),
            ],
        );
    }

    #[test]
    fn all_float_literal_forms() {
        assert_eq!(
            kinds("3.25 0.5 1_000.0 1.0e10 2.5e-3 1e5"),
            vec![
                TokenKind::Literal(Literal::F64(3.25)),
                TokenKind::Literal(Literal::F64(0.5)),
                TokenKind::Literal(Literal::F64(1_000.0)),
                TokenKind::Literal(Literal::F64(1.0e10)),
                TokenKind::Literal(Literal::F64(2.5e-3)),
                TokenKind::Literal(Literal::F64(1e5)),
            ],
        );
    }

    #[test]
    fn string_literal_value() {
        assert_eq!(
            kinds(r#""StructureBattery""#),
            vec![TokenKind::Literal(Literal::String(
                "StructureBattery".into()
            ))],
        );
    }

    #[test]
    fn hello_world_program() {
        assert_eq!(
            kinds("device light: d0;\n\nfn main() {\n    light.On = 1.0;\n}"),
            vec![
                TokenKind::Keyword(Keyword::Device),
                TokenKind::Identifier("light".into()),
                TokenKind::Punctuator(Punctuator::Colon),
                TokenKind::Identifier("d0".into()),
                TokenKind::Punctuator(Punctuator::Semi),
                TokenKind::Keyword(Keyword::Fn),
                TokenKind::Identifier("main".into()),
                TokenKind::Punctuator(Punctuator::LParen),
                TokenKind::Punctuator(Punctuator::RParen),
                TokenKind::Punctuator(Punctuator::LBrace),
                TokenKind::Identifier("light".into()),
                TokenKind::Punctuator(Punctuator::Dot),
                TokenKind::Identifier("On".into()),
                TokenKind::Operator(Operator::Eq),
                TokenKind::Literal(Literal::F64(1.0)),
                TokenKind::Punctuator(Punctuator::Semi),
                TokenKind::Punctuator(Punctuator::RBrace),
            ],
        );
    }

    #[test]
    fn comments_are_stripped() {
        assert_eq!(
            kinds("let x = 10; // comment\n/* block */ let y = 20;"),
            vec![
                TokenKind::Keyword(Keyword::Let),
                TokenKind::Identifier("x".into()),
                TokenKind::Operator(Operator::Eq),
                TokenKind::Literal(Literal::I53(10)),
                TokenKind::Punctuator(Punctuator::Semi),
                TokenKind::Keyword(Keyword::Let),
                TokenKind::Identifier("y".into()),
                TokenKind::Operator(Operator::Eq),
                TokenKind::Literal(Literal::I53(20)),
                TokenKind::Punctuator(Punctuator::Semi),
            ],
        );
    }

    #[test]
    fn full_program() {
        assert_eq!(
            kinds(
                r#"
const MAX_TEMP: f64 = 350.0;
device sensor: d0;
device heater: d1;
fn clamp_temp(temp: f64) -> f64 {
    if temp > MAX_TEMP { return MAX_TEMP; }
    return temp;
}
fn main() {
    loop {
        let t: f64 = sensor.Temperature;
        heater.Setting = clamp_temp(t);
        yield;
    }
}
"#
            ),
            vec![
                // const MAX_TEMP: f64 = 350.0;
                TokenKind::Keyword(Keyword::Const),
                TokenKind::Identifier("MAX_TEMP".into()),
                TokenKind::Punctuator(Punctuator::Colon),
                TokenKind::Keyword(Keyword::F64),
                TokenKind::Operator(Operator::Eq),
                TokenKind::Literal(Literal::F64(350.0)),
                TokenKind::Punctuator(Punctuator::Semi),
                // device sensor: d0;
                TokenKind::Keyword(Keyword::Device),
                TokenKind::Identifier("sensor".into()),
                TokenKind::Punctuator(Punctuator::Colon),
                TokenKind::Identifier("d0".into()),
                TokenKind::Punctuator(Punctuator::Semi),
                // device heater: d1;
                TokenKind::Keyword(Keyword::Device),
                TokenKind::Identifier("heater".into()),
                TokenKind::Punctuator(Punctuator::Colon),
                TokenKind::Identifier("d1".into()),
                TokenKind::Punctuator(Punctuator::Semi),
                // fn clamp_temp(temp: f64) -> f64 {
                TokenKind::Keyword(Keyword::Fn),
                TokenKind::Identifier("clamp_temp".into()),
                TokenKind::Punctuator(Punctuator::LParen),
                TokenKind::Identifier("temp".into()),
                TokenKind::Punctuator(Punctuator::Colon),
                TokenKind::Keyword(Keyword::F64),
                TokenKind::Punctuator(Punctuator::RParen),
                TokenKind::Punctuator(Punctuator::Arrow),
                TokenKind::Keyword(Keyword::F64),
                TokenKind::Punctuator(Punctuator::LBrace),
                //     if temp > MAX_TEMP { return MAX_TEMP; }
                TokenKind::Keyword(Keyword::If),
                TokenKind::Identifier("temp".into()),
                TokenKind::Operator(Operator::Gt),
                TokenKind::Identifier("MAX_TEMP".into()),
                TokenKind::Punctuator(Punctuator::LBrace),
                TokenKind::Keyword(Keyword::Return),
                TokenKind::Identifier("MAX_TEMP".into()),
                TokenKind::Punctuator(Punctuator::Semi),
                TokenKind::Punctuator(Punctuator::RBrace),
                //     return temp;
                TokenKind::Keyword(Keyword::Return),
                TokenKind::Identifier("temp".into()),
                TokenKind::Punctuator(Punctuator::Semi),
                // }
                TokenKind::Punctuator(Punctuator::RBrace),
                // fn main() {
                TokenKind::Keyword(Keyword::Fn),
                TokenKind::Identifier("main".into()),
                TokenKind::Punctuator(Punctuator::LParen),
                TokenKind::Punctuator(Punctuator::RParen),
                TokenKind::Punctuator(Punctuator::LBrace),
                //     loop {
                TokenKind::Keyword(Keyword::Loop),
                TokenKind::Punctuator(Punctuator::LBrace),
                //         let t: f64 = sensor.Temperature;
                TokenKind::Keyword(Keyword::Let),
                TokenKind::Identifier("t".into()),
                TokenKind::Punctuator(Punctuator::Colon),
                TokenKind::Keyword(Keyword::F64),
                TokenKind::Operator(Operator::Eq),
                TokenKind::Identifier("sensor".into()),
                TokenKind::Punctuator(Punctuator::Dot),
                TokenKind::Identifier("Temperature".into()),
                TokenKind::Punctuator(Punctuator::Semi),
                //         heater.Setting = clamp_temp(t);
                TokenKind::Identifier("heater".into()),
                TokenKind::Punctuator(Punctuator::Dot),
                TokenKind::Identifier("Setting".into()),
                TokenKind::Operator(Operator::Eq),
                TokenKind::Identifier("clamp_temp".into()),
                TokenKind::Punctuator(Punctuator::LParen),
                TokenKind::Identifier("t".into()),
                TokenKind::Punctuator(Punctuator::RParen),
                TokenKind::Punctuator(Punctuator::Semi),
                //         yield;
                TokenKind::Keyword(Keyword::Yield),
                TokenKind::Punctuator(Punctuator::Semi),
                //     }
                TokenKind::Punctuator(Punctuator::RBrace),
                // }
                TokenKind::Punctuator(Punctuator::RBrace),
            ],
        );
    }

    #[test]
    fn empty_source() {
        let k = kinds("");
        assert!(k.is_empty());
    }

    #[test]
    fn whitespace_only() {
        let k = kinds("   \t\n\r\n  ");
        assert!(k.is_empty());
    }

    #[test]
    fn identifiers() {
        assert_eq!(
            kinds("x sensor_1 _unused maxTemp i"),
            vec![
                TokenKind::Identifier("x".into()),
                TokenKind::Identifier("sensor_1".into()),
                TokenKind::Identifier("_unused".into()),
                TokenKind::Identifier("maxTemp".into()),
                TokenKind::Identifier("i".into()),
            ]
        );
    }

    #[test]
    fn contextual_identifiers() {
        // batch_read, batch_write, hash, select, Average etc. are NOT reserved
        assert_eq!(
            kinds("batch_read batch_write hash select Average Sum Minimum Maximum"),
            vec![
                TokenKind::Identifier("batch_read".into()),
                TokenKind::Identifier("batch_write".into()),
                TokenKind::Identifier("hash".into()),
                TokenKind::Identifier("select".into()),
                TokenKind::Identifier("Average".into()),
                TokenKind::Identifier("Sum".into()),
                TokenKind::Identifier("Minimum".into()),
                TokenKind::Identifier("Maximum".into()),
            ]
        );
    }

    #[test]
    fn unterminated_block_comment() {
        let d = diags("/* never closed");
        assert_eq!(d.len(), 1);
        assert!(d[0].message.contains("unterminated block comment"));
    }

    #[test]
    fn unterminated_string() {
        let d = diags("\"hello");
        assert_eq!(d.len(), 1);
        assert!(d[0].message.contains("unterminated string"));
    }

    #[test]
    fn unknown_character() {
        let d = diags("let @ x");
        assert_eq!(d.len(), 1);
        assert!(d[0].message.contains("unexpected character"));
    }

    #[test]
    fn multiple_errors_recovered() {
        let (tokens, d) = tokenize("let @ # x");
        // Should still get let and x despite errors
        let k: Vec<_> = tokens
            .iter()
            .map(|t| &t.kind)
            .filter(|k| **k != TokenKind::Eof)
            .collect();
        assert_eq!(
            k,
            vec![
                &TokenKind::Keyword(Keyword::Let),
                &TokenKind::Identifier("x".into()),
            ]
        );
        assert_eq!(d.len(), 2);
    }

    #[test]
    fn arrow_vs_minus() {
        assert_eq!(kinds("->"), vec![TokenKind::Punctuator(Punctuator::Arrow)]);
        assert_eq!(
            kinds("- >"),
            vec![
                TokenKind::Operator(Operator::Minus),
                TokenKind::Operator(Operator::Gt)
            ]
        );
    }

    #[test]
    fn dot_dot_vs_dot() {
        assert_eq!(kinds(".."), vec![TokenKind::Punctuator(Punctuator::DotDot)]);
        assert_eq!(
            kinds(". ."),
            vec![
                TokenKind::Punctuator(Punctuator::Dot),
                TokenKind::Punctuator(Punctuator::Dot)
            ]
        );
    }

    #[test]
    fn span_accuracy() {
        let (tokens, _) = tokenize("let x = 42;");
        assert_eq!(tokens[0].span, Span::new(0, 3)); // "let"
        assert_eq!(tokens[1].span, Span::new(4, 5)); // "x"
        assert_eq!(tokens[2].span, Span::new(6, 7)); // "="
        assert_eq!(tokens[3].span, Span::new(8, 10)); // "42"
        assert_eq!(tokens[4].span, Span::new(10, 11)); // ";"
    }

    #[test]
    fn eof_at_end() {
        let (tokens, _) = tokenize("x");
        assert_eq!(tokens.last().unwrap().kind, TokenKind::Eof);
    }

    #[test]
    fn hex_missing_digits() {
        let d = diags("0x");
        assert_eq!(d.len(), 1);
        assert!(d[0].message.contains("expected hex digits"));
    }

    #[test]
    fn for_range_syntax() {
        assert_eq!(
            kinds("for i in 0..6"),
            vec![
                TokenKind::Keyword(Keyword::For),
                TokenKind::Identifier("i".into()),
                TokenKind::Keyword(Keyword::In),
                TokenKind::Literal(Literal::I53(0)),
                TokenKind::Punctuator(Punctuator::DotDot),
                TokenKind::Literal(Literal::I53(6)),
            ]
        );
    }

    #[test]
    fn device_field_access() {
        assert_eq!(
            kinds("sensor.Temperature"),
            vec![
                TokenKind::Identifier("sensor".into()),
                TokenKind::Punctuator(Punctuator::Dot),
                TokenKind::Identifier("Temperature".into()),
            ]
        );
    }

    #[test]
    fn fn_return_type() {
        assert_eq!(
            kinds("fn foo() -> f64"),
            vec![
                TokenKind::Keyword(Keyword::Fn),
                TokenKind::Identifier("foo".into()),
                TokenKind::Punctuator(Punctuator::LParen),
                TokenKind::Punctuator(Punctuator::RParen),
                TokenKind::Punctuator(Punctuator::Arrow),
                TokenKind::Keyword(Keyword::F64),
            ]
        );
    }

    #[test]
    fn zero_literal() {
        assert_eq!(kinds("0"), vec![TokenKind::Literal(Literal::I53(0))]);
    }

    #[test]
    fn string_with_spaces() {
        assert_eq!(
            kinds(r#""My Sensor""#),
            vec![TokenKind::Literal(Literal::String("My Sensor".into()))]
        );
    }

    #[test]
    fn slot_access_syntax() {
        // sensor.slot(0).Occupied
        assert_eq!(
            kinds("sensor.slot(0).Occupied"),
            vec![
                TokenKind::Identifier("sensor".into()),
                TokenKind::Punctuator(Punctuator::Dot),
                TokenKind::Identifier("slot".into()),
                TokenKind::Punctuator(Punctuator::LParen),
                TokenKind::Literal(Literal::I53(0)),
                TokenKind::Punctuator(Punctuator::RParen),
                TokenKind::Punctuator(Punctuator::Dot),
                TokenKind::Identifier("Occupied".into()),
            ]
        );
    }

    #[test]
    fn label_token() {
        assert_eq!(
            kinds("'outer: loop"),
            vec![
                TokenKind::Label("outer".into()),
                TokenKind::Punctuator(Punctuator::Colon),
                TokenKind::Keyword(Keyword::Loop),
            ]
        );
    }

    #[test]
    fn label_in_break() {
        assert_eq!(
            kinds("break 'done;"),
            vec![
                TokenKind::Keyword(Keyword::Break),
                TokenKind::Label("done".into()),
                TokenKind::Punctuator(Punctuator::Semi),
            ]
        );
    }

    #[test]
    fn bare_apostrophe_error() {
        let d = diags("'");
        assert_eq!(d.len(), 1);
        assert!(d[0].message.contains("expected identifier after"));
    }

    #[test]
    fn dotdoteq_tokenizes_correctly() {
        assert_eq!(
            kinds("0..=9"),
            vec![
                TokenKind::Literal(Literal::I53(0)),
                TokenKind::Punctuator(Punctuator::DotDotEq),
                TokenKind::Literal(Literal::I53(9)),
            ],
        );
    }

    #[test]
    fn integer_overflow_at_lex_time() {
        let d = diags("99999999999999999999");
        assert_eq!(d.len(), 1);
        assert!(
            d[0].message.contains("invalid integer literal"),
            "{}",
            d[0].message,
        );
    }

    #[test]
    fn float_negative_exponent_with_underscores() {
        assert_eq!(
            kinds("1_0.0e-1_0"),
            vec![TokenKind::Literal(Literal::F64(1_0.0e-1_0))],
        );
    }

    #[test]
    fn nested_block_comment_not_supported() {
        // The lexer closes at the first `*/`, so `/* /* */` is a complete comment.
        let k = kinds("/* /* */ 42");
        assert_eq!(k, vec![TokenKind::Literal(Literal::I53(42))]);
    }

    #[test]
    fn empty_label_produces_diagnostic() {
        let (tokens, diagnostics) = tokenize("' ");
        assert!(!diagnostics.is_empty(), "expected an error for empty label");
        assert!(
            diagnostics[0].message.contains("expected identifier after"),
            "unexpected message: {}",
            diagnostics[0].message
        );
        // Error recovery still emits a Label token with an empty name.
        assert!(
            tokens
                .iter()
                .any(|t| matches!(&t.kind, TokenKind::Label(n) if n.is_empty()))
        );
    }

    #[test]
    fn unterminated_block_comment_produces_diagnostic() {
        let (tokens, diagnostics) = tokenize("/* no closing");
        assert!(
            !diagnostics.is_empty(),
            "expected an error for unterminated block comment"
        );
        assert!(
            diagnostics[0]
                .message
                .contains("unterminated block comment"),
            "unexpected message: {}",
            diagnostics[0].message
        );
        // After the unterminated comment, only the Eof token remains.
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::Eof);
    }

    #[test]
    fn hex_with_no_digits_produces_diagnostic() {
        let (tokens, diagnostics) = tokenize("0x");
        assert!(
            !diagnostics.is_empty(),
            "expected an error for empty hex literal"
        );
        assert!(
            diagnostics[0]
                .message
                .contains("expected hex digits after '0x'"),
            "unexpected message: {}",
            diagnostics[0].message
        );
        // Error recovery emits a zero literal.
        assert!(matches!(
            tokens[0].kind,
            TokenKind::Literal(Literal::I53(0))
        ));
    }

    #[test]
    fn octal_with_no_digits_produces_diagnostic() {
        let (tokens, diagnostics) = tokenize("0o");
        assert!(
            !diagnostics.is_empty(),
            "expected an error for empty octal literal"
        );
        assert!(
            diagnostics[0]
                .message
                .contains("expected octal digits after '0o'"),
            "unexpected message: {}",
            diagnostics[0].message
        );
        assert!(matches!(
            tokens[0].kind,
            TokenKind::Literal(Literal::I53(0))
        ));
    }

    #[test]
    fn binary_with_no_digits_produces_diagnostic() {
        let (tokens, diagnostics) = tokenize("0b");
        assert!(
            !diagnostics.is_empty(),
            "expected an error for empty binary literal"
        );
        assert!(
            diagnostics[0]
                .message
                .contains("expected binary digits after '0b'"),
            "unexpected message: {}",
            diagnostics[0].message
        );
        assert!(matches!(
            tokens[0].kind,
            TokenKind::Literal(Literal::I53(0))
        ));
    }

    #[test]
    fn dot_dot_eq_tokenizes_as_inclusive_range() {
        assert_eq!(
            kinds("..="),
            vec![TokenKind::Punctuator(Punctuator::DotDotEq)],
        );
    }

    #[test]
    fn unterminated_string_produces_diagnostic() {
        let (tokens, diagnostics) = tokenize("\"hello");
        assert!(
            !diagnostics.is_empty(),
            "expected an error for unterminated string"
        );
        assert!(
            diagnostics[0]
                .message
                .contains("unterminated string literal"),
            "unexpected message: {}",
            diagnostics[0].message
        );
        // Error recovery returns the partial content as a string token.
        assert!(matches!(&tokens[0].kind, TokenKind::Literal(Literal::String(s)) if s == "hello"));
    }

    #[test]
    fn integer_exceeding_i53_range_produces_diagnostic() {
        // 2^53 + 1 = 9007199254740993 is one past the maximum i53 value.
        let (tokens, diagnostics) = tokenize("9007199254740993");
        assert!(
            !diagnostics.is_empty(),
            "expected an error for out-of-range integer"
        );
        assert!(
            diagnostics[0].message.contains("out of i53 range"),
            "unexpected message: {}",
            diagnostics[0].message
        );
        // A token is still emitted with the parsed value for error recovery.
        assert!(matches!(
            tokens[0].kind,
            TokenKind::Literal(Literal::I53(_))
        ));
    }

    #[test]
    fn unexpected_character_produces_diagnostic() {
        let (_, diagnostics) = tokenize("@");
        assert!(
            !diagnostics.is_empty(),
            "expected an error for unexpected character"
        );
        assert!(
            diagnostics[0].message.contains("unexpected character '@'"),
            "unexpected message: {}",
            diagnostics[0].message
        );
    }
}
