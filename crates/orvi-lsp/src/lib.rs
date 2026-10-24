use std::collections::HashMap;
use std::sync::RwLock;

use orvi_syntax::{Span, parse};
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::{
    Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, InitializeParams, InitializeResult, InitializedParams, MessageType,
    Position, Range, ServerCapabilities, ServerInfo, TextDocumentSyncCapability,
    TextDocumentSyncKind,
};
use tower_lsp_server::{Client, LanguageServer};

const SERVER_NAME: &str = "orvi-lsp";

#[derive(Debug)]
pub struct Backend {
    client: Client,
    documents: RwLock<HashMap<String, String>>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            documents: RwLock::new(HashMap::new()),
        }
    }

    async fn publish(
        &self,
        uri: tower_lsp_server::ls_types::Uri,
        source: &str,
        version: Option<i32>,
    ) {
        self.client
            .publish_diagnostics(uri, diagnostics(source), version)
            .await;
    }
}

impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                ..ServerCapabilities::default()
            },
            server_info: Some(ServerInfo {
                name: SERVER_NAME.into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            offset_encoding: None,
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "OrviScript language server initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let document = params.text_document;
        self.documents
            .write()
            .expect("document store lock poisoned")
            .insert(document.uri.to_string(), document.text.clone());
        self.publish(document.uri, &document.text, Some(document.version))
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let Some(change) = params.content_changes.into_iter().last() else {
            return;
        };
        let document = params.text_document;
        self.documents
            .write()
            .expect("document store lock poisoned")
            .insert(document.uri.to_string(), change.text.clone());
        self.publish(document.uri, &change.text, Some(document.version))
            .await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents
            .write()
            .expect("document store lock poisoned")
            .remove(&uri.to_string());
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
    }
}

pub fn diagnostics(source: &str) -> Vec<Diagnostic> {
    let parsed = parse(source);
    parsed
        .lexical_errors
        .iter()
        .map(|error| diagnostic(source, error.span, error.to_string()))
        .chain(
            parsed
                .parse_errors
                .iter()
                .map(|error| diagnostic(source, error.span, error.message.clone())),
        )
        .collect()
}

fn diagnostic(source: &str, span: Span, message: String) -> Diagnostic {
    Diagnostic {
        range: span_range(source, span),
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("orviscript".into()),
        message,
        ..Diagnostic::default()
    }
}

pub fn span_range(source: &str, span: Span) -> Range {
    Range {
        start: position(source, span.start()),
        end: position(source, span.end()),
    }
}

fn position(source: &str, offset: usize) -> Position {
    let offset = offset.min(source.len());
    let offset = if source.is_char_boundary(offset) {
        offset
    } else {
        (0..offset)
            .rev()
            .find(|candidate| source.is_char_boundary(*candidate))
            .unwrap_or(0)
    };
    let prefix = &source[..offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() as u32;
    let line_text = prefix
        .rsplit_once('\n')
        .map_or(prefix, |(_, current_line)| current_line);
    let character = line_text.encode_utf16().count() as u32;
    Position::new(line, character)
}

#[cfg(test)]
mod tests {
    use super::{diagnostics, span_range};
    use orvi_syntax::Span;
    use tower_lsp_server::ls_types::{DiagnosticSeverity, Position, Range};

    #[test]
    fn reports_parser_errors_from_shared_frontend() {
        let diagnostics = diagnostics("let answer = ;");

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(diagnostics[0].source.as_deref(), Some("orviscript"));
        assert!(diagnostics[0].message.contains("expected expression"));
    }

    #[test]
    fn accepts_valid_programs_without_diagnostics() {
        assert!(diagnostics("let answer = 42; print(answer);").is_empty());
    }

    #[test]
    fn converts_utf8_offsets_to_utf16_lsp_positions() {
        let source = "let face = \"😀\";\n@";
        let emoji_start = source.find('😀').expect("emoji offset");
        let emoji_end = emoji_start + '😀'.len_utf8();

        assert_eq!(
            span_range(
                source,
                Span::new(emoji_start, emoji_end).expect("valid span")
            ),
            Range::new(Position::new(0, 12), Position::new(0, 14))
        );
        assert_eq!(diagnostics(source)[0].range.start, Position::new(1, 0));
    }
}
