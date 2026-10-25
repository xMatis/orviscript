use std::collections::HashMap;
use std::sync::RwLock;

use orvi_sema::{Analysis, SymbolOccurrence, analyze, analyze_program};
use orvi_syntax::{Span, TokenKind, format_source, lex, parse};
use tower_lsp_server::jsonrpc::{Error, Result};
use tower_lsp_server::ls_types::{
    CompletionItem, CompletionItemKind, CompletionOptions, CompletionParams, CompletionResponse,
    Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentFormattingParams, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverContents, HoverParams, HoverProviderCapability,
    InitializeParams, InitializeResult, InitializedParams, Location, MarkupContent, MarkupKind,
    MessageType, OneOf, Position, PrepareRenameResponse, Range, ReferenceParams, RenameOptions,
    RenameParams, SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens,
    SemanticTokensFullOptions, SemanticTokensLegend, SemanticTokensOptions, SemanticTokensParams,
    SemanticTokensResult, ServerCapabilities, ServerInfo, TextDocumentPositionParams,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, WorkDoneProgressOptions,
    WorkspaceEdit,
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

    fn document(&self, uri: &tower_lsp_server::ls_types::Uri) -> Option<String> {
        self.documents
            .read()
            .expect("document store lock poisoned")
            .get(&uri.to_string())
            .cloned()
    }
}

impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                completion_provider: Some(CompletionOptions::default()),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                semantic_tokens_provider: Some(semantic_tokens_options().into()),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                })),
                document_formatting_provider: Some(OneOf::Left(true)),
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

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let document = params.text_document_position.text_document;
        let source = self.document(&document.uri);
        Ok(Some(CompletionResponse::Array(completion_items(
            source.as_deref(),
        ))))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let document = params.text_document_position_params.text_document;
        let Some(source) = self.document(&document.uri) else {
            return Ok(None);
        };
        Ok(hover_at(
            &source,
            params.text_document_position_params.position,
        ))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let Some(source) = self.document(&params.text_document.uri) else {
            return Ok(None);
        };
        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data: semantic_tokens(&source),
        })))
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let Some(source) = self.document(&params.text_document.uri) else {
            return Ok(None);
        };
        Ok(format_document(&source).map(|edit| vec![edit]))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let document = params.text_document_position_params.text_document;
        let Some(source) = self.document(&document.uri) else {
            return Ok(None);
        };
        let Some((_, symbol)) = symbol_at(&source, params.text_document_position_params.position)
        else {
            return Ok(None);
        };
        Ok(Some(GotoDefinitionResponse::Scalar(Location::new(
            document.uri,
            span_range(&source, symbol.definition),
        ))))
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let document = params.text_document_position.text_document;
        let Some(source) = self.document(&document.uri) else {
            return Ok(None);
        };
        let Some((analysis, selected)) = symbol_at(&source, params.text_document_position.position)
        else {
            return Ok(None);
        };
        let locations = analysis
            .symbols
            .iter()
            .filter(|symbol| {
                symbol.definition == selected.definition
                    && (params.context.include_declaration || !symbol.is_definition)
            })
            .map(|symbol| Location::new(document.uri.clone(), span_range(&source, symbol.span)))
            .collect();
        Ok(Some(locations))
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let Some(source) = self.document(&params.text_document.uri) else {
            return Ok(None);
        };
        let Some((_, symbol)) = symbol_at(&source, params.position) else {
            return Ok(None);
        };
        Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
            range: span_range(&source, symbol.span),
            placeholder: symbol.name.clone(),
        }))
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        if !is_identifier(&params.new_name) {
            return Err(Error::invalid_params("new name is not a valid identifier"));
        }
        let document = params.text_document_position.text_document;
        let Some(source) = self.document(&document.uri) else {
            return Ok(None);
        };
        let Some((analysis, selected)) = symbol_at(&source, params.text_document_position.position)
        else {
            return Ok(None);
        };
        if analysis.symbols.iter().any(|symbol| {
            symbol.is_definition
                && symbol.name == params.new_name
                && symbol.definition != selected.definition
        }) {
            return Err(Error::invalid_params(format!(
                "name '{}' is already declared in this document",
                params.new_name
            )));
        }
        let edits = analysis
            .symbols
            .iter()
            .filter(|symbol| symbol.definition == selected.definition)
            .map(|symbol| TextEdit::new(span_range(&source, symbol.span), params.new_name.clone()))
            .collect();
        Ok(Some(WorkspaceEdit {
            changes: Some(HashMap::from([(document.uri, edits)])),
            ..WorkspaceEdit::default()
        }))
    }
}

fn symbol_at(source: &str, position: Position) -> Option<(Analysis, SymbolOccurrence)> {
    let parsed = parse(source);
    if parsed.has_errors() {
        return None;
    }
    let offset = offset_at_position(source, position)?;
    let analysis = analyze_program(&parsed.program);
    let symbol = analysis
        .symbols
        .iter()
        .find(|symbol| symbol.span.start() <= offset && offset <= symbol.span.end())?
        .clone();
    Some((analysis, symbol))
}

fn is_identifier(name: &str) -> bool {
    let lexed = lex(name);
    lexed.errors.is_empty()
        && lexed.tokens.len() == 2
        && lexed.tokens[0].kind == TokenKind::Identifier
        && lexed.tokens[0].span.text(name) == Some(name)
}

pub fn diagnostics(source: &str) -> Vec<Diagnostic> {
    let parsed = parse(source);
    let mut diagnostics = parsed
        .lexical_errors
        .iter()
        .map(|error| diagnostic(source, error.span, error.to_string()))
        .chain(
            parsed
                .parse_errors
                .iter()
                .map(|error| diagnostic(source, error.span, error.message.clone())),
        )
        .collect::<Vec<_>>();
    if !parsed.has_errors() {
        diagnostics.extend(
            analyze(&parsed.program)
                .into_iter()
                .map(|error| diagnostic(source, error.span, error.message)),
        );
    }
    diagnostics
}

fn semantic_tokens_options() -> SemanticTokensOptions {
    SemanticTokensOptions {
        work_done_progress_options: WorkDoneProgressOptions::default(),
        legend: semantic_tokens_legend(),
        range: Some(false),
        full: Some(SemanticTokensFullOptions::Bool(true)),
    }
}

pub fn semantic_tokens_legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::VARIABLE,
            SemanticTokenType::FUNCTION,
            SemanticTokenType::PARAMETER,
            SemanticTokenType::KEYWORD,
            SemanticTokenType::STRING,
            SemanticTokenType::NUMBER,
            SemanticTokenType::COMMENT,
            SemanticTokenType::OPERATOR,
            SemanticTokenType::TYPE,
            SemanticTokenType::PROPERTY,
        ],
        token_modifiers: vec![
            SemanticTokenModifier::DECLARATION,
            SemanticTokenModifier::READONLY,
            SemanticTokenModifier::DEFAULT_LIBRARY,
        ],
    }
}

pub fn semantic_tokens(source: &str) -> Vec<SemanticToken> {
    let lexed = lex(source);
    let significant = lexed
        .tokens
        .iter()
        .enumerate()
        .filter(|(_, token)| !matches!(token.kind, TokenKind::Whitespace | TokenKind::EndOfFile))
        .collect::<Vec<_>>();
    let mut result = Vec::new();
    let mut previous_line = 0;
    let mut previous_start = 0;
    let mut in_parameters = false;
    let mut expects_parameters = false;

    for (position_in_significant, (_, token)) in significant.iter().enumerate() {
        let previous = position_in_significant
            .checked_sub(1)
            .and_then(|index| significant.get(index))
            .map(|(_, token)| token.kind);
        let next = significant
            .get(position_in_significant + 1)
            .map(|(_, token)| token.kind);

        if token.kind == TokenKind::Fn {
            expects_parameters = true;
        } else if expects_parameters && token.kind == TokenKind::LeftParenthesis {
            in_parameters = true;
            expects_parameters = false;
        } else if in_parameters && token.kind == TokenKind::RightParenthesis {
            in_parameters = false;
        }

        let text = token.text(source).unwrap_or_default();
        let classification = classify_token(token.kind, previous, next, in_parameters, text);
        let Some((token_type, modifiers)) = classification else {
            continue;
        };
        let start = position(source, token.span.start());
        let end = position(source, token.span.end());
        if start.line != end.line {
            continue;
        }
        result.push(SemanticToken {
            delta_line: start.line - previous_line,
            delta_start: if start.line == previous_line {
                start.character - previous_start
            } else {
                start.character
            },
            length: end.character - start.character,
            token_type,
            token_modifiers_bitset: modifiers,
        });
        previous_line = start.line;
        previous_start = start.character;
    }
    result
}

fn classify_token(
    kind: TokenKind,
    previous: Option<TokenKind>,
    next: Option<TokenKind>,
    in_parameters: bool,
    text: &str,
) -> Option<(u32, u32)> {
    const DECLARATION: u32 = 1;
    const READONLY: u32 = 2;
    const DEFAULT_LIBRARY: u32 = 4;

    let classification = match kind {
        TokenKind::Identifier
            if matches!(
                text,
                "Option"
                    | "Result"
                    | "any"
                    | "bool"
                    | "float"
                    | "fn"
                    | "int"
                    | "list"
                    | "number"
                    | "string"
            ) =>
        {
            (8, 0)
        }
        TokenKind::Identifier if previous == Some(TokenKind::Struct) => (8, DECLARATION),
        TokenKind::Identifier if previous == Some(TokenKind::Dot) => (9, 0),
        TokenKind::Identifier if matches!(previous, Some(TokenKind::Colon | TokenKind::Arrow)) => {
            (8, 0)
        }
        TokenKind::Identifier if previous == Some(TokenKind::Fn) => (1, DECLARATION),
        TokenKind::Identifier if in_parameters => (2, DECLARATION),
        TokenKind::Identifier if previous == Some(TokenKind::Let) => (0, DECLARATION),
        TokenKind::Identifier if previous == Some(TokenKind::Const) => (0, DECLARATION | READONLY),
        TokenKind::Identifier if text == "print" => (1, DEFAULT_LIBRARY),
        TokenKind::Identifier if next == Some(TokenKind::LeftParenthesis) => (1, 0),
        TokenKind::Identifier => (0, 0),
        TokenKind::Let
        | TokenKind::Const
        | TokenKind::Fn
        | TokenKind::Struct
        | TokenKind::Match
        | TokenKind::Using
        | TokenKind::Defer
        | TokenKind::If
        | TokenKind::Else
        | TokenKind::While
        | TokenKind::Return
        | TokenKind::True
        | TokenKind::False
        | TokenKind::Null => (3, 0),
        TokenKind::String => (4, 0),
        TokenKind::Integer | TokenKind::Float => (5, 0),
        TokenKind::LineComment => (6, 0),
        TokenKind::Plus
        | TokenKind::PlusEqual
        | TokenKind::Minus
        | TokenKind::MinusEqual
        | TokenKind::Arrow
        | TokenKind::Star
        | TokenKind::StarEqual
        | TokenKind::StarStar
        | TokenKind::Slash
        | TokenKind::SlashEqual
        | TokenKind::Percent
        | TokenKind::Equal
        | TokenKind::EqualEqual
        | TokenKind::FatArrow
        | TokenKind::Bang
        | TokenKind::BangEqual
        | TokenKind::Less
        | TokenKind::LessEqual
        | TokenKind::Greater
        | TokenKind::GreaterEqual
        | TokenKind::AmpersandAmpersand
        | TokenKind::PipePipe
        | TokenKind::Question
        | TokenKind::DotDot
        | TokenKind::DotDotEqual => (7, 0),
        _ => return None,
    };
    Some(classification)
}

fn completion_items(source: Option<&str>) -> Vec<CompletionItem> {
    let mut items = [
        (
            "let",
            CompletionItemKind::KEYWORD,
            "mutable variable declaration",
        ),
        (
            "const",
            CompletionItemKind::KEYWORD,
            "immutable variable declaration",
        ),
        ("fn", CompletionItemKind::KEYWORD, "function declaration"),
        ("struct", CompletionItemKind::KEYWORD, "struct declaration"),
        (
            "match",
            CompletionItemKind::KEYWORD,
            "pattern matching expression",
        ),
        (
            "using",
            CompletionItemKind::KEYWORD,
            "scoped resource cleanup",
        ),
        ("defer", CompletionItemKind::KEYWORD, "deferred cleanup"),
        ("if", CompletionItemKind::KEYWORD, "conditional branch"),
        ("else", CompletionItemKind::KEYWORD, "alternative branch"),
        ("while", CompletionItemKind::KEYWORD, "loop"),
        (
            "return",
            CompletionItemKind::KEYWORD,
            "return from a function",
        ),
        ("true", CompletionItemKind::CONSTANT, "boolean value"),
        ("false", CompletionItemKind::CONSTANT, "boolean value"),
        ("null", CompletionItemKind::CONSTANT, "null value"),
        (
            "print",
            CompletionItemKind::FUNCTION,
            "built-in output function",
        ),
        ("int", CompletionItemKind::TYPE_PARAMETER, "integer type"),
        (
            "float",
            CompletionItemKind::TYPE_PARAMETER,
            "floating-point type",
        ),
        ("number", CompletionItemKind::TYPE_PARAMETER, "numeric type"),
        ("string", CompletionItemKind::TYPE_PARAMETER, "string type"),
        ("bool", CompletionItemKind::TYPE_PARAMETER, "boolean type"),
        ("any", CompletionItemKind::TYPE_PARAMETER, "dynamic type"),
        ("list", CompletionItemKind::TYPE_PARAMETER, "list type"),
        ("Option", CompletionItemKind::ENUM, "optional value type"),
        ("Result", CompletionItemKind::ENUM, "success or error type"),
        (
            "Some",
            CompletionItemKind::CONSTRUCTOR,
            "present Option value",
        ),
        (
            "None",
            CompletionItemKind::ENUM_MEMBER,
            "missing Option value",
        ),
        (
            "Ok",
            CompletionItemKind::CONSTRUCTOR,
            "successful Result value",
        ),
        (
            "Err",
            CompletionItemKind::CONSTRUCTOR,
            "failed Result value",
        ),
    ]
    .into_iter()
    .map(|(label, kind, detail)| CompletionItem {
        label: label.into(),
        kind: Some(kind),
        detail: Some(detail.into()),
        ..CompletionItem::default()
    })
    .collect::<Vec<_>>();
    if let Some(source) = source {
        let parsed = parse(source);
        if !parsed.has_errors() {
            let analysis = analyze_program(&parsed.program);
            for symbol in analysis
                .symbols
                .into_iter()
                .filter(|symbol| symbol.is_definition)
            {
                if items.iter().any(|item| item.label == symbol.name) {
                    continue;
                }
                let kind = match symbol.kind {
                    orvi_sema::SymbolKind::Function => CompletionItemKind::FUNCTION,
                    orvi_sema::SymbolKind::Struct => CompletionItemKind::STRUCT,
                    orvi_sema::SymbolKind::Variable
                    | orvi_sema::SymbolKind::Constant
                    | orvi_sema::SymbolKind::Parameter => CompletionItemKind::VARIABLE,
                };
                items.push(CompletionItem {
                    label: symbol.name,
                    kind: Some(kind),
                    detail: symbol.type_name,
                    ..CompletionItem::default()
                });
            }
        }
    }
    items
}

fn hover_at(source: &str, position: Position) -> Option<Hover> {
    let offset = offset_at_position(source, position)?;
    let token = lex(source)
        .tokens
        .into_iter()
        .find(|token| token.span.start() <= offset && offset < token.span.end())?;
    let text = token.text(source)?;
    let description = match text {
        "let" => "`let` declares a mutable variable.",
        "const" => "`const` declares an immutable variable.",
        "fn" => "`fn` declares a function.",
        "struct" => "`struct` declares a stable set of typed fields.",
        "match" => "`match` selects an expression using exhaustive patterns.",
        "using" => "`using name = value => cleanup` registers scoped cleanup.",
        "defer" => "`defer expression` runs the expression when the scope exits.",
        "if" => "`if` executes a branch when its condition is `true`.",
        "else" => "`else` provides an alternative branch.",
        "while" => "`while` repeats a block while its condition is `true`.",
        "return" => "`return` exits the current function.",
        "true" | "false" => "Built-in boolean value.",
        "null" => "Built-in null value.",
        "print" => "`print(...)` writes its arguments to standard output.",
        "Some" => "`Some(value)` creates a present `Option` value.",
        "None" => "`None` is an `Option` without a value.",
        "Ok" => "`Ok(value)` creates a successful `Result`.",
        "Err" => "`Err(error)` creates a failed `Result`.",
        _ => {
            let (_, symbol) = symbol_at(source, position)?;
            let kind = match symbol.kind {
                orvi_sema::SymbolKind::Variable => "let",
                orvi_sema::SymbolKind::Constant => "const",
                orvi_sema::SymbolKind::Function => "fn",
                orvi_sema::SymbolKind::Struct => "struct",
                orvi_sema::SymbolKind::Parameter => "parameter",
            };
            let type_name = symbol.type_name.unwrap_or_else(|| "any".into());
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!("```orviscript\n{kind} {}: {type_name}\n```", symbol.name),
                }),
                range: Some(span_range(source, symbol.span)),
            });
        }
    };
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: description.into(),
        }),
        range: Some(span_range(source, token.span)),
    })
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

fn format_document(source: &str) -> Option<TextEdit> {
    let formatted = format_source(source).ok()?;
    Some(TextEdit::new(
        Range::new(Position::new(0, 0), position(source, source.len())),
        formatted,
    ))
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

fn offset_at_position(source: &str, position: Position) -> Option<usize> {
    let line = source.split_inclusive('\n').nth(position.line as usize)?;
    let line_start = line.as_ptr() as usize - source.as_ptr() as usize;
    let mut utf16_units = 0_u32;
    for (byte_offset, character) in line.char_indices() {
        if utf16_units == position.character {
            return Some(line_start + byte_offset);
        }
        utf16_units += character.len_utf16() as u32;
        if utf16_units > position.character {
            return None;
        }
    }
    (utf16_units == position.character).then_some(line_start + line.len())
}

#[cfg(test)]
mod tests {
    use super::{
        completion_items, diagnostics, format_document, hover_at, is_identifier, semantic_tokens,
        span_range, symbol_at,
    };
    use orvi_syntax::Span;
    use tower_lsp_server::ls_types::{
        CompletionItemKind, DiagnosticSeverity, HoverContents, Position, Range,
    };

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
    fn reports_semantic_errors_from_shared_analyzer() {
        let diagnostics = diagnostics("let answer = missing;");

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message, "undefined name 'missing'");
    }

    #[test]
    fn formats_a_complete_document_with_the_shared_formatter() {
        let edit = format_document("let value=1+2").expect("format edit");
        assert_eq!(edit.new_text, "let value = 1 + 2\n");
        assert_eq!(edit.range.start, Position::new(0, 0));
        assert_eq!(edit.range.end, Position::new(0, 13));
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

    #[test]
    fn classifies_semantic_tokens_and_modifiers() {
        let tokens = semantic_tokens("const limit = 3; fn add(value) { return value + limit; }");

        assert!(tokens.iter().any(|token| token.token_type == 1));
        assert!(tokens.iter().any(|token| token.token_type == 2));
        assert!(
            tokens
                .iter()
                .any(|token| token.token_modifiers_bitset & 1 == 1)
        );
        assert!(
            tokens
                .iter()
                .any(|token| token.token_modifiers_bitset & 2 == 2)
        );
    }

    #[test]
    fn offers_implemented_keywords_and_builtin() {
        let items = completion_items(None);

        assert!(items.iter().any(|item| item.label == "let"));
        assert!(items.iter().any(|item| {
            item.label == "print" && item.kind == Some(CompletionItemKind::FUNCTION)
        }));
        assert!(items.iter().any(|item| item.label == "struct"));
    }

    #[test]
    fn explains_builtin_on_hover() {
        let hover = hover_at("print(42);", Position::new(0, 2)).expect("hover result");
        let HoverContents::Markup(contents) = hover.contents else {
            panic!("expected markdown hover");
        };
        assert!(contents.value.contains("standard output"));
    }

    #[test]
    fn explains_declared_symbol_types_on_hover() {
        let hover = hover_at("let answer: int = 42; print(answer);", Position::new(0, 30))
            .expect("hover result");
        let HoverContents::Markup(contents) = hover.contents else {
            panic!("expected markdown hover");
        };
        assert!(contents.value.contains("answer: int"));
    }

    #[test]
    fn completes_symbols_declared_in_the_document() {
        let items = completion_items(Some("let answer: int = 42;"));
        let answer = items
            .iter()
            .find(|item| item.label == "answer")
            .expect("symbol completion");
        assert_eq!(answer.detail.as_deref(), Some("int"));
    }

    #[test]
    fn resolves_a_reference_to_its_definition() {
        let source = "let answer = 42; print(answer);";
        let (_, symbol) = symbol_at(source, Position::new(0, 25)).expect("resolved symbol");

        assert_eq!(
            span_range(source, symbol.definition).start,
            Position::new(0, 4)
        );
        assert!(!symbol.is_definition);
    }

    #[test]
    fn validates_rename_identifiers_with_the_language_lexer() {
        assert!(is_identifier("wynik"));
        assert!(is_identifier("żółw"));
        assert!(!is_identifier("let"));
        assert!(!is_identifier("two words"));
    }
}
