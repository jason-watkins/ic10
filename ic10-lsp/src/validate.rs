use std::collections::HashMap;

use crate::instructions::{InstructionSignature, OperandKind, INSTRUCTION_MAP};
use crate::parser::{Line, LineKind, Span, Token};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub span: Span,
    pub message: String,
}

const MAX_LINES: usize = 128;

pub fn validate(lines: &[Line]) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    let mut labels: HashMap<&str, &Token> = HashMap::new();
    let mut aliases: HashMap<&str, &Token> = HashMap::new();
    let mut defines: HashMap<&str, &Token> = HashMap::new();
    let mut code_line_count = 0usize;

    // First pass: collect labels, aliases, defines and check for duplicates
    for line in lines {
        match &line.kind {
            LineKind::Empty => {}
            LineKind::Label { name } => {
                code_line_count += 1;
                if let Some(previous) = labels.get(name.text.as_str()) {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        span: name.span.clone(),
                        message: format!(
                            "duplicate label '{}' (first defined at line {})",
                            name.text,
                            line_of_span(lines, &previous.span) + 1,
                        ),
                    });
                } else {
                    labels.insert(&name.text, name);
                }
            }
            LineKind::Instruction { opcode, operands } => {
                code_line_count += 1;
                if opcode.text == "alias" {
                    if let Some(name_token) = operands.first() {
                        if let Some(previous) = aliases.get(name_token.text.as_str()) {
                            diagnostics.push(Diagnostic {
                                severity: Severity::Warning,
                                span: name_token.span.clone(),
                                message: format!(
                                    "alias '{}' redefined (first defined at line {})",
                                    name_token.text,
                                    line_of_span(lines, &previous.span) + 1,
                                ),
                            });
                        }
                        aliases.insert(&name_token.text, name_token);
                    }
                } else if opcode.text == "define" {
                    if let Some(name_token) = operands.first() {
                        if let Some(previous) = defines.get(name_token.text.as_str()) {
                            diagnostics.push(Diagnostic {
                                severity: Severity::Error,
                                span: name_token.span.clone(),
                                message: format!(
                                    "duplicate define '{}' (first defined at line {})",
                                    name_token.text,
                                    line_of_span(lines, &previous.span) + 1,
                                ),
                            });
                        } else {
                            defines.insert(&name_token.text, name_token);
                        }
                    }
                }
            }
        }
    }

    if code_line_count > MAX_LINES {
        if let Some(line) = lines.last() {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                span: Span {
                    start: line.offset,
                    end: line.offset + 1,
                },
                message: format!(
                    "program has {code_line_count} lines, exceeding the {MAX_LINES} line limit"
                ),
            });
        }
    }

    // Second pass: validate instructions
    for line in lines {
        if let LineKind::Instruction { opcode, operands } = &line.kind {
            if opcode.text == "alias" || opcode.text == "define" {
                validate_directive(opcode, operands, &aliases, &defines, &mut diagnostics);
                continue;
            }

            let Some(signature) = INSTRUCTION_MAP.get(opcode.text.as_str()) else {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    span: opcode.span.clone(),
                    message: format!("unknown instruction '{}'", opcode.text),
                });
                continue;
            };

            validate_operand_count(opcode, operands, signature, &mut diagnostics);

            for (i, operand) in operands.iter().enumerate() {
                if let Some(&expected_kind) = signature.operands.get(i) {
                    validate_operand(
                        operand,
                        expected_kind,
                        &aliases,
                        &defines,
                        &labels,
                        &mut diagnostics,
                    );
                }
            }
        }
    }

    diagnostics
}

fn line_of_span(lines: &[Line], span: &Span) -> usize {
    for line in lines {
        if span.start >= line.offset {
            if let Some(next) = lines.get(line.line_number + 1) {
                if span.start < next.offset {
                    return line.line_number;
                }
            } else {
                return line.line_number;
            }
        }
    }
    0
}

fn validate_directive(
    opcode: &Token,
    operands: &[Token],
    aliases: &HashMap<&str, &Token>,
    defines: &HashMap<&str, &Token>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if opcode.text == "alias" {
        if operands.len() != 2 {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                span: opcode.span.clone(),
                message: format!(
                    "'alias' expects 2 operands (name target), got {}",
                    operands.len()
                ),
            });
            return;
        }
        let name = &operands[0];
        let target = &operands[1];

        if !is_valid_identifier(&name.text) {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                span: name.span.clone(),
                message: format!("invalid alias name '{}'", name.text),
            });
        }
        if !is_register(&target.text) && !is_device(&target.text) {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                span: target.span.clone(),
                message: format!(
                    "alias target must be a register or device, got '{}'",
                    target.text,
                ),
            });
        }
    } else if opcode.text == "define" {
        if operands.len() != 2 {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                span: opcode.span.clone(),
                message: format!(
                    "'define' expects 2 operands (name value), got {}",
                    operands.len()
                ),
            });
            return;
        }
        let name = &operands[0];
        if !is_valid_identifier(&name.text) {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                span: name.span.clone(),
                message: format!("invalid define name '{}'", name.text),
            });
        }
        let _ = (aliases, defines);
    }
}

fn validate_operand_count(
    opcode: &Token,
    operands: &[Token],
    signature: &InstructionSignature,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let expected = signature.operands.len();
    let actual = operands.len();
    if actual != expected {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            span: opcode.span.clone(),
            message: format!(
                "'{}' expects {} operand{}, got {}",
                opcode.text,
                expected,
                if expected == 1 { "" } else { "s" },
                actual,
            ),
        });
    }
}

fn validate_operand(
    token: &Token,
    expected: OperandKind,
    aliases: &HashMap<&str, &Token>,
    defines: &HashMap<&str, &Token>,
    labels: &HashMap<&str, &Token>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match expected {
        OperandKind::Register => {
            if !is_register(&token.text)
                && !is_indirect_register(&token.text)
                && !is_register_alias(&token.text, aliases)
            {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    span: token.span.clone(),
                    message: format!("expected register, got '{}'", token.text),
                });
            }
        }
        OperandKind::RegisterOrNumber => {
            if !is_register(&token.text)
                && !is_indirect_register(&token.text)
                && !is_number(&token.text)
                && !is_named_constant(&token.text)
                && !is_hash_macro(&token.text)
                && !is_logic_type_dot(&token.text)
                && !is_known_name(&token.text, aliases, defines, labels)
            {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    span: token.span.clone(),
                    message: format!("expected register or number, got '{}'", token.text),
                });
            }
        }
        OperandKind::Device => {
            if !is_device(&token.text) && !is_device_alias(&token.text, aliases) {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    span: token.span.clone(),
                    message: format!("expected device (d0-d5, db), got '{}'", token.text),
                });
            }
        }
        OperandKind::DeviceOrIndirectDevice => {
            if !is_device(&token.text)
                && !is_device_connection(&token.text)
                && !is_indirect_device(&token.text)
                && !is_device_alias(&token.text, aliases)
            {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    span: token.span.clone(),
                    message: format!(
                        "expected device (d0-d5, db, dr0-dr15), got '{}'",
                        token.text
                    ),
                });
            }
        }
        OperandKind::LogicType
        | OperandKind::LogicSlotType
        | OperandKind::BatchMode
        | OperandKind::ReagentMode => {}
        OperandKind::Target => {
            if !is_register(&token.text)
                && !is_indirect_register(&token.text)
                && !is_number(&token.text)
                && !is_known_name(&token.text, aliases, defines, labels)
            {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    span: token.span.clone(),
                    message: format!("undefined label or invalid target '{}'", token.text),
                });
            }
        }
        OperandKind::AliasName
        | OperandKind::AliasTarget
        | OperandKind::DefineName
        | OperandKind::DefineValue => {
            // Handled by validate_directive
        }
    }
}

fn is_register(text: &str) -> bool {
    if text == "ra" || text == "sp" {
        return true;
    }
    if let Some(rest) = text.strip_prefix('r') {
        if let Ok(n) = rest.parse::<u8>() {
            return n <= 15;
        }
    }
    false
}

fn is_indirect_register(text: &str) -> bool {
    if text.len() < 3 {
        return false;
    }
    let stripped = text.trim_start_matches('r');
    if stripped.len() >= text.len() {
        return false;
    }
    // Must start with at least two 'r's
    let prefix_count = text.len() - stripped.len();
    if prefix_count < 2 {
        return false;
    }
    if let Ok(n) = stripped.parse::<u8>() {
        return n <= 15;
    }
    false
}

fn is_device(text: &str) -> bool {
    matches!(text, "d0" | "d1" | "d2" | "d3" | "d4" | "d5" | "db")
}

fn is_device_connection(text: &str) -> bool {
    if let Some((device, connection)) = text.split_once(':') {
        if !is_device(device) {
            return false;
        }
        if let Ok(n) = connection.parse::<u8>() {
            return n <= 6;
        }
    }
    false
}

fn is_indirect_device(text: &str) -> bool {
    if let Some(rest) = text.strip_prefix("dr") {
        if let Ok(n) = rest.parse::<u8>() {
            return n <= 15;
        }
    }
    false
}

fn is_number(text: &str) -> bool {
    if text.starts_with('$') {
        return text.len() > 1 && text[1..].chars().all(|c| c.is_ascii_hexdigit() || c == '_');
    }
    if text.starts_with('%') {
        return text.len() > 1 && text[1..].chars().all(|c| c == '0' || c == '1' || c == '_');
    }
    // Allow optional leading '-'
    let s = text.strip_prefix('-').unwrap_or(text);
    if s.is_empty() {
        return false;
    }
    let mut has_dot = false;
    let mut has_exp = false;
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() || c == '_' {
            chars.next();
        } else if c == '.' && !has_dot && !has_exp {
            has_dot = true;
            chars.next();
        } else if (c == 'e' || c == 'E') && !has_exp {
            has_exp = true;
            chars.next();
            if let Some(&sign) = chars.peek() {
                if sign == '+' || sign == '-' {
                    chars.next();
                }
            }
        } else {
            return false;
        }
    }
    true
}

fn is_named_constant(text: &str) -> bool {
    matches!(text, "pinf" | "ninf" | "nan")
}

fn is_hash_macro(text: &str) -> bool {
    text.starts_with("HASH(") && text.ends_with(')')
}

fn is_logic_type_dot(text: &str) -> bool {
    text.starts_with("LogicType.")
}

fn is_valid_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_register_alias(text: &str, aliases: &HashMap<&str, &Token>) -> bool {
    if let Some(alias_token) = aliases.get(text) {
        // Walk the alias chain to check if it resolves to a register
        let _ = alias_token;
        return aliases.contains_key(text);
    }
    false
}

fn is_device_alias(text: &str, aliases: &HashMap<&str, &Token>) -> bool {
    aliases.contains_key(text)
}

fn is_known_name(
    text: &str,
    aliases: &HashMap<&str, &Token>,
    defines: &HashMap<&str, &Token>,
    labels: &HashMap<&str, &Token>,
) -> bool {
    aliases.contains_key(text) || defines.contains_key(text) || labels.contains_key(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    fn validate_source(source: &str) -> Vec<Diagnostic> {
        let lines = parser::parse(source);
        validate(&lines)
    }

    fn errors(diagnostics: &[Diagnostic]) -> Vec<&str> {
        diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| d.message.as_str())
            .collect()
    }

    fn warnings(diagnostics: &[Diagnostic]) -> Vec<&str> {
        diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .map(|d| d.message.as_str())
            .collect()
    }

    #[test]
    fn valid_program() {
        let diagnostics = validate_source("move r0 42\nadd r1 r0 3\nyield\nj 0");
        assert!(errors(&diagnostics).is_empty());
    }

    #[test]
    fn unknown_instruction() {
        let diagnostics = validate_source("nop r0");
        let errs = errors(&diagnostics);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("unknown instruction 'nop'"));
    }

    #[test]
    fn wrong_operand_count() {
        let diagnostics = validate_source("add r0 r1");
        let errs = errors(&diagnostics);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("expects 3 operands"));
    }

    #[test]
    fn invalid_register() {
        let diagnostics = validate_source("move r16 42");
        let errs = errors(&diagnostics);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("expected register"));
    }

    #[test]
    fn valid_device_load() {
        let diagnostics = validate_source("l r0 d0 Temperature");
        assert!(errors(&diagnostics).is_empty());
        assert!(warnings(&diagnostics).is_empty());
    }

    #[test]
    fn valid_alias() {
        let diagnostics = validate_source("alias sensor d0\nl r0 sensor Temperature");
        assert!(errors(&diagnostics).is_empty());
    }

    #[test]
    fn valid_define() {
        let diagnostics = validate_source("define threshold 42\nmove r0 threshold");
        assert!(errors(&diagnostics).is_empty());
    }

    #[test]
    fn duplicate_label() {
        let diagnostics = validate_source("start:\nmove r0 0\nstart:");
        let errs = errors(&diagnostics);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("duplicate label 'start'"));
    }

    #[test]
    fn valid_labels_and_jumps() {
        let diagnostics = validate_source("start:\nmove r0 0\nj start");
        assert!(errors(&diagnostics).is_empty());
    }

    #[test]
    fn undefined_label_in_jump() {
        let diagnostics = validate_source("j missing");
        let errs = errors(&diagnostics);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("undefined label or invalid target 'missing'"));
    }

    #[test]
    fn valid_indirect_register() {
        let diagnostics = validate_source("move rr0 42");
        assert!(errors(&diagnostics).is_empty());
    }

    #[test]
    fn valid_indirect_device() {
        let diagnostics = validate_source("l r0 dr0 Temperature");
        assert!(errors(&diagnostics).is_empty());
    }

    #[test]
    fn valid_batch_operation() {
        let diagnostics = validate_source("lb r0 123 Temperature Average");
        assert!(errors(&diagnostics).is_empty());
    }

    #[test]
    fn line_limit() {
        let mut source = String::new();
        for _ in 0..129 {
            source.push_str("yield\n");
        }
        let diagnostics = validate_source(&source);
        let errs = errors(&diagnostics);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("129 lines"));
    }

    #[test]
    fn valid_hash_macro() {
        let diagnostics = validate_source("lb r0 HASH(\"Sensor\") Temperature Average");
        assert!(errors(&diagnostics).is_empty());
    }

    #[test]
    fn valid_named_constants() {
        let diagnostics = validate_source("move r0 pinf\nmove r1 ninf\nmove r2 nan");
        assert!(errors(&diagnostics).is_empty());
    }

    #[test]
    fn valid_hex_literal() {
        let diagnostics = validate_source("move r0 $DEADBEEF");
        assert!(errors(&diagnostics).is_empty());
    }

    #[test]
    fn valid_binary_literal() {
        let diagnostics = validate_source("move r0 %01101001");
        assert!(errors(&diagnostics).is_empty());
    }

    #[test]
    fn valid_special_registers() {
        let diagnostics = validate_source("move ra 0\nmove sp 0\npush ra\npop sp");
        assert!(errors(&diagnostics).is_empty());
    }

    #[test]
    fn valid_device_connection() {
        let diagnostics = validate_source("l r0 d0:0 Channel3");
        assert!(errors(&diagnostics).is_empty());
    }

    #[test]
    fn logic_type_as_register() {
        let diagnostics = validate_source("l r0 d0 r1");
        assert!(errors(&diagnostics).is_empty());
        assert!(warnings(&diagnostics).is_empty());
    }
}
