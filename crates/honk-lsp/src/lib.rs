use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TryRecvError, TrySendError};
use honk::workspace::{
    WorkspaceCheckRequest, WorkspaceCompileError, WorkspaceConfig, WorkspaceDiagnostic,
    WorkspaceDiagnosticKind,
};
use honk::{CompilerErrorLocation, CompilerResolutionFact, CompilerSemanticFact};
use honk_service::semantic::{
    range_from_one_based_spot, SemanticHover, SemanticNodeId, SemanticSession, SemanticSymbol,
    SemanticSymbolKind, SemanticTextRange,
};
use honk_service::{CompilerHandle, CompilerService, CompilerServiceConfig, DocumentUpdate};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    Cancel as CancelNotification, DidChangeTextDocument, DidChangeWatchedFiles,
    DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument, Exit, LogMessage,
    Notification as LspNotification, PublishDiagnostics, ShowMessage,
};
use lsp_types::request::{
    DocumentSymbolRequest, GotoDefinition, HoverRequest, Request as LspRequest,
};
use lsp_types::{
    CancelParams, Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverContents, HoverParams, HoverProviderCapability,
    InitializeParams, InitializeResult, Location, LogMessageParams, MarkupContent, MarkupKind,
    MessageType, NumberOrString, OneOf, Position, PositionEncodingKind, PublishDiagnosticsParams,
    Range, ServerCapabilities, ServerInfo, ShowMessageParams, SymbolKind,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
    TextDocumentSyncSaveOptions, Uri,
};
use serde::Deserialize;
use tracing::{debug, error, info, warn};

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
    active: Option<PathBuf>,
    documents: HashMap<PathBuf, OpenDocument>,
}

impl EditorSnapshot {
    fn target(&self, configured_entry: Option<&Path>) -> Option<PathBuf> {
        configured_entry
            .map(Path::to_path_buf)
            .or_else(|| self.active.clone())
            .or_else(|| self.documents.keys().min().cloned())
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
    Hover { byte_offset: u32 },
}

enum SemanticQueryResult {
    DocumentSymbols(Vec<SemanticSymbol>),
    Hover(Option<SemanticHover>),
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

#[derive(Default)]
struct SemanticState {
    pending: HashMap<RequestId, PendingSemanticRequest>,
    type_facts: HashMap<PathBuf, DocumentTypeFacts>,
    resolution_facts: HashMap<PathBuf, DocumentResolutionFacts>,
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
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
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
        "honk language server initialized"
    );

    let state = Arc::new(Mutex::new(EditorSnapshot::default()));
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

    Ok(ResolvedConfig {
        workspace: WorkspaceConfig {
            prelude,
            dependencies,
            subject_type_jam,
            dbug: config.dbug,
            vet: config.vet,
        },
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
            semantics.resolution_facts.clear();
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
            semantics.resolution_facts.clear();
            schedule_check(trigger);
        }
        DidCloseTextDocument::METHOD => {
            let params: DidCloseTextDocumentParams = parse_notification(notification)?;
            let path = uri_to_file_path(&params.text_document.uri)?;
            let mut snapshot = lock_snapshot(state)?;
            snapshot.generation = snapshot.generation.saturating_add(1);
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
            semantics.resolution_facts.clear();
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
            let _: DidSaveTextDocumentParams = parse_notification(notification)?;
            {
                let mut snapshot = lock_snapshot(state)?;
                snapshot.generation = snapshot.generation.saturating_add(1);
            }
            schedule_check(trigger);
        }
        DidChangeWatchedFiles::METHOD => {
            {
                let mut snapshot = lock_snapshot(state)?;
                snapshot.generation = snapshot.generation.saturating_add(1);
            }
            semantics.resolution_facts.clear();
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
            let definition = match definition_at(
                &semantics.resolution_facts, &path, document.version, &document.text, byte_offset,
            ) {
                Some((target, location)) => {
                    let snapshot = lock_snapshot(state)?.clone();
                    match compiler_definition_to_lsp(
                        &snapshot, &target, &config.workspace.dependencies, &location,
                    ) {
                        Ok(location) => location.map(GotoDefinitionResponse::Scalar),
                        Err(error) => {
                            warn!(%error, "compiler definition location is unavailable");
                            None
                        }
                    }
                }
                None => None,
            };
            connection
                .sender
                .send(Response::new_ok(request.id, serde_json::to_value(definition)?).into())?;
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
        SemanticQuery::DocumentSymbols => None,
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
                let result = match semantics.snapshot(&path, i64::from(version), source.as_ref()) {
                    Ok(snapshot) => match query {
                        SemanticQuery::DocumentSymbols => {
                            SemanticQueryResult::DocumentSymbols(snapshot.symbols.clone())
                        }
                        SemanticQuery::Hover { byte_offset } => {
                            SemanticQueryResult::Hover(snapshot.hover(byte_offset))
                        }
                    },
                    Err(error) => SemanticQueryResult::Unavailable(error.to_string()),
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
        let is_current = lock_snapshot(state)?
            .documents
            .get(&request.path)
            .is_some_and(|document| {
                document.version == request.version
                    && document.text.as_str() == request.source.as_ref()
            });
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
        return vec![enclosing];
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
            kind: match symbol.kind {
                SemanticSymbolKind::Arm => SymbolKind::FUNCTION,
                SemanticSymbolKind::Mold => SymbolKind::STRUCT,
            },
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
            Ok(check) => (None, check.semantic_facts, check.resolution_facts),
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
                    semantics.resolution_facts.clear();
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
                    update_resolution_facts(
                        &snapshot, &target, &config.workspace.dependencies, resolution_facts,
                        &mut semantics.resolution_facts,
                    );
                }
            }
            Ok(WorkerEvent::Error {
                generation,
                message,
            }) => {
                let current = lock_snapshot(state)?.generation;
                if generation == u64::MAX || generation == current {
                    semantics.type_facts.clear();
                    semantics.resolution_facts.clear();
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
    facts: Vec<CompilerResolutionFact>,
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
            by_path.entry(path).or_default().push(fact);
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

    use honk::{CompilerErrorLocation, CompilerResolutionFact, CompilerSemanticFact};
    use lsp_server::{Connection, ErrorCode, Message, RequestId, ResponseKind};
    use lsp_types::Position;

    use super::{
        byte_to_lsp_position, cancel_semantic_request, definition_at, drain_semantic_events,
        drain_worker_events, file_path_to_uri, inferred_type_at, lsp_position_to_byte,
        uri_to_file_path, workspace_diagnostic_to_lsp, DocumentResolutionFacts, DocumentTypeFacts,
        EditorSnapshot, OpenDocument, PendingSemanticRequest, ResolvedConfig, SemanticEvent,
        SemanticQueryResult, SemanticState, WorkerEvent, WorkspaceConfig, WorkspaceDiagnostic,
        WorkspaceDiagnosticKind,
    };

    #[test]
    fn file_uri_round_trip_preserves_spaces() {
        let path = PathBuf::from("/tmp/honk lsp/demo.hoon");
        let uri = file_path_to_uri(&path).expect("file URI");
        assert_eq!(uri_to_file_path(&uri).expect("file path"), path);
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
