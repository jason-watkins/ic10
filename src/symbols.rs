use tower_lsp::lsp_types::*;

use ic20c::ir::ast::{Item, Program as AstProgram};

use crate::convert::{span_to_range, type_name};

pub fn document_symbols_from_ast(ast: &AstProgram, source: &str) -> Vec<DocumentSymbol> {
    let mut symbols = Vec::new();

    for item in &ast.items {
        match item {
            Item::Const(c) => {
                #[allow(deprecated)]
                symbols.push(DocumentSymbol {
                    name: c.name.clone(),
                    detail: Some(format!("const: {}", type_name(&c.ty))),
                    kind: SymbolKind::CONSTANT,
                    tags: None,
                    deprecated: None,
                    range: span_to_range(source, c.span),
                    selection_range: span_to_range(source, c.span),
                    children: None,
                });
            }
            Item::Device(d) => {
                #[allow(deprecated)]
                symbols.push(DocumentSymbol {
                    name: d.name.clone(),
                    detail: Some(format!("device: {:?}", d.pin)),
                    kind: SymbolKind::VARIABLE,
                    tags: None,
                    deprecated: None,
                    range: span_to_range(source, d.span),
                    selection_range: span_to_range(source, d.span),
                    children: None,
                });
            }
            Item::Static(s) => {
                #[allow(deprecated)]
                symbols.push(DocumentSymbol {
                    name: s.name.clone(),
                    detail: Some(format!(
                        "static{}: {}",
                        if s.mutable { " mut" } else { "" },
                        type_name(&s.ty)
                    )),
                    kind: SymbolKind::VARIABLE,
                    tags: None,
                    deprecated: None,
                    range: span_to_range(source, s.span),
                    selection_range: span_to_range(source, s.span),
                    children: None,
                });
            }
            Item::Fn(f) => {
                let params: Vec<String> = f
                    .params
                    .iter()
                    .map(|p| format!("{}: {}", p.name, type_name(&p.ty)))
                    .collect();
                let return_str = f
                    .return_type
                    .as_ref()
                    .map(|t| format!(" -> {}", type_name(t)))
                    .unwrap_or_default();
                let detail = format!("fn({}){}", params.join(", "), return_str);

                let children: Vec<DocumentSymbol> = f
                    .params
                    .iter()
                    .map(|p| {
                        #[allow(deprecated)]
                        DocumentSymbol {
                            name: p.name.clone(),
                            detail: Some(type_name(&p.ty).to_string()),
                            kind: SymbolKind::VARIABLE,
                            tags: None,
                            deprecated: None,
                            range: span_to_range(source, p.span),
                            selection_range: span_to_range(source, p.span),
                            children: None,
                        }
                    })
                    .collect();

                #[allow(deprecated)]
                symbols.push(DocumentSymbol {
                    name: f.name.clone(),
                    detail: Some(detail),
                    kind: SymbolKind::FUNCTION,
                    tags: None,
                    deprecated: None,
                    range: span_to_range(source, f.span),
                    selection_range: span_to_range(source, f.span),
                    children: if children.is_empty() {
                        None
                    } else {
                        Some(children)
                    },
                });
            }
        }
    }

    symbols
}
