use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};

use anyhow::{bail, Context, Result};
use honk::workspace::{
    normalize_workspace_document_path, WorkspaceArena, WorkspaceCheckOutput, WorkspaceCheckRequest,
    WorkspaceCompileError, WorkspaceCompileOutput, WorkspaceCompileRequest, WorkspaceCompiler,
    WorkspaceConfig, WorkspaceSourceSnapshot,
};
use tokio::sync::oneshot;

pub mod semantic;

pub const DEFAULT_MAX_COMPILES: u64 = 256;
pub const DEFAULT_WORKER_STACK_BYTES: usize = 4 * 1024 * 1024 * 1024;
const COMMAND_QUEUE_CAPACITY: usize = 8;

#[derive(Clone, Debug)]
pub struct CompilerServiceConfig {
    pub workspace: WorkspaceConfig,
    /// Zero disables automatic process rotation.
    pub max_compiles: u64,
    pub worker_stack_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentUpdate {
    pub path: PathBuf,
    pub version: i64,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DocumentUpdateResult {
    /// Monotonic revision across all accepted document changes.
    pub revision: u64,
}

#[derive(Debug)]
pub enum CompilerServiceError {
    QueueFull,
    Unavailable,
    ActorPanicked(String),
    InvalidDocumentPath(std::io::Error),
    StaleDocumentVersion {
        path: PathBuf,
        current: i64,
        received: i64,
    },
}

impl fmt::Display for CompilerServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QueueFull => formatter.write_str("compiler request queue is full"),
            Self::Unavailable => formatter.write_str("compiler actor is not available"),
            Self::ActorPanicked(message) => write!(formatter, "compiler actor panicked: {message}"),
            Self::InvalidDocumentPath(error) => write!(formatter, "invalid document path: {error}"),
            Self::StaleDocumentVersion {
                path,
                current,
                received,
            } => write!(
                formatter,
                "stale document version {received} for {}; current version is {current}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for CompilerServiceError {}

#[derive(Debug)]
pub struct ServiceCompileOutput {
    pub result: std::result::Result<WorkspaceCompileOutput, WorkspaceCompileError>,
    pub compile_index: u64,
    pub restart_required: bool,
    /// Document revision against which this request was compiled.
    pub document_revision: u64,
}

#[derive(Debug)]
pub struct ServiceCheckOutput {
    pub result: std::result::Result<WorkspaceCheckOutput, WorkspaceCompileError>,
    pub check_index: u64,
    pub restart_required: bool,
    /// Document revision against which this request was checked.
    pub document_revision: u64,
}

#[derive(Clone)]
pub struct CompilerHandle {
    sender: mpsc::SyncSender<CompilerCommand>,
    completed: Arc<AtomicU64>,
    max_compiles: u64,
}

impl CompilerHandle {
    pub fn completed_compiles(&self) -> u64 {
        self.completed.load(Ordering::Relaxed)
    }

    pub fn max_compiles(&self) -> u64 {
        self.max_compiles
    }

    pub async fn compile(
        &self,
        request: WorkspaceCompileRequest,
    ) -> std::result::Result<ServiceCompileOutput, CompilerServiceError> {
        let (reply, response) = oneshot::channel();
        self.try_send(CompilerCommand::Compile(CompileCommand { request, reply }))?;
        match response.await {
            Ok(ActorCompileReply::Completed(output)) => Ok(output),
            Ok(ActorCompileReply::Panicked(message)) => {
                Err(CompilerServiceError::ActorPanicked(message))
            }
            Err(_) => Err(CompilerServiceError::Unavailable),
        }
    }

    pub async fn check(
        &self,
        request: WorkspaceCheckRequest,
    ) -> std::result::Result<ServiceCheckOutput, CompilerServiceError> {
        let (reply, response) = oneshot::channel();
        self.try_send(CompilerCommand::Check(CheckCommand { request, reply }))?;
        match response.await {
            Ok(ActorCheckReply::Completed(output)) => Ok(output),
            Ok(ActorCheckReply::Panicked(message)) => {
                Err(CompilerServiceError::ActorPanicked(message))
            }
            Err(_) => Err(CompilerServiceError::Unavailable),
        }
    }

    pub async fn update_document(
        &self,
        update: DocumentUpdate,
    ) -> std::result::Result<DocumentUpdateResult, CompilerServiceError> {
        let (reply, response) = oneshot::channel();
        self.try_send(CompilerCommand::UpdateDocument { update, reply })?;
        response
            .await
            .map_err(|_| CompilerServiceError::Unavailable)?
    }

    pub async fn close_document(
        &self,
        path: PathBuf,
    ) -> std::result::Result<DocumentUpdateResult, CompilerServiceError> {
        let (reply, response) = oneshot::channel();
        self.try_send(CompilerCommand::CloseDocument { path, reply })?;
        response
            .await
            .map_err(|_| CompilerServiceError::Unavailable)?
    }

    fn try_send(&self, command: CompilerCommand) -> std::result::Result<(), CompilerServiceError> {
        match self.sender.try_send(command) {
            Ok(()) => Ok(()),
            Err(mpsc::TrySendError::Full(_)) => Err(CompilerServiceError::QueueFull),
            Err(mpsc::TrySendError::Disconnected(_)) => Err(CompilerServiceError::Unavailable),
        }
    }
}

pub struct CompilerService {
    handle: CompilerHandle,
    exhausted: oneshot::Receiver<()>,
}

impl CompilerService {
    pub fn spawn(config: CompilerServiceConfig) -> Result<Self> {
        Self::spawn_with_documents(config, std::iter::empty())
    }

    /// Starts a compiler epoch with the supplied editor documents already
    /// shadowing the filesystem.
    ///
    /// Seeding the initial snapshot avoids constructing a filesystem-only
    /// epoch and immediately replacing it when an editor's first request is
    /// for unsaved contents. The ordinary daemon/batch entry point continues
    /// to call [`Self::spawn`] with an empty snapshot.
    pub fn spawn_with_documents(
        config: CompilerServiceConfig,
        documents: impl IntoIterator<Item = DocumentUpdate>,
    ) -> Result<Self> {
        let mut store = DocumentStore::default();
        for document in documents {
            store.update(document).map_err(anyhow::Error::new)?;
        }
        let (exhausted_sender, exhausted) = oneshot::channel();
        let handle = spawn_compiler(config, exhausted_sender, store)?;
        Ok(Self { handle, exhausted })
    }

    pub fn handle(&self) -> CompilerHandle {
        self.handle.clone()
    }

    pub fn into_parts(self) -> (CompilerHandle, oneshot::Receiver<()>) {
        (self.handle, self.exhausted)
    }
}

struct CompileCommand {
    request: WorkspaceCompileRequest,
    reply: oneshot::Sender<ActorCompileReply>,
}

struct CheckCommand {
    request: WorkspaceCheckRequest,
    reply: oneshot::Sender<ActorCheckReply>,
}

enum CompilerWorkCommand {
    Compile(CompileCommand),
    Check(CheckCommand),
}

impl From<CompilerWorkCommand> for CompilerCommand {
    fn from(command: CompilerWorkCommand) -> Self {
        match command {
            CompilerWorkCommand::Compile(command) => Self::Compile(command),
            CompilerWorkCommand::Check(command) => Self::Check(command),
        }
    }
}

enum CompilerCommand {
    Compile(CompileCommand),
    Check(CheckCommand),
    UpdateDocument {
        update: DocumentUpdate,
        reply: oneshot::Sender<std::result::Result<DocumentUpdateResult, CompilerServiceError>>,
    },
    CloseDocument {
        path: PathBuf,
        reply: oneshot::Sender<std::result::Result<DocumentUpdateResult, CompilerServiceError>>,
    },
}

enum ActorCompileReply {
    Completed(ServiceCompileOutput),
    Panicked(String),
}

enum ActorCheckReply {
    Completed(ServiceCheckOutput),
    Panicked(String),
}

enum CompilerEpochExit {
    InputsChanged,
    DocumentsChanged,
    Closed,
    Exhausted,
}

#[derive(Clone)]
struct OpenDocument {
    version: i64,
    text: String,
}

#[derive(Default)]
struct DocumentStore {
    revision: u64,
    documents: HashMap<PathBuf, OpenDocument>,
}

impl DocumentStore {
    fn update(
        &mut self,
        update: DocumentUpdate,
    ) -> std::result::Result<(DocumentUpdateResult, bool), CompilerServiceError> {
        let path = normalize_workspace_document_path(&update.path)
            .map_err(CompilerServiceError::InvalidDocumentPath)?;
        if let Some(current) = self.documents.get(&path) {
            if update.version <= current.version {
                return Err(CompilerServiceError::StaleDocumentVersion {
                    path,
                    current: current.version,
                    received: update.version,
                });
            }
        }
        let contents_changed = self
            .documents
            .get(&path)
            .is_none_or(|current| current.text != update.text);
        self.documents.insert(
            path,
            OpenDocument {
                version: update.version,
                text: update.text,
            },
        );
        self.revision += 1;
        Ok((
            DocumentUpdateResult {
                revision: self.revision,
            },
            contents_changed,
        ))
    }

    fn close(
        &mut self,
        path: &Path,
    ) -> std::result::Result<(DocumentUpdateResult, bool), CompilerServiceError> {
        let path = normalize_workspace_document_path(path)
            .map_err(CompilerServiceError::InvalidDocumentPath)?;
        let contents_changed = self.documents.remove(&path).is_some();
        if contents_changed {
            self.revision += 1;
        }
        Ok((
            DocumentUpdateResult {
                revision: self.revision,
            },
            contents_changed,
        ))
    }

    fn snapshot(&self) -> std::io::Result<WorkspaceSourceSnapshot> {
        WorkspaceSourceSnapshot::try_new(
            self.revision,
            self.documents
                .iter()
                .map(|(path, document)| (path.clone(), document.text.clone())),
        )
    }
}

struct CompilerActorState {
    pending: Option<CompilerWorkCommand>,
    ready_sender: Option<mpsc::SyncSender<std::result::Result<(), String>>>,
    completed: Arc<AtomicU64>,
    max_compiles: u64,
    cache_invalidated: bool,
    exhausted: Option<oneshot::Sender<()>>,
    documents: DocumentStore,
}

fn signal_exhausted(exhausted: &mut Option<oneshot::Sender<()>>) {
    if let Some(exhausted) = exhausted.take() {
        let _ = exhausted.send(());
    }
}

fn run_compiler_epoch(
    compiler: &mut WorkspaceCompiler<'_>,
    receiver: &mpsc::Receiver<CompilerCommand>,
    state: &mut CompilerActorState,
) -> CompilerEpochExit {
    if let Some(ready_sender) = state.ready_sender.take() {
        let _ = ready_sender.send(Ok(()));
    }

    loop {
        let command = match state.pending.take() {
            Some(command) => command.into(),
            None => match receiver.recv() {
                Ok(command) => command,
                Err(_) => return CompilerEpochExit::Closed,
            },
        };

        match command {
            CompilerCommand::UpdateDocument { update, reply } => {
                let result = state.documents.update(update);
                let sources_changed = matches!(result, Ok((_, true)));
                let _ = reply.send(result.map(|(result, _)| result));
                if sources_changed {
                    state.cache_invalidated = true;
                    if !apply_document_snapshot(compiler, &state.documents) {
                        return CompilerEpochExit::DocumentsChanged;
                    }
                }
            }
            CompilerCommand::CloseDocument { path, reply } => {
                let result = state.documents.close(&path);
                let sources_changed = matches!(result, Ok((_, true)));
                let _ = reply.send(result.map(|(result, _)| result));
                if sources_changed {
                    state.cache_invalidated = true;
                    if !apply_document_snapshot(compiler, &state.documents) {
                        return CompilerEpochExit::DocumentsChanged;
                    }
                }
            }
            CompilerCommand::Compile(command) => {
                let inputs_changed =
                    panic::catch_unwind(AssertUnwindSafe(|| compiler.inputs_changed()));
                if matches!(inputs_changed, Ok(Ok(true))) {
                    state.pending = Some(CompilerWorkCommand::Compile(command));
                    state.cache_invalidated = true;
                    return CompilerEpochExit::InputsChanged;
                }

                let compile_index = state.completed.fetch_add(1, Ordering::Relaxed) + 1;
                let restart_required =
                    state.max_compiles != 0 && compile_index >= state.max_compiles;
                let document_revision = state.documents.revision;
                let result = match inputs_changed {
                    Ok(Ok(false)) => panic::catch_unwind(AssertUnwindSafe(|| {
                        compiler.compile_current(&command.request)
                    })),
                    Ok(Ok(true)) => unreachable!("handled above"),
                    Ok(Err(error)) => Ok(Err(error)),
                    Err(payload) => Err(payload),
                };

                match result {
                    Ok(Ok(mut output)) => {
                        output.cache_invalidated = state.cache_invalidated;
                        state.cache_invalidated = false;
                        let _ = command.reply.send(ActorCompileReply::Completed(
                            ServiceCompileOutput {
                                result: Ok(output),
                                compile_index,
                                restart_required,
                                document_revision,
                            },
                        ));
                    }
                    Ok(Err(error)) => {
                        let _ = command.reply.send(ActorCompileReply::Completed(
                            ServiceCompileOutput {
                                result: Err(error),
                                compile_index,
                                restart_required,
                                document_revision,
                            },
                        ));
                    }
                    Err(payload) => {
                        let _ =
                            command
                                .reply
                                .send(ActorCompileReply::Panicked(panic_payload_message(
                                    payload.as_ref(),
                                )));
                        signal_exhausted(&mut state.exhausted);
                        return CompilerEpochExit::Exhausted;
                    }
                }

                if restart_required {
                    signal_exhausted(&mut state.exhausted);
                    return CompilerEpochExit::Exhausted;
                }
            }
            CompilerCommand::Check(command) => {
                let inputs_changed =
                    panic::catch_unwind(AssertUnwindSafe(|| compiler.inputs_changed()));
                if matches!(inputs_changed, Ok(Ok(true))) {
                    state.pending = Some(CompilerWorkCommand::Check(command));
                    state.cache_invalidated = true;
                    return CompilerEpochExit::InputsChanged;
                }

                let check_index = state.completed.fetch_add(1, Ordering::Relaxed) + 1;
                let restart_required = state.max_compiles != 0 && check_index >= state.max_compiles;
                let document_revision = state.documents.revision;
                let result = match inputs_changed {
                    Ok(Ok(false)) => panic::catch_unwind(AssertUnwindSafe(|| {
                        compiler.check_current(&command.request)
                    })),
                    Ok(Ok(true)) => unreachable!("handled above"),
                    Ok(Err(error)) => Ok(Err(error)),
                    Err(payload) => Err(payload),
                };

                match result {
                    Ok(Ok(mut output)) => {
                        output.cache_invalidated = state.cache_invalidated;
                        state.cache_invalidated = false;
                        let _ =
                            command
                                .reply
                                .send(ActorCheckReply::Completed(ServiceCheckOutput {
                                    result: Ok(output),
                                    check_index,
                                    restart_required,
                                    document_revision,
                                }));
                    }
                    Ok(Err(error)) => {
                        let _ =
                            command
                                .reply
                                .send(ActorCheckReply::Completed(ServiceCheckOutput {
                                    result: Err(error),
                                    check_index,
                                    restart_required,
                                    document_revision,
                                }));
                    }
                    Err(payload) => {
                        let _ =
                            command
                                .reply
                                .send(ActorCheckReply::Panicked(panic_payload_message(
                                    payload.as_ref(),
                                )));
                        signal_exhausted(&mut state.exhausted);
                        return CompilerEpochExit::Exhausted;
                    }
                }

                if restart_required {
                    signal_exhausted(&mut state.exhausted);
                    return CompilerEpochExit::Exhausted;
                }
            }
        }
    }
}

fn apply_document_snapshot(
    compiler: &mut WorkspaceCompiler<'_>,
    documents: &DocumentStore,
) -> bool {
    documents
        .snapshot()
        .ok()
        .and_then(|sources| compiler.update_source_snapshot(sources).ok())
        .unwrap_or(false)
}

fn spawn_compiler(
    config: CompilerServiceConfig,
    exhausted: oneshot::Sender<()>,
    documents: DocumentStore,
) -> Result<CompilerHandle> {
    let (sender, receiver) = mpsc::sync_channel::<CompilerCommand>(COMMAND_QUEUE_CAPACITY);
    let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
    let completed = Arc::new(AtomicU64::new(0));
    let actor_completed = Arc::clone(&completed);
    let max_compiles = config.max_compiles;

    std::thread::Builder::new()
        .name("honk-compiler".to_string())
        .stack_size(config.worker_stack_bytes)
        .spawn(move || {
            let workspace_config = config.workspace;
            let mut state = CompilerActorState {
                pending: None,
                ready_sender: Some(ready_sender),
                completed: actor_completed,
                max_compiles,
                cache_invalidated: false,
                exhausted: Some(exhausted),
                documents,
            };

            loop {
                let sources = match state.documents.snapshot() {
                    Ok(sources) => sources,
                    Err(error) => {
                        if let Some(ready_sender) = state.ready_sender.take() {
                            let _ = ready_sender.send(Err(error.to_string()));
                        }
                        break;
                    }
                };
                let mut arena = WorkspaceArena::new();
                let epoch = panic::catch_unwind(AssertUnwindSafe(|| {
                    if sources.is_empty() {
                        arena.with_compiler(workspace_config.clone(), |compiler| {
                            run_compiler_epoch(compiler, &receiver, &mut state)
                        })
                    } else {
                        arena.with_source_snapshot(workspace_config.clone(), sources, |compiler| {
                            run_compiler_epoch(compiler, &receiver, &mut state)
                        })
                    }
                }));

                match epoch {
                    Ok(Ok(CompilerEpochExit::InputsChanged)) => {}
                    Ok(Ok(CompilerEpochExit::DocumentsChanged)) => {
                        if !wait_for_compile_after_document_change(&receiver, &mut state) {
                            break;
                        }
                    }
                    Ok(Ok(CompilerEpochExit::Closed | CompilerEpochExit::Exhausted)) => break,
                    Ok(Err(error)) => {
                        if let Some(ready_sender) = state.ready_sender.take() {
                            let _ = ready_sender.send(Err(error.to_string()));
                            break;
                        }
                        let command = match state.pending.take() {
                            Some(command) => command,
                            None => match receiver.recv() {
                                Ok(CompilerCommand::Compile(command)) => {
                                    CompilerWorkCommand::Compile(command)
                                }
                                Ok(CompilerCommand::Check(command)) => {
                                    CompilerWorkCommand::Check(command)
                                }
                                Ok(CompilerCommand::UpdateDocument { update, reply }) => {
                                    let result = state.documents.update(update);
                                    let _ = reply.send(result.map(|(result, _)| result));
                                    continue;
                                }
                                Ok(CompilerCommand::CloseDocument { path, reply }) => {
                                    let result = state.documents.close(&path);
                                    let _ = reply.send(result.map(|(result, _)| result));
                                    continue;
                                }
                                Err(_) => break,
                            },
                        };
                        let operation_index = state.completed.fetch_add(1, Ordering::Relaxed) + 1;
                        let restart_required =
                            state.max_compiles != 0 && operation_index >= state.max_compiles;
                        match command {
                            CompilerWorkCommand::Compile(command) => {
                                let _ = command.reply.send(ActorCompileReply::Completed(
                                    ServiceCompileOutput {
                                        result: Err(error),
                                        compile_index: operation_index,
                                        restart_required,
                                        document_revision: state.documents.revision,
                                    },
                                ));
                            }
                            CompilerWorkCommand::Check(command) => {
                                let _ = command.reply.send(ActorCheckReply::Completed(
                                    ServiceCheckOutput {
                                        result: Err(error),
                                        check_index: operation_index,
                                        restart_required,
                                        document_revision: state.documents.revision,
                                    },
                                ));
                            }
                        }
                        if restart_required {
                            signal_exhausted(&mut state.exhausted);
                            break;
                        }
                    }
                    Err(payload) => {
                        let message = panic_payload_message(payload.as_ref());
                        if let Some(ready_sender) = state.ready_sender.take() {
                            let _ = ready_sender
                                .send(Err(format!("compiler initialization panicked: {message}")));
                        } else if let Some(command) = state.pending.take() {
                            match command {
                                CompilerWorkCommand::Compile(command) => {
                                    let _ =
                                        command.reply.send(ActorCompileReply::Panicked(message));
                                }
                                CompilerWorkCommand::Check(command) => {
                                    let _ = command.reply.send(ActorCheckReply::Panicked(message));
                                }
                            }
                            signal_exhausted(&mut state.exhausted);
                        } else {
                            signal_exhausted(&mut state.exhausted);
                        }
                        break;
                    }
                }
            }
        })
        .context("failed to spawn honk compiler actor")?;

    match ready_receiver.recv() {
        Ok(Ok(())) => Ok(CompilerHandle {
            sender,
            completed,
            max_compiles,
        }),
        Ok(Err(error)) => bail!(error),
        Err(_) => bail!("honk compiler actor exited before initialization completed"),
    }
}

/// Coalesce editor notifications while no compiler epoch exists. This keeps
/// didChange/didClose acknowledgement cheap and builds exactly one fresh epoch
/// when the next compile or check request arrives.
fn wait_for_compile_after_document_change(
    receiver: &mpsc::Receiver<CompilerCommand>,
    state: &mut CompilerActorState,
) -> bool {
    loop {
        match receiver.recv() {
            Ok(CompilerCommand::Compile(command)) => {
                state.pending = Some(CompilerWorkCommand::Compile(command));
                return true;
            }
            Ok(CompilerCommand::Check(command)) => {
                state.pending = Some(CompilerWorkCommand::Check(command));
                return true;
            }
            Ok(CompilerCommand::UpdateDocument { update, reply }) => {
                let result = state.documents.update(update);
                let _ = reply.send(result.map(|(result, _)| result));
            }
            Ok(CompilerCommand::CloseDocument { path, reply }) => {
                let result = state.documents.close(&path);
                let _ = reply.send(result.map(|(result, _)| result));
            }
            Err(_) => return false,
        }
    }
}

fn panic_payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{CompilerServiceError, DocumentStore, DocumentUpdate};

    #[test]
    fn document_versions_are_strictly_monotonic() {
        let mut store = DocumentStore::default();
        let path = PathBuf::from("document.hoon");
        let (first, changed) = store
            .update(DocumentUpdate {
                path: path.clone(),
                version: 3,
                text: "42\n".to_string(),
            })
            .expect("first update");
        assert_eq!(first.revision, 1);
        assert!(changed);

        let stale = store
            .update(DocumentUpdate {
                path,
                version: 3,
                text: "43\n".to_string(),
            })
            .expect_err("duplicate version should be stale");
        assert!(matches!(
            stale,
            CompilerServiceError::StaleDocumentVersion { .. }
        ));
    }
}
