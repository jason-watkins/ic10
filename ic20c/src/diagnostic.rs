//! Compiler diagnostics with source-location tracking and terminal rendering.

use std::fmt;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";

/// Byte-offset span into the original source text.
///
/// Every token, AST node, and diagnostic carries a `Span` so that error
/// messages can point at the exact source location.  `Span` is `Copy` and
/// two words wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// Byte offset of the first character (inclusive).
    pub start: usize,
    /// Byte offset one past the last character (exclusive).
    pub end: usize,
}

impl Span {
    /// Creates a span from byte offset `start` (inclusive) to `end` (exclusive).
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// Derive 1-based line and column numbers from a source string.
    pub fn line_col(&self, source: &str) -> (u32, u32) {
        let mut line = 1u32;
        let mut col = 1u32;
        for (i, ch) in source.char_indices() {
            if i >= self.start {
                break;
            }
            if ch == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        (line, col)
    }

    /// Return the source text covered by this span.
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.start..self.end]
    }
}

/// Extract the text of a 1-based line number from source, without the newline.
fn source_line(source: &str, line_num: u32) -> &str {
    let mut current = 1u32;
    let mut line_start = 0usize;
    if line_num > 1 {
        for (i, ch) in source.char_indices() {
            if ch == '\n' {
                current += 1;
                if current == line_num {
                    line_start = i + 1;
                    break;
                }
            }
        }
    }
    let line_end = source[line_start..]
        .find('\n')
        .map(|e| line_start + e)
        .unwrap_or(source.len());
    source[line_start..line_end].trim_end_matches('\r')
}

/// Number of display characters covered by a span (minimum 1 for EOF/zero-width).
fn caret_len(source: &str, span: Span) -> usize {
    let start = span.start;
    let end = span.end.min(source.len());
    source[start..end].chars().count().max(1)
}

/// Severity level for a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// A hard error that prevents compilation from succeeding.
    Error,
    /// A non-fatal warning; the program is still compiled.
    Warning,
}

/// A compiler diagnostic tied to a source location.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// Error or warning.
    pub severity: Severity,
    /// Where in the source this diagnostic points.
    pub span: Span,
    /// Human-readable description of the problem.
    pub message: String,
}

impl Diagnostic {
    /// Creates an error diagnostic at `span` with the given `message`.
    pub fn error(span: Span, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            span,
            message: message.into(),
        }
    }

    /// Creates a warning diagnostic at `span` with the given `message`.
    pub fn warning(span: Span, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            span,
            message: message.into(),
        }
    }

    /// Return a value that implements [`Display`] with full Rust-style source context.
    ///
    /// Example output:
    /// ```text
    /// error: unexpected character '@'
    ///  --> hello.ic20:5:9
    ///   |
    /// 5 |     let @ x = 1;
    ///   |         ^ unexpected character '@'
    /// ```
    pub fn display<'a>(&'a self, source: &'a str, filename: &'a str) -> DiagnosticDisplay<'a> {
        DiagnosticDisplay {
            diagnostic: self,
            source,
            filename,
        }
    }
}

/// Wrapper that implements [`fmt::Display`] for a [`Diagnostic`] together
/// with the source text and filename needed to render source context.
pub struct DiagnosticDisplay<'a> {
    pub diagnostic: &'a Diagnostic,
    pub source: &'a str,
    pub filename: &'a str,
}

impl fmt::Display for DiagnosticDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let d = self.diagnostic;
        let (line_num, col) = d.span.line_col(self.source);

        let (sev_color, sev_label) = match d.severity {
            Severity::Error => (RED, "error"),
            Severity::Warning => (YELLOW, "warning"),
        };

        // Width of the line-number gutter, e.g. 1 for lines 1–9, 2 for 10–99.
        let margin = line_num.to_string().len();
        let pad = " ".repeat(margin);

        writeln!(
            f,
            "{BOLD}{sev_color}{sev_label}{RESET}{BOLD}: {msg}{RESET}",
            msg = d.message
        )?;

        writeln!(
            f,
            "{CYAN}{pad} -->{RESET} {}:{}:{}",
            self.filename, line_num, col
        )?;

        writeln!(f, "{CYAN}{pad} |{RESET}")?;

        let line_text = source_line(self.source, line_num);
        writeln!(f, "{CYAN}{line_num} |{RESET} {line_text}")?;

        let caret_offset = col as usize - 1;
        let carets = "^".repeat(caret_len(self.source, d.span));
        write!(
            f,
            "{CYAN}{pad} |{RESET} {space}{BOLD}{sev_color}{carets}{RESET}",
            space = " ".repeat(caret_offset),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(source: &str, span: Span, severity: Severity, msg: &str) -> String {
        let d = Diagnostic {
            severity,
            span,
            message: msg.into(),
        };
        // Strip ANSI codes for easy assertion
        let raw = format!("{}", d.display(source, "test.ic20"));
        strip_ansi(&raw)
    }

    fn strip_ansi(s: &str) -> String {
        // Remove ESC [ ... m sequences
        let mut out = String::new();
        let mut chars = s.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\x1b' {
                // consume until 'm'
                for c in chars.by_ref() {
                    if c == 'm' {
                        break;
                    }
                }
            } else {
                out.push(ch);
            }
        }
        out
    }

    #[test]
    fn error_format() {
        let source = "let @ x = 1;";
        //                 ^ offset 4
        let out = render(
            source,
            Span::new(4, 5),
            Severity::Error,
            "unexpected character '@'",
        );
        assert!(
            out.contains("error: unexpected character '@'"),
            "header: {out}"
        );
        assert!(out.contains("--> test.ic20:1:5"), "location: {out}");
        assert!(out.contains("let @ x = 1;"), "source line: {out}");
        assert!(out.contains("    ^"), "caret: {out}");
    }

    #[test]
    fn warning_format() {
        let source = "let x = x as i53;";
        let out = render(
            source,
            Span::new(8, 17),
            Severity::Warning,
            "identity cast has no effect",
        );
        assert!(
            out.contains("warning: identity cast has no effect"),
            "{out}"
        );
        assert!(out.contains("--> test.ic20:1:9"), "{out}");
        assert!(out.contains("^^^^^^^^^"), "{out}");
    }

    #[test]
    fn multiline_source_correct_line() {
        let source = "let a = 1;\nlet b = @;\nlet c = 3;";
        //                                   ^ line 2, col 9, offset 19
        let out = render(
            source,
            Span::new(19, 20),
            Severity::Error,
            "unexpected character",
        );
        assert!(out.contains("--> test.ic20:2:9"), "{out}");
        assert!(out.contains("let b = @;"), "{out}");
        // line number is shown in gutter
        assert!(out.contains("2 |"), "{out}");
    }

    #[test]
    fn multi_char_span_produces_multiple_carets() {
        let source = "0xZZZZ";
        let out = render(source, Span::new(0, 6), Severity::Error, "invalid literal");
        let caret_line = out.lines().last().unwrap();
        // caret line is "  | ^^^^^^" — strip the gutter prefix
        assert!(caret_line.ends_with("^^^^^^"), "got: {caret_line:?}");
    }

    #[test]
    fn zero_width_span_produces_one_caret() {
        let source = "x";
        let out = render(source, Span::new(1, 1), Severity::Error, "eof");
        let caret_line = out.lines().last().unwrap();
        assert!(caret_line.ends_with("^"), "got: {caret_line:?}");
    }

    #[test]
    fn display_impl_works_with_format_macro() {
        let source = "bad!";
        let d = Diagnostic::error(Span::new(3, 4), "unexpected '!'");
        let s = format!("{}", d.display(source, "f.ic20"));
        let plain = strip_ansi(&s);
        assert!(plain.contains("error"), "output should contain severity");
        assert!(plain.contains("unexpected '!'"), "output should contain message");
        assert!(plain.contains("f.ic20"), "output should contain filename");
    }

    #[test]
    fn tab_character_column_alignment() {
        let source = "\tlet x = @;";
        let out = render(source, Span::new(9, 10), Severity::Error, "bad char");
        assert!(out.contains("bad char"), "{out}");
        assert!(out.contains("let x = @;"), "source line should appear: {out}");
    }

    #[test]
    fn very_long_source_line_no_panic() {
        let long_ident: String = "x".repeat(1500);
        let source = format!("let {} = 1;", long_ident);
        let out = render(&source, Span::new(0, 3), Severity::Warning, "unused");
        assert!(out.contains("unused"), "{out}");
    }

    #[test]
    fn span_at_start_of_file() {
        let source = "let x = 1;";
        let out = render(source, Span::new(0, 3), Severity::Error, "unexpected let");
        assert!(out.contains("--> test.ic20:1:1"), "location: {out}");
        assert!(out.contains("^^^"), "caret: {out}");
    }
}
