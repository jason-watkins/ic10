mod convert;
mod definition;
mod hover;
mod rename;
mod symbols;

use std::collections::HashMap;
use std::sync::Mutex;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use ic20c::bind;
use ic20c::diagnostic::{Diagnostic as CompilerDiagnostic, Severity as CompilerSeverity};
use ic20c::ir::ast::{Item, Program as AstProgram};
use ic20c::ir::bound::Program as BoundProgram;
use ic20c::parser;

use convert::{
    compiler_to_lsp_diagnostics, contains, position_to_offset, span_to_range, type_name,
};
use definition::find_definition_in_bound;
use hover::find_hover_in_bound;
use rename::find_rename_target;
use symbols::document_symbols_from_ast;

struct DocumentState {
    source: String,
    ast: AstProgram,
    bound: Option<BoundProgram>,
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

    fn analyze(&self, uri: &Url, source: String) -> Vec<CompilerDiagnostic> {
        let mut all_diagnostics = Vec::new();

        let (ast, parse_diagnostics) = parser::parse(&source);
        all_diagnostics.extend(parse_diagnostics);

        let has_parse_errors = all_diagnostics
            .iter()
            .any(|d| d.severity == CompilerSeverity::Error);

        let bound = if !has_parse_errors {
            match bind::bind(&ast) {
                Ok((program, bind_diagnostics)) => {
                    all_diagnostics.extend(bind_diagnostics);
                    Some(program)
                }
                Err(diagnostics) => {
                    all_diagnostics.extend(diagnostics);
                    None
                }
            }
        } else {
            None
        };

        let mut documents = self.documents.lock().unwrap();
        documents.insert(uri.clone(), DocumentState { source, ast, bound });

        all_diagnostics
    }
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
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: Default::default(),
                })),
                ..ServerCapabilities::default()
            },
            ..InitializeResult::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "IC20 language server initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let source = params.text_document.text;
        let diagnostics = self.analyze(&uri, source.clone());
        let lsp_diagnostics = compiler_to_lsp_diagnostics(&diagnostics, &source);
        self.client
            .publish_diagnostics(uri, lsp_diagnostics, None)
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        if let Some(change) = params.content_changes.into_iter().last() {
            let source = change.text;
            let diagnostics = self.analyze(&uri, source.clone());
            let lsp_diagnostics = compiler_to_lsp_diagnostics(&diagnostics, &source);
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

        // Try bound IR hover first (more information available)
        if let Some(bound) = &state.bound
            && let Some(result) = find_hover_in_bound(bound, offset)
        {
            return Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: result.text,
                }),
                range: Some(span_to_range(&state.source, result.span)),
            }));
        }

        // Fall back to AST-level hover for consts and devices
        for item in &state.ast.items {
            match item {
                Item::Const(c) if contains(c.span, offset) => {
                    return Ok(Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: format!("```ic20\nconst {}: {}\n```", c.name, type_name(&c.ty)),
                        }),
                        range: Some(span_to_range(&state.source, c.span)),
                    }));
                }
                Item::Static(s) if contains(s.span, offset) => {
                    return Ok(Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: format!(
                                "```ic20\nstatic{} {}: {}\n```",
                                if s.mutable { " mut" } else { "" },
                                s.name,
                                type_name(&s.ty)
                            ),
                        }),
                        range: Some(span_to_range(&state.source, s.span)),
                    }));
                }
                Item::Device(d) if contains(d.span, offset) => {
                    return Ok(Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: format!("```ic20\ndevice {}: {:?}\n```", d.name, d.pin),
                        }),
                        range: Some(span_to_range(&state.source, d.span)),
                    }));
                }
                _ => {}
            }
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

        if let Some(bound) = &state.bound
            && let Some(def) = find_definition_in_bound(bound, &state.ast, offset)
        {
            let range = span_to_range(&state.source, def.span);
            return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                uri: uri.clone(),
                range,
            })));
        }

        Ok(None)
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let uri = &params.text_document.uri;
        let position = params.position;

        let documents = self.documents.lock().unwrap();
        let Some(state) = documents.get(uri) else {
            return Ok(None);
        };
        let Some(bound) = &state.bound else {
            return Ok(None);
        };

        let offset = position_to_offset(&state.source, position);
        let Some((name, name_span, _)) =
            find_rename_target(bound, &state.ast, &state.source, offset)
        else {
            return Ok(None);
        };

        Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
            range: span_to_range(&state.source, name_span),
            placeholder: name,
        }))
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let new_name = &params.new_name;

        let documents = self.documents.lock().unwrap();
        let Some(state) = documents.get(uri) else {
            return Ok(None);
        };
        let Some(bound) = &state.bound else {
            return Ok(None);
        };

        let offset = position_to_offset(&state.source, position);
        let Some((_, _, all_spans)) = find_rename_target(bound, &state.ast, &state.source, offset)
        else {
            return Ok(None);
        };

        let edits: Vec<TextEdit> = all_spans
            .into_iter()
            .map(|span| TextEdit {
                range: span_to_range(&state.source, span),
                new_text: new_name.clone(),
            })
            .collect();

        let mut changes = HashMap::new();
        changes.insert(uri.clone(), edits);

        Ok(Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }))
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

        let symbols = document_symbols_from_ast(&state.ast, &state.source);
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
