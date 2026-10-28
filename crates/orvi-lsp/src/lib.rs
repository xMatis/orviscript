use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use orvi_project::{Project, resolve_import};
use orvi_sema::{
    Analysis, STANDARD_MODULES, StandardMember, SymbolOccurrence, analyze_program,
    analyze_program_with_modules, standard_module,
};
use orvi_syntax::{
    Program, RuleDiagnostic, RuleSeverity, Span, Statement, StatementKind, TokenKind,
    format_source, lex, parse,
};
use tower_lsp_server::jsonrpc::{Error, Result};
use tower_lsp_server::ls_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams,
    CodeActionProviderCapability, CodeActionResponse, CompletionItem, CompletionItemKind,
    CompletionOptions, CompletionParams, CompletionResponse, Diagnostic, DiagnosticSeverity,
    DidChangeConfigurationParams, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentFormattingParams, DocumentSymbol, DocumentSymbolParams,
    DocumentSymbolResponse, Documentation, GotoDefinitionParams, GotoDefinitionResponse, Hover,
    HoverContents, HoverParams, HoverProviderCapability, InitializeParams, InitializeResult,
    InitializedParams, InsertTextFormat, Location, MarkupContent, MarkupKind, MessageType,
    NumberOrString, OneOf, Position, PrepareRenameResponse, Range, ReferenceParams, RenameOptions,
    RenameParams, SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens,
    SemanticTokensFullOptions, SemanticTokensLegend, SemanticTokensOptions, SemanticTokensParams,
    SemanticTokensResult, ServerCapabilities, ServerInfo, SymbolKind as LspSymbolKind,
    TextDocumentPositionParams, TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, Uri,
    WorkDoneProgressOptions, WorkspaceEdit,
};
use tower_lsp_server::{Client, LanguageServer};

const SERVER_NAME: &str = "orvi-lsp";

#[derive(Debug)]
pub struct Backend {
    client: Client,
    documents: RwLock<HashMap<String, String>>,
    diagnostic_settings: RwLock<DiagnosticSettings>,
}

#[derive(Clone, Debug)]
struct DiagnosticSettings {
    enabled_categories: HashSet<String>,
    minimum_severity: RuleSeverity,
}

impl Default for DiagnosticSettings {
    fn default() -> Self {
        Self {
            enabled_categories: HashSet::new(),
            minimum_severity: RuleSeverity::Hint,
        }
    }
}

impl DiagnosticSettings {
    fn from_value(value: &serde_json::Value) -> Self {
        let diagnostics = value.get("diagnostics").unwrap_or(value);
        let enabled_categories = diagnostics
            .get("enabledCategories")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_owned)
            .collect();
        let minimum_severity = match diagnostics
            .get("minimumSeverity")
            .and_then(serde_json::Value::as_str)
        {
            Some("error") => RuleSeverity::Error,
            Some("warning") => RuleSeverity::Warning,
            Some("information") => RuleSeverity::Information,
            _ => RuleSeverity::Hint,
        };
        Self {
            enabled_categories,
            minimum_severity,
        }
    }

    fn allows(&self, rule: &RuleDiagnostic) -> bool {
        (self.enabled_categories.is_empty() || self.enabled_categories.contains(rule.category))
            && severity_rank(rule.severity) <= severity_rank(self.minimum_severity)
    }
}

#[derive(Clone, Debug)]
struct ProjectDocument {
    uri: Uri,
    path: PathBuf,
    source: String,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            documents: RwLock::new(HashMap::new()),
            diagnostic_settings: RwLock::new(DiagnosticSettings::default()),
        }
    }

    async fn publish(
        &self,
        uri: tower_lsp_server::ls_types::Uri,
        source: &str,
        version: Option<i32>,
    ) {
        let project_documents = self.project_documents(&uri);
        let modules = uri
            .to_file_path()
            .map(|path| imported_programs(path.as_ref(), source, &project_documents))
            .unwrap_or_default();
        let settings = self
            .diagnostic_settings
            .read()
            .expect("diagnostic settings lock poisoned")
            .clone();
        let mut current_diagnostics =
            diagnostics_with_modules_and_settings(source, &modules, &settings);
        if let Some(path) = uri.to_file_path() {
            let documents = self.documents.read().expect("document store lock poisoned");
            current_diagnostics.extend(import_diagnostics(path.as_ref(), source, &documents));
        }
        self.client
            .publish_diagnostics(uri, current_diagnostics, version)
            .await;
    }

    fn document(&self, uri: &Uri) -> Option<String> {
        let open = self
            .documents
            .read()
            .expect("document store lock poisoned")
            .get(&uri.to_string())
            .cloned();
        open.or_else(|| {
            uri.to_file_path()
                .and_then(|path| fs::read_to_string(path.as_ref()).ok())
        })
    }

    fn project_documents(&self, uri: &Uri) -> Vec<ProjectDocument> {
        let documents = self.documents.read().expect("document store lock poisoned");
        load_project_documents(uri, &documents)
    }

    async fn republish_other_open_project_documents(&self, changed_uri: &Uri) {
        let open_uris = self
            .documents
            .read()
            .expect("document store lock poisoned")
            .keys()
            .filter_map(|uri| uri.parse::<Uri>().ok())
            .collect::<Vec<_>>();
        let changed_path = changed_uri.to_file_path();
        let documents = self.project_documents(changed_uri);
        for document in documents {
            if changed_path
                .as_ref()
                .is_some_and(|path| same_path(path.as_ref(), &document.path))
            {
                continue;
            }
            let Some(open_uri) = open_uris.iter().find(|uri| {
                uri.to_file_path()
                    .is_some_and(|path| same_path(path.as_ref(), &document.path))
            }) else {
                continue;
            };
            self.publish(open_uri.clone(), &document.source, None).await;
        }
    }
}

impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        if let Some(options) = params.initialization_options.as_ref() {
            *self
                .diagnostic_settings
                .write()
                .expect("diagnostic settings lock poisoned") =
                DiagnosticSettings::from_value(options);
        }
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![".".into(), "\"".into(), ":".into(), ",".into()]),
                    ..CompletionOptions::default()
                }),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                semantic_tokens_provider: Some(semantic_tokens_options().into()),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                })),
                document_formatting_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
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
        self.publish(document.uri.clone(), &document.text, Some(document.version))
            .await;
        self.republish_other_open_project_documents(&document.uri)
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
        self.publish(document.uri.clone(), &change.text, Some(document.version))
            .await;
        self.republish_other_open_project_documents(&document.uri)
            .await;
    }

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        *self
            .diagnostic_settings
            .write()
            .expect("diagnostic settings lock poisoned") =
            DiagnosticSettings::from_value(&params.settings);
        let documents = self
            .documents
            .read()
            .expect("document store lock poisoned")
            .iter()
            .filter_map(|(uri, source)| Some((uri.parse::<Uri>().ok()?, source.clone())))
            .collect::<Vec<_>>();
        for (uri, source) in documents {
            self.publish(uri, &source, None).await;
        }
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
        let position = params.text_document_position.position;
        let items = if let Some(source) = source.as_deref()
            && let Some(items) = standard_completion_items(source, position)
        {
            items
        } else {
            let items = completion_items(source.as_deref());
            if let Some(source) = source.as_deref()
                && let Some(path) = document.uri.to_file_path()
            {
                let imported = import_completion_items(
                    path.as_ref(),
                    source,
                    position,
                    &self.project_documents(&document.uri),
                );
                if !imported.is_empty() {
                    return Ok(Some(CompletionResponse::Array(imported)));
                }
            }
            items
        };
        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let document = params.text_document_position_params.text_document;
        let Some(source) = self.document(&document.uri) else {
            return Ok(None);
        };
        if let Some(hover) =
            standard_hover_at(&source, params.text_document_position_params.position)
        {
            return Ok(Some(hover));
        }
        if let Some(path) = document.uri.to_file_path()
            && let Some((_, selected)) =
                symbol_at(&source, params.text_document_position_params.position)
            && let Some(identity) = symbol_identity(path.as_ref(), &source, &selected)
            && !same_path(path.as_ref(), &identity.0)
            && let Some(description) =
                project_symbol_description(&self.project_documents(&document.uri), &identity)
        {
            return Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!("```orviscript\n{description}\n```"),
                }),
                range: Some(span_range(&source, selected.span)),
            }));
        }
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

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let Some(source) = self.document(&params.text_document.uri) else {
            return Ok(None);
        };
        let settings = self
            .diagnostic_settings
            .read()
            .expect("diagnostic settings lock poisoned")
            .clone();
        Ok(Some(code_actions_with_settings(
            params.text_document.uri,
            &source,
            params.range,
            &settings,
        )))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let Some(source) = self.document(&params.text_document.uri) else {
            return Ok(None);
        };
        Ok(Some(DocumentSymbolResponse::Nested(document_symbols(
            &source,
        ))))
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
        if let Some(path) = document.uri.to_file_path()
            && let Some(identity) = symbol_identity(path.as_ref(), &source, &symbol)
            && let Some(target) = self
                .project_documents(&document.uri)
                .into_iter()
                .find(|candidate| same_path(&candidate.path, &identity.0))
            && let Some(span) = exported_definition_span(&target.source, &identity.1)
        {
            return Ok(Some(GotoDefinitionResponse::Scalar(Location::new(
                target.uri,
                span_range(&target.source, span),
            ))));
        }
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
        let locations = if let Some(path) = document.uri.to_file_path()
            && let Some(identity) = symbol_identity(path.as_ref(), &source, &selected)
        {
            project_symbol_locations(
                &self.project_documents(&document.uri),
                &identity,
                params.context.include_declaration,
            )
        } else {
            analysis
                .symbols
                .iter()
                .filter(|symbol| {
                    symbol.definition == selected.definition
                        && (params.context.include_declaration || !symbol.is_definition)
                })
                .map(|symbol| Location::new(document.uri.clone(), span_range(&source, symbol.span)))
                .collect()
        };
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
        if let Some(path) = document.uri.to_file_path()
            && let Some(identity) = symbol_identity(path.as_ref(), &source, &selected)
        {
            let project_documents = self.project_documents(&document.uri);
            if project_documents.iter().any(|document| {
                let parsed = parse(&document.source);
                !parsed.has_errors()
                    && analyze_program(&parsed.program)
                        .symbols
                        .iter()
                        .any(|symbol| symbol.is_definition && symbol.name == params.new_name)
            }) {
                return Err(Error::invalid_params(format!(
                    "name '{}' is already declared in this project",
                    params.new_name
                )));
            }
            let mut changes = HashMap::new();
            for location in project_symbol_locations(&project_documents, &identity, true) {
                changes
                    .entry(location.uri)
                    .or_insert_with(Vec::new)
                    .push(TextEdit::new(location.range, params.new_name.clone()));
            }
            return Ok(Some(WorkspaceEdit {
                changes: Some(changes),
                ..WorkspaceEdit::default()
            }));
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

fn load_project_documents(uri: &Uri, overlays: &HashMap<String, String>) -> Vec<ProjectDocument> {
    let Some(path) = uri.to_file_path().map(|path| path.into_owned()) else {
        return Vec::new();
    };
    let Ok(project) = Project::discover(&path) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    collect_project_source_files(&project, &mut HashSet::new(), &mut paths);
    if !paths.iter().any(|candidate| same_path(candidate, &path)) {
        paths.push(path);
    }
    paths
        .into_iter()
        .filter_map(|path| {
            let uri = Uri::from_file_path(&path)?;
            let source =
                overlay_source(overlays, &path).or_else(|| fs::read_to_string(&path).ok())?;
            Some(ProjectDocument { uri, path, source })
        })
        .collect()
}

fn collect_project_source_files(
    project: &Project,
    visited: &mut HashSet<PathBuf>,
    paths: &mut Vec<PathBuf>,
) {
    let root = project
        .root
        .canonicalize()
        .unwrap_or_else(|_| project.root.clone());
    if !visited.insert(root) {
        return;
    }
    if let Ok(mut project_paths) = project.source_files() {
        paths.append(&mut project_paths);
    }
    for name in project.manifest.dependencies.keys() {
        if let Ok(dependency) = project.dependency(name) {
            collect_project_source_files(&dependency, visited, paths);
        }
    }
}

fn overlay_source(overlays: &HashMap<String, String>, path: &Path) -> Option<String> {
    overlays.iter().find_map(|(uri, source)| {
        let uri = uri.parse::<Uri>().ok()?;
        let overlay_path = uri.to_file_path()?;
        same_path(overlay_path.as_ref(), path).then(|| source.clone())
    })
}

fn same_path(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    left == right
}

fn symbol_identity(
    module_path: &Path,
    source: &str,
    selected: &SymbolOccurrence,
) -> Option<(PathBuf, String)> {
    let parsed = parse(source);
    if parsed.has_errors() {
        return None;
    }
    for statement in &parsed.program.statements {
        if let StatementKind::Import {
            names,
            source: import_source,
        } = &statement.kind
            && !import_source.starts_with("std:")
            && let Some(name) = names
                .iter()
                .find(|name| name.span == selected.definition && name.name == selected.name)
        {
            let path = resolve_import(module_path, import_source).ok()?;
            return Some((path, name.name.clone()));
        }
        let StatementKind::Export(declaration) = &statement.kind else {
            continue;
        };
        if declaration_name(declaration)
            .is_some_and(|name| name.span == selected.definition && name.name == selected.name)
        {
            return Some((
                module_path
                    .canonicalize()
                    .unwrap_or_else(|_| module_path.to_path_buf()),
                selected.name.clone(),
            ));
        }
    }
    None
}

fn declaration_name(statement: &Statement) -> Option<&orvi_syntax::Identifier> {
    match &statement.kind {
        StatementKind::Variable { name, .. }
        | StatementKind::Function { name, .. }
        | StatementKind::Struct { name, .. }
        | StatementKind::Interface { name, .. } => Some(name),
        _ => None,
    }
}

fn exported_definition_span(source: &str, name: &str) -> Option<Span> {
    let parsed = parse(source);
    if parsed.has_errors() {
        return None;
    }
    parsed.program.statements.iter().find_map(|statement| {
        let StatementKind::Export(declaration) = &statement.kind else {
            return None;
        };
        declaration_name(declaration)
            .filter(|identifier| identifier.name == name)
            .map(|identifier| identifier.span)
    })
}

fn import_definition_spans(
    module_path: &Path,
    program: &Program,
    identity: &(PathBuf, String),
) -> Vec<Span> {
    program
        .statements
        .iter()
        .filter_map(|statement| {
            let StatementKind::Import { names, source } = &statement.kind else {
                return None;
            };
            if source.starts_with("std:") {
                return None;
            }
            let target = resolve_import(module_path, source).ok()?;
            if !same_path(&target, &identity.0) {
                return None;
            }
            Some(
                names
                    .iter()
                    .filter(|name| name.name == identity.1)
                    .map(|name| name.span)
                    .collect::<Vec<_>>(),
            )
        })
        .flatten()
        .collect()
}

fn project_symbol_locations(
    documents: &[ProjectDocument],
    identity: &(PathBuf, String),
    include_declaration: bool,
) -> Vec<Location> {
    let mut locations = Vec::new();
    for document in documents {
        let parsed = parse(&document.source);
        if parsed.has_errors() {
            continue;
        }
        let analysis = analyze_program(&parsed.program);
        let mut definitions = import_definition_spans(&document.path, &parsed.program, identity);
        if same_path(&document.path, &identity.0)
            && let Some(span) = exported_definition_span(&document.source, &identity.1)
        {
            definitions.push(span);
        }
        locations.extend(
            analysis
                .symbols
                .iter()
                .filter(|symbol| {
                    definitions.contains(&symbol.definition)
                        && (include_declaration || !symbol.is_definition)
                })
                .map(|symbol| {
                    Location::new(
                        document.uri.clone(),
                        span_range(&document.source, symbol.span),
                    )
                }),
        );
    }
    locations
}

fn import_diagnostics(
    module_path: &Path,
    source: &str,
    overlays: &HashMap<String, String>,
) -> Vec<Diagnostic> {
    let parsed = parse(source);
    if parsed.has_errors() {
        return Vec::new();
    }
    let mut result = Vec::new();
    for statement in &parsed.program.statements {
        let StatementKind::Import {
            names,
            source: import_source,
        } = &statement.kind
        else {
            continue;
        };
        if import_source.starts_with("std:") {
            continue;
        }
        let target_path = match resolve_import(module_path, import_source) {
            Ok(path) => path,
            Err(error) => {
                result.push(diagnostic(source, statement.span, error.to_string()));
                continue;
            }
        };
        let target_source = overlay_source(overlays, &target_path)
            .or_else(|| fs::read_to_string(&target_path).ok());
        let Some(target_source) = target_source else {
            result.push(diagnostic(
                source,
                statement.span,
                format!("could not resolve module '{}'", target_path.display()),
            ));
            continue;
        };
        let target = parse(&target_source);
        if target.has_errors() {
            result.push(diagnostic(
                source,
                statement.span,
                format!(
                    "imported module '{}' contains syntax errors",
                    target_path.display()
                ),
            ));
            continue;
        }
        let exports = target
            .program
            .statements
            .iter()
            .filter_map(|statement| {
                let StatementKind::Export(declaration) = &statement.kind else {
                    return None;
                };
                declaration_name(declaration).map(|name| name.name.as_str())
            })
            .collect::<HashSet<_>>();
        for name in names {
            if !exports.contains(name.name.as_str()) {
                result.push(diagnostic(
                    source,
                    name.span,
                    format!(
                        "module '{}' has no export '{}'",
                        target_path.display(),
                        name.name
                    ),
                ));
            }
        }
    }
    result
}

fn import_completion_items(
    module_path: &Path,
    source: &str,
    cursor: Position,
    documents: &[ProjectDocument],
) -> Vec<CompletionItem> {
    let Some(offset) = offset_at_position(source, cursor) else {
        return Vec::new();
    };
    let parsed = parse(source);
    if parsed.has_errors() {
        return Vec::new();
    }
    let Some(import_source) = parsed.program.statements.iter().find_map(|statement| {
        let StatementKind::Import {
            source: import_source,
            ..
        } = &statement.kind
        else {
            return None;
        };
        if statement.span.start() > offset || offset > statement.span.end() {
            return None;
        }
        let prefix = &source_text(source, statement.span)[..offset - statement.span.start()];
        (!prefix.contains(" from ")).then_some(import_source)
    }) else {
        return Vec::new();
    };
    if let Some(module) = import_source.strip_prefix("std:") {
        return standard_module(module)
            .map(|module| {
                module
                    .members
                    .iter()
                    .map(|member| standard_member_completion(member, false))
                    .collect()
            })
            .unwrap_or_default();
    }
    let Ok(target_path) = resolve_import(module_path, import_source) else {
        return Vec::new();
    };
    let Some(target) = documents
        .iter()
        .find(|document| same_path(&document.path, &target_path))
    else {
        return Vec::new();
    };
    let target = parse(&target.source);
    if target.has_errors() {
        return Vec::new();
    }
    target
        .program
        .statements
        .iter()
        .filter_map(|statement| {
            let StatementKind::Export(declaration) = &statement.kind else {
                return None;
            };
            let name = declaration_name(declaration)?;
            let kind = match &declaration.kind {
                StatementKind::Function { .. } => CompletionItemKind::FUNCTION,
                StatementKind::Struct { .. } => CompletionItemKind::STRUCT,
                StatementKind::Interface { .. } => CompletionItemKind::INTERFACE,
                StatementKind::Variable { .. } => CompletionItemKind::VARIABLE,
                _ => return None,
            };
            Some(CompletionItem {
                label: name.name.clone(),
                kind: Some(kind),
                detail: Some(format!("export from {import_source}")),
                ..CompletionItem::default()
            })
        })
        .collect()
}

fn source_text(source: &str, span: Span) -> &str {
    &source[span.start()..span.end()]
}

fn standard_completion_items(source: &str, cursor: Position) -> Option<Vec<CompletionItem>> {
    let offset = offset_at_position(source, cursor)?;
    let prefix = &source[..offset];

    if let Some(from) = prefix.rfind("from \"") {
        let typed = &prefix[from + "from \"".len()..];
        if !typed.contains('"')
            && (typed.is_empty() || "std:".starts_with(typed) || typed.starts_with("std:"))
        {
            return Some(
                STANDARD_MODULES
                    .iter()
                    .map(|module| CompletionItem {
                        label: format!("std:{}", module.name),
                        kind: Some(CompletionItemKind::MODULE),
                        detail: Some(module.description.into()),
                        documentation: Some(Documentation::MarkupContent(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: format!(
                                "`std:{}`\n\n{}\n\nExports: {}",
                                module.name,
                                module.description,
                                module
                                    .members
                                    .iter()
                                    .map(|member| format!("`{}`", member.name))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                        })),
                        insert_text: Some(format!("std:{}", module.name)),
                        sort_text: Some(format!("0_{}", module.name)),
                        ..CompletionItem::default()
                    })
                    .collect(),
            );
        }
    }

    let word_start = prefix
        .char_indices()
        .rev()
        .find(|(_, character)| !character.is_alphanumeric() && *character != '_')
        .map_or(0, |(index, character)| index + character.len_utf8());
    let before_word = &prefix[..word_start];
    let before_dot = before_word.strip_suffix('.')?;
    let module_start = before_dot
        .char_indices()
        .rev()
        .find(|(_, character)| !character.is_alphanumeric() && *character != '_')
        .map_or(0, |(index, character)| index + character.len_utf8());
    let module = standard_module(&before_dot[module_start..])?;
    Some(
        module
            .members
            .iter()
            .map(|member| standard_member_completion(member, true))
            .collect(),
    )
}

fn standard_member_completion(member: &StandardMember, call: bool) -> CompletionItem {
    CompletionItem {
        label: member.name.into(),
        kind: Some(CompletionItemKind::FUNCTION),
        detail: Some(member.signature.into()),
        documentation: Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: member.description.into(),
        })),
        insert_text: Some(if call {
            standard_member_snippet(member)
        } else {
            member.name.into()
        }),
        insert_text_format: call.then_some(InsertTextFormat::SNIPPET),
        sort_text: Some(format!("0_{}", member.name)),
        ..CompletionItem::default()
    }
}

fn standard_member_snippet(member: &StandardMember) -> String {
    let Some(parameters) = member
        .signature
        .split_once('(')
        .and_then(|(_, rest)| rest.split_once(')'))
        .map(|(parameters, _)| parameters)
    else {
        return member.name.into();
    };
    if parameters.is_empty() {
        return format!("{}()", member.name);
    }
    let placeholders = parameters
        .split(", ")
        .enumerate()
        .map(|(index, parameter)| {
            let name = parameter
                .split_once(':')
                .map_or(parameter, |(name, _)| name);
            format!("${{{}:{}}}", index + 1, name)
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{}({placeholders})", member.name)
}

fn standard_hover_at(source: &str, cursor: Position) -> Option<Hover> {
    let offset = offset_at_position(source, cursor)?;
    let token = lex(source)
        .tokens
        .into_iter()
        .find(|token| token.span.start() <= offset && offset < token.span.end())?;
    let member_name = token.text(source)?;
    let direct_module = source[..token.span.start()]
        .strip_suffix('.')
        .and_then(|prefix| {
            let start = prefix
                .char_indices()
                .rev()
                .find(|(_, character)| !character.is_alphanumeric() && *character != '_')
                .map_or(0, |(index, character)| index + character.len_utf8());
            standard_module(&prefix[start..])
        });
    let imported_module = parse(source)
        .program
        .statements
        .into_iter()
        .find_map(|statement| {
            let StatementKind::Import { names, source } = statement.kind else {
                return None;
            };
            names
                .iter()
                .any(|name| name.as_str() == member_name)
                .then(|| source.strip_prefix("std:").and_then(standard_module))
                .flatten()
        });
    let module = direct_module.or(imported_module)?;
    let member = module
        .members
        .iter()
        .find(|member| member.name == member_name)?;
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!(
                "```orviscript\n{}\n```\n\n{}\n\nModule: `std:{}`",
                member.signature, member.description, module.name
            ),
        }),
        range: Some(span_range(source, token.span)),
    })
}

fn project_symbol_description(
    documents: &[ProjectDocument],
    identity: &(PathBuf, String),
) -> Option<String> {
    let target = documents
        .iter()
        .find(|document| same_path(&document.path, &identity.0))?;
    let definition = exported_definition_span(&target.source, &identity.1)?;
    let parsed = parse(&target.source);
    let symbol = analyze_program(&parsed.program)
        .symbols
        .into_iter()
        .find(|symbol| symbol.is_definition && symbol.definition == definition)?;
    let kind = match symbol.kind {
        orvi_sema::SymbolKind::Variable => "let",
        orvi_sema::SymbolKind::Constant => "const",
        orvi_sema::SymbolKind::Function => "fn",
        orvi_sema::SymbolKind::Struct => "struct",
        orvi_sema::SymbolKind::Interface => "interface",
        orvi_sema::SymbolKind::Import => "import",
        orvi_sema::SymbolKind::Task => "task",
        orvi_sema::SymbolKind::Parameter => "parameter",
    };
    Some(format!(
        "{kind} {}: {}",
        symbol.name,
        symbol.type_name.unwrap_or_else(|| "any".into())
    ))
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
    diagnostics_with_modules(source, &HashMap::new())
}

fn diagnostics_with_modules(source: &str, modules: &HashMap<String, Program>) -> Vec<Diagnostic> {
    diagnostics_with_modules_and_settings(source, modules, &DiagnosticSettings::default())
}

fn diagnostics_with_modules_and_settings(
    source: &str,
    modules: &HashMap<String, Program>,
    settings: &DiagnosticSettings,
) -> Vec<Diagnostic> {
    let parsed = parse(source);
    let mut diagnostics = parsed
        .lexical_errors
        .iter()
        .map(|error| diagnostic(source, error.span, error.to_string()))
        .chain(
            parsed
                .parse_errors
                .iter()
                .filter(|error| {
                    !parsed.rule_diagnostics.iter().any(|diagnostic| {
                        settings.allows(diagnostic)
                            && diagnostic.severity == RuleSeverity::Error
                            && diagnostic.span == error.span
                    })
                })
                .map(|error| diagnostic(source, error.span, error.message.clone())),
        )
        .collect::<Vec<_>>();
    diagnostics.extend(
        parsed
            .rule_diagnostics
            .iter()
            .filter(|rule| settings.allows(rule))
            .map(|rule| rule_diagnostic(source, rule)),
    );
    if !parsed.has_errors() {
        diagnostics.extend(
            analyze_program_with_modules(&parsed.program, modules)
                .errors
                .into_iter()
                .map(|error| diagnostic(source, error.span, error.message)),
        );
    }
    diagnostics
}

#[cfg(test)]
fn code_actions(uri: Uri, source: &str, requested: Range) -> CodeActionResponse {
    code_actions_with_settings(uri, source, requested, &DiagnosticSettings::default())
}

fn code_actions_with_settings(
    uri: Uri,
    source: &str,
    requested: Range,
    settings: &DiagnosticSettings,
) -> CodeActionResponse {
    let start = offset_at_position(source, requested.start).unwrap_or(0);
    let end = offset_at_position(source, requested.end).unwrap_or(start);
    parse(source)
        .rule_diagnostics
        .into_iter()
        .filter(|rule| settings.allows(rule))
        .filter_map(|rule| {
            let replacement = rule.replacement?;
            if rule.span.end() < start || end < rule.span.start() {
                return None;
            }
            let original = rule.span.text(source).unwrap_or_default();
            let edit = TextEdit::new(span_range(source, rule.span), replacement.into());
            let mut changes = HashMap::new();
            changes.insert(uri.clone(), vec![edit]);
            Some(CodeActionOrCommand::CodeAction(CodeAction {
                title: format!("Replace '{original}' with '{replacement}'"),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![rule_diagnostic(source, &rule)]),
                edit: Some(WorkspaceEdit {
                    changes: Some(changes),
                    ..WorkspaceEdit::default()
                }),
                is_preferred: Some(true),
                ..CodeAction::default()
            }))
        })
        .collect()
}

fn severity_rank(severity: RuleSeverity) -> u8 {
    match severity {
        RuleSeverity::Error => 1,
        RuleSeverity::Warning => 2,
        RuleSeverity::Information => 3,
        RuleSeverity::Hint => 4,
    }
}

fn imported_programs(
    module_path: &Path,
    source: &str,
    documents: &[ProjectDocument],
) -> HashMap<String, Program> {
    let parsed = parse(source);
    if parsed.has_errors() {
        return HashMap::new();
    }
    parsed
        .program
        .statements
        .iter()
        .filter_map(|statement| {
            let StatementKind::Import {
                source: import_source,
                ..
            } = &statement.kind
            else {
                return None;
            };
            if import_source.starts_with("std:") {
                return None;
            }
            let target_path = resolve_import(module_path, import_source).ok()?;
            let target = documents
                .iter()
                .find(|document| same_path(&document.path, &target_path))?;
            let target = parse(&target.source);
            (!target.has_errors()).then(|| (import_source.clone(), target.program))
        })
        .collect()
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
                    | "Future"
                    | "Task"
                    | "Worker"
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
        TokenKind::Identifier
            if matches!(previous, Some(TokenKind::Struct | TokenKind::Interface)) =>
        {
            (8, DECLARATION)
        }
        TokenKind::Identifier if previous == Some(TokenKind::Dot) => (9, 0),
        TokenKind::Identifier if matches!(previous, Some(TokenKind::Colon | TokenKind::Arrow)) => {
            (8, 0)
        }
        TokenKind::Identifier if matches!(previous, Some(TokenKind::Fn | TokenKind::Task)) => {
            (1, DECLARATION)
        }
        TokenKind::Identifier if in_parameters => (2, DECLARATION),
        TokenKind::Identifier if previous == Some(TokenKind::Let) => (0, DECLARATION),
        TokenKind::Identifier if previous == Some(TokenKind::Const) => (0, DECLARATION | READONLY),
        TokenKind::Identifier if text == "print" => (1, DEFAULT_LIBRARY),
        TokenKind::Identifier if next == Some(TokenKind::LeftParenthesis) => (1, 0),
        TokenKind::Identifier => (0, 0),
        TokenKind::Let
        | TokenKind::Const
        | TokenKind::Fn
        | TokenKind::Async
        | TokenKind::Await
        | TokenKind::Struct
        | TokenKind::Interface
        | TokenKind::Import
        | TokenKind::Export
        | TokenKind::From
        | TokenKind::Task
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
        (
            "async",
            CompletionItemKind::KEYWORD,
            "asynchronous function declaration",
        ),
        (
            "await",
            CompletionItemKind::KEYWORD,
            "wait for a Future<T> result",
        ),
        ("struct", CompletionItemKind::KEYWORD, "struct declaration"),
        (
            "interface",
            CompletionItemKind::KEYWORD,
            "structural interface declaration",
        ),
        ("import", CompletionItemKind::KEYWORD, "named module import"),
        (
            "export",
            CompletionItemKind::KEYWORD,
            "public module declaration",
        ),
        ("from", CompletionItemKind::KEYWORD, "module import source"),
        (
            "task",
            CompletionItemKind::KEYWORD,
            "task declaration and dependency result module",
        ),
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
        (
            "void",
            CompletionItemKind::TYPE_PARAMETER,
            "absence of a value",
        ),
        ("list", CompletionItemKind::TYPE_PARAMETER, "list type"),
        ("map", CompletionItemKind::TYPE_PARAMETER, "map type"),
        (
            "Future",
            CompletionItemKind::TYPE_PARAMETER,
            "lazy asynchronous result type",
        ),
        (
            "Task",
            CompletionItemKind::TYPE_PARAMETER,
            "scoped asynchronous task handle",
        ),
        (
            "Worker",
            CompletionItemKind::TYPE_PARAMETER,
            "isolated parallel CPU worker handle",
        ),
        ("context", CompletionItemKind::VARIABLE, "execution context"),
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
    for module in STANDARD_MODULES {
        if items.iter().any(|item| item.label == module.name) {
            continue;
        }
        items.push(CompletionItem {
            label: module.name.into(),
            kind: Some(CompletionItemKind::MODULE),
            detail: Some(format!("std:{}", module.name)),
            documentation: Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: module.description.into(),
            })),
            ..CompletionItem::default()
        });
    }
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
                    orvi_sema::SymbolKind::Function | orvi_sema::SymbolKind::Task => {
                        CompletionItemKind::FUNCTION
                    }
                    orvi_sema::SymbolKind::Struct => CompletionItemKind::STRUCT,
                    orvi_sema::SymbolKind::Interface => CompletionItemKind::INTERFACE,
                    orvi_sema::SymbolKind::Import => CompletionItemKind::VARIABLE,
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
        "async" => "`async fn` declares a function whose call returns a lazy `Future<T>`.",
        "await" => "`await` executes a `Future<T>` and produces its `T` result.",
        "Future" => "`Future<T>` is the lazy result type returned by an `async fn`.",
        "Task" => "`Task<T>` is a scoped handle returned by `task.spawn(Future<T>)`.",
        "Worker" => {
            "`Worker<T>` is an isolated CPU worker handle returned by `worker.spawn(source, input)`."
        }
        "struct" => "`struct` declares a stable set of typed fields.",
        "interface" => "`interface` declares a structural field contract.",
        "import" => "`import { name } from \"module\"` imports selected names.",
        "export" => "`export` makes a top-level declaration available to importers.",
        "from" => "`from` introduces an import source.",
        "task" => "`task name -> Result<T, E> { ... }` declares an inert workflow task.",
        "match" => "`match` selects an expression using exhaustive patterns.",
        "using" => "`using name = value => cleanup` registers scoped cleanup.",
        "defer" => "`defer expression` runs the expression when the scope exits.",
        "if" => "`if` executes a branch when its condition is `true`.",
        "else" => "`else` provides an alternative branch.",
        "while" => "`while` repeats a block while its condition is `true`.",
        "return" => "`return` exits the current function.",
        "true" | "false" => "Built-in boolean value.",
        "null" => "Built-in null value.",
        "void" => "`void` is the absence-of-value return type and is distinct from `null`.",
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
                orvi_sema::SymbolKind::Interface => "interface",
                orvi_sema::SymbolKind::Import => "import",
                orvi_sema::SymbolKind::Task => "task",
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

#[allow(deprecated)]
fn document_symbols(source: &str) -> Vec<DocumentSymbol> {
    let parsed = parse(source);
    if parsed.has_errors() {
        return Vec::new();
    }
    analyze_program(&parsed.program)
        .symbols
        .into_iter()
        .filter(|symbol| symbol.is_definition)
        .map(|symbol| {
            let kind = match symbol.kind {
                orvi_sema::SymbolKind::Function | orvi_sema::SymbolKind::Task => {
                    LspSymbolKind::FUNCTION
                }
                orvi_sema::SymbolKind::Struct => LspSymbolKind::STRUCT,
                orvi_sema::SymbolKind::Interface => LspSymbolKind::INTERFACE,
                orvi_sema::SymbolKind::Import
                | orvi_sema::SymbolKind::Variable
                | orvi_sema::SymbolKind::Constant
                | orvi_sema::SymbolKind::Parameter => LspSymbolKind::VARIABLE,
            };
            let range = span_range(source, symbol.span);
            DocumentSymbol {
                name: symbol.name,
                detail: symbol.type_name,
                kind,
                tags: None,
                deprecated: None,
                range,
                selection_range: range,
                children: None,
            }
        })
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

fn rule_diagnostic(source: &str, rule: &RuleDiagnostic) -> Diagnostic {
    let severity = match rule.severity {
        RuleSeverity::Error => DiagnosticSeverity::ERROR,
        RuleSeverity::Warning => DiagnosticSeverity::WARNING,
        RuleSeverity::Information => DiagnosticSeverity::INFORMATION,
        RuleSeverity::Hint => DiagnosticSeverity::HINT,
    };
    Diagnostic {
        range: span_range(source, rule.span),
        severity: Some(severity),
        code: Some(NumberOrString::String(rule.id.into())),
        source: Some("orviscript".into()),
        message: rule.message.into(),
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
        DiagnosticSettings, code_actions, code_actions_with_settings, completion_items,
        diagnostics, diagnostics_with_modules_and_settings, document_symbols, format_document,
        hover_at, import_completion_items, import_diagnostics, is_identifier,
        load_project_documents, position, project_symbol_description, project_symbol_locations,
        same_path, semantic_tokens, span_range, standard_completion_items, standard_hover_at,
        symbol_at, symbol_identity,
    };
    use orvi_syntax::Span;
    use std::collections::HashMap;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tower_lsp_server::ls_types::{
        CodeActionOrCommand, CompletionItemKind, DiagnosticSeverity, HoverContents,
        InsertTextFormat, NumberOrString, Position, Range, Uri,
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
    fn reports_foreign_syntax_and_builds_a_quick_fix() {
        let source = "function greet() {}";
        let diagnostics = diagnostics(source);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].code,
            Some(NumberOrString::String("ORVI1001".into()))
        );
        assert_eq!(
            diagnostics[0].range,
            Range::new(Position::new(0, 0), Position::new(0, 8))
        );

        let uri = "file:///foreign.orvi".parse::<Uri>().unwrap();
        let actions = code_actions(
            uri.clone(),
            source,
            Range::new(Position::new(0, 0), Position::new(0, 8)),
        );
        let CodeActionOrCommand::CodeAction(action) = &actions[0] else {
            panic!("expected a code action");
        };
        assert_eq!(action.title, "Replace 'function' with 'fn'");
        let edit = &action.edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri][0];
        assert_eq!(edit.new_text, "fn");
        assert_eq!(edit.range, diagnostics[0].range);
    }

    #[test]
    fn filters_rule_diagnostics_and_actions_by_lsp_settings() {
        let source = "function greet() {}";
        let settings = DiagnosticSettings::from_value(&serde_json::json!({
            "diagnostics": {
                "enabledCategories": ["another_category"],
                "minimumSeverity": "hint"
            }
        }));
        let diagnostics = diagnostics_with_modules_and_settings(source, &HashMap::new(), &settings);
        assert!(diagnostics.is_empty());

        let uri = "file:///foreign.orvi".parse::<Uri>().unwrap();
        let actions = code_actions_with_settings(
            uri,
            source,
            Range::new(Position::new(0, 0), Position::new(0, 8)),
            &settings,
        );
        assert!(actions.is_empty());
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
        assert!(items.iter().any(|item| item.label == "interface"));
        assert!(items.iter().any(|item| item.label == "task"));
        assert!(items.iter().any(|item| item.label == "export"));
        assert!(items.iter().any(|item| item.label == "void"));
        assert!(items.iter().any(|item| item.label == "async"));
        assert!(items.iter().any(|item| item.label == "await"));
        assert!(items.iter().any(|item| item.label == "Future"));
        assert!(items.iter().any(|item| item.label == "Task"));
        assert!(items.iter().any(|item| item.label == "Worker"));
        assert!(items.iter().any(|item| item.label == "http"));
        assert!(items.iter().any(|item| item.label == "time"));
        assert!(items.iter().any(|item| item.label == "worker"));
        assert_eq!(items.iter().filter(|item| item.label == "task").count(), 1);
        assert!(!items.iter().any(|item| item.label == "read_text"));
        assert!(!items.iter().any(|item| item.label == "sleep_async"));
    }

    #[test]
    fn completes_standard_modules_and_only_the_selected_module_members() {
        let module_source = "import { value } from \"std:f";
        let modules =
            standard_completion_items(module_source, Position::new(0, module_source.len() as u32))
                .expect("standard module completion");
        assert!(modules.iter().any(|item| item.label == "std:fs"));
        assert!(modules.iter().any(|item| item.label == "std:http"));

        let import_source = "import { read } from \"std:fs\";";
        let members = import_completion_items(
            std::path::Path::new("main.orvi"),
            import_source,
            Position::new(0, 12),
            &[],
        );
        let read_text = members
            .iter()
            .find(|item| item.label == "read_text")
            .expect("filesystem member completion");
        assert!(
            read_text
                .detail
                .as_deref()
                .unwrap()
                .contains("Result<string")
        );
        assert!(!members.iter().any(|item| item.label == "sleep_async"));
    }

    #[test]
    fn completes_standard_module_calls_with_argument_snippets() {
        let source = "fs.re";
        let members = standard_completion_items(source, Position::new(0, source.len() as u32))
            .expect("member completion");
        let read_text = members
            .iter()
            .find(|item| item.label == "read_text")
            .expect("read_text completion");
        assert_eq!(
            read_text.insert_text.as_deref(),
            Some("read_text(${1:path})")
        );
        assert_eq!(
            read_text.insert_text_format,
            Some(InsertTextFormat::SNIPPET)
        );
    }

    #[test]
    fn reports_task_diagnostics_and_document_symbols() {
        let source = "task build -> Result<null, string> { return Ok(null) } task deploy -> Result<null, string> { needs: [missing]; return Ok(null) }";
        let diagnostics = diagnostics(source);
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("undefined task dependency"))
        );
        let symbols = document_symbols(source);
        assert!(symbols.iter().any(|symbol| symbol.name == "build"));
        assert!(symbols.iter().any(|symbol| symbol.name == "deploy"));
    }

    #[test]
    fn explains_standard_api_on_hover() {
        let source = "fs.read_text(\"file\")";
        let hover = standard_hover_at(source, Position::new(0, 5)).expect("hover result");
        let HoverContents::Markup(contents) = hover.contents else {
            panic!("expected markdown hover");
        };
        assert!(contents.value.contains("Result<string, string>"));
        assert!(contents.value.contains("std:fs"));

        let imported = "import { read_text } from \"std:fs\"; read_text(\"file\")";
        let use_column = imported.rfind("read_text").unwrap() as u32 + 2;
        let hover =
            standard_hover_at(imported, Position::new(0, use_column)).expect("import hover");
        let HoverContents::Markup(contents) = hover.contents else {
            panic!("expected markdown hover");
        };
        assert!(contents.value.contains("Reads a UTF-8 text file"));
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
    fn explains_async_language_items_on_hover() {
        for (source, column, expected) in [
            ("async fn work() {}", 1, "Future<T>"),
            ("await work()", 1, "executes"),
            ("let work: Future<int>", 11, "lazy result type"),
        ] {
            let hover = hover_at(source, Position::new(0, column)).expect("hover result");
            let HoverContents::Markup(contents) = hover.contents else {
                panic!("expected markdown hover");
            };
            assert!(contents.value.contains(expected), "{}", contents.value);
        }
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

    #[test]
    fn indexes_definitions_and_references_across_project_modules() {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("orvi-lsp-project-{id}"));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("orvi.toml"),
            "[project]\nname = \"demo\"\nversion = \"0.1.0\"\nentry = \"src/main.orvi\"\n",
        )
        .unwrap();
        let module = "export fn greet(name: string) -> string { return \"Hi \" + name; }";
        let entry = "import { greet } from \"./greet.orvi\"; print(greet(\"Ada\"));";
        fs::write(root.join("src/greet.orvi"), module).unwrap();
        fs::write(root.join("src/main.orvi"), entry).unwrap();

        let entry_uri = Uri::from_file_path(root.join("src/main.orvi")).unwrap();
        let documents = load_project_documents(&entry_uri, &HashMap::new());
        assert_eq!(documents.len(), 2);
        let use_offset = entry.rfind("greet").unwrap() + 1;
        let (_, selected) = symbol_at(entry, position(entry, use_offset)).unwrap();
        let identity = symbol_identity(&root.join("src/main.orvi"), entry, &selected).unwrap();
        assert_eq!(identity.1, "greet");
        let locations = project_symbol_locations(&documents, &identity, true);
        assert_eq!(locations.len(), 3);
        assert_eq!(
            project_symbol_locations(&documents, &identity, false).len(),
            1
        );
        let completions = import_completion_items(
            &root.join("src/main.orvi"),
            entry,
            Position::new(0, 12),
            &documents,
        );
        assert!(completions.iter().any(|item| item.label == "greet"));
        assert!(
            project_symbol_description(&documents, &identity)
                .unwrap()
                .contains("fn greet")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_missing_files_and_private_imports() {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("orvi-lsp-imports-{id}"));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("module.orvi"), "const hidden = 1;").unwrap();

        let source = "import { hidden } from \"./module.orvi\";";
        let errors = import_diagnostics(&root.join("main.orvi"), source, &HashMap::new());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("has no export 'hidden'"));

        let missing = import_diagnostics(
            &root.join("main.orvi"),
            "import { value } from \"./missing.orvi\";",
            &HashMap::new(),
        );
        assert_eq!(missing.len(), 1);
        assert!(missing[0].message.contains("could not resolve module"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn indexes_path_package_exports() {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("orvi-lsp-package-{id}"));
        let app = base.join("app");
        let package = base.join("math");
        fs::create_dir_all(app.join("src")).unwrap();
        fs::create_dir_all(package.join("src")).unwrap();
        fs::write(
            app.join("orvi.toml"),
            "[project]\nname = \"app\"\nversion = \"0.1.0\"\nentry = \"src/main.orvi\"\n\n[dependencies]\nmath = { path = \"../math\" }\n",
        )
        .unwrap();
        fs::write(
            package.join("orvi.toml"),
            "[project]\nname = \"math\"\nversion = \"1.0.0\"\nentry = \"src/main.orvi\"\n",
        )
        .unwrap();
        let module = "export fn double(value: int) -> int { return value * 2 }";
        let entry = "import { double } from \"pkg:math\"; print(double(21))";
        fs::write(package.join("src/main.orvi"), module).unwrap();
        fs::write(app.join("src/main.orvi"), entry).unwrap();

        let entry_uri = Uri::from_file_path(app.join("src/main.orvi")).unwrap();
        let documents = load_project_documents(&entry_uri, &HashMap::new());
        assert_eq!(documents.len(), 2);
        assert!(import_diagnostics(&app.join("src/main.orvi"), entry, &HashMap::new()).is_empty());
        let completions = import_completion_items(
            &app.join("src/main.orvi"),
            entry,
            Position::new(0, 12),
            &documents,
        );
        assert!(completions.iter().any(|item| item.label == "double"));
        let use_offset = entry.rfind("double").unwrap() + 1;
        let (_, selected) = symbol_at(entry, position(entry, use_offset)).unwrap();
        let identity = symbol_identity(&app.join("src/main.orvi"), entry, &selected).unwrap();
        assert!(same_path(&identity.0, &package.join("src/main.orvi")));

        fs::remove_dir_all(base).unwrap();
    }
}
