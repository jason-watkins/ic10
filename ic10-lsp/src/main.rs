mod instructions;
mod parser;
mod validate;

use std::collections::HashMap;
use std::sync::Mutex;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use parser::{Line, LineKind};
use validate::{Diagnostic as IC10Diagnostic, Severity};

struct DocumentState {
    source: String,
    lines: Vec<Line>,
}

struct Backend {
    client: Client,
    documents: Mutex<HashMap<Url, DocumentState>>,
}

impl Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            documents: Mutex::new(HashMap::new()),
        }
    }

    fn analyze(&self, uri: &Url, source: String) -> Vec<IC10Diagnostic> {
        let lines = parser::parse(&source);
        let diagnostics = validate::validate(&lines);

        let mut documents = self.documents.lock().unwrap();
        documents.insert(uri.clone(), DocumentState { source, lines });

        diagnostics
    }
}

fn to_lsp_diagnostics(diagnostics: &[IC10Diagnostic], source: &str) -> Vec<Diagnostic> {
    diagnostics
        .iter()
        .map(|d| {
            let severity = match d.severity {
                Severity::Error => DiagnosticSeverity::ERROR,
                Severity::Warning => DiagnosticSeverity::WARNING,
            };
            Diagnostic {
                range: span_to_range(source, d.span.start, d.span.end),
                severity: Some(severity),
                source: Some("ic10".to_string()),
                message: d.message.clone(),
                ..Diagnostic::default()
            }
        })
        .collect()
}

fn span_to_range(source: &str, start: usize, end: usize) -> Range {
    Range {
        start: offset_to_position(source, start),
        end: offset_to_position(source, end),
    }
}

fn offset_to_position(source: &str, offset: usize) -> Position {
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

fn position_to_offset(source: &str, position: Position) -> usize {
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

fn find_token_at_offset(
    lines: &[Line],
    offset: usize,
) -> Option<(&parser::Token, &Line)> {
    for line in lines {
        match &line.kind {
            LineKind::Label { name } => {
                if offset >= name.span.start && offset < name.span.end {
                    return Some((name, line));
                }
            }
            LineKind::Instruction { opcode, operands } => {
                if offset >= opcode.span.start && offset < opcode.span.end {
                    return Some((opcode, line));
                }
                for operand in operands {
                    if offset >= operand.span.start && offset < operand.span.end {
                        return Some((operand, line));
                    }
                }
            }
            LineKind::Empty => {}
        }
    }
    None
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                ..ServerCapabilities::default()
            },
            ..InitializeResult::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "IC10 language server initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let source = params.text_document.text;
        let diagnostics = self.analyze(&uri, source.clone());
        let lsp_diagnostics = to_lsp_diagnostics(&diagnostics, &source);
        self.client
            .publish_diagnostics(uri, lsp_diagnostics, None)
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        if let Some(change) = params.content_changes.into_iter().last() {
            let source = change.text;
            let diagnostics = self.analyze(&uri, source.clone());
            let lsp_diagnostics = to_lsp_diagnostics(&diagnostics, &source);
            self.client
                .publish_diagnostics(uri, lsp_diagnostics, None)
                .await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.client
            .publish_diagnostics(uri.clone(), vec![], None)
            .await;
        self.documents.lock().unwrap().remove(&uri);
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let documents = self.documents.lock().unwrap();
        let Some(state) = documents.get(uri) else {
            return Ok(None);
        };

        let offset = position_to_offset(&state.source, position);
        let Some((token, _line)) = find_token_at_offset(&state.lines, offset) else {
            return Ok(None);
        };

        if let Some(signature) = instructions::INSTRUCTION_MAP.get(token.text.as_str()) {
            let operand_strs: Vec<&str> = signature
                .operands
                .iter()
                .map(|o| match o {
                    instructions::OperandKind::Register => "r?",
                    instructions::OperandKind::RegisterOrNumber => "r?|num",
                    instructions::OperandKind::Device => "d?",
                    instructions::OperandKind::DeviceOrIndirectDevice => "d?|dr?",
                    instructions::OperandKind::LogicType => "logicType",
                    instructions::OperandKind::LogicSlotType => "logicSlotType",
                    instructions::OperandKind::BatchMode => "batchMode",
                    instructions::OperandKind::ReagentMode => "reagentMode",
                    instructions::OperandKind::Target => "target",
                    instructions::OperandKind::AliasName => "name",
                    instructions::OperandKind::AliasTarget => "target",
                    instructions::OperandKind::DefineName => "name",
                    instructions::OperandKind::DefineValue => "value",
                })
                .collect();

            let text = format!(
                "```ic10\n{} {}\n```\n{}",
                signature.name,
                operand_strs.join(" "),
                signature.description,
            );

            return Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: text,
                }),
                range: Some(span_to_range(
                    &state.source,
                    token.span.start,
                    token.span.end,
                )),
            }));
        }

        Ok(None)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let documents = self.documents.lock().unwrap();
        let Some(state) = documents.get(uri) else {
            return Ok(None);
        };

        let offset = position_to_offset(&state.source, position);
        let Some((token, _)) = find_token_at_offset(&state.lines, offset) else {
            return Ok(None);
        };

        for line in &state.lines {
            match &line.kind {
                LineKind::Label { name } if name.text == token.text => {
                    return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                        uri: uri.clone(),
                        range: span_to_range(&state.source, name.span.start, name.span.end),
                    })));
                }
                LineKind::Instruction { opcode, operands }
                    if (opcode.text == "alias" || opcode.text == "define")
                        && operands.first().is_some_and(|n| n.text == token.text) =>
                {
                    let name = &operands[0];
                    return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                        uri: uri.clone(),
                        range: span_to_range(&state.source, name.span.start, name.span.end),
                    })));
                }
                _ => {}
            }
        }

        Ok(None)
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = &params.text_document.uri;

        let documents = self.documents.lock().unwrap();
        let Some(state) = documents.get(uri) else {
            return Ok(None);
        };

        let mut symbols = Vec::new();

        for line in &state.lines {
            match &line.kind {
                LineKind::Label { name } => {
                    let range = span_to_range(&state.source, name.span.start, name.span.end);
                    #[allow(deprecated)]
                    symbols.push(DocumentSymbol {
                        name: name.text.clone(),
                        detail: Some("label".to_string()),
                        kind: SymbolKind::KEY,
                        range,
                        selection_range: range,
                        children: None,
                        tags: None,
                        deprecated: None,
                    });
                }
                LineKind::Instruction { opcode, operands } if opcode.text == "alias" => {
                    if let Some(name) = operands.first() {
                        let range =
                            span_to_range(&state.source, name.span.start, name.span.end);
                        let detail = operands.get(1).map(|t| t.text.clone());
                        #[allow(deprecated)]
                        symbols.push(DocumentSymbol {
                            name: name.text.clone(),
                            detail: detail.map(|t| format!("alias → {t}")),
                            kind: SymbolKind::VARIABLE,
                            range,
                            selection_range: range,
                            children: None,
                            tags: None,
                            deprecated: None,
                        });
                    }
                }
                LineKind::Instruction { opcode, operands } if opcode.text == "define" => {
                    if let Some(name) = operands.first() {
                        let range =
                            span_to_range(&state.source, name.span.start, name.span.end);
                        let detail = operands.get(1).map(|t| t.text.clone());
                        #[allow(deprecated)]
                        symbols.push(DocumentSymbol {
                            name: name.text.clone(),
                            detail: detail.map(|v| format!("= {v}")),
                            kind: SymbolKind::CONSTANT,
                            range,
                            selection_range: range,
                            children: None,
                            tags: None,
                            deprecated: None,
                        });
                    }
                }
                _ => {}
            }
        }

        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
