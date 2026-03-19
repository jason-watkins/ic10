#[derive(Debug, Clone)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub text: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum LineKind {
    Empty,
    Label { name: Token },
    Instruction { opcode: Token, operands: Vec<Token> },
}

#[derive(Debug, Clone)]
pub struct Line {
    pub line_number: usize,
    pub offset: usize,
    pub kind: LineKind,
}

pub fn parse(source: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut offset = 0;

    for (line_number, raw_line) in source.lines().enumerate() {
        let line_start = offset;

        let code = match raw_line.find('#') {
            Some(pos) => &raw_line[..pos],
            None => raw_line,
        };

        let trimmed = code.trim();

        let kind = if trimmed.is_empty() {
            LineKind::Empty
        } else if let Some(label) = trimmed.strip_suffix(':') {
            let label = label.trim();
            if !label.is_empty() && !label.contains(' ') {
                let label_start = line_start + raw_line.find(label).unwrap_or(0);
                LineKind::Label {
                    name: Token {
                        text: label.to_string(),
                        span: Span {
                            start: label_start,
                            end: label_start + label.len(),
                        },
                    },
                }
            } else {
                LineKind::Empty
            }
        } else {
            let tokens = tokenize(trimmed, line_start + raw_line.find(trimmed).unwrap_or(0));
            if let Some((first, rest)) = tokens.split_first() {
                LineKind::Instruction {
                    opcode: first.clone(),
                    operands: rest.to_vec(),
                }
            } else {
                LineKind::Empty
            }
        };

        lines.push(Line {
            line_number,
            offset: line_start,
            kind,
        });

        offset += raw_line.len() + 1; // +1 for newline
    }

    lines
}

fn tokenize(text: &str, base_offset: usize) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut pos = 0;
    let bytes = text.as_bytes();

    while pos < bytes.len() {
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos >= bytes.len() {
            break;
        }

        // Handle HASH("...") as a single token
        if bytes[pos..].starts_with(b"HASH(") {
            let start = pos;
            if let Some(close_paren) = text[pos..].find(')') {
                pos += close_paren + 1;
                tokens.push(Token {
                    text: text[start..pos].to_string(),
                    span: Span {
                        start: base_offset + start,
                        end: base_offset + pos,
                    },
                });
                continue;
            }
        }

        // Handle LogicType.Xxx as a single token
        if bytes[pos..].starts_with(b"LogicType.") {
            let start = pos;
            pos += 10; // skip "LogicType."
            while pos < bytes.len() && (bytes[pos].is_ascii_alphanumeric() || bytes[pos] == b'_') {
                pos += 1;
            }
            tokens.push(Token {
                text: text[start..pos].to_string(),
                span: Span {
                    start: base_offset + start,
                    end: base_offset + pos,
                },
            });
            continue;
        }

        let start = pos;

        // Handle device:connection syntax (e.g., d0:0) as a single token
        while pos < bytes.len() && !bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }

        tokens.push(Token {
            text: text[start..pos].to_string(),
            span: Span {
                start: base_offset + start,
                end: base_offset + pos,
            },
        });
    }

    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_line() {
        let lines = parse("");
        assert_eq!(lines.len(), 0);
    }

    #[test]
    fn parse_comment_only() {
        let lines = parse("# this is a comment");
        assert_eq!(lines.len(), 1);
        assert!(matches!(lines[0].kind, LineKind::Empty));
    }

    #[test]
    fn parse_label() {
        let lines = parse("start:");
        assert_eq!(lines.len(), 1);
        match &lines[0].kind {
            LineKind::Label { name } => assert_eq!(name.text, "start"),
            other => panic!("expected label, got {other:?}"),
        }
    }

    #[test]
    fn parse_instruction() {
        let lines = parse("add r0 r1 r2");
        assert_eq!(lines.len(), 1);
        match &lines[0].kind {
            LineKind::Instruction { opcode, operands } => {
                assert_eq!(opcode.text, "add");
                assert_eq!(operands.len(), 3);
                assert_eq!(operands[0].text, "r0");
                assert_eq!(operands[1].text, "r1");
                assert_eq!(operands[2].text, "r2");
            }
            other => panic!("expected instruction, got {other:?}"),
        }
    }

    #[test]
    fn parse_instruction_with_comment() {
        let lines = parse("move r0 42 # set temp");
        match &lines[0].kind {
            LineKind::Instruction { opcode, operands } => {
                assert_eq!(opcode.text, "move");
                assert_eq!(operands.len(), 2);
            }
            other => panic!("expected instruction, got {other:?}"),
        }
    }

    #[test]
    fn parse_hash_macro() {
        let lines = parse("lb r0 HASH(\"StructureGasSensor\") Temperature Average");
        match &lines[0].kind {
            LineKind::Instruction { opcode, operands } => {
                assert_eq!(opcode.text, "lb");
                assert_eq!(operands.len(), 4);
                assert_eq!(operands[0].text, "r0");
                assert_eq!(operands[1].text, "HASH(\"StructureGasSensor\")");
                assert_eq!(operands[2].text, "Temperature");
                assert_eq!(operands[3].text, "Average");
            }
            other => panic!("expected instruction, got {other:?}"),
        }
    }

    #[test]
    fn parse_device_connection() {
        let lines = parse("l r0 d0:0 Channel3");
        match &lines[0].kind {
            LineKind::Instruction { opcode, operands } => {
                assert_eq!(opcode.text, "l");
                assert_eq!(operands[0].text, "r0");
                assert_eq!(operands[1].text, "d0:0");
                assert_eq!(operands[2].text, "Channel3");
            }
            other => panic!("expected instruction, got {other:?}"),
        }
    }
}
