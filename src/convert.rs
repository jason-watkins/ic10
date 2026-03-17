use tower_lsp::lsp_types::*;

use ic20c::diagnostic::{Diagnostic as CompilerDiagnostic, Severity as CompilerSeverity, Span};
use ic20c::ir::{Intrinsic, Type};

pub fn span_to_range(source: &str, span: Span) -> Range {
    let start = offset_to_position(source, span.start);
    let end = offset_to_position(source, span.end);
    Range { start, end }
}

pub fn offset_to_position(source: &str, offset: usize) -> Position {
    let offset = offset.min(source.len());
    let mut line = 0u32;
    let mut col = 0u32;
    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    Position::new(line, col)
}

pub fn position_to_offset(source: &str, position: Position) -> usize {
    let mut line = 0u32;
    let mut col = 0u32;
    for (i, ch) in source.char_indices() {
        if line == position.line && col == position.character {
            return i;
        }
        if ch == '\n' {
            if line == position.line {
                return i;
            }
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    source.len()
}

pub fn compiler_to_lsp_diagnostics(
    diagnostics: &[CompilerDiagnostic],
    source: &str,
) -> Vec<Diagnostic> {
    diagnostics
        .iter()
        .map(|d| {
            let severity = match d.severity {
                CompilerSeverity::Error => DiagnosticSeverity::ERROR,
                CompilerSeverity::Warning => DiagnosticSeverity::WARNING,
            };
            Diagnostic {
                range: span_to_range(source, d.span),
                severity: Some(severity),
                source: Some("ic20".to_string()),
                message: d.message.clone(),
                ..Diagnostic::default()
            }
        })
        .collect()
}

pub fn type_name(ty: &Type) -> &'static str {
    match ty {
        Type::Bool => "bool",
        Type::I53 => "i53",
        Type::F64 => "f64",
        Type::Unit => "()",
    }
}

pub fn intrinsic_signature(intrinsic: &Intrinsic) -> &'static str {
    match intrinsic {
        Intrinsic::Abs => "fn abs(f64) -> f64",
        Intrinsic::Ceil => "fn ceil(f64) -> f64",
        Intrinsic::Floor => "fn floor(f64) -> f64",
        Intrinsic::Round => "fn round(f64) -> f64",
        Intrinsic::Trunc => "fn trunc(f64) -> f64",
        Intrinsic::Sqrt => "fn sqrt(f64) -> f64",
        Intrinsic::Exp => "fn exp(f64) -> f64",
        Intrinsic::Log => "fn log(f64) -> f64",
        Intrinsic::Sin => "fn sin(f64) -> f64",
        Intrinsic::Cos => "fn cos(f64) -> f64",
        Intrinsic::Tan => "fn tan(f64) -> f64",
        Intrinsic::Asin => "fn asin(f64) -> f64",
        Intrinsic::Acos => "fn acos(f64) -> f64",
        Intrinsic::Atan => "fn atan(f64) -> f64",
        Intrinsic::Atan2 => "fn atan2(y: f64, x: f64) -> f64",
        Intrinsic::Pow => "fn pow(base: f64, exp: f64) -> f64",
        Intrinsic::Min => "fn min(f64, f64) -> f64",
        Intrinsic::Max => "fn max(f64, f64) -> f64",
        Intrinsic::Lerp => "fn lerp(a: f64, b: f64, t: f64) -> f64",
        Intrinsic::Clamp => "fn clamp(x: f64, lo: f64, hi: f64) -> f64",
        Intrinsic::Rand => "fn rand() -> f64",
        Intrinsic::IsNan => "fn is_nan(f64) -> bool",
    }
}

pub fn contains(span: Span, offset: usize) -> bool {
    offset >= span.start && offset < span.end
}
