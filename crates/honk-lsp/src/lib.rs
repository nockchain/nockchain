use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TryRecvError, TrySendError};
use honk::pipeline::{self, NativeImportKind, NativeSourceView, ResolvedNativeImport, ScopeMode};
use honk::workspace::{
    WorkspaceCheckRequest, WorkspaceCompileError, WorkspaceConfig, WorkspaceDiagnostic,
    WorkspaceDiagnosticKind, WorkspaceSourceSnapshot,
};
use honk::{CompilerErrorLocation, CompilerResolutionFact, CompilerSemanticFact};
use honk_service::semantic::{
    completion_term_range, hoon_rune_at, hoon_term_at, range_from_one_based_spot,
    structural_completions, structural_declaration_ranges, structural_definition,
    structural_rune_definition, structural_symbols, validate_rename_name, SemanticCompletion,
    SemanticCompletionKind, SemanticHover, SemanticNodeId, SemanticRename, SemanticRenameEdit,
    SemanticRenameError, SemanticRenameTarget, SemanticSession, SemanticStructuralSymbol,
    SemanticSymbol, SemanticSymbolKind, SemanticTextRange,
};
use honk_service::{CompilerHandle, CompilerService, CompilerServiceConfig, DocumentUpdate};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    Cancel as CancelNotification, DidChangeTextDocument, DidChangeWatchedFiles,
    DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument, Exit, LogMessage,
    Notification as LspNotification, PublishDiagnostics, ShowMessage,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest, PrepareRenameRequest,
    References, Rename, Request as LspRequest, WorkspaceSymbolRequest,
};
use lsp_types::{
    CancelParams, CompletionItem, CompletionItemKind, CompletionList, CompletionOptions,
    CompletionParams, CompletionResponse, CompletionTextEdit, Diagnostic, DiagnosticSeverity,
    DidChangeTextDocumentParams, DidChangeWatchedFilesParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentChanges, DocumentSymbol,
    DocumentSymbolParams, DocumentSymbolResponse, FileChangeType, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverContents, HoverParams, HoverProviderCapability,
    InitializeParams, InitializeResult, Location, LogMessageParams, MarkupContent, MarkupKind,
    MessageType, NumberOrString, OneOf, OptionalVersionedTextDocumentIdentifier, Position,
    PositionEncodingKind, PrepareRenameResponse, PublishDiagnosticsParams, Range, ReferenceParams,
    RenameOptions, RenameParams, ServerCapabilities, ServerInfo, ShowMessageParams, SymbolKind,
    TextDocumentEdit, TextDocumentPositionParams, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit, Uri, WorkspaceEdit,
    WorkspaceSymbol, WorkspaceSymbolOptions, WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use serde::Deserialize;
use tracing::{debug, error, info, warn};
use walkdir::{DirEntry, WalkDir};

pub const SERVER_NAME: &str = "honk-lsp";
pub const LSP_BASELINE_VERSION: &str = "3.17";

#[derive(Clone, Debug)]
pub struct LspConfig {
    pub prelude: Option<PathBuf>,
    pub dependencies: Option<PathBuf>,
    pub entry: Option<PathBuf>,
    pub subject_type_jam: Option<PathBuf>,
    pub dbug: bool,
    pub vet: bool,
    pub max_compiles: u64,
    pub worker_stack_bytes: usize,
    pub check_delay_ms: u64,
}

#[derive(Clone, Debug)]
struct ResolvedConfig {
    workspace: WorkspaceConfig,
    workspace_files: HashSet<PathBuf>,
    entry: Option<PathBuf>,
    max_compiles: u64,
    worker_stack_bytes: usize,
    check_delay: Duration,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitializationOptions {
    prelude: Option<PathBuf>,
    dependencies: Option<PathBuf>,
    entry: Option<PathBuf>,
    check_delay_ms: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OpenDocument {
    uri: Uri,
    version: i32,
    text: String,
}

#[derive(Clone, Debug, Default)]
struct EditorSnapshot {
    generation: u64,
    layout_revision: u64,
    active: Option<PathBuf>,
    documents: HashMap<PathBuf, OpenDocument>,
    path_revisions: HashMap<PathBuf, u64>,
    source_root: PathBuf,
    workspace_files: HashSet<PathBuf>,
}

struct SemanticWorkspace {
    dependencies: PathBuf,
    prelude: PathBuf,
    roots: Vec<PathBuf>,
    sources: WorkspaceSourceSnapshot,
    versions: HashMap<PathBuf, i32>,
    path_revisions: HashMap<PathBuf, u64>,
    layout_revision: u64,
}

struct SemanticDefinition {
    path: PathBuf,
    source: Arc<str>,
    range: SemanticTextRange,
}

struct SemanticReference {
    path: PathBuf,
    source: Arc<str>,
    range: SemanticTextRange,
}

struct SemanticDocumentEdits {
    path: PathBuf,
    source: Arc<str>,
    version: Option<i32>,
    edits: Vec<SemanticRenameEdit>,
}

struct SemanticCompletionResult {
    replacement_range: SemanticTextRange,
    candidates: Vec<RankedCompletion>,
}

struct SemanticWorkspaceSymbol {
    path: PathBuf,
    source: Arc<str>,
    name: String,
    kind: SemanticSymbolKind,
    range: SemanticTextRange,
}

struct RankedCompletion {
    completion: SemanticCompletion,
    rank: u16,
}

impl EditorSnapshot {
    fn target(&self, configured_entry: Option<&Path>) -> Option<PathBuf> {
        configured_entry
            .map(Path::to_path_buf)
            .or_else(|| self.active.clone())
            .or_else(|| self.documents.keys().min().cloned())
    }

    fn mark_path_changed(&mut self, path: PathBuf) {
        self.path_revisions.insert(path, self.generation);
    }
}

enum WorkerEvent {
    Checked {
        generation: u64,
        target: PathBuf,
        diagnostic: Option<WorkspaceDiagnostic>,
        semantic_facts: Vec<CompilerSemanticFact>,
        resolution_facts: Vec<CompilerResolutionFact>,
    },
    Error {
        generation: u64,
        message: String,
    },
}

enum SemanticCommand {
    Query(SemanticJob),
    Close(PathBuf),
    Stop,
}

struct SemanticJob {
    id: RequestId,
    path: PathBuf,
    version: i32,
    source: Arc<str>,
    query: SemanticQuery,
    cancelled: Arc<AtomicBool>,
}

enum SemanticQuery {
    DocumentSymbols,
    WorkspaceSymbols {
        query: String,
        workspace: Arc<SemanticWorkspace>,
    },
    Hover {
        byte_offset: u32,
    },
    Definition {
        byte_offset: u32,
        workspace: Arc<SemanticWorkspace>,
    },
    Completion {
        byte_offset: u32,
        workspace: Arc<SemanticWorkspace>,
    },
    References {
        byte_offset: u32,
        include_declaration: bool,
        workspace: Arc<SemanticWorkspace>,
    },
    PrepareRename {
        byte_offset: u32,
        workspace: Arc<SemanticWorkspace>,
    },
    Rename {
        byte_offset: u32,
        new_name: String,
        workspace: Arc<SemanticWorkspace>,
    },
}

enum SemanticQueryResult {
    DocumentSymbols(Vec<SemanticSymbol>),
    WorkspaceSymbols(Vec<SemanticWorkspaceSymbol>),
    Hover(Option<SemanticHover>),
    Definition(Option<SemanticDefinition>),
    Completion(SemanticCompletionResult),
    References(Option<Vec<SemanticReference>>),
    PrepareRename(Option<SemanticRenameTarget>),
    Rename(Option<Vec<SemanticDocumentEdits>>),
    RequestError { code: ErrorCode, message: String },
    Unavailable(String),
}

struct SemanticEvent {
    id: RequestId,
    path: PathBuf,
    version: i32,
    result: SemanticQueryResult,
}

struct PendingSemanticRequest {
    path: PathBuf,
    version: i32,
    source: Arc<str>,
    cancelled: Arc<AtomicBool>,
    hover_offset: Option<u32>,
    workspace_generation: Option<u64>,
    document_bound: bool,
}

struct DocumentTypeFacts {
    version: i32,
    facts: Vec<CompilerSemanticFact>,
}

struct DocumentResolutionFacts {
    version: i32,
    target: PathBuf,
    facts: Vec<CompilerResolutionFact>,
}

struct WorkspaceResolutionFacts {
    generation: u64,
    target: PathBuf,
    prelude: PathBuf,
    facts: Arc<[CompilerResolutionFact]>,
    by_name: HashMap<String, Vec<usize>>,
    by_definition: HashMap<CompilerDefinitionIdentity, Vec<usize>>,
}

impl WorkspaceResolutionFacts {
    fn new(
        generation: u64,
        target: PathBuf,
        dependencies: &Path,
        prelude: &Path,
        facts: Arc<[CompilerResolutionFact]>,
    ) -> Self {
        let mut by_name = HashMap::<String, Vec<usize>>::new();
        let mut by_definition = HashMap::<CompilerDefinitionIdentity, Vec<usize>>::new();
        for (index, fact) in facts.iter().enumerate() {
            by_name.entry(fact.name.clone()).or_default().push(index);
            by_definition
                .entry(compiler_definition_identity(fact, &target, dependencies))
                .or_default()
                .push(index);
        }
        Self {
            generation,
            target,
            prelude: prelude.to_path_buf(),
            facts,
            by_name,
            by_definition,
        }
    }
}

#[derive(Default)]
struct SemanticState {
    pending: HashMap<RequestId, PendingSemanticRequest>,
    type_facts: HashMap<PathBuf, DocumentTypeFacts>,
    resolution_facts: HashMap<PathBuf, DocumentResolutionFacts>,
    workspace_resolution: Option<WorkspaceResolutionFacts>,
}

impl SemanticState {
    fn clear_resolution_facts(&mut self) {
        self.resolution_facts.clear();
        self.workspace_resolution = None;
    }
}

pub fn run_stdio(config: LspConfig) -> Result<()> {
    let (connection, io_threads) = Connection::stdio();
    run_connection(connection, config)?;
    io_threads
        .join()
        .context("failed to join LSP stdio threads")
}

pub fn run_connection(connection: Connection, config: LspConfig) -> Result<()> {
    let (initialize_id, initialize_value) = connection
        .initialize_start()
        .context("LSP initialize handshake failed")?;
    let initialize: InitializeParams = match serde_json::from_value(initialize_value) {
        Ok(initialize) => initialize,
        Err(error) => {
            connection.sender.send(
                Response::new_err(
                    initialize_id,
                    ErrorCode::InvalidParams as i32,
                    format!("invalid LSP initialize parameters: {error}"),
                )
                .into(),
            )?;
            return Err(error).context("invalid LSP initialize parameters");
        }
    };
    let resolved = match resolve_config(config, &initialize) {
        Ok(resolved) => resolved,
        Err(error) => {
            connection.sender.send(
                Response::new_err(
                    initialize_id,
                    ErrorCode::InvalidParams as i32,
                    format!("invalid honk initialization: {error:#}"),
                )
                .into(),
            )?;
            return Err(error);
        }
    };

    let capabilities = ServerCapabilities {
        position_encoding: Some(PositionEncodingKind::UTF16),
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::FULL),
                save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                ..TextDocumentSyncOptions::default()
            },
        )),
        document_symbol_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Right(WorkspaceSymbolOptions {
            resolve_provider: Some(false),
            work_done_progress_options: Default::default(),
        })),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        completion_provider: Some(CompletionOptions::default()),
        references_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        })),
        experimental: Some(serde_json::json!({
            "honk": {
                "lspBaseline": LSP_BASELINE_VERSION,
                "artifactFreeChecks": true,
                "fullDocumentSync": true,
                "semanticSnapshots": true,
                "semanticWorker": true,
                "compilerResolutionFacts": true
            }
        })),
        ..ServerCapabilities::default()
    };
    connection
        .initialize_finish(
            initialize_id,
            serde_json::to_value(InitializeResult {
                capabilities,
                server_info: Some(ServerInfo {
                    name: SERVER_NAME.to_string(),
                    version: Some(env!("CARGO_PKG_VERSION").to_string()),
                }),
            })?,
        )
        .context("LSP initialized handshake failed")?;

    info!(
        prelude = %resolved.workspace.prelude.display(),
        dependencies = %resolved.workspace.dependencies.display(),
        entry = ?resolved.entry,
        workspace_files = resolved.workspace_files.len(),
        "honk language server initialized"
    );

    let state = Arc::new(Mutex::new(EditorSnapshot {
        source_root: resolved.workspace.dependencies.clone(),
        workspace_files: resolved.workspace_files.clone(),
        ..EditorSnapshot::default()
    }));
    let (trigger_sender, trigger_receiver) = crossbeam_channel::bounded(1);
    let (event_sender, event_receiver) = crossbeam_channel::unbounded();
    let stopping = Arc::new(AtomicBool::new(false));
    let check_worker = spawn_check_worker(
        resolved.clone(),
        Arc::clone(&state),
        trigger_receiver,
        event_sender,
        Arc::clone(&stopping),
    )?;
    let (semantic_sender, semantic_receiver) = crossbeam_channel::unbounded();
    let (semantic_event_sender, semantic_event_receiver) = crossbeam_channel::unbounded();
    let semantic_worker = spawn_semantic_worker(
        semantic_receiver, semantic_event_sender, resolved.worker_stack_bytes,
    )?;

    let mut published = HashSet::<String>::new();
    let mut semantics = SemanticState::default();
    let mut shutdown = false;
    while !shutdown {
        drain_worker_events(
            &connection, &state, &resolved, &event_receiver, &mut published, &mut semantics,
        )?;
        drain_semantic_events(
            &connection, &state, &semantic_event_receiver, &mut semantics.pending,
            &semantics.type_facts,
        )?;
        match connection.receiver.recv_timeout(Duration::from_millis(25)) {
            Ok(Message::Request(request)) => {
                if connection.handle_shutdown(&request)? {
                    shutdown = true;
                } else {
                    handle_request(
                        &connection, &state, &resolved, &semantic_sender, &mut semantics, request,
                    )?;
                }
            }
            Ok(Message::Notification(notification)) => {
                if notification.method == Exit::METHOD {
                    shutdown = true;
                } else {
                    handle_notification(
                        &connection, &state, &trigger_sender, notification, &mut published,
                        &semantic_sender, &mut semantics,
                    )?;
                }
            }
            Ok(Message::Response(response)) => {
                debug!(?response, "ignoring unexpected client response");
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    cancel_all_semantic_requests(
        &connection,
        &mut semantics.pending,
        ErrorCode::ServerCancelled,
        "honk language server is shutting down",
    )?;
    let _ = semantic_sender.send(SemanticCommand::Stop);
    stopping.store(true, Ordering::Release);
    schedule_check(&trigger_sender);
    let check_result = check_worker.join().map_err(|payload| {
        anyhow!(
            "honk LSP check worker panicked: {}",
            panic_message(payload.as_ref())
        )
    });
    let semantic_result = semantic_worker.join().map_err(|payload| {
        anyhow!(
            "honk LSP semantic worker panicked: {}",
            panic_message(payload.as_ref())
        )
    });
    check_result?;
    semantic_result?;
    Ok(())
}

fn resolve_config(config: LspConfig, initialize: &InitializeParams) -> Result<ResolvedConfig> {
    let root = workspace_root(initialize)?;
    let initialization_options = initialize
        .initialization_options
        .clone()
        .map(serde_json::from_value::<InitializationOptions>)
        .transpose()
        .context("invalid honk initializationOptions")?
        .unwrap_or_default();

    let dependencies = config
        .dependencies
        .or(initialization_options.dependencies)
        .map(|path| resolve_path(&root, path))
        .unwrap_or_else(|| {
            let hoon = root.join("hoon");
            if hoon.is_dir() {
                hoon
            } else {
                root.clone()
            }
        });
    let prelude = config
        .prelude
        .or(initialization_options.prelude)
        .map(|path| resolve_path(&root, path))
        .unwrap_or_else(|| dependencies.join("common/hoon.hoon"));
    let entry = config
        .entry
        .or(initialization_options.entry)
        .map(|path| resolve_path(&root, path));
    let subject_type_jam = config
        .subject_type_jam
        .map(|path| resolve_path(&root, path));
    let workspace_files = discover_workspace_files(&dependencies)?;

    Ok(ResolvedConfig {
        workspace: WorkspaceConfig {
            prelude,
            dependencies,
            subject_type_jam,
            dbug: config.dbug,
            vet: config.vet,
        },
        workspace_files,
        entry,
        max_compiles: config.max_compiles,
        worker_stack_bytes: config.worker_stack_bytes,
        check_delay: Duration::from_millis(
            initialization_options
                .check_delay_ms
                .unwrap_or(config.check_delay_ms),
        ),
    })
}

fn discover_workspace_files(dependencies: &Path) -> Result<HashSet<PathBuf>> {
    let mut files = HashSet::new();
    if !dependencies.is_dir() {
        return Ok(files);
    }
    for entry in WalkDir::new(dependencies)
        .follow_links(false)
        .into_iter()
        .filter_entry(include_workspace_entry)
    {
        let entry = entry.with_context(|| {
            format!(
                "failed to enumerate Hoon workspace sources under {}",
                dependencies.display()
            )
        })?;
        if entry.path().is_file() && is_hoon_source(entry.path()) {
            files.insert(entry.into_path());
        }
    }
    Ok(files)
}

fn include_workspace_entry(entry: &DirEntry) -> bool {
    if entry.depth() == 0 || !entry.file_type().is_dir() {
        return true;
    }
    !matches!(
        entry.file_name().to_str(),
        Some(".git" | ".direnv" | "node_modules" | "target")
    )
}

fn is_hoon_source(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "hoon")
}

fn workspace_root(initialize: &InitializeParams) -> Result<PathBuf> {
    if let Some(folder) = initialize
        .workspace_folders
        .as_ref()
        .and_then(|folders| folders.first())
    {
        return uri_to_file_path(&folder.uri);
    }
    #[allow(deprecated)]
    if let Some(uri) = &initialize.root_uri {
        return uri_to_file_path(uri);
    }
    std::env::current_dir().context("LSP client supplied no workspace root")
}

fn resolve_path(root: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

fn handle_notification(
    connection: &Connection,
    state: &Arc<Mutex<EditorSnapshot>>,
    trigger: &Sender<()>,
    notification: Notification,
    published: &mut HashSet<String>,
    semantic_sender: &Sender<SemanticCommand>,
    semantics: &mut SemanticState,
) -> Result<()> {
    match notification.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let params: DidOpenTextDocumentParams = parse_notification(notification)?;
            let path = uri_to_file_path(&params.text_document.uri)?;
            let mut snapshot = lock_snapshot(state)?;
            snapshot.generation = snapshot.generation.saturating_add(1);
            snapshot.mark_path_changed(path.clone());
            snapshot.active = Some(path.clone());
            snapshot.documents.insert(
                path.clone(),
                OpenDocument {
                    uri: params.text_document.uri,
                    version: params.text_document.version,
                    text: params.text_document.text,
                },
            );
            drop(snapshot);
            cancel_semantic_requests_for_path(
                connection,
                &mut semantics.pending,
                &path,
                ErrorCode::ServerCancelled,
                "document was reopened with new contents",
            )?;
            semantics.type_facts.remove(&path);
            semantics.clear_resolution_facts();
            schedule_check(trigger);
        }
        DidChangeTextDocument::METHOD => {
            let params: DidChangeTextDocumentParams = parse_notification(notification)?;
            let path = uri_to_file_path(&params.text_document.uri)?;
            let Some(change) = params.content_changes.last() else {
                warn!(uri = %params.text_document.uri.as_str(), "ignoring empty didChange");
                return Ok(());
            };
            if change.range.is_some() {
                send_log(
                    connection,
                    MessageType::ERROR,
                    "honk-lsp negotiated full document synchronization but received a ranged edit",
                )?;
                return Ok(());
            }
            let mut snapshot = lock_snapshot(state)?;
            if let Some(current) = snapshot.documents.get(&path) {
                if params.text_document.version <= current.version {
                    warn!(
                        uri = %params.text_document.uri.as_str(),
                        received = params.text_document.version,
                        current = current.version,
                        "ignoring stale didChange"
                    );
                    return Ok(());
                }
            }
            snapshot.generation = snapshot.generation.saturating_add(1);
            snapshot.mark_path_changed(path.clone());
            snapshot.active = Some(path.clone());
            snapshot.documents.insert(
                path.clone(),
                OpenDocument {
                    uri: params.text_document.uri,
                    version: params.text_document.version,
                    text: change.text.clone(),
                },
            );
            drop(snapshot);
            cancel_semantic_requests_for_path(
                connection,
                &mut semantics.pending,
                &path,
                ErrorCode::ServerCancelled,
                "document changed before the semantic query completed",
            )?;
            semantics.type_facts.remove(&path);
            semantics.clear_resolution_facts();
            schedule_check(trigger);
        }
        DidCloseTextDocument::METHOD => {
            let params: DidCloseTextDocumentParams = parse_notification(notification)?;
            let path = uri_to_file_path(&params.text_document.uri)?;
            let mut snapshot = lock_snapshot(state)?;
            snapshot.generation = snapshot.generation.saturating_add(1);
            snapshot.mark_path_changed(path.clone());
            snapshot.documents.remove(&path);
            if snapshot.active.as_ref() == Some(&path) {
                snapshot.active = snapshot.documents.keys().min().cloned();
            }
            drop(snapshot);
            cancel_semantic_requests_for_path(
                connection,
                &mut semantics.pending,
                &path,
                ErrorCode::ServerCancelled,
                "document closed before the semantic query completed",
            )?;
            semantics.type_facts.remove(&path);
            semantics.clear_resolution_facts();
            let _ = semantic_sender.send(SemanticCommand::Close(path));
            publish_diagnostics(
                connection,
                params.text_document.uri.clone(),
                Vec::new(),
                None,
            )?;
            published.remove(params.text_document.uri.as_str());
            schedule_check(trigger);
        }
        DidSaveTextDocument::METHOD => {
            let params: DidSaveTextDocumentParams = parse_notification(notification)?;
            let path = uri_to_file_path(&params.text_document.uri)?;
            {
                let mut snapshot = lock_snapshot(state)?;
                snapshot.generation = snapshot.generation.saturating_add(1);
                snapshot.mark_path_changed(path);
            }
            schedule_check(trigger);
        }
        DidChangeWatchedFiles::METHOD => {
            let params: DidChangeWatchedFilesParams = parse_notification(notification)?;
            {
                let mut snapshot = lock_snapshot(state)?;
                snapshot.generation = snapshot.generation.saturating_add(1);
                for change in params.changes {
                    let Ok(path) = uri_to_file_path(&change.uri) else {
                        continue;
                    };
                    if !is_hoon_source(&path) {
                        continue;
                    }
                    snapshot.mark_path_changed(path.clone());
                    if matches!(
                        change.typ,
                        FileChangeType::CREATED | FileChangeType::DELETED
                    ) {
                        snapshot.layout_revision = snapshot.generation;
                    }
                    if path.starts_with(&snapshot.source_root) {
                        let membership_changed =
                            if change.typ == FileChangeType::DELETED || !path.is_file() {
                                snapshot.workspace_files.remove(&path)
                            } else {
                                snapshot.workspace_files.insert(path)
                            };
                        if membership_changed {
                            snapshot.layout_revision = snapshot.generation;
                        }
                    }
                }
            }
            semantics.clear_resolution_facts();
            schedule_check(trigger);
        }
        CancelNotification::METHOD => {
            let params: CancelParams = parse_notification(notification)?;
            cancel_semantic_request(
                connection,
                &mut semantics.pending,
                request_id(params.id),
                ErrorCode::RequestCanceled,
                "semantic request cancelled by the client",
            )?;
        }
        "$/setTrace" => {}
        method if method.starts_with("$/") => {
            debug!(method, "ignoring optional LSP notification");
        }
        method => {
            debug!(method, "ignoring unsupported LSP notification");
        }
    }
    Ok(())
}

fn handle_request(
    connection: &Connection,
    state: &Arc<Mutex<EditorSnapshot>>,
    config: &ResolvedConfig,
    semantic_sender: &Sender<SemanticCommand>,
    semantics: &mut SemanticState,
    request: Request,
) -> Result<()> {
    match request.method.as_str() {
        DocumentSymbolRequest::METHOD => {
            let params: DocumentSymbolParams = match serde_json::from_value(request.params) {
                Ok(params) => params,
                Err(error) => {
                    return send_request_error(
                        connection,
                        request.id,
                        ErrorCode::InvalidParams,
                        format!("invalid document symbol parameters: {error}"),
                    );
                }
            };
            let document = match open_document(state, &params.text_document.uri) {
                Ok(document) => document,
                Err(error) => {
                    return send_request_error(
                        connection,
                        request.id,
                        ErrorCode::InvalidParams,
                        format!("invalid document symbol URI: {error:#}"),
                    );
                }
            };
            let Some((path, document)) = document else {
                connection
                    .sender
                    .send(Response::new_ok(request.id, serde_json::Value::Null).into())?;
                return Ok(());
            };
            enqueue_semantic_query(
                connection,
                semantic_sender,
                &mut semantics.pending,
                request.id,
                path,
                document,
                SemanticQuery::DocumentSymbols,
            )?;
        }
        WorkspaceSymbolRequest::METHOD => {
            let params: WorkspaceSymbolParams = match serde_json::from_value(request.params) {
                Ok(params) => params,
                Err(error) => {
                    return send_request_error(
                        connection,
                        request.id,
                        ErrorCode::InvalidParams,
                        format!("invalid workspace symbol parameters: {error}"),
                    );
                }
            };
            let workspace = semantic_workspace(state, config)?;
            enqueue_workspace_semantic_query(
                connection,
                semantic_sender,
                &mut semantics.pending,
                request.id,
                config.workspace.dependencies.clone(),
                SemanticQuery::WorkspaceSymbols {
                    query: params.query,
                    workspace,
                },
            )?;
        }
        HoverRequest::METHOD => {
            let params: HoverParams = match serde_json::from_value(request.params) {
                Ok(params) => params,
                Err(error) => {
                    return send_request_error(
                        connection,
                        request.id,
                        ErrorCode::InvalidParams,
                        format!("invalid hover parameters: {error}"),
                    );
                }
            };
            let text_document = &params.text_document_position_params.text_document;
            let position = params.text_document_position_params.position;
            let document = match open_document(state, &text_document.uri) {
                Ok(document) => document,
                Err(error) => {
                    return send_request_error(
                        connection,
                        request.id,
                        ErrorCode::InvalidParams,
                        format!("invalid hover URI: {error:#}"),
                    );
                }
            };
            let Some((path, document)) = document else {
                connection
                    .sender
                    .send(Response::new_ok(request.id, serde_json::Value::Null).into())?;
                return Ok(());
            };
            let Some(byte_offset) = lsp_position_to_byte(&document.text, position) else {
                connection
                    .sender
                    .send(Response::new_ok(request.id, serde_json::Value::Null).into())?;
                return Ok(());
            };
            enqueue_semantic_query(
                connection,
                semantic_sender,
                &mut semantics.pending,
                request.id,
                path,
                document,
                SemanticQuery::Hover { byte_offset },
            )?;
        }
        Completion::METHOD => {
            let params: CompletionParams = match serde_json::from_value(request.params) {
                Ok(params) => params,
                Err(error) => {
                    return send_request_error(
                        connection,
                        request.id,
                        ErrorCode::InvalidParams,
                        format!("invalid completion parameters: {error}"),
                    );
                }
            };
            let text_document = &params.text_document_position.text_document;
            let position = params.text_document_position.position;
            let document = match open_document(state, &text_document.uri) {
                Ok(document) => document,
                Err(error) => {
                    return send_request_error(
                        connection,
                        request.id,
                        ErrorCode::InvalidParams,
                        format!("invalid completion URI: {error:#}"),
                    );
                }
            };
            let Some((path, document)) = document else {
                connection
                    .sender
                    .send(Response::new_ok(request.id, serde_json::Value::Null).into())?;
                return Ok(());
            };
            let Some(byte_offset) = lsp_position_to_byte(&document.text, position) else {
                connection
                    .sender
                    .send(Response::new_ok(request.id, serde_json::Value::Null).into())?;
                return Ok(());
            };
            enqueue_semantic_query(
                connection,
                semantic_sender,
                &mut semantics.pending,
                request.id,
                path,
                document,
                SemanticQuery::Completion {
                    byte_offset,
                    workspace: semantic_workspace(state, config)?,
                },
            )?;
        }
        GotoDefinition::METHOD => {
            let params: GotoDefinitionParams = match serde_json::from_value(request.params) {
                Ok(params) => params,
                Err(error) => {
                    return send_request_error(
                        connection,
                        request.id,
                        ErrorCode::InvalidParams,
                        format!("invalid definition parameters: {error}"),
                    );
                }
            };
            let text_document = &params.text_document_position_params.text_document;
            let position = params.text_document_position_params.position;
            let document = match open_document(state, &text_document.uri) {
                Ok(document) => document,
                Err(error) => {
                    return send_request_error(
                        connection,
                        request.id,
                        ErrorCode::InvalidParams,
                        format!("invalid definition URI: {error:#}"),
                    );
                }
            };
            let Some((path, document)) = document else {
                connection
                    .sender
                    .send(Response::new_ok(request.id, serde_json::Value::Null).into())?;
                return Ok(());
            };
            let Some(byte_offset) = lsp_position_to_byte(&document.text, position) else {
                connection
                    .sender
                    .send(Response::new_ok(request.id, serde_json::Value::Null).into())?;
                return Ok(());
            };
            let editor = lock_snapshot(state)?.clone();
            let compiler_definition = if hoon_rune_at(&document.text, byte_offset).is_some() {
                None
            } else {
                match definition_at(
                    &semantics.resolution_facts, &path, document.version, &document.text,
                    byte_offset,
                ) {
                    Some((target, location))
                        if paths_match(
                            &compiler_location_path(
                                Some(&location),
                                &target,
                                &config.workspace.dependencies,
                            ),
                            &config.workspace.prelude,
                        ) =>
                    {
                        None
                    }
                    Some((target, location)) => {
                        match compiler_definition_to_lsp(
                            &editor, &target, &config.workspace.dependencies, &location,
                        ) {
                            Ok(location) => location.map(GotoDefinitionResponse::Scalar),
                            Err(error) => {
                                warn!(%error, "compiler definition location is unavailable");
                                None
                            }
                        }
                    }
                    None => None,
                }
            };
            if let Some(definition) = compiler_definition {
                connection.sender.send(
                    Response::new_ok(request.id, serde_json::to_value(Some(definition))?).into(),
                )?;
            } else {
                enqueue_semantic_query(
                    connection,
                    semantic_sender,
                    &mut semantics.pending,
                    request.id,
                    path,
                    document,
                    SemanticQuery::Definition {
                        byte_offset,
                        workspace: semantic_workspace(state, config)?,
                    },
                )?;
            }
        }
        References::METHOD => {
            let params: ReferenceParams = match serde_json::from_value(request.params) {
                Ok(params) => params,
                Err(error) => {
                    return send_request_error(
                        connection,
                        request.id,
                        ErrorCode::InvalidParams,
                        format!("invalid references parameters: {error}"),
                    );
                }
            };
            let text_document = &params.text_document_position.text_document;
            let position = params.text_document_position.position;
            let document = match open_document(state, &text_document.uri) {
                Ok(document) => document,
                Err(error) => {
                    return send_request_error(
                        connection,
                        request.id,
                        ErrorCode::InvalidParams,
                        format!("invalid references URI: {error:#}"),
                    );
                }
            };
            let Some((path, document)) = document else {
                connection
                    .sender
                    .send(Response::new_ok(request.id, serde_json::Value::Null).into())?;
                return Ok(());
            };
            let Some(byte_offset) = lsp_position_to_byte(&document.text, position) else {
                connection
                    .sender
                    .send(Response::new_ok(request.id, serde_json::Value::Null).into())?;
                return Ok(());
            };
            if let Some(index) = semantics.workspace_resolution.as_ref() {
                let editor = lock_snapshot(state)?.clone();
                match compiler_workspace_references_at(
                    &editor,
                    &path,
                    document.text.as_str(),
                    byte_offset,
                    params.context.include_declaration,
                    &config.workspace.dependencies,
                    index,
                ) {
                    Ok(Some(references)) => {
                        connection.sender.send(
                            Response::new_ok(request.id, serde_json::to_value(references)?).into(),
                        )?;
                        return Ok(());
                    }
                    Ok(None) => {}
                    Err(error) => {
                        warn!(%error, "compiler workspace references are unavailable");
                    }
                }
            }
            enqueue_semantic_query(
                connection,
                semantic_sender,
                &mut semantics.pending,
                request.id,
                path,
                document,
                SemanticQuery::References {
                    byte_offset,
                    include_declaration: params.context.include_declaration,
                    workspace: semantic_workspace(state, config)?,
                },
            )?;
        }
        PrepareRenameRequest::METHOD => {
            let params: TextDocumentPositionParams = match serde_json::from_value(request.params) {
                Ok(params) => params,
                Err(error) => {
                    return send_request_error(
                        connection,
                        request.id,
                        ErrorCode::InvalidParams,
                        format!("invalid prepare rename parameters: {error}"),
                    );
                }
            };
            let document = match open_document(state, &params.text_document.uri) {
                Ok(document) => document,
                Err(error) => {
                    return send_request_error(
                        connection,
                        request.id,
                        ErrorCode::InvalidParams,
                        format!("invalid prepare rename URI: {error:#}"),
                    );
                }
            };
            let Some((path, document)) = document else {
                connection
                    .sender
                    .send(Response::new_ok(request.id, serde_json::Value::Null).into())?;
                return Ok(());
            };
            let Some(byte_offset) = lsp_position_to_byte(&document.text, params.position) else {
                connection
                    .sender
                    .send(Response::new_ok(request.id, serde_json::Value::Null).into())?;
                return Ok(());
            };
            enqueue_semantic_query(
                connection,
                semantic_sender,
                &mut semantics.pending,
                request.id,
                path,
                document,
                SemanticQuery::PrepareRename {
                    byte_offset,
                    workspace: semantic_workspace(state, config)?,
                },
            )?;
        }
        Rename::METHOD => {
            let params: RenameParams = match serde_json::from_value(request.params) {
                Ok(params) => params,
                Err(error) => {
                    return send_request_error(
                        connection,
                        request.id,
                        ErrorCode::InvalidParams,
                        format!("invalid rename parameters: {error}"),
                    );
                }
            };
            let text_document = &params.text_document_position.text_document;
            let position = params.text_document_position.position;
            let document = match open_document(state, &text_document.uri) {
                Ok(document) => document,
                Err(error) => {
                    return send_request_error(
                        connection,
                        request.id,
                        ErrorCode::InvalidParams,
                        format!("invalid rename URI: {error:#}"),
                    );
                }
            };
            let Some((path, document)) = document else {
                connection
                    .sender
                    .send(Response::new_ok(request.id, serde_json::Value::Null).into())?;
                return Ok(());
            };
            let Some(byte_offset) = lsp_position_to_byte(&document.text, position) else {
                connection
                    .sender
                    .send(Response::new_ok(request.id, serde_json::Value::Null).into())?;
                return Ok(());
            };
            enqueue_semantic_query(
                connection,
                semantic_sender,
                &mut semantics.pending,
                request.id,
                path,
                document,
                SemanticQuery::Rename {
                    byte_offset,
                    new_name: params.new_name,
                    workspace: semantic_workspace(state, config)?,
                },
            )?;
        }
        _ => {
            return send_request_error(
                connection,
                request.id,
                ErrorCode::MethodNotFound,
                format!("unsupported request: {}", request.method),
            );
        }
    }
    Ok(())
}

fn enqueue_semantic_query(
    connection: &Connection,
    semantic_sender: &Sender<SemanticCommand>,
    pending_semantics: &mut HashMap<RequestId, PendingSemanticRequest>,
    id: RequestId,
    path: PathBuf,
    document: OpenDocument,
    query: SemanticQuery,
) -> Result<()> {
    if pending_semantics.contains_key(&id) {
        return send_request_error(
            connection,
            id,
            ErrorCode::InvalidRequest,
            "duplicate in-flight JSON-RPC request ID".to_string(),
        );
    }

    let source = Arc::<str>::from(document.text);
    let cancelled = Arc::new(AtomicBool::new(false));
    let hover_offset = match &query {
        SemanticQuery::Hover { byte_offset } => Some(*byte_offset),
        SemanticQuery::DocumentSymbols
        | SemanticQuery::WorkspaceSymbols { .. }
        | SemanticQuery::Definition { .. }
        | SemanticQuery::Completion { .. }
        | SemanticQuery::References { .. }
        | SemanticQuery::PrepareRename { .. }
        | SemanticQuery::Rename { .. } => None,
    };
    let workspace_generation = match &query {
        SemanticQuery::WorkspaceSymbols { workspace, .. }
        | SemanticQuery::Definition { workspace, .. }
        | SemanticQuery::Completion { workspace, .. }
        | SemanticQuery::References { workspace, .. }
        | SemanticQuery::PrepareRename { workspace, .. }
        | SemanticQuery::Rename { workspace, .. } => Some(workspace.sources.revision()),
        SemanticQuery::DocumentSymbols | SemanticQuery::Hover { .. } => None,
    };
    let job = SemanticJob {
        id: id.clone(),
        path: path.clone(),
        version: document.version,
        source: Arc::clone(&source),
        query,
        cancelled: Arc::clone(&cancelled),
    };
    if semantic_sender.send(SemanticCommand::Query(job)).is_err() {
        return send_request_error(
            connection,
            id,
            ErrorCode::InternalError,
            "honk semantic worker is unavailable".to_string(),
        );
    }
    pending_semantics.insert(
        id,
        PendingSemanticRequest {
            path,
            version: document.version,
            source,
            cancelled,
            hover_offset,
            workspace_generation,
            document_bound: true,
        },
    );
    Ok(())
}

fn enqueue_workspace_semantic_query(
    connection: &Connection,
    semantic_sender: &Sender<SemanticCommand>,
    pending_semantics: &mut HashMap<RequestId, PendingSemanticRequest>,
    id: RequestId,
    workspace_path: PathBuf,
    query: SemanticQuery,
) -> Result<()> {
    if pending_semantics.contains_key(&id) {
        return send_request_error(
            connection,
            id,
            ErrorCode::InvalidRequest,
            "duplicate in-flight JSON-RPC request ID".to_string(),
        );
    }
    let SemanticQuery::WorkspaceSymbols { workspace, .. } = &query else {
        return send_request_error(
            connection,
            id,
            ErrorCode::InternalError,
            "invalid workspace semantic query".to_string(),
        );
    };
    let generation = workspace.sources.revision();
    let source = Arc::<str>::from("");
    let cancelled = Arc::new(AtomicBool::new(false));
    let job = SemanticJob {
        id: id.clone(),
        path: workspace_path.clone(),
        version: 0,
        source: Arc::clone(&source),
        query,
        cancelled: Arc::clone(&cancelled),
    };
    if semantic_sender.send(SemanticCommand::Query(job)).is_err() {
        return send_request_error(
            connection,
            id,
            ErrorCode::InternalError,
            "honk semantic worker is unavailable".to_string(),
        );
    }
    pending_semantics.insert(
        id,
        PendingSemanticRequest {
            path: workspace_path,
            version: 0,
            source,
            cancelled,
            hover_offset: None,
            workspace_generation: Some(generation),
            document_bound: false,
        },
    );
    Ok(())
}

fn send_request_error(
    connection: &Connection,
    id: lsp_server::RequestId,
    code: ErrorCode,
    message: String,
) -> Result<()> {
    connection
        .sender
        .send(Response::new_err(id, code as i32, message).into())?;
    Ok(())
}

fn open_document(
    state: &Arc<Mutex<EditorSnapshot>>,
    uri: &Uri,
) -> Result<Option<(PathBuf, OpenDocument)>> {
    let path = uri_to_file_path(uri)?;
    let document = lock_snapshot(state)?.documents.get(&path).cloned();
    Ok(document.map(|document| (path, document)))
}

fn semantic_workspace(
    state: &Arc<Mutex<EditorSnapshot>>,
    config: &ResolvedConfig,
) -> Result<Arc<SemanticWorkspace>> {
    let editor = lock_snapshot(state)?.clone();
    let mut roots = editor.workspace_files.iter().cloned().collect::<Vec<_>>();
    roots.extend(editor.documents.keys().cloned());
    if let Some(entry) = &config.entry {
        roots.push(entry.clone());
    }
    roots.sort();
    roots.dedup();
    let versions = editor
        .documents
        .iter()
        .map(|(path, document)| (path.clone(), document.version))
        .collect();
    let raw_path_revisions = editor.path_revisions.clone();
    let layout_revision = editor.layout_revision;
    let sources = WorkspaceSourceSnapshot::try_new(
        editor.generation,
        editor
            .documents
            .into_iter()
            .map(|(path, document)| (path, document.text)),
    )
    .context("failed to snapshot open documents for semantic workspace lookup")?;
    let mut path_revisions = HashMap::<PathBuf, u64>::new();
    for (path, revision) in raw_path_revisions {
        let identity = sources.canonicalize(&path).unwrap_or(path);
        path_revisions
            .entry(identity)
            .and_modify(|current| *current = (*current).max(revision))
            .or_insert(revision);
    }
    Ok(Arc::new(SemanticWorkspace {
        dependencies: config.workspace.dependencies.clone(),
        prelude: config.workspace.prelude.clone(),
        roots,
        sources,
        versions,
        path_revisions,
        layout_revision,
    }))
}

fn request_id(id: NumberOrString) -> RequestId {
    match id {
        NumberOrString::Number(id) => RequestId::from(id),
        NumberOrString::String(id) => RequestId::from(id),
    }
}

fn cancel_semantic_request(
    connection: &Connection,
    pending: &mut HashMap<RequestId, PendingSemanticRequest>,
    id: RequestId,
    code: ErrorCode,
    message: &str,
) -> Result<()> {
    let Some(request) = pending.remove(&id) else {
        return Ok(());
    };
    request.cancelled.store(true, Ordering::Release);
    send_request_error(connection, id, code, message.to_string())
}

fn cancel_semantic_requests_for_path(
    connection: &Connection,
    pending: &mut HashMap<RequestId, PendingSemanticRequest>,
    path: &Path,
    code: ErrorCode,
    message: &str,
) -> Result<()> {
    let ids = pending
        .iter()
        .filter(|(_, request)| request.path == path)
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    for id in ids {
        cancel_semantic_request(connection, pending, id, code, message)?;
    }
    Ok(())
}

fn cancel_all_semantic_requests(
    connection: &Connection,
    pending: &mut HashMap<RequestId, PendingSemanticRequest>,
    code: ErrorCode,
    message: &str,
) -> Result<()> {
    let ids = pending.keys().cloned().collect::<Vec<_>>();
    for id in ids {
        cancel_semantic_request(connection, pending, id, code, message)?;
    }
    Ok(())
}

fn spawn_semantic_worker(
    commands: Receiver<SemanticCommand>,
    events: Sender<SemanticEvent>,
    stack_bytes: usize,
) -> Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("honk-lsp-semantics".to_string())
        .stack_size(stack_bytes)
        .spawn(move || semantic_worker_loop(&commands, &events))
        .context("failed to spawn honk LSP semantic worker")
}

fn semantic_worker_loop(commands: &Receiver<SemanticCommand>, events: &Sender<SemanticEvent>) {
    let mut semantics = SemanticSession::default();
    let mut structural_graph = None::<CachedStructuralWorkspaceGraph>;
    while let Ok(command) = commands.recv() {
        match command {
            SemanticCommand::Query(job) => {
                let SemanticJob {
                    id,
                    path,
                    version,
                    source,
                    query,
                    cancelled,
                } = job;
                if cancelled.load(Ordering::Acquire) {
                    continue;
                }
                let result = match query {
                    SemanticQuery::DocumentSymbols => {
                        match semantics.snapshot(&path, i64::from(version), source.as_ref()) {
                            Ok(snapshot) => {
                                SemanticQueryResult::DocumentSymbols(snapshot.symbols.clone())
                            }
                            Err(error) => SemanticQueryResult::Unavailable(error.to_string()),
                        }
                    }
                    SemanticQuery::WorkspaceSymbols { query, workspace } => {
                        let graph =
                            cached_structural_workspace_graph(&mut structural_graph, &workspace);
                        SemanticQueryResult::WorkspaceSymbols(graph.workspace_symbols(&query))
                    }
                    SemanticQuery::Hover { byte_offset } => {
                        match semantics.snapshot(&path, i64::from(version), source.as_ref()) {
                            Ok(snapshot) => SemanticQueryResult::Hover(snapshot.hover(byte_offset)),
                            Err(error) => SemanticQueryResult::Unavailable(error.to_string()),
                        }
                    }
                    SemanticQuery::Definition {
                        byte_offset,
                        workspace,
                    } => match structural_workspace_definition(
                        &mut semantics,
                        &path,
                        version,
                        Arc::clone(&source),
                        byte_offset,
                        &workspace,
                    ) {
                        Ok(definition) => SemanticQueryResult::Definition(definition),
                        Err(error) => SemanticQueryResult::Unavailable(error.to_string()),
                    },
                    SemanticQuery::Completion {
                        byte_offset,
                        workspace,
                    } => match structural_workspace_completions(
                        &mut semantics,
                        &path,
                        version,
                        source.as_ref(),
                        byte_offset,
                        &workspace,
                    ) {
                        Ok(completions) => SemanticQueryResult::Completion(completions),
                        Err(error) => SemanticQueryResult::Unavailable(error.to_string()),
                    },
                    SemanticQuery::References {
                        byte_offset,
                        include_declaration,
                        workspace,
                    } => {
                        let graph =
                            cached_structural_workspace_graph(&mut structural_graph, &workspace);
                        match structural_workspace_references(
                            &mut semantics,
                            StructuralReferenceQuery {
                                path: &path,
                                version,
                                source: Arc::clone(&source),
                                byte_offset,
                                include_declaration,
                            },
                            &workspace,
                            graph,
                        ) {
                            Ok(references) => SemanticQueryResult::References(references),
                            Err(error) => SemanticQueryResult::Unavailable(error.to_string()),
                        }
                    }
                    SemanticQuery::PrepareRename {
                        byte_offset,
                        workspace,
                    } => {
                        let graph =
                            cached_structural_workspace_graph(&mut structural_graph, &workspace);
                        match structural_workspace_prepare_rename(
                            &mut semantics,
                            &path,
                            version,
                            source.as_ref(),
                            byte_offset,
                            &workspace,
                            graph,
                        ) {
                            Ok(target) => SemanticQueryResult::PrepareRename(target),
                            Err(error) => SemanticQueryResult::Unavailable(error.to_string()),
                        }
                    }
                    SemanticQuery::Rename {
                        byte_offset,
                        new_name,
                        workspace,
                    } => {
                        let graph =
                            cached_structural_workspace_graph(&mut structural_graph, &workspace);
                        match structural_workspace_rename(
                            &mut semantics,
                            StructuralRenameQuery {
                                path: &path,
                                version,
                                source: Arc::clone(&source),
                                byte_offset,
                                new_name: &new_name,
                            },
                            &workspace,
                            graph,
                        ) {
                            Ok(Ok(rename)) => SemanticQueryResult::Rename(rename),
                            Ok(Err(error)) => SemanticQueryResult::RequestError {
                                code: ErrorCode::InvalidParams,
                                message: error.to_string(),
                            },
                            Err(error) => SemanticQueryResult::Unavailable(error.to_string()),
                        }
                    }
                };
                if cancelled.load(Ordering::Acquire) {
                    continue;
                }
                if events
                    .send(SemanticEvent {
                        id,
                        path,
                        version,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
            SemanticCommand::Close(path) => semantics.close(&path),
            SemanticCommand::Stop => break,
        }
    }
}

fn structural_workspace_definition(
    semantics: &mut SemanticSession,
    path: &Path,
    version: i32,
    source: Arc<str>,
    byte_offset: u32,
    workspace: &SemanticWorkspace,
) -> Result<Option<SemanticDefinition>> {
    let (name, rune, local) = {
        let snapshot = semantics
            .snapshot(path, i64::from(version), source.as_ref())
            .map_err(anyhow::Error::new)?;
        (
            hoon_term_at(source.as_ref(), byte_offset).map(str::to_owned),
            hoon_rune_at(source.as_ref(), byte_offset).map(str::to_owned),
            snapshot.definition(source.as_ref(), byte_offset),
        )
    };
    if let Some(range) = local {
        return Ok(Some(SemanticDefinition {
            path: path.to_path_buf(),
            source,
            range,
        }));
    }
    if let Some(rune) = rune {
        let prelude_source = workspace
            .sources
            .read_to_string(&workspace.prelude)
            .with_context(|| format!("failed to read prelude {}", workspace.prelude.display()))?;
        return Ok(
            structural_rune_definition(&prelude_source, &rune).map(|range| SemanticDefinition {
                path: workspace.prelude.clone(),
                source: Arc::from(prelude_source),
                range,
            }),
        );
    }
    let Some(name) = name else {
        return Ok(None);
    };

    structural_external_definition(path, &name, workspace)
}

fn structural_external_definition(
    path: &Path,
    name: &str,
    workspace: &SemanticWorkspace,
) -> Result<Option<SemanticDefinition>> {
    let mut visited = HashSet::<PathBuf>::new();
    if let Ok(canonical) = workspace.sources.canonicalize(path) {
        visited.insert(canonical);
    }
    let mut frontier = hoon_imports(path, workspace);
    while !frontier.is_empty() {
        let mut next = Vec::new();
        let mut definitions = Vec::new();
        for dependency in frontier {
            let identity = workspace
                .sources
                .canonicalize(&dependency)
                .unwrap_or_else(|_| dependency.clone());
            if !visited.insert(identity) {
                continue;
            }
            let Ok(dependency_source) = workspace.sources.read_to_string(&dependency) else {
                continue;
            };
            if let Some(range) = structural_definition(&dependency_source, name) {
                definitions.push(SemanticDefinition {
                    path: dependency.clone(),
                    source: Arc::from(dependency_source.as_str()),
                    range,
                });
            }
            next.extend(hoon_imports(&dependency, workspace));
        }
        match definitions.len() {
            0 => frontier = next,
            1 => return Ok(definitions.pop()),
            _ => return Ok(None),
        }
    }

    let prelude_source = workspace
        .sources
        .read_to_string(&workspace.prelude)
        .with_context(|| format!("failed to read prelude {}", workspace.prelude.display()))?;
    Ok(
        structural_definition(&prelude_source, name).map(|range| SemanticDefinition {
            path: workspace.prelude.clone(),
            source: Arc::from(prelude_source),
            range,
        }),
    )
}

struct StructuralReferenceQuery<'path> {
    path: &'path Path,
    version: i32,
    source: Arc<str>,
    byte_offset: u32,
    include_declaration: bool,
}

fn structural_workspace_references(
    semantics: &mut SemanticSession,
    query: StructuralReferenceQuery<'_>,
    workspace: &SemanticWorkspace,
    graph: &StructuralWorkspaceGraph,
) -> Result<Option<Vec<SemanticReference>>> {
    let StructuralReferenceQuery {
        path,
        version,
        source,
        byte_offset,
        include_declaration,
    } = query;
    let (name, local, local_definition, lexical_binding) = {
        let snapshot = semantics
            .snapshot(path, i64::from(version), source.as_ref())
            .map_err(anyhow::Error::new)?;
        (
            hoon_term_at(source.as_ref(), byte_offset).map(str::to_owned),
            snapshot.references(source.as_ref(), byte_offset, include_declaration),
            snapshot.definition(source.as_ref(), byte_offset),
            snapshot
                .prepare_rename(source.as_ref(), byte_offset)
                .is_some(),
        )
    };
    if lexical_binding {
        let Some(ranges) = local else {
            return Ok(None);
        };
        return Ok(Some(
            ranges
                .into_iter()
                .map(|range| SemanticReference {
                    path: path.to_path_buf(),
                    source: Arc::clone(&source),
                    range,
                })
                .collect(),
        ));
    }
    let Some(name) = name else {
        return Ok(None);
    };
    let declarations = graph.declarations(&name);
    let current_identity = semantic_path_identity(path, workspace);
    let target_identity = local_definition
        .map(|range| (current_identity.clone(), range))
        .or_else(|| graph.external_definition(&current_identity, &declarations));
    let Some(target_identity) = target_identity else {
        return Ok(None);
    };
    let mut references = Vec::<SemanticReference>::new();
    for (candidate_identity, candidate) in &graph.documents {
        let is_target = candidate_identity == &target_identity.0;
        if !is_target
            && graph.external_definition(candidate_identity, &declarations)
                != Some(target_identity.clone())
        {
            continue;
        }
        let candidate_version = if candidate_identity == &current_identity {
            i64::from(version)
        } else {
            structural_document_version(candidate, workspace)
        };
        let snapshot = match semantics.snapshot(
            &candidate.path,
            candidate_version,
            candidate.source.as_ref(),
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                debug!(path = %candidate.path.display(), %error, "structural reference index is unavailable");
                continue;
            }
        };
        let ranges = if is_target {
            snapshot
                .references(
                    candidate.source.as_ref(),
                    target_identity.1.start,
                    include_declaration,
                )
                .unwrap_or_default()
        } else {
            snapshot.external_reference_ranges(candidate.source.as_ref(), &name)
        };
        references.extend(ranges.into_iter().map(|range| SemanticReference {
            path: candidate.path.clone(),
            source: Arc::clone(&candidate.source),
            range,
        }));
    }
    if include_declaration
        && !references.iter().any(|reference| {
            semantic_path_identity(&reference.path, workspace) == target_identity.0
                && reference.range == target_identity.1
        })
    {
        if let Some(target) = graph.documents.get(&target_identity.0) {
            references.push(SemanticReference {
                path: target.path.clone(),
                source: Arc::clone(&target.source),
                range: target_identity.1,
            });
        }
    }
    references.sort_by(|left, right| {
        (
            semantic_path_identity(&left.path, workspace),
            left.range.start,
            left.range.end,
        )
            .cmp(&(
                semantic_path_identity(&right.path, workspace),
                right.range.start,
                right.range.end,
            ))
    });
    references.dedup_by(|left, right| {
        semantic_path_identity(&left.path, workspace)
            == semantic_path_identity(&right.path, workspace)
            && left.range == right.range
    });
    Ok(Some(references))
}

struct StructuralSymbolTarget {
    name: String,
    request_range: SemanticTextRange,
    definition: (PathBuf, SemanticTextRange),
}

fn structural_workspace_prepare_rename(
    semantics: &mut SemanticSession,
    path: &Path,
    version: i32,
    source: &str,
    byte_offset: u32,
    workspace: &SemanticWorkspace,
    graph: &StructuralWorkspaceGraph,
) -> Result<Option<SemanticRenameTarget>> {
    let snapshot = semantics
        .snapshot(path, i64::from(version), source)
        .map_err(anyhow::Error::new)?;
    if let Some(target) = snapshot.prepare_rename(source, byte_offset) {
        return Ok(Some(target));
    }
    if !graph.complete {
        return Ok(None);
    }
    Ok(
        structural_symbol_target(snapshot, path, source, byte_offset, workspace, graph).map(
            |target| SemanticRenameTarget {
                name: target.name,
                range: target.request_range,
            },
        ),
    )
}

struct StructuralRenameQuery<'query> {
    path: &'query Path,
    version: i32,
    source: Arc<str>,
    byte_offset: u32,
    new_name: &'query str,
}

fn structural_workspace_rename(
    semantics: &mut SemanticSession,
    query: StructuralRenameQuery<'_>,
    workspace: &SemanticWorkspace,
    graph: &StructuralWorkspaceGraph,
) -> Result<Result<Option<Vec<SemanticDocumentEdits>>, SemanticRenameError>> {
    let StructuralRenameQuery {
        path,
        version,
        source,
        byte_offset,
        new_name,
    } = query;
    let snapshot = semantics
        .snapshot(path, i64::from(version), source.as_ref())
        .map_err(anyhow::Error::new)?;
    match snapshot.rename(source.as_ref(), byte_offset, new_name) {
        Ok(Some(rename)) => {
            return Ok(Ok(Some(vec![local_rename_document(
                path, version, source, rename,
            )])));
        }
        Ok(None) => {}
        Err(error) => return Ok(Err(error)),
    }
    if !graph.complete {
        bail!("workspace source graph is incomplete; structural rename was declined");
    }
    if let Err(error) = validate_rename_name(new_name) {
        return Ok(Err(error));
    }
    let Some(target) = structural_symbol_target(
        snapshot,
        path,
        source.as_ref(),
        byte_offset,
        workspace,
        graph,
    ) else {
        return Ok(Ok(None));
    };
    let Some(references) = structural_workspace_references(
        semantics,
        StructuralReferenceQuery {
            path,
            version,
            source: Arc::clone(&source),
            byte_offset,
            include_declaration: true,
        },
        workspace,
        graph,
    )?
    else {
        return Ok(Ok(None));
    };

    if new_name != target.name
        && structural_rename_would_collide(
            semantics, &target, &references, new_name, workspace, graph,
        )?
    {
        return Ok(Err(SemanticRenameError::WouldCapture(new_name.to_string())));
    }

    let mut documents = HashMap::<PathBuf, SemanticDocumentEdits>::new();
    for reference in references {
        let identity = semantic_path_identity(&reference.path, workspace);
        let document = documents
            .entry(identity)
            .or_insert_with(|| SemanticDocumentEdits {
                version: workspace_document_version(&reference.path, workspace),
                path: reference.path.clone(),
                source: Arc::clone(&reference.source),
                edits: Vec::new(),
            });
        document.edits.push(SemanticRenameEdit {
            range: reference.range,
            new_text: new_name.to_string(),
        });
    }
    let mut documents = documents.into_values().collect::<Vec<_>>();
    for document in &mut documents {
        document
            .edits
            .sort_by_key(|edit| (edit.range.start, edit.range.end));
    }
    documents.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(Ok(Some(documents)))
}

fn structural_symbol_target(
    snapshot: &honk_service::semantic::SemanticSnapshot,
    path: &Path,
    source: &str,
    byte_offset: u32,
    workspace: &SemanticWorkspace,
    graph: &StructuralWorkspaceGraph,
) -> Option<StructuralSymbolTarget> {
    let name = hoon_term_at(source, byte_offset)?.to_string();
    let request_range = completion_term_range(source, byte_offset)?;
    if request_range.start == request_range.end
        || source.get(request_range.start as usize..request_range.end as usize) != Some(&name)
    {
        return None;
    }
    let current_identity = semantic_path_identity(path, workspace);
    let declarations = graph.declarations(&name);
    let definition = snapshot
        .definition(source, byte_offset)
        .map(|range| (current_identity.clone(), range))
        .or_else(|| graph.external_definition(&current_identity, &declarations))?;
    Some(StructuralSymbolTarget {
        name,
        request_range,
        definition,
    })
}

fn structural_rename_would_collide(
    semantics: &mut SemanticSession,
    target: &StructuralSymbolTarget,
    references: &[SemanticReference],
    new_name: &str,
    workspace: &SemanticWorkspace,
    graph: &StructuralWorkspaceGraph,
) -> Result<bool> {
    let Some(target_document) = graph.documents.get(&target.definition.0) else {
        return Ok(true);
    };
    if !structural_declaration_ranges(target_document.source.as_ref(), new_name).is_empty() {
        return Ok(true);
    }

    let mut virtual_declarations = graph.declarations(new_name);
    virtual_declarations.insert(target.definition.0.clone(), target.definition.1);
    let mut edits_by_document = HashMap::<PathBuf, Vec<SemanticTextRange>>::new();
    for reference in references {
        edits_by_document
            .entry(semantic_path_identity(&reference.path, workspace))
            .or_default()
            .push(reference.range);
    }
    for (identity, document) in &graph.documents {
        let edited_ranges = edits_by_document.get(identity);
        let resolves_new_target = identity == &target.definition.0
            || graph.external_definition(identity, &virtual_declarations)
                == Some(target.definition.clone());
        if edited_ranges.is_some() && !resolves_new_target {
            return Ok(true);
        }
        if !resolves_new_target {
            continue;
        }
        if edited_ranges.is_some()
            && identity != &target.definition.0
            && !structural_declaration_ranges(document.source.as_ref(), new_name).is_empty()
        {
            return Ok(true);
        }
        let snapshot_version = workspace_document_version(&document.path, workspace)
            .map(i64::from)
            .unwrap_or_else(|| structural_document_version(document, workspace));
        let snapshot = semantics
            .snapshot(&document.path, snapshot_version, document.source.as_ref())
            .map_err(anyhow::Error::new)
            .with_context(|| {
                format!(
                    "failed to prove rename safety in {}",
                    document.path.display()
                )
            })?;
        if snapshot
            .external_reference_ranges(document.source.as_ref(), new_name)
            .into_iter()
            .next()
            .is_some()
        {
            return Ok(true);
        }
        if edited_ranges.is_some_and(|ranges| {
            let use_ranges = ranges
                .iter()
                .copied()
                .filter(|range| identity != &target.definition.0 || range != &target.definition.1)
                .collect::<Vec<_>>();
            snapshot.external_rename_would_capture(&use_ranges, new_name)
        }) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn local_rename_document(
    path: &Path,
    version: i32,
    source: Arc<str>,
    rename: SemanticRename,
) -> SemanticDocumentEdits {
    SemanticDocumentEdits {
        path: path.to_path_buf(),
        source,
        version: Some(version),
        edits: rename.edits,
    }
}

fn workspace_document_version(path: &Path, workspace: &SemanticWorkspace) -> Option<i32> {
    workspace.versions.get(path).copied().or_else(|| {
        let identity = semantic_path_identity(path, workspace);
        workspace.versions.iter().find_map(|(candidate, version)| {
            (semantic_path_identity(candidate, workspace) == identity).then_some(*version)
        })
    })
}

fn structural_document_version(
    document: &StructuralWorkspaceDocument,
    workspace: &SemanticWorkspace,
) -> i64 {
    workspace_document_version(&document.path, workspace)
        .map(i64::from)
        .unwrap_or_else(|| i64::try_from(document.source_revision).unwrap_or(i64::MAX))
}

#[derive(Clone)]
struct StructuralWorkspaceDocument {
    path: PathBuf,
    source: Arc<str>,
    source_revision: u64,
    imports: Arc<[PathBuf]>,
    imports_complete: bool,
    declarations: Arc<[SemanticStructuralSymbol]>,
}

struct StructuralWorkspaceGraph {
    documents: HashMap<PathBuf, StructuralWorkspaceDocument>,
    prelude: PathBuf,
    complete: bool,
    layout_revision: u64,
}

struct CachedStructuralWorkspaceGraph {
    revision: u64,
    graph: StructuralWorkspaceGraph,
}

fn cached_structural_workspace_graph<'a>(
    cached: &'a mut Option<CachedStructuralWorkspaceGraph>,
    workspace: &SemanticWorkspace,
) -> &'a StructuralWorkspaceGraph {
    let revision = workspace.sources.revision();
    if cached
        .as_ref()
        .is_none_or(|cached| cached.revision != revision)
    {
        let previous = cached.take();
        *cached = Some(CachedStructuralWorkspaceGraph {
            revision,
            graph: StructuralWorkspaceGraph::refresh(
                workspace,
                previous.as_ref().map(|cached| &cached.graph),
            ),
        });
    }
    &cached.as_ref().expect("structural graph was cached").graph
}

impl StructuralWorkspaceGraph {
    fn refresh(workspace: &SemanticWorkspace, previous: Option<&Self>) -> Self {
        let mut pending = workspace.roots.clone();
        let mut preferred_paths = HashMap::<PathBuf, PathBuf>::new();
        for path in &workspace.roots {
            preferred_paths
                .entry(semantic_path_identity(path, workspace))
                .or_insert_with(|| path.clone());
        }
        let prelude = semantic_path_identity(&workspace.prelude, workspace);
        preferred_paths.insert(prelude.clone(), workspace.prelude.clone());
        pending.push(workspace.prelude.clone());
        let mut documents = HashMap::<PathBuf, StructuralWorkspaceDocument>::new();
        let mut visited = HashSet::<PathBuf>::new();
        let mut complete = true;
        let layout_changed =
            previous.is_none_or(|previous| previous.layout_revision != workspace.layout_revision);
        while let Some(candidate) = pending.pop() {
            let identity = semantic_path_identity(&candidate, workspace);
            if !visited.insert(identity.clone()) {
                continue;
            }
            let previous_document = previous.and_then(|graph| graph.documents.get(&identity));
            let candidate = preferred_paths
                .get(&identity)
                .cloned()
                .or_else(|| previous_document.map(|document| document.path.clone()))
                .unwrap_or(candidate);
            let source_revision = workspace_file_revision(workspace, &candidate, &identity);
            let source_unchanged = previous_document.is_some_and(|document| {
                document.path == candidate && document.source_revision == source_revision
            });
            let (candidate_source, declarations) = if source_unchanged {
                let document = previous_document.expect("unchanged structural document");
                (
                    Arc::clone(&document.source),
                    Arc::clone(&document.declarations),
                )
            } else {
                let source = match workspace.sources.read_to_string(&candidate) {
                    Ok(source) => Arc::<str>::from(source),
                    Err(error) => {
                        debug!(path = %candidate.display(), %error, "structural reference source is unavailable");
                        complete = false;
                        continue;
                    }
                };
                let declarations = Arc::from(structural_symbols(source.as_ref()));
                (source, declarations)
            };
            let (imports, imports_complete) = if source_unchanged && !layout_changed {
                let document = previous_document.expect("unchanged structural document");
                (Arc::clone(&document.imports), document.imports_complete)
            } else {
                match resolved_hoon_imports_result(&candidate, workspace) {
                    Ok(imports) => {
                        let identities = imports
                            .into_iter()
                            .map(|import| semantic_path_identity(&import.path, workspace))
                            .collect::<Vec<_>>();
                        (Arc::from(identities), true)
                    }
                    Err(error) => {
                        debug!(path = %candidate.display(), %error, "semantic import scan failed");
                        (Arc::from([]), false)
                    }
                }
            };
            complete &= imports_complete;
            pending.extend(imports.iter().cloned());
            documents.insert(
                identity,
                StructuralWorkspaceDocument {
                    path: candidate,
                    source: candidate_source,
                    source_revision,
                    imports,
                    imports_complete,
                    declarations,
                },
            );
        }
        if !documents.contains_key(&prelude) {
            complete = false;
        }
        Self {
            documents,
            prelude,
            complete,
            layout_revision: workspace.layout_revision,
        }
    }

    fn declarations(&self, name: &str) -> HashMap<PathBuf, SemanticTextRange> {
        self.documents
            .iter()
            .filter_map(|(identity, document)| {
                let mut matches = document
                    .declarations
                    .iter()
                    .filter(|declaration| declaration.name == name);
                let declaration = matches.next()?;
                matches
                    .next()
                    .is_none()
                    .then(|| (identity.clone(), declaration.range))
            })
            .collect()
    }

    fn workspace_symbols(&self, query: &str) -> Vec<SemanticWorkspaceSymbol> {
        let query = query.to_ascii_lowercase();
        let mut symbols = self
            .documents
            .values()
            .flat_map(|document| {
                document
                    .declarations
                    .iter()
                    .filter(|declaration| {
                        query.is_empty() || declaration.name.to_ascii_lowercase().contains(&query)
                    })
                    .map(|declaration| SemanticWorkspaceSymbol {
                        path: document.path.clone(),
                        source: Arc::clone(&document.source),
                        name: declaration.name.clone(),
                        kind: declaration.kind,
                        range: declaration.range,
                    })
            })
            .collect::<Vec<_>>();
        symbols.sort_by(|left, right| {
            (left.name.as_str(), left.path.as_path(), left.range.start).cmp(&(
                right.name.as_str(),
                right.path.as_path(),
                right.range.start,
            ))
        });
        symbols.truncate(2_048);
        symbols
    }

    fn external_definition(
        &self,
        from: &Path,
        declarations: &HashMap<PathBuf, SemanticTextRange>,
    ) -> Option<(PathBuf, SemanticTextRange)> {
        let mut visited = HashSet::<PathBuf>::from([from.to_path_buf()]);
        let mut frontier = self.documents.get(from)?.imports.to_vec();
        while !frontier.is_empty() {
            let mut definitions = Vec::new();
            let mut next = Vec::new();
            for identity in frontier {
                if !visited.insert(identity.clone()) {
                    continue;
                }
                if let Some(range) = declarations.get(&identity) {
                    definitions.push((identity.clone(), *range));
                }
                if let Some(document) = self.documents.get(&identity) {
                    next.extend(document.imports.iter().cloned());
                }
            }
            match definitions.len() {
                0 => frontier = next,
                1 => return definitions.pop(),
                _ => return None,
            }
        }
        declarations
            .get(&self.prelude)
            .map(|range| (self.prelude.clone(), *range))
    }
}

fn workspace_file_revision(workspace: &SemanticWorkspace, path: &Path, identity: &Path) -> u64 {
    workspace
        .path_revisions
        .get(identity)
        .or_else(|| workspace.path_revisions.get(path))
        .copied()
        .unwrap_or(0)
}

fn semantic_path_identity(path: &Path, workspace: &SemanticWorkspace) -> PathBuf {
    workspace
        .sources
        .canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
}

fn structural_workspace_completions(
    semantics: &mut SemanticSession,
    path: &Path,
    version: i32,
    source: &str,
    byte_offset: u32,
    workspace: &SemanticWorkspace,
) -> Result<SemanticCompletionResult> {
    let replacement_range = completion_term_range(source, byte_offset)
        .ok_or_else(|| anyhow!("completion cursor is outside the current document"))?;
    let prefix = source
        .get(usize::try_from(replacement_range.start)?..usize::try_from(byte_offset)?)
        .ok_or_else(|| anyhow!("completion prefix is not a valid source range"))?;
    let local = match semantics.snapshot(path, i64::from(version), source) {
        Ok(snapshot) => snapshot.completions(byte_offset),
        Err(error) => {
            debug!(path = %path.display(), %error, "local completion index is unavailable");
            Vec::new()
        }
    };
    let mut seen = HashSet::<String>::new();
    let mut candidates = local
        .into_iter()
        .filter_map(|completion| {
            seen.insert(completion.name.clone()).then(|| {
                let rank = match completion.kind {
                    SemanticCompletionKind::Binding => 0,
                    SemanticCompletionKind::Arm | SemanticCompletionKind::Mold => 10,
                };
                RankedCompletion { completion, rank }
            })
        })
        .collect::<Vec<_>>();

    let direct_imports = resolved_hoon_imports(path, workspace);
    let mut imported_faces = HashMap::<String, PathBuf>::new();
    for import in &direct_imports {
        if let Some(face) = &import.face {
            imported_faces.insert(face.clone(), import.path.clone());
        }
    }
    for (name, dependency) in imported_faces {
        if seen.insert(name.clone()) {
            candidates.push(RankedCompletion {
                completion: SemanticCompletion {
                    name,
                    kind: SemanticCompletionKind::Arm,
                    detail: format!(
                        "imported Hoon gate — {}",
                        completion_path_label(&dependency, workspace)
                    ),
                },
                rank: 20,
            });
        }
    }

    let mut visited = HashSet::<PathBuf>::new();
    if let Ok(canonical) = workspace.sources.canonicalize(path) {
        visited.insert(canonical);
    }
    let mut frontier = direct_imports
        .into_iter()
        .map(|import| import.path)
        .collect::<Vec<_>>();
    let mut depth = 0u16;
    while !frontier.is_empty() {
        let mut next = Vec::new();
        let mut level = HashMap::<String, Vec<(SemanticCompletion, PathBuf)>>::new();
        for dependency in frontier {
            let identity = workspace
                .sources
                .canonicalize(&dependency)
                .unwrap_or_else(|_| dependency.clone());
            if !visited.insert(identity) {
                continue;
            }
            let Ok(dependency_source) = workspace.sources.read_to_string(&dependency) else {
                continue;
            };
            for completion in structural_completions(&dependency_source) {
                if !seen.contains(&completion.name) {
                    level
                        .entry(completion.name.clone())
                        .or_default()
                        .push((completion, dependency.clone()));
                }
            }
            next.extend(hoon_imports(&dependency, workspace));
        }
        for (name, mut definitions) in level {
            seen.insert(name);
            if definitions.len() == 1 {
                let (mut completion, dependency) = definitions.pop().expect("one definition");
                completion.detail = format!(
                    "imported {} — {}",
                    completion.detail,
                    completion_path_label(&dependency, workspace)
                );
                candidates.push(RankedCompletion {
                    completion,
                    rank: 100u16.saturating_add(depth),
                });
            }
        }
        frontier = next;
        depth = depth.saturating_add(1);
    }

    let prelude_source = workspace
        .sources
        .read_to_string(&workspace.prelude)
        .with_context(|| format!("failed to read prelude {}", workspace.prelude.display()))?;
    for mut completion in structural_completions(&prelude_source) {
        if seen.insert(completion.name.clone()) {
            completion.detail = format!(
                "standard library {} — {}",
                completion.detail,
                completion_path_label(&workspace.prelude, workspace)
            );
            candidates.push(RankedCompletion {
                completion,
                rank: 10_000,
            });
        }
    }

    candidates.retain(|candidate| candidate.completion.name.starts_with(prefix));
    candidates.sort_by(|left, right| {
        (left.rank, left.completion.name.as_str())
            .cmp(&(right.rank, right.completion.name.as_str()))
    });
    Ok(SemanticCompletionResult {
        replacement_range,
        candidates,
    })
}

fn completion_path_label(path: &Path, workspace: &SemanticWorkspace) -> String {
    path.strip_prefix(&workspace.dependencies)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn hoon_imports(path: &Path, workspace: &SemanticWorkspace) -> Vec<PathBuf> {
    resolved_hoon_imports(path, workspace)
        .into_iter()
        .map(|import| import.path)
        .collect()
}

fn resolved_hoon_imports(path: &Path, workspace: &SemanticWorkspace) -> Vec<ResolvedNativeImport> {
    match resolved_hoon_imports_result(path, workspace) {
        Ok(imports) => imports,
        Err(error) => {
            debug!(path = %path.display(), %error, "semantic import scan failed");
            Vec::new()
        }
    }
}

fn resolved_hoon_imports_result(
    path: &Path,
    workspace: &SemanticWorkspace,
) -> Result<Vec<ResolvedNativeImport>> {
    let imports = pipeline::resolve_native_imports_with_source(
        path,
        &workspace.dependencies,
        ScopeMode::Standard,
        &workspace.sources,
    )?;
    Ok(imports
        .into_iter()
        .filter(|import| import.kind == NativeImportKind::Hoon)
        .collect())
}

fn drain_semantic_events(
    connection: &Connection,
    state: &Arc<Mutex<EditorSnapshot>>,
    events: &Receiver<SemanticEvent>,
    pending: &mut HashMap<RequestId, PendingSemanticRequest>,
    type_facts: &HashMap<PathBuf, DocumentTypeFacts>,
) -> Result<()> {
    for event in events.try_iter() {
        let Some(request) = pending.remove(&event.id) else {
            continue;
        };
        if request.cancelled.load(Ordering::Acquire) {
            continue;
        }
        if request.path != event.path || request.version != event.version {
            send_request_error(
                connection,
                event.id,
                ErrorCode::InternalError,
                "semantic worker returned a mismatched document snapshot".to_string(),
            )?;
            continue;
        }
        let editor = lock_snapshot(state)?;
        let workspace_is_current = request
            .workspace_generation
            .is_none_or(|generation| generation == editor.generation);
        let is_current = workspace_is_current
            && (!request.document_bound
                || editor.documents.get(&request.path).is_some_and(|document| {
                    document.version == request.version
                        && document.text.as_str() == request.source.as_ref()
                }));
        drop(editor);
        if !is_current {
            request.cancelled.store(true, Ordering::Release);
            send_request_error(
                connection,
                event.id,
                ErrorCode::ServerCancelled,
                "document changed before the semantic query completed".to_string(),
            )?;
            continue;
        }

        let result = match event.result {
            SemanticQueryResult::DocumentSymbols(symbols) => {
                serde_json::to_value(Some(DocumentSymbolResponse::Nested(
                    semantic_symbols_to_lsp(&symbols, request.source.as_ref()),
                )))?
            }
            SemanticQueryResult::WorkspaceSymbols(symbols) => {
                let symbols = symbols
                    .into_iter()
                    .filter_map(|symbol| {
                        let range = semantic_range_to_lsp(symbol.source.as_ref(), symbol.range)?;
                        let uri = file_path_to_uri(&symbol.path).ok()?;
                        Some(WorkspaceSymbol {
                            name: symbol.name,
                            kind: semantic_symbol_kind_to_lsp(symbol.kind),
                            tags: None,
                            container_name: Some(symbol.path.display().to_string()),
                            location: OneOf::Left(Location::new(uri, range)),
                            data: None,
                        })
                    })
                    .collect();
                serde_json::to_value(Some(WorkspaceSymbolResponse::Nested(symbols)))?
            }
            SemanticQueryResult::Hover(hover) => {
                let inferred = request.hover_offset.and_then(|offset| {
                    inferred_type_at(
                        type_facts,
                        &request.path,
                        request.version,
                        request.source.as_ref(),
                        offset,
                    )
                });
                let hover = match (hover, inferred) {
                    (Some(mut hover), Some((_, summary))) => {
                        hover.markdown.push_str("\n\nInferred type: **`");
                        hover.markdown.push_str(&summary.replace('`', "\\`"));
                        hover.markdown.push_str("`**");
                        Some(Hover {
                            contents: HoverContents::Markup(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: hover.markdown,
                            }),
                            range: semantic_range_to_lsp(request.source.as_ref(), hover.range),
                        })
                    }
                    (Some(hover), None) => Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: hover.markdown,
                        }),
                        range: semantic_range_to_lsp(request.source.as_ref(), hover.range),
                    }),
                    (None, Some((range, summary))) => Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: format!("Inferred type: **`{}`**", summary.replace('`', "\\`")),
                        }),
                        range: semantic_range_to_lsp(request.source.as_ref(), range),
                    }),
                    (None, None) => None,
                };
                serde_json::to_value(hover)?
            }
            SemanticQueryResult::Definition(definition) => {
                let definition = definition
                    .map(|definition| -> Result<Option<GotoDefinitionResponse>> {
                        let uri = file_path_to_uri(&definition.path)?;
                        Ok(
                            semantic_range_to_lsp(definition.source.as_ref(), definition.range)
                                .map(|range| {
                                    GotoDefinitionResponse::Scalar(Location::new(uri, range))
                                }),
                        )
                    })
                    .transpose()?
                    .flatten();
                serde_json::to_value(definition)?
            }
            SemanticQueryResult::Completion(completion) => {
                let replacement_range =
                    semantic_range_to_lsp(request.source.as_ref(), completion.replacement_range);
                let items = replacement_range
                    .map(|replacement_range| {
                        completion
                            .candidates
                            .into_iter()
                            .map(|candidate| {
                                let name = candidate.completion.name;
                                CompletionItem {
                                    label: name.clone(),
                                    kind: Some(match candidate.completion.kind {
                                        SemanticCompletionKind::Binding => {
                                            CompletionItemKind::VARIABLE
                                        }
                                        SemanticCompletionKind::Arm => CompletionItemKind::FUNCTION,
                                        SemanticCompletionKind::Mold => CompletionItemKind::STRUCT,
                                    }),
                                    detail: Some(candidate.completion.detail),
                                    sort_text: Some(format!("{:05}-{}", candidate.rank, name)),
                                    text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                                        replacement_range, name,
                                    ))),
                                    ..CompletionItem::default()
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                serde_json::to_value(Some(CompletionResponse::List(CompletionList {
                    is_incomplete: false,
                    items,
                })))?
            }
            SemanticQueryResult::References(references) => {
                let references = references
                    .map(|references| -> Result<Vec<Location>> {
                        let mut locations = Vec::with_capacity(references.len());
                        for reference in references {
                            let Some(range) =
                                semantic_range_to_lsp(reference.source.as_ref(), reference.range)
                            else {
                                continue;
                            };
                            locations
                                .push(Location::new(file_path_to_uri(&reference.path)?, range));
                        }
                        Ok(locations)
                    })
                    .transpose()?;
                serde_json::to_value(references)?
            }
            SemanticQueryResult::PrepareRename(target) => {
                let target = target.and_then(|target| {
                    semantic_range_to_lsp(request.source.as_ref(), target.range).map(|range| {
                        PrepareRenameResponse::RangeWithPlaceholder {
                            range,
                            placeholder: target.name,
                        }
                    })
                });
                serde_json::to_value(target)?
            }
            SemanticQueryResult::Rename(documents) => {
                let edit = documents
                    .map(|documents| {
                        documents
                            .into_iter()
                            .map(|document| {
                                let edits = document
                                    .edits
                                    .into_iter()
                                    .filter_map(|edit| {
                                        semantic_range_to_lsp(document.source.as_ref(), edit.range)
                                            .map(|range| {
                                                OneOf::Left(TextEdit::new(range, edit.new_text))
                                            })
                                    })
                                    .collect::<Vec<_>>();
                                Ok::<_, anyhow::Error>(TextDocumentEdit {
                                    text_document: OptionalVersionedTextDocumentIdentifier {
                                        uri: file_path_to_uri(&document.path)?,
                                        version: document.version,
                                    },
                                    edits,
                                })
                            })
                            .collect::<Result<Vec<_>>>()
                            .map(|edits| WorkspaceEdit {
                                changes: None,
                                document_changes: Some(DocumentChanges::Edits(edits)),
                                change_annotations: None,
                            })
                    })
                    .transpose()?;
                serde_json::to_value(edit)?
            }
            SemanticQueryResult::RequestError { code, message } => {
                send_request_error(connection, event.id, code, message)?;
                continue;
            }
            SemanticQueryResult::Unavailable(message) => {
                debug!(
                    path = %request.path.display(),
                    %message,
                    "semantic snapshot unavailable"
                );
                serde_json::Value::Null
            }
        };
        connection
            .sender
            .send(Response::new_ok(event.id, result).into())?;
    }
    Ok(())
}

fn inferred_type_at(
    type_facts: &HashMap<PathBuf, DocumentTypeFacts>,
    path: &Path,
    version: i32,
    source: &str,
    byte_offset: u32,
) -> Option<(SemanticTextRange, String)> {
    let document = type_facts.get(path)?;
    if document.version != version {
        return None;
    }
    document
        .facts
        .iter()
        .filter_map(|fact| {
            let location = &fact.location;
            let range = range_from_one_based_spot(
                source, location.start_line?, location.start_col?, location.end_line?,
                location.end_col?,
            )?;
            if range.contains(byte_offset) {
                Some((range, fact.type_summary.clone()))
            } else {
                None
            }
        })
        .min_by_key(|(range, _)| range.end.saturating_sub(range.start))
}

fn definition_at(
    resolution_facts: &HashMap<PathBuf, DocumentResolutionFacts>,
    path: &Path,
    version: i32,
    source: &str,
    byte_offset: u32,
) -> Option<(PathBuf, CompilerErrorLocation)> {
    let document = resolution_facts.get(path)?;
    if document.version != version {
        return None;
    }
    document
        .facts
        .iter()
        .flat_map(|fact| {
            compiler_resolution_use_ranges(source, fact)
                .into_iter()
                .map(move |range| (range, fact))
        })
        .filter(|(range, _)| range.contains(byte_offset))
        .min_by_key(|(range, _)| range.end.saturating_sub(range.start))
        .map(|(_, fact)| (document.target.clone(), fact.definition_location.clone()))
}

type CompilerDefinitionIdentity = (
    PathBuf,
    Option<u64>,
    Option<u64>,
    Option<u64>,
    Option<u64>,
    String,
);

fn compiler_workspace_references_at(
    snapshot: &EditorSnapshot,
    path: &Path,
    source: &str,
    byte_offset: u32,
    include_declaration: bool,
    dependencies: &Path,
    index: &WorkspaceResolutionFacts,
) -> Result<Option<Vec<Location>>> {
    if index.generation != snapshot.generation {
        return Ok(None);
    }
    let Some(name) = hoon_term_at(source, byte_offset) else {
        return Ok(None);
    };
    let Some(candidate_indices) = index.by_name.get(name) else {
        return Ok(None);
    };
    let target_fact = candidate_indices
        .iter()
        .map(|candidate| &index.facts[*candidate])
        .filter(|fact| {
            let use_path =
                compiler_location_path(Some(&fact.use_location), &index.target, dependencies);
            paths_match(&use_path, path)
                && compiler_resolution_use_ranges(source, fact)
                    .into_iter()
                    .any(|range| range.contains(byte_offset))
        })
        .min_by_key(|fact| {
            compiler_resolution_use_ranges(source, fact)
                .into_iter()
                .filter(|range| range.contains(byte_offset))
                .map(|range| range.end.saturating_sub(range.start))
                .min()
                .unwrap_or(u32::MAX)
        })
        .or_else(|| {
            candidate_indices
                .iter()
                .map(|candidate| &index.facts[*candidate])
                .filter(|fact| {
                    let definition_path = compiler_location_path(
                        Some(&fact.definition_location),
                        &index.target,
                        dependencies,
                    );
                    paths_match(&definition_path, path)
                        && compiler_location_range(source, &fact.definition_location)
                            .is_some_and(|range| range.contains(byte_offset))
                })
                .min_by_key(|fact| {
                    compiler_location_range(source, &fact.definition_location)
                        .map(|range| range.end.saturating_sub(range.start))
                        .unwrap_or(u32::MAX)
                })
        });
    let Some(target_fact) = target_fact else {
        return Ok(None);
    };
    let definition_path = compiler_location_path(
        Some(&target_fact.definition_location),
        &index.target,
        dependencies,
    );
    if paths_match(&definition_path, &index.prelude) {
        return Ok(None);
    }
    let target_identity = compiler_definition_identity(target_fact, &index.target, dependencies);
    let Some(reference_indices) = index.by_definition.get(&target_identity) else {
        return Ok(None);
    };
    let mut source_cache = HashMap::<PathBuf, (Uri, String)>::new();
    let mut references = Vec::<Location>::new();
    for fact in reference_indices
        .iter()
        .map(|reference| &index.facts[*reference])
    {
        let use_path =
            compiler_location_path(Some(&fact.use_location), &index.target, dependencies);
        let cache_key = normalized_path(&use_path);
        if !source_cache.contains_key(&cache_key) {
            match editor_source(snapshot, &use_path) {
                Ok(document) => {
                    source_cache.insert(cache_key.clone(), document);
                }
                Err(error) => {
                    debug!(path = %use_path.display(), %error, "reference source is unavailable");
                    continue;
                }
            }
        }
        let Some((uri, use_source)) = source_cache.get(&cache_key) else {
            continue;
        };
        references.extend(
            compiler_resolution_use_ranges(use_source, fact)
                .into_iter()
                .filter_map(|range| {
                    semantic_range_to_lsp(use_source, range)
                        .map(|range| Location::new(uri.clone(), range))
                }),
        );
        if let Some(anchor) = compiler_qualified_reference_anchor(use_source, fact) {
            references.extend(
                qualified_reference_ranges(use_source, &anchor, &fact.name)
                    .into_iter()
                    .filter_map(|range| {
                        semantic_range_to_lsp(use_source, range)
                            .map(|range| Location::new(uri.clone(), range))
                    }),
            );
        }
    }
    if include_declaration {
        let Some(declaration) = compiler_definition_to_lsp(
            snapshot, &index.target, dependencies, &target_fact.definition_location,
        )?
        else {
            return Ok(None);
        };
        references.push(declaration);
    }
    references.sort_by(|left, right| {
        (
            left.uri.as_str(),
            left.range.start.line,
            left.range.start.character,
            left.range.end.line,
            left.range.end.character,
        )
            .cmp(&(
                right.uri.as_str(),
                right.range.start.line,
                right.range.start.character,
                right.range.end.line,
                right.range.end.character,
            ))
    });
    references.dedup();
    Ok(Some(references))
}

fn compiler_definition_identity(
    fact: &CompilerResolutionFact,
    target: &Path,
    dependencies: &Path,
) -> CompilerDefinitionIdentity {
    let location = &fact.definition_location;
    (
        normalized_path(&compiler_location_path(
            Some(location),
            target,
            dependencies,
        )),
        location.start_line,
        location.start_col,
        location.end_line,
        location.end_col,
        fact.name.clone(),
    )
}

fn compiler_location_range(
    source: &str,
    location: &CompilerErrorLocation,
) -> Option<SemanticTextRange> {
    range_from_one_based_spot(
        source, location.start_line?, location.start_col?, location.end_line?, location.end_col?,
    )
}

fn normalized_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn paths_match(left: &Path, right: &Path) -> bool {
    left == right || normalized_path(left) == normalized_path(right)
}

fn editor_source(snapshot: &EditorSnapshot, path: &Path) -> Result<(Uri, String)> {
    let open_document = snapshot.documents.get(path).or_else(|| {
        snapshot
            .documents
            .iter()
            .find_map(|(candidate, document)| paths_match(candidate, path).then_some(document))
    });
    match open_document {
        Some(document) => Ok((document.uri.clone(), document.text.clone())),
        None => Ok((
            file_path_to_uri(path)?,
            std::fs::read_to_string(path)
                .with_context(|| format!("failed to read semantic source {}", path.display()))?,
        )),
    }
}

fn compiler_resolution_use_ranges(
    source: &str,
    fact: &CompilerResolutionFact,
) -> Vec<SemanticTextRange> {
    let location = &fact.use_location;
    let Some(enclosing) = (|| {
        range_from_one_based_spot(
            source, location.start_line?, location.start_col?, location.end_line?,
            location.end_col?,
        )
    })() else {
        return Vec::new();
    };
    if fact.name == "$" {
        return first_hoon_term_range(source, enclosing)
            .into_iter()
            .collect();
    }
    let Ok(start) = usize::try_from(enclosing.start) else {
        return Vec::new();
    };
    let Ok(end) = usize::try_from(enclosing.end) else {
        return Vec::new();
    };
    let Some(haystack) = source.get(start..end) else {
        return Vec::new();
    };
    let mut ranges = Vec::new();
    for (relative, _) in haystack.match_indices(&fact.name) {
        let match_start = start + relative;
        let match_end = match_start + fact.name.len();
        let is_term_byte =
            |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-';
        let left_boundary = match_start == 0 || !is_term_byte(source.as_bytes()[match_start - 1]);
        let right_boundary =
            match_end == source.len() || !is_term_byte(source.as_bytes()[match_end]);
        if left_boundary && right_boundary {
            ranges.push(SemanticTextRange {
                start: u32::try_from(match_start).expect("enclosing semantic range is u32-sized"),
                end: u32::try_from(match_end).expect("enclosing semantic range is u32-sized"),
            });
        }
    }
    if ranges.is_empty() {
        vec![enclosing]
    } else {
        ranges
    }
}

fn compiler_qualified_reference_anchor(
    source: &str,
    fact: &CompilerResolutionFact,
) -> Option<String> {
    let name_range = compiler_resolution_use_ranges(source, fact)
        .into_iter()
        .find(|range| {
            let Ok(start) = usize::try_from(range.start) else {
                return false;
            };
            let Ok(end) = usize::try_from(range.end) else {
                return false;
            };
            source.get(start..end) == Some(fact.name.as_str())
        })?;
    let start = usize::try_from(name_range.start).ok()?;
    let end = usize::try_from(name_range.end).ok()?;
    let suffix = source.get(end..)?.strip_prefix(':')?;
    let qualifier_len = suffix
        .bytes()
        .take_while(|byte| is_hoon_term_byte(*byte))
        .count();
    if qualifier_len == 0 {
        return None;
    }
    source
        .get(start..end + 1 + qualifier_len)
        .map(str::to_owned)
}

fn qualified_reference_ranges(source: &str, anchor: &str, name: &str) -> Vec<SemanticTextRange> {
    source
        .match_indices(anchor)
        .filter_map(|(start, _)| {
            let end = start.checked_add(anchor.len())?;
            let left_boundary = start == 0 || !is_hoon_term_byte(source.as_bytes()[start - 1]);
            let right_boundary = end == source.len()
                || !is_hoon_term_byte(*source.as_bytes().get(end)?)
                    && source.as_bytes().get(end) != Some(&b':');
            if !left_boundary || !right_boundary {
                return None;
            }
            Some(SemanticTextRange {
                start: u32::try_from(start).ok()?,
                end: u32::try_from(start.checked_add(name.len())?).ok()?,
            })
        })
        .collect()
}

fn is_hoon_term_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
}

fn first_hoon_term_range(source: &str, enclosing: SemanticTextRange) -> Option<SemanticTextRange> {
    let start = usize::try_from(enclosing.start).ok()?;
    let end = usize::try_from(enclosing.end).ok()?;
    let bytes = source.get(start..end)?.as_bytes();
    let relative_start = bytes.iter().position(u8::is_ascii_lowercase)?;
    let relative_end = bytes[relative_start..]
        .iter()
        .position(|byte| !is_hoon_term_byte(*byte))
        .map_or(bytes.len(), |length| relative_start + length);
    Some(SemanticTextRange {
        start: u32::try_from(start.checked_add(relative_start)?).ok()?,
        end: u32::try_from(start.checked_add(relative_end)?).ok()?,
    })
}

fn compiler_definition_to_lsp(
    snapshot: &EditorSnapshot,
    target: &Path,
    dependencies: &Path,
    location: &CompilerErrorLocation,
) -> Result<Option<Location>> {
    let path = compiler_location_path(Some(location), target, dependencies);
    let open_document = snapshot.documents.get(&path).or_else(|| {
        let canonical = path.canonicalize().ok()?;
        snapshot.documents.iter().find_map(|(candidate, document)| {
            (candidate.canonicalize().ok().as_deref() == Some(canonical.as_path()))
                .then_some(document)
        })
    });
    let (uri, source) = match open_document {
        Some(document) => (document.uri.clone(), document.text.clone()),
        None => (
            file_path_to_uri(&path)?,
            std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read definition source {}", path.display()))?,
        ),
    };
    let Some(range) = range_from_one_based_spot(
        &source,
        location
            .start_line
            .context("definition has no start line")?,
        location
            .start_col
            .context("definition has no start column")?,
        location.end_line.context("definition has no end line")?,
        location.end_col.context("definition has no end column")?,
    ) else {
        return Ok(None);
    };
    Ok(semantic_range_to_lsp(&source, range).map(|range| Location::new(uri, range)))
}

fn semantic_symbols_to_lsp(symbols: &[SemanticSymbol], source: &str) -> Vec<DocumentSymbol> {
    let mut children = HashMap::<Option<SemanticNodeId>, Vec<&SemanticSymbol>>::new();
    for symbol in symbols {
        children.entry(symbol.parent).or_default().push(symbol);
    }

    fn convert(
        symbol: &SemanticSymbol,
        source: &str,
        children: &HashMap<Option<SemanticNodeId>, Vec<&SemanticSymbol>>,
    ) -> DocumentSymbol {
        let empty_range = || Range::new(Position::new(0, 0), Position::new(0, 0));
        #[allow(deprecated)]
        DocumentSymbol {
            name: symbol.name.clone(),
            detail: Some(symbol.detail.clone()),
            kind: semantic_symbol_kind_to_lsp(symbol.kind),
            tags: None,
            deprecated: None,
            range: semantic_range_to_lsp(source, symbol.range).unwrap_or_else(empty_range),
            selection_range: semantic_range_to_lsp(source, symbol.selection_range)
                .unwrap_or_else(empty_range),
            children: children.get(&Some(symbol.id)).map(|nested| {
                nested
                    .iter()
                    .map(|child| convert(child, source, children))
                    .collect()
            }),
        }
    }

    children
        .remove(&None)
        .unwrap_or_default()
        .into_iter()
        .map(|symbol| convert(symbol, source, &children))
        .collect()
}

fn semantic_symbol_kind_to_lsp(kind: SemanticSymbolKind) -> SymbolKind {
    match kind {
        SemanticSymbolKind::Arm => SymbolKind::FUNCTION,
        SemanticSymbolKind::Mold => SymbolKind::STRUCT,
    }
}

fn semantic_range_to_lsp(source: &str, range: SemanticTextRange) -> Option<Range> {
    Some(Range::new(
        byte_to_lsp_position(source, range.start)?,
        byte_to_lsp_position(source, range.end)?,
    ))
}

fn lsp_position_to_byte(source: &str, position: Position) -> Option<u32> {
    let (start, end) = source_line_bounds(source, position.line)?;
    let line = &source[start..end];
    let target = usize::try_from(position.character).ok()?;
    let mut utf16_column = 0usize;
    for (byte, character) in line.char_indices() {
        if utf16_column == target {
            return u32::try_from(start + byte).ok();
        }
        utf16_column += character.len_utf16();
        if utf16_column > target {
            return None;
        }
    }
    (utf16_column == target)
        .then(|| u32::try_from(end).ok())
        .flatten()
}

fn byte_to_lsp_position(source: &str, byte_offset: u32) -> Option<Position> {
    let byte_offset = usize::try_from(byte_offset).ok()?;
    if byte_offset > source.len() || !source.is_char_boundary(byte_offset) {
        return None;
    }
    let prefix = &source[..byte_offset];
    let line = u32::try_from(prefix.bytes().filter(|byte| *byte == b'\n').count()).ok()?;
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    let character = u32::try_from(source[line_start..byte_offset].encode_utf16().count()).ok()?;
    Some(Position::new(line, character))
}

fn source_line_bounds(source: &str, line: u32) -> Option<(usize, usize)> {
    let mut start = 0usize;
    for _ in 0..line {
        start += source[start..].find('\n')? + 1;
    }
    let mut end = source[start..]
        .find('\n')
        .map_or(source.len(), |relative| start + relative);
    if end > start && source.as_bytes()[end - 1] == b'\r' {
        end -= 1;
    }
    Some((start, end))
}

fn parse_notification<T: serde::de::DeserializeOwned>(notification: Notification) -> Result<T> {
    serde_json::from_value(notification.params)
        .with_context(|| format!("invalid parameters for {}", notification.method))
}

fn lock_snapshot(
    state: &Arc<Mutex<EditorSnapshot>>,
) -> Result<std::sync::MutexGuard<'_, EditorSnapshot>> {
    state
        .lock()
        .map_err(|_| anyhow!("editor state lock was poisoned"))
}

fn schedule_check(trigger: &Sender<()>) {
    match trigger.try_send(()) {
        Ok(()) | Err(TrySendError::Full(())) => {}
        Err(TrySendError::Disconnected(())) => {}
    }
}

fn spawn_check_worker(
    config: ResolvedConfig,
    state: Arc<Mutex<EditorSnapshot>>,
    trigger: Receiver<()>,
    events: Sender<WorkerEvent>,
    stopping: Arc<AtomicBool>,
) -> Result<std::thread::JoinHandle<()>> {
    let worker = std::thread::Builder::new()
        .name("honk-lsp-checks".to_string())
        .spawn(move || {
            if let Err(error) = check_worker_loop(config, state, trigger, &events, &stopping) {
                error!(%error, "honk LSP check worker stopped");
                let _ = events.send(WorkerEvent::Error {
                    generation: u64::MAX,
                    message: error.to_string(),
                });
            }
        })
        .context("failed to spawn honk LSP check worker")?;
    Ok(worker)
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_string())
}

fn check_worker_loop(
    config: ResolvedConfig,
    state: Arc<Mutex<EditorSnapshot>>,
    trigger: Receiver<()>,
    events: &Sender<WorkerEvent>,
    stopping: &AtomicBool,
) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .context("failed to create LSP compiler runtime")?;
    let mut compiler: Option<CompilerHandle> = None;
    let mut applied = HashMap::<PathBuf, OpenDocument>::new();

    while !stopping.load(Ordering::Acquire) {
        if trigger.recv().is_err() {
            break;
        }
        if stopping.load(Ordering::Acquire) {
            break;
        }
        debounce(&trigger, config.check_delay, stopping);
        let mut snapshot = lock_snapshot(&state)?.clone();
        let Some(mut target) = snapshot.target(config.entry.as_deref()) else {
            continue;
        };

        if compiler.is_none() {
            info!("initializing persistent honk editor compiler");
            let initial_documents = snapshot
                .documents
                .iter()
                .map(|(path, document)| DocumentUpdate {
                    path: path.clone(),
                    version: i64::from(document.version),
                    text: document.text.clone(),
                })
                .collect::<Vec<_>>();
            let service = CompilerService::spawn_with_documents(
                CompilerServiceConfig {
                    workspace: config.workspace.clone(),
                    max_compiles: config.max_compiles,
                    worker_stack_bytes: config.worker_stack_bytes,
                },
                initial_documents,
            )
            .context("failed to initialize persistent honk editor compiler")?;
            compiler = Some(service.handle());
            applied.clone_from(&snapshot.documents);

            // Initialization can be expensive. Collapse edits received while
            // the compiler was starting into the first actual check.
            snapshot = lock_snapshot(&state)?.clone();
            let Some(latest_target) = snapshot.target(config.entry.as_deref()) else {
                continue;
            };
            target = latest_target;
        }

        let handle = compiler.as_ref().context("compiler handle missing")?;
        reconcile_documents(&runtime, handle, &snapshot.documents, &mut applied)?;
        let output = runtime
            .block_on(handle.check(WorkspaceCheckRequest {
                entry: target.clone(),
            }))
            .context("honk editor check request failed")?;
        let (diagnostic, semantic_facts, resolution_facts) = match output.result {
            Ok(check) => {
                debug!(
                    target = %target.display(),
                    path_hits = check.cache_stats.path_hits,
                    path_misses = check.cache_stats.path_misses,
                    invalidated_paths = check.cache_stats.invalidated_paths,
                    check_hits = check.cache_stats.check_hits,
                    check_misses = check.cache_stats.check_misses,
                    invalidated_checks = check.cache_stats.invalidated_checks,
                    "honk editor check cache result"
                );
                (None, check.semantic_facts, check.resolution_facts)
            }
            Err(WorkspaceCompileError { diagnostic }) => (Some(diagnostic), Vec::new(), Vec::new()),
        };
        events
            .send(WorkerEvent::Checked {
                generation: snapshot.generation,
                target,
                diagnostic,
                semantic_facts,
                resolution_facts,
            })
            .context("LSP event loop stopped")?;

        if output.restart_required {
            info!("rotating honk editor compiler after configured operation limit");
            compiler = None;
            applied.clear();
        }
    }
    Ok(())
}

fn debounce(trigger: &Receiver<()>, delay: Duration, stopping: &AtomicBool) {
    if delay.is_zero() {
        while trigger.try_recv().is_ok() {}
        return;
    }
    while !stopping.load(Ordering::Acquire) {
        match trigger.recv_timeout(delay) {
            Ok(()) => {}
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn reconcile_documents(
    runtime: &tokio::runtime::Runtime,
    compiler: &CompilerHandle,
    current: &HashMap<PathBuf, OpenDocument>,
    applied: &mut HashMap<PathBuf, OpenDocument>,
) -> Result<()> {
    let closed = applied
        .keys()
        .filter(|path| !current.contains_key(*path))
        .cloned()
        .collect::<Vec<_>>();
    for path in closed {
        runtime
            .block_on(compiler.close_document(path.clone()))
            .with_context(|| format!("failed to close editor document {}", path.display()))?;
        applied.remove(&path);
    }

    let mut paths = current.keys().cloned().collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        let document = current.get(&path).context("document disappeared")?;
        if applied.get(&path) == Some(document) {
            continue;
        }
        runtime
            .block_on(compiler.update_document(DocumentUpdate {
                path: path.clone(),
                version: i64::from(document.version),
                text: document.text.clone(),
            }))
            .with_context(|| format!("failed to update editor document {}", path.display()))?;
        applied.insert(path, document.clone());
    }
    Ok(())
}

fn drain_worker_events(
    connection: &Connection,
    state: &Arc<Mutex<EditorSnapshot>>,
    config: &ResolvedConfig,
    events: &Receiver<WorkerEvent>,
    published: &mut HashSet<String>,
    semantics: &mut SemanticState,
) -> Result<()> {
    loop {
        match events.try_recv() {
            Ok(WorkerEvent::Checked {
                generation,
                target,
                diagnostic,
                semantic_facts,
                resolution_facts,
            }) => {
                let snapshot = lock_snapshot(state)?.clone();
                if generation != snapshot.generation {
                    debug!(
                        generation,
                        current = snapshot.generation,
                        "dropping stale diagnostics"
                    );
                    continue;
                }
                clear_published(connection, published)?;
                if let Some(diagnostic) = diagnostic {
                    semantics.type_facts.clear();
                    semantics.clear_resolution_facts();
                    let (uri, lsp_diagnostic) = workspace_diagnostic_to_lsp(
                        &diagnostic, &target, &config.workspace.dependencies,
                    )?;
                    let path = uri_to_file_path(&uri).ok();
                    let version = path
                        .as_ref()
                        .and_then(|path| snapshot.documents.get(path))
                        .map(|document| document.version);
                    publish_diagnostics(connection, uri.clone(), vec![lsp_diagnostic], version)?;
                    published.insert(uri.as_str().to_string());
                } else {
                    update_type_facts(
                        &snapshot, &target, &config.workspace.dependencies, semantic_facts,
                        &mut semantics.type_facts,
                    );
                    let workspace_facts = Arc::<[CompilerResolutionFact]>::from(resolution_facts);
                    update_resolution_facts(
                        &snapshot,
                        &target,
                        &config.workspace.dependencies,
                        workspace_facts.as_ref(),
                        &mut semantics.resolution_facts,
                    );
                    semantics.workspace_resolution = Some(WorkspaceResolutionFacts::new(
                        generation, target, &config.workspace.dependencies,
                        &config.workspace.prelude, workspace_facts,
                    ));
                }
            }
            Ok(WorkerEvent::Error {
                generation,
                message,
            }) => {
                let current = lock_snapshot(state)?.generation;
                if generation == u64::MAX || generation == current {
                    semantics.type_facts.clear();
                    semantics.clear_resolution_facts();
                    clear_published(connection, published)?;
                    send_show_message(connection, MessageType::ERROR, message)?;
                }
            }
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => break,
        }
    }
    Ok(())
}

fn clear_published(connection: &Connection, published: &mut HashSet<String>) -> Result<()> {
    for uri in published.drain() {
        let uri = Uri::from_str(&uri).context("stored invalid diagnostic URI")?;
        publish_diagnostics(connection, uri, Vec::new(), None)?;
    }
    Ok(())
}

fn update_type_facts(
    snapshot: &EditorSnapshot,
    target: &Path,
    dependencies: &Path,
    facts: Vec<CompilerSemanticFact>,
    current: &mut HashMap<PathBuf, DocumentTypeFacts>,
) {
    let mut by_path = HashMap::<PathBuf, Vec<CompilerSemanticFact>>::new();
    let canonical_target = target.canonicalize().ok();
    for fact in facts {
        let path = compiler_location_path_with_canonical_target(
            Some(&fact.location),
            target,
            canonical_target.as_deref(),
            dependencies,
        );
        if snapshot.documents.contains_key(&path) {
            by_path.entry(path).or_default().push(fact);
        }
    }
    for (path, document) in &snapshot.documents {
        match by_path.remove(path) {
            Some(facts) if !facts.is_empty() => {
                current.insert(
                    path.clone(),
                    DocumentTypeFacts {
                        version: document.version,
                        facts,
                    },
                );
            }
            _ if current
                .get(path)
                .is_some_and(|facts| facts.version != document.version) =>
            {
                current.remove(path);
            }
            _ => {}
        }
    }
}

fn update_resolution_facts(
    snapshot: &EditorSnapshot,
    target: &Path,
    dependencies: &Path,
    facts: &[CompilerResolutionFact],
    current: &mut HashMap<PathBuf, DocumentResolutionFacts>,
) {
    let mut by_path = HashMap::<PathBuf, Vec<CompilerResolutionFact>>::new();
    let canonical_target = target.canonicalize().ok();
    for fact in facts {
        let path = compiler_location_path_with_canonical_target(
            Some(&fact.use_location),
            target,
            canonical_target.as_deref(),
            dependencies,
        );
        if snapshot.documents.contains_key(&path) {
            by_path.entry(path).or_default().push(fact.clone());
        }
    }
    for (path, document) in &snapshot.documents {
        match by_path.remove(path) {
            Some(facts) if !facts.is_empty() => {
                current.insert(
                    path.clone(),
                    DocumentResolutionFacts {
                        version: document.version,
                        target: target.to_path_buf(),
                        facts,
                    },
                );
            }
            _ if current.get(path).is_some_and(|facts| {
                facts.version != document.version || facts.target != target
            }) =>
            {
                current.remove(path);
            }
            _ => {}
        }
    }
}

fn compiler_location_path(
    location: Option<&CompilerErrorLocation>,
    target: &Path,
    dependencies: &Path,
) -> PathBuf {
    let canonical_target = target.canonicalize().ok();
    compiler_location_path_with_canonical_target(
        location,
        target,
        canonical_target.as_deref(),
        dependencies,
    )
}

fn compiler_location_path_with_canonical_target(
    location: Option<&CompilerErrorLocation>,
    target: &Path,
    canonical_target: Option<&Path>,
    dependencies: &Path,
) -> PathBuf {
    location
        .and_then(|location| location.file.as_deref())
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else if target.ends_with(&path)
                || canonical_target.is_some_and(|canonical| canonical.ends_with(&path))
            {
                target.to_path_buf()
            } else {
                dependencies.join(path)
            }
        })
        .unwrap_or_else(|| target.to_path_buf())
}

fn workspace_diagnostic_to_lsp(
    diagnostic: &WorkspaceDiagnostic,
    target: &Path,
    dependencies: &Path,
) -> Result<(Uri, Diagnostic)> {
    let location = diagnostic.location.as_ref();
    let path = compiler_location_path(location, target, dependencies);
    let uri = file_path_to_uri(&path)?;
    let range = location.map_or_else(default_diagnostic_range, |location| {
        let start = Position::new(
            one_based_to_zero(location.start_line),
            one_based_to_zero(location.start_col),
        );
        let mut end = Position::new(
            one_based_to_zero(location.end_line.or(location.start_line)),
            one_based_to_zero(location.end_col.or(location.start_col)),
        );
        if end <= start {
            end = Position::new(start.line, start.character.saturating_add(1));
        }
        Range::new(start, end)
    });
    let code = match diagnostic.kind {
        WorkspaceDiagnosticKind::Parse => "parse",
        WorkspaceDiagnosticKind::UnsupportedExpr => "unsupported-expression",
        WorkspaceDiagnosticKind::Backend => "backend",
        WorkspaceDiagnosticKind::Decode => "decode",
        WorkspaceDiagnosticKind::Noun => "noun",
        WorkspaceDiagnosticKind::Io => "io",
        WorkspaceDiagnosticKind::Internal => "internal",
    };
    Ok((
        uri,
        Diagnostic::new(
            range,
            Some(DiagnosticSeverity::ERROR),
            Some(NumberOrString::String(format!("honk.{code}"))),
            Some("honk".to_string()),
            diagnostic.message.clone(),
            None,
            None,
        ),
    ))
}

fn one_based_to_zero(value: Option<u64>) -> u32 {
    value
        .unwrap_or(1)
        .saturating_sub(1)
        .min(u64::from(u32::MAX)) as u32
}

fn default_diagnostic_range() -> Range {
    Range::new(Position::new(0, 0), Position::new(0, 1))
}

fn publish_diagnostics(
    connection: &Connection,
    uri: Uri,
    diagnostics: Vec<Diagnostic>,
    version: Option<i32>,
) -> Result<()> {
    connection.sender.send(
        Notification::new(
            PublishDiagnostics::METHOD.to_string(),
            PublishDiagnosticsParams::new(uri, diagnostics, version),
        )
        .into(),
    )?;
    Ok(())
}

fn send_log(connection: &Connection, typ: MessageType, message: impl Into<String>) -> Result<()> {
    connection.sender.send(
        Notification::new(
            LogMessage::METHOD.to_string(),
            LogMessageParams {
                typ,
                message: message.into(),
            },
        )
        .into(),
    )?;
    Ok(())
}

fn send_show_message(
    connection: &Connection,
    typ: MessageType,
    message: impl Into<String>,
) -> Result<()> {
    connection.sender.send(
        Notification::new(
            ShowMessage::METHOD.to_string(),
            ShowMessageParams {
                typ,
                message: message.into(),
            },
        )
        .into(),
    )?;
    Ok(())
}

fn uri_to_file_path(uri: &Uri) -> Result<PathBuf> {
    let url = url::Url::parse(uri.as_str())
        .with_context(|| format!("invalid document URI: {}", uri.as_str()))?;
    if url.scheme() != "file" {
        bail!("honk-lsp only supports file URIs, received {url}");
    }
    url.to_file_path()
        .map_err(|()| anyhow!("cannot convert file URI to a path: {url}"))
}

fn file_path_to_uri(path: &Path) -> Result<Uri> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let url = url::Url::from_file_path(&absolute)
        .map_err(|()| anyhow!("cannot convert path to a file URI: {}", absolute.display()))?;
    Uri::from_str(url.as_str()).context("generated invalid file URI")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use honk::workspace::WorkspaceSourceSnapshot;
    use honk::{CompilerErrorLocation, CompilerResolutionFact, CompilerSemanticFact};
    use lsp_server::{Connection, ErrorCode, Message, RequestId, ResponseKind};
    use lsp_types::Position;
    use tempfile::TempDir;

    use super::{
        byte_to_lsp_position, cancel_semantic_request, definition_at, drain_semantic_events,
        drain_worker_events, file_path_to_uri, inferred_type_at, lsp_position_to_byte,
        uri_to_file_path, workspace_diagnostic_to_lsp, DocumentResolutionFacts, DocumentTypeFacts,
        EditorSnapshot, OpenDocument, PendingSemanticRequest, ResolvedConfig, SemanticEvent,
        SemanticQueryResult, SemanticState, SemanticWorkspace, StructuralWorkspaceGraph,
        WorkerEvent, WorkspaceConfig, WorkspaceDiagnostic, WorkspaceDiagnosticKind,
    };

    #[test]
    fn file_uri_round_trip_preserves_spaces() {
        let path = PathBuf::from("/tmp/honk lsp/demo.hoon");
        let uri = file_path_to_uri(&path).expect("file URI");
        assert_eq!(uri_to_file_path(&uri).expect("file path"), path);
    }

    #[test]
    fn structural_graph_refresh_reuses_unchanged_source_indexes() {
        let temp = TempDir::new().expect("temporary workspace");
        let prelude = temp.path().join("hoon.hoon");
        let changed = temp.path().join("changed.hoon");
        let unchanged = temp.path().join("unchanged.hoon");
        std::fs::write(
            &prelude, "|%\n++  list  |*  a=$-(* *)  [~ u=[i=a t=(list a)]]\n--\n",
        )
        .expect("prelude");
        std::fs::write(&changed, "|%\n+$  before  @\n--\n").expect("changed source");
        std::fs::write(&unchanged, "|%\n+$  stable  @\n--\n").expect("unchanged source");
        let workspace = SemanticWorkspace {
            dependencies: temp.path().to_path_buf(),
            prelude: prelude.clone(),
            roots: vec![changed.clone(), unchanged.clone()],
            sources: WorkspaceSourceSnapshot::try_new(1, std::iter::empty::<(PathBuf, String)>())
                .expect("initial source snapshot"),
            versions: HashMap::new(),
            path_revisions: HashMap::new(),
            layout_revision: 0,
        };
        let initial = StructuralWorkspaceGraph::refresh(&workspace, None);
        let unchanged_identity = super::semantic_path_identity(&unchanged, &workspace);
        let initial_unchanged = Arc::clone(
            &initial
                .documents
                .get(&unchanged_identity)
                .expect("unchanged document")
                .source,
        );
        let initial_unchanged_declarations = Arc::clone(
            &initial
                .documents
                .get(&unchanged_identity)
                .expect("unchanged document")
                .declarations,
        );

        std::fs::write(&changed, "|%\n+$  after  @\n--\n").expect("update changed source");
        let refreshed_workspace = SemanticWorkspace {
            sources: WorkspaceSourceSnapshot::try_new(2, std::iter::empty::<(PathBuf, String)>())
                .expect("refreshed source snapshot"),
            path_revisions: HashMap::from([(changed.clone(), 2)]),
            ..workspace
        };
        let refreshed = StructuralWorkspaceGraph::refresh(&refreshed_workspace, Some(&initial));
        let refreshed_unchanged = &refreshed
            .documents
            .get(&unchanged_identity)
            .expect("refreshed unchanged document")
            .source;
        assert!(Arc::ptr_eq(&initial_unchanged, refreshed_unchanged));
        assert!(Arc::ptr_eq(
            &initial_unchanged_declarations,
            &refreshed
                .documents
                .get(&unchanged_identity)
                .expect("refreshed unchanged document")
                .declarations
        ));
        assert!(refreshed
            .workspace_symbols("after")
            .iter()
            .any(|symbol| symbol.path == changed));
        assert!(refreshed.workspace_symbols("before").is_empty());
    }

    #[test]
    fn compiler_locations_are_converted_to_zero_based_lsp_ranges() {
        let target = PathBuf::from("/tmp/demo.hoon");
        let diagnostic = WorkspaceDiagnostic {
            kind: WorkspaceDiagnosticKind::Parse,
            message: "bad rune".to_string(),
            location: Some(CompilerErrorLocation {
                file: Some(target.display().to_string()),
                start_line: Some(3),
                start_col: Some(5),
                end_line: Some(3),
                end_col: Some(8),
                ..CompilerErrorLocation::default()
            }),
        };
        let (_, converted) =
            workspace_diagnostic_to_lsp(&diagnostic, &target, PathBuf::from("/tmp").as_path())
                .expect("LSP diagnostic");
        assert_eq!(converted.range.start.line, 2);
        assert_eq!(converted.range.start.character, 4);
        assert_eq!(converted.range.end.character, 7);
    }

    #[test]
    fn semantic_positions_round_trip_as_utf16() {
        let source = "😀 |=  a=@\n42\n";
        let rune_byte = u32::try_from(source.find('|').expect("rune byte")).expect("small source");
        let position = byte_to_lsp_position(source, rune_byte).expect("LSP position");
        assert_eq!(position, Position::new(0, 3));
        assert_eq!(lsp_position_to_byte(source, position), Some(rune_byte));
        assert_eq!(
            lsp_position_to_byte(source, Position::new(0, 1)),
            None,
            "a position inside an UTF-16 surrogate pair must be rejected"
        );
    }

    #[test]
    fn inferred_type_uses_the_narrowest_current_compiler_spot() {
        let path = PathBuf::from("/tmp/typed.hoon");
        let source = "|=  a=@\n  a\n";
        let offset = u32::try_from(source.rfind('a').expect("body offset")).expect("small source");
        let facts = HashMap::from([(
            path.clone(),
            DocumentTypeFacts {
                version: 7,
                facts: vec![
                    CompilerSemanticFact {
                        location: CompilerErrorLocation {
                            start_line: Some(1),
                            start_col: Some(1),
                            end_line: Some(2),
                            end_col: Some(4),
                            ..CompilerErrorLocation::default()
                        },
                        type_summary: "gate".to_string(),
                    },
                    CompilerSemanticFact {
                        location: CompilerErrorLocation {
                            start_line: Some(2),
                            start_col: Some(3),
                            end_line: Some(2),
                            end_col: Some(4),
                            ..CompilerErrorLocation::default()
                        },
                        type_summary: "@".to_string(),
                    },
                ],
            },
        )]);

        let (_, summary) =
            inferred_type_at(&facts, &path, 7, source, offset).expect("inferred type");
        assert_eq!(summary, "@");
        assert!(inferred_type_at(&facts, &path, 8, source, offset).is_none());
    }

    #[test]
    fn definition_uses_the_narrowest_current_compiler_resolution() {
        let path = PathBuf::from("/tmp/use.hoon");
        let target = PathBuf::from("/tmp/entry.hoon");
        let source = "(helper answer)\n";
        let offset =
            u32::try_from(source.find("answer").expect("answer offset")).expect("small source");
        let wide_definition = CompilerErrorLocation {
            file: Some("lib/helper.hoon".to_string()),
            start_line: Some(1),
            start_col: Some(1),
            end_line: Some(2),
            end_col: Some(3),
            ..CompilerErrorLocation::default()
        };
        let narrow_definition = CompilerErrorLocation {
            file: Some("lib/answer.hoon".to_string()),
            start_line: Some(4),
            start_col: Some(3),
            end_line: Some(4),
            end_col: Some(9),
            ..CompilerErrorLocation::default()
        };
        let wide_definition_for_assertion = wide_definition.clone();
        let facts = HashMap::from([(
            path.clone(),
            DocumentResolutionFacts {
                version: 7,
                target: target.clone(),
                facts: vec![
                    CompilerResolutionFact {
                        use_location: CompilerErrorLocation {
                            start_line: Some(1),
                            start_col: Some(1),
                            end_line: Some(1),
                            end_col: Some(16),
                            ..CompilerErrorLocation::default()
                        },
                        definition_location: wide_definition,
                        name: "helper".to_string(),
                    },
                    CompilerResolutionFact {
                        use_location: CompilerErrorLocation {
                            start_line: Some(1),
                            start_col: Some(9),
                            end_line: Some(1),
                            end_col: Some(15),
                            ..CompilerErrorLocation::default()
                        },
                        definition_location: narrow_definition.clone(),
                        name: "answer".to_string(),
                    },
                ],
            },
        )]);

        assert_eq!(
            definition_at(&facts, &path, 7, source, offset),
            Some((target, narrow_definition))
        );
        let helper_offset =
            u32::try_from(source.find("helper").expect("helper offset")).expect("small source");
        assert_eq!(
            definition_at(&facts, &path, 7, source, helper_offset),
            Some((
                PathBuf::from("/tmp/entry.hoon"),
                wide_definition_for_assertion
            ))
        );
        assert!(definition_at(&facts, &path, 7, source, 7).is_none());
        assert!(definition_at(&facts, &path, 8, source, offset).is_none());
    }

    #[test]
    fn stale_worker_generation_does_not_publish_diagnostics() {
        let (server, client) = Connection::memory();
        let state = Arc::new(Mutex::new(EditorSnapshot {
            generation: 2,
            ..EditorSnapshot::default()
        }));
        let (sender, receiver) = crossbeam_channel::unbounded();
        sender
            .send(WorkerEvent::Checked {
                generation: 1,
                target: PathBuf::from("/tmp/stale.hoon"),
                diagnostic: Some(WorkspaceDiagnostic {
                    kind: WorkspaceDiagnosticKind::Parse,
                    message: "stale".to_string(),
                    location: None,
                }),
                semantic_facts: Vec::new(),
                resolution_facts: Vec::new(),
            })
            .expect("worker event");
        let config = ResolvedConfig {
            workspace: WorkspaceConfig {
                prelude: PathBuf::from("/tmp/prelude.hoon"),
                dependencies: PathBuf::from("/tmp"),
                subject_type_jam: None,
                dbug: true,
                vet: true,
            },
            workspace_files: Default::default(),
            entry: None,
            max_compiles: 0,
            worker_stack_bytes: 1024 * 1024,
            check_delay: Duration::ZERO,
        };
        let mut published = std::collections::HashSet::new();
        let mut semantics = SemanticState::default();

        drain_worker_events(
            &server, &state, &config, &receiver, &mut published, &mut semantics,
        )
        .expect("drain stale event");

        assert!(client.receiver.try_recv().is_err());
    }

    #[test]
    fn client_cancellation_completes_the_semantic_request_once() {
        let (server, client) = Connection::memory();
        let id = RequestId::from(7);
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut pending = HashMap::from([(
            id.clone(),
            PendingSemanticRequest {
                path: PathBuf::from("/tmp/cancel.hoon"),
                version: 1,
                source: Arc::from("42\n"),
                cancelled: Arc::clone(&cancelled),
                hover_offset: None,
                workspace_generation: None,
                document_bound: true,
            },
        )]);

        cancel_semantic_request(
            &server,
            &mut pending,
            id.clone(),
            ErrorCode::RequestCanceled,
            "cancelled",
        )
        .expect("cancel semantic request");

        assert!(cancelled.load(Ordering::Acquire));
        assert!(pending.is_empty());
        let Message::Response(response) = client.receiver.recv().expect("cancel response") else {
            panic!("expected a cancellation response");
        };
        assert_eq!(response.id, id);
        let ResponseKind::Err { error } = response.response_kind else {
            panic!("expected a cancellation error");
        };
        assert_eq!(error.code, ErrorCode::RequestCanceled as i32);
        assert!(client.receiver.try_recv().is_err());
    }

    #[test]
    fn stale_semantic_results_are_server_cancelled() {
        let (server, client) = Connection::memory();
        let path = PathBuf::from("/tmp/stale-semantic.hoon");
        let uri = file_path_to_uri(&path).expect("file URI");
        let state = Arc::new(Mutex::new(EditorSnapshot {
            documents: HashMap::from([(
                path.clone(),
                OpenDocument {
                    uri,
                    version: 2,
                    text: "43\n".to_string(),
                },
            )]),
            ..EditorSnapshot::default()
        }));
        let id = RequestId::from(8);
        let mut pending = HashMap::from([(
            id.clone(),
            PendingSemanticRequest {
                path: path.clone(),
                version: 1,
                source: Arc::from("42\n"),
                cancelled: Arc::new(AtomicBool::new(false)),
                hover_offset: Some(0),
                workspace_generation: None,
                document_bound: true,
            },
        )]);
        let (sender, receiver) = crossbeam_channel::unbounded();
        sender
            .send(SemanticEvent {
                id: id.clone(),
                path,
                version: 1,
                result: SemanticQueryResult::Hover(None),
            })
            .expect("semantic event");

        drain_semantic_events(&server, &state, &receiver, &mut pending, &HashMap::new())
            .expect("drain stale semantic event");

        assert!(pending.is_empty());
        let Message::Response(response) = client.receiver.recv().expect("stale response") else {
            panic!("expected a stale-result response");
        };
        assert_eq!(response.id, id);
        let ResponseKind::Err { error } = response.response_kind else {
            panic!("expected a stale-result error");
        };
        assert_eq!(error.code, ErrorCode::ServerCancelled as i32);
    }
}
