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
use honk_service::semantic::{
    SemanticNodeId, SemanticSession, SemanticSnapshot, SemanticSymbol, SemanticSymbolKind,
    SemanticTextRange,
};
use honk_service::{CompilerHandle, CompilerService, CompilerServiceConfig, DocumentUpdate};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument, DidOpenTextDocument,
    DidSaveTextDocument, Exit, LogMessage, Notification as LspNotification, PublishDiagnostics,
    ShowMessage,
};
use lsp_types::request::{DocumentSymbolRequest, HoverRequest, Request as LspRequest};
use lsp_types::{
    Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentSymbol, DocumentSymbolParams,
    DocumentSymbolResponse, Hover, HoverContents, HoverParams, HoverProviderCapability,
    InitializeParams, InitializeResult, LogMessageParams, MarkupContent, MarkupKind, MessageType,
    NumberOrString, OneOf, Position, PositionEncodingKind, PublishDiagnosticsParams, Range,
    ServerCapabilities, ServerInfo, ShowMessageParams, SymbolKind, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextDocumentSyncOptions, TextDocumentSyncSaveOptions, Uri,
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
    },
    Error {
        generation: u64,
        message: String,
    },
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
        experimental: Some(serde_json::json!({
            "honk": {
                "lspBaseline": LSP_BASELINE_VERSION,
                "artifactFreeChecks": true,
                "fullDocumentSync": true,
                "semanticSnapshots": true
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

    let mut published = HashSet::<String>::new();
    let mut semantics = SemanticSession::default();
    let mut shutdown = false;
    while !shutdown {
        drain_worker_events(
            &connection, &state, &resolved, &event_receiver, &mut published,
        )?;
        match connection.receiver.recv_timeout(Duration::from_millis(25)) {
            Ok(Message::Request(request)) => {
                if connection.handle_shutdown(&request)? {
                    shutdown = true;
                } else {
                    handle_request(&connection, &state, &mut semantics, request)?;
                }
            }
            Ok(Message::Notification(notification)) => {
                if notification.method == Exit::METHOD {
                    shutdown = true;
                } else {
                    handle_notification(
                        &connection, &state, &trigger_sender, notification, &mut published,
                        &mut semantics,
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

    stopping.store(true, Ordering::Release);
    schedule_check(&trigger_sender);
    check_worker.join().map_err(|payload| {
        anyhow!(
            "honk LSP check worker panicked: {}",
            panic_message(payload.as_ref())
        )
    })?;
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
    semantics: &mut SemanticSession,
) -> Result<()> {
    match notification.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let params: DidOpenTextDocumentParams = parse_notification(notification)?;
            let path = uri_to_file_path(&params.text_document.uri)?;
            let mut snapshot = lock_snapshot(state)?;
            snapshot.generation = snapshot.generation.saturating_add(1);
            snapshot.active = Some(path.clone());
            snapshot.documents.insert(
                path,
                OpenDocument {
                    uri: params.text_document.uri,
                    version: params.text_document.version,
                    text: params.text_document.text,
                },
            );
            drop(snapshot);
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
                path,
                OpenDocument {
                    uri: params.text_document.uri,
                    version: params.text_document.version,
                    text: change.text.clone(),
                },
            );
            drop(snapshot);
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
            semantics.close(&path);
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
            schedule_check(trigger);
        }
        "$/cancelRequest" | "$/setTrace" => {}
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
    semantics: &mut SemanticSession,
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
            let result = document.and_then(|(path, document)| {
                match semantics.snapshot(&path, i64::from(document.version), &document.text) {
                    Ok(snapshot) => Some(DocumentSymbolResponse::Nested(semantic_symbols_to_lsp(
                        snapshot, &document.text,
                    ))),
                    Err(error) => {
                        debug!(%error, path = %path.display(), "semantic snapshot unavailable");
                        None
                    }
                }
            });
            connection
                .sender
                .send(Response::new_ok(request.id, result).into())?;
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
            let result = document.and_then(|(path, document)| {
                let byte_offset = lsp_position_to_byte(&document.text, position)?;
                let snapshot =
                    match semantics.snapshot(&path, i64::from(document.version), &document.text) {
                        Ok(snapshot) => snapshot,
                        Err(error) => {
                            debug!(%error, path = %path.display(), "semantic snapshot unavailable");
                            return None;
                        }
                    };
                let hover = snapshot.hover(byte_offset)?;
                Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: hover.markdown,
                    }),
                    range: semantic_range_to_lsp(&document.text, hover.range),
                })
            });
            connection
                .sender
                .send(Response::new_ok(request.id, result).into())?;
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

fn semantic_symbols_to_lsp(snapshot: &SemanticSnapshot, source: &str) -> Vec<DocumentSymbol> {
    let mut children = HashMap::<Option<SemanticNodeId>, Vec<&SemanticSymbol>>::new();
    for symbol in &snapshot.symbols {
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
        let diagnostic = match output.result {
            Ok(_) => None,
            Err(WorkspaceCompileError { diagnostic }) => Some(diagnostic),
        };
        events
            .send(WorkerEvent::Checked {
                generation: snapshot.generation,
                target,
                diagnostic,
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
) -> Result<()> {
    loop {
        match events.try_recv() {
            Ok(WorkerEvent::Checked {
                generation,
                target,
                diagnostic,
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
                }
            }
            Ok(WorkerEvent::Error {
                generation,
                message,
            }) => {
                let current = lock_snapshot(state)?.generation;
                if generation == u64::MAX || generation == current {
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

fn workspace_diagnostic_to_lsp(
    diagnostic: &WorkspaceDiagnostic,
    target: &Path,
    dependencies: &Path,
) -> Result<(Uri, Diagnostic)> {
    let location = diagnostic.location.as_ref();
    let path = location
        .and_then(|location| location.file.as_deref())
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                dependencies.join(path)
            }
        })
        .unwrap_or_else(|| target.to_path_buf());
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
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use honk::CompilerErrorLocation;
    use lsp_server::Connection;
    use lsp_types::Position;

    use super::{
        byte_to_lsp_position, drain_worker_events, file_path_to_uri, lsp_position_to_byte,
        uri_to_file_path, workspace_diagnostic_to_lsp, EditorSnapshot, ResolvedConfig, WorkerEvent,
        WorkspaceConfig, WorkspaceDiagnostic, WorkspaceDiagnosticKind,
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

        drain_worker_events(&server, &state, &config, &receiver, &mut published)
            .expect("drain stale event");

        assert!(client.receiver.try_recv().is_err());
    }
}
