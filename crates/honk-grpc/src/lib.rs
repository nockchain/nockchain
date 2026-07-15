use std::any::Any;
use std::future::Future;
use std::net::SocketAddr;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};

use anyhow::{bail, Context, Result};
use honk::workspace::{
    ArtifactMode, WorkspaceArena, WorkspaceCompileError, WorkspaceCompileOutput,
    WorkspaceCompileRequest, WorkspaceCompiler, WorkspaceConfig, WorkspaceDiagnostic,
    WorkspaceDiagnosticKind,
};
use honk_grpc_proto::v1::honk_compiler_server::{HonkCompiler, HonkCompilerServer};
use honk_grpc_proto::v1::{self as pb};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;
use tonic::{Request, Response, Status};
use tonic_reflection::server::Builder as ReflectionBuilder;

pub const PROTOCOL_VERSION: u32 = 1;
pub const PROTOCOL_NAME: &str = "honk.compiler.v1";
pub const DEFAULT_MAX_COMPILES: u64 = 256;
pub const DEFAULT_WORKER_STACK_BYTES: usize = 4 * 1024 * 1024 * 1024;
const COMMAND_QUEUE_CAPACITY: usize = 8;
const MAX_MESSAGE_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct DaemonConfig {
    pub workspace: WorkspaceConfig,
    /// Zero disables automatic process rotation.
    pub max_compiles: u64,
    pub worker_stack_bytes: usize,
}

#[derive(Clone)]
struct CompilerHandle {
    sender: mpsc::SyncSender<CompileCommand>,
    completed: Arc<AtomicU64>,
    max_compiles: u64,
}

struct CompileCommand {
    request: WorkspaceCompileRequest,
    reply: oneshot::Sender<ActorReply>,
}

enum ActorReply {
    Completed {
        result: std::result::Result<WorkspaceCompileOutput, WorkspaceCompileError>,
        compile_index: u64,
        restart_required: bool,
    },
    Panicked(String),
}

enum CompilerEpochExit {
    InputsChanged,
    Closed,
    Exhausted,
}

struct CompilerActorState {
    pending: Option<CompileCommand>,
    ready_sender: Option<mpsc::SyncSender<std::result::Result<(), String>>>,
    completed: Arc<AtomicU64>,
    max_compiles: u64,
    cache_invalidated: bool,
    exhausted: Option<oneshot::Sender<()>>,
}

fn signal_exhausted(exhausted: &mut Option<oneshot::Sender<()>>) {
    if let Some(exhausted) = exhausted.take() {
        let _ = exhausted.send(());
    }
}

fn run_compiler_epoch(
    compiler: &mut WorkspaceCompiler<'_>,
    receiver: &mpsc::Receiver<CompileCommand>,
    state: &mut CompilerActorState,
) -> CompilerEpochExit {
    if let Some(ready_sender) = state.ready_sender.take() {
        let _ = ready_sender.send(Ok(()));
    }

    loop {
        let command = match state.pending.take() {
            Some(command) => command,
            None => match receiver.recv() {
                Ok(command) => command,
                Err(_) => return CompilerEpochExit::Closed,
            },
        };

        let inputs_changed = panic::catch_unwind(AssertUnwindSafe(|| compiler.inputs_changed()));
        if matches!(inputs_changed, Ok(Ok(true))) {
            state.pending = Some(command);
            return CompilerEpochExit::InputsChanged;
        }

        let compile_index = state.completed.fetch_add(1, Ordering::Relaxed) + 1;
        let restart_required = state.max_compiles != 0 && compile_index >= state.max_compiles;
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
                let _ = command.reply.send(ActorReply::Completed {
                    result: Ok(output),
                    compile_index,
                    restart_required,
                });
            }
            Ok(Err(error)) => {
                let _ = command.reply.send(ActorReply::Completed {
                    result: Err(error),
                    compile_index,
                    restart_required,
                });
            }
            Err(payload) => {
                let _ = command
                    .reply
                    .send(ActorReply::Panicked(panic_payload_message(
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

impl CompilerHandle {
    fn spawn(config: DaemonConfig, exhausted: oneshot::Sender<()>) -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<CompileCommand>(COMMAND_QUEUE_CAPACITY);
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
                };

                loop {
                    let mut arena = WorkspaceArena::new();
                    let epoch = panic::catch_unwind(AssertUnwindSafe(|| {
                        arena.with_compiler(workspace_config.clone(), |compiler| {
                            run_compiler_epoch(compiler, &receiver, &mut state)
                        })
                    }));

                    match epoch {
                        Ok(Ok(CompilerEpochExit::InputsChanged)) => {
                            // The compiler is dropped before `arena` at the end of
                            // this iteration. The next epoch replays the request
                            // that observed the edit against entirely fresh noun
                            // storage and caches.
                            state.cache_invalidated = true;
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
                                    Ok(command) => command,
                                    Err(_) => break,
                                },
                            };
                            let compile_index = state.completed.fetch_add(1, Ordering::Relaxed) + 1;
                            let restart_required =
                                state.max_compiles != 0 && compile_index >= state.max_compiles;
                            let _ = command.reply.send(ActorReply::Completed {
                                result: Err(error),
                                compile_index,
                                restart_required,
                            });
                            if restart_required {
                                signal_exhausted(&mut state.exhausted);
                                break;
                            }
                        }
                        Err(payload) => {
                            let message = panic_payload_message(payload.as_ref());
                            if let Some(ready_sender) = state.ready_sender.take() {
                                let _ = ready_sender.send(Err(format!(
                                    "compiler initialization panicked: {message}"
                                )));
                            } else if let Some(command) = state.pending.take() {
                                let _ = command.reply.send(ActorReply::Panicked(message));
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
            Ok(Ok(())) => Ok(Self {
                sender,
                completed,
                max_compiles,
            }),
            Ok(Err(error)) => bail!(error),
            Err(_) => bail!("honk compiler actor exited before initialization completed"),
        }
    }

    async fn compile(&self, request: WorkspaceCompileRequest) -> Result<ActorReply, Status> {
        let (reply, response) = oneshot::channel();
        match self.sender.try_send(CompileCommand { request, reply }) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => {
                return Err(Status::resource_exhausted("compiler request queue is full"));
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                return Err(Status::unavailable("compiler actor is not available"));
            }
        }
        response
            .await
            .map_err(|_| Status::unavailable("compiler actor exited without a response"))
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

#[derive(Clone)]
struct GrpcCompilerService {
    compiler: CompilerHandle,
}

#[tonic::async_trait]
impl HonkCompiler for GrpcCompilerService {
    async fn get_server_info(
        &self,
        _request: Request<pb::GetServerInfoRequest>,
    ) -> std::result::Result<Response<pb::GetServerInfoResponse>, Status> {
        Ok(Response::new(pb::GetServerInfoResponse {
            protocol_version: PROTOCOL_VERSION,
            protocol_name: PROTOCOL_NAME.to_string(),
            server_version: env!("CARGO_PKG_VERSION").to_string(),
            capabilities: vec![
                "compile".to_string(),
                "structured-diagnostics".to_string(),
                "source-invalidation".to_string(),
                "bounded-process-lifetime".to_string(),
            ],
            max_compiles: self.compiler.max_compiles,
            completed_compiles: self.compiler.completed.load(Ordering::Relaxed),
        }))
    }

    async fn compile(
        &self,
        request: Request<pb::CompileRequest>,
    ) -> std::result::Result<Response<pb::CompileResponse>, Status> {
        let request = request.into_inner();
        if request.protocol_version != PROTOCOL_VERSION {
            return Err(Status::failed_precondition(format!(
                "unsupported protocol version {}; server requires {}",
                request.protocol_version, PROTOCOL_VERSION
            )));
        }
        if request.entry_path.is_empty() {
            return Err(Status::invalid_argument("entry_path must not be empty"));
        }
        let mode = match pb::ArtifactMode::try_from(request.artifact_mode) {
            Ok(pb::ArtifactMode::Standard) => ArtifactMode::Standard,
            Ok(pb::ArtifactMode::Arbitrary) => ArtifactMode::Arbitrary,
            Ok(pb::ArtifactMode::Dynock) => ArtifactMode::Dynock,
            Ok(pb::ArtifactMode::DynockTyped) => ArtifactMode::DynockTyped,
            Ok(pb::ArtifactMode::Unspecified) | Err(_) => {
                return Err(Status::invalid_argument(
                    "artifact_mode must be a recognized non-zero value",
                ));
            }
        };
        let compile_request = WorkspaceCompileRequest {
            entry: PathBuf::from(request.entry_path),
            mode,
            directory_files: request
                .directory_files
                .map(|files| files.paths.into_iter().map(PathBuf::from).collect()),
        };
        let reply = self.compiler.compile(compile_request).await?;
        let response = match reply {
            ActorReply::Completed {
                result: Ok(output),
                compile_index,
                restart_required,
            } => pb::CompileResponse {
                request_id: request.request_id,
                artifact: output.artifact,
                diagnostics: Vec::new(),
                cache_invalidated: output.cache_invalidated,
                compile_index,
                restart_required,
            },
            ActorReply::Completed {
                result: Err(error),
                compile_index,
                restart_required,
            } => pb::CompileResponse {
                request_id: request.request_id,
                artifact: Vec::new(),
                diagnostics: vec![diagnostic_to_proto(error.diagnostic)],
                cache_invalidated: false,
                compile_index,
                restart_required,
            },
            ActorReply::Panicked(message) => {
                return Err(Status::internal(format!(
                    "compiler actor panicked: {message}"
                )));
            }
        };
        Ok(Response::new(response))
    }
}

fn diagnostic_to_proto(diagnostic: WorkspaceDiagnostic) -> pb::Diagnostic {
    let kind = match diagnostic.kind {
        WorkspaceDiagnosticKind::Parse => pb::DiagnosticKind::Parse,
        WorkspaceDiagnosticKind::UnsupportedExpr => pb::DiagnosticKind::UnsupportedExpr,
        WorkspaceDiagnosticKind::Backend => pb::DiagnosticKind::Backend,
        WorkspaceDiagnosticKind::Decode => pb::DiagnosticKind::Decode,
        WorkspaceDiagnosticKind::Noun => pb::DiagnosticKind::Noun,
        WorkspaceDiagnosticKind::Io => pb::DiagnosticKind::Io,
        WorkspaceDiagnosticKind::Internal => pb::DiagnosticKind::Internal,
    };
    let location = diagnostic.location.map(|location| pb::SourceLocation {
        file: location.file,
        start_byte: location.start_byte.map(|value| value as u64),
        end_byte: location.end_byte.map(|value| value as u64),
        start_line: location.start_line,
        start_column: location.start_col,
        end_line: location.end_line,
        end_column: location.end_col,
    });
    pb::Diagnostic {
        kind: kind as i32,
        message: diagnostic.message,
        location,
    }
}

pub struct HonkServer {
    compiler: CompilerHandle,
    exhausted: oneshot::Receiver<()>,
}

impl HonkServer {
    pub fn new(config: DaemonConfig) -> Result<Self> {
        let (exhausted_sender, exhausted) = oneshot::channel();
        let compiler = CompilerHandle::spawn(config, exhausted_sender)?;
        Ok(Self {
            compiler,
            exhausted,
        })
    }

    pub async fn serve<F>(self, listener: TcpListener, external_shutdown: F) -> Result<()>
    where
        F: Future<Output = ()> + Send,
    {
        let local_addr = listener
            .local_addr()
            .context("failed to read bound address")?;
        ensure_loopback(local_addr)?;
        let service = GrpcCompilerService {
            compiler: self.compiler,
        };
        let service = HonkCompilerServer::new(service)
            .max_decoding_message_size(MAX_MESSAGE_BYTES)
            .max_encoding_message_size(MAX_MESSAGE_BYTES);
        let (health_reporter, health_service) = tonic_health::server::health_reporter();
        health_reporter
            .set_serving::<HonkCompilerServer<GrpcCompilerService>>()
            .await;
        let reflection = ReflectionBuilder::configure()
            .register_encoded_file_descriptor_set(honk_grpc_proto::FILE_DESCRIPTOR_SET)
            .register_encoded_file_descriptor_set(tonic_health::pb::FILE_DESCRIPTOR_SET)
            .build_v1()
            .context("failed to build gRPC reflection service")?;
        let exhausted = self.exhausted;
        tokio::pin!(external_shutdown);

        Server::builder()
            .add_service(health_service)
            .add_service(reflection)
            .add_service(service)
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async move {
                tokio::select! {
                    _ = &mut external_shutdown => {}
                    _ = exhausted => {}
                }
            })
            .await
            .context("honk gRPC server failed")?;
        Ok(())
    }
}

pub async fn bind_loopback(address: SocketAddr) -> Result<TcpListener> {
    ensure_loopback(address)?;
    TcpListener::bind(address)
        .await
        .with_context(|| format!("failed to bind honkd to {address}"))
}

fn ensure_loopback(address: SocketAddr) -> Result<()> {
    if !address.ip().is_loopback() {
        bail!("honkd only accepts loopback addresses; rejected {address}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ensure_loopback;

    #[test]
    fn accepts_ipv4_and_ipv6_loopback() {
        assert!(ensure_loopback("127.0.0.1:0".parse().expect("valid address")).is_ok());
        assert!(ensure_loopback("[::1]:0".parse().expect("valid address")).is_ok());
    }

    #[test]
    fn rejects_wildcard_and_routable_addresses() {
        assert!(ensure_loopback("0.0.0.0:0".parse().expect("valid address")).is_err());
        assert!(ensure_loopback("192.0.2.1:0".parse().expect("valid address")).is_err());
    }
}
