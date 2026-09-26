use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use honk::workspace::{
    ArtifactMode, WorkspaceCompileRequest, WorkspaceDiagnostic, WorkspaceDiagnosticKind,
};
use honk_grpc_proto::v1::honk_compiler_server::{HonkCompiler, HonkCompilerServer};
use honk_grpc_proto::v1::{self as pb};
use honk_service::{CompilerHandle, CompilerService, CompilerServiceError, ServiceCompileOutput};
pub use honk_service::{
    CompilerServiceConfig as DaemonConfig, DEFAULT_MAX_COMPILES, DEFAULT_WORKER_STACK_BYTES,
};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;
use tonic::{Request, Response, Status};
use tonic_reflection::server::Builder as ReflectionBuilder;

pub const PROTOCOL_VERSION: u32 = 1;
pub const PROTOCOL_NAME: &str = "honk.compiler.v1";
const MAX_MESSAGE_BYTES: usize = 256 * 1024 * 1024;

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
            max_compiles: self.compiler.max_compiles(),
            completed_compiles: self.compiler.completed_compiles(),
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
        let ServiceCompileOutput {
            result,
            compile_index,
            restart_required,
            document_revision: _,
        } = self
            .compiler
            .compile(compile_request)
            .await
            .map_err(service_error_to_status)?;
        let response = match result {
            Ok(output) => pb::CompileResponse {
                request_id: request.request_id,
                artifact: output.artifact,
                diagnostics: Vec::new(),
                cache_invalidated: output.cache_invalidated,
                compile_index,
                restart_required,
            },
            Err(error) => pb::CompileResponse {
                request_id: request.request_id,
                artifact: Vec::new(),
                diagnostics: vec![diagnostic_to_proto(error.diagnostic)],
                cache_invalidated: false,
                compile_index,
                restart_required,
            },
        };
        Ok(Response::new(response))
    }
}

fn service_error_to_status(error: CompilerServiceError) -> Status {
    match error {
        CompilerServiceError::QueueFull => {
            Status::resource_exhausted("compiler request queue is full")
        }
        CompilerServiceError::Unavailable => Status::unavailable("compiler actor is not available"),
        CompilerServiceError::ActorPanicked(message) => {
            Status::internal(format!("compiler actor panicked: {message}"))
        }
        CompilerServiceError::InvalidDocumentPath(error) => {
            Status::invalid_argument(format!("invalid document path: {error}"))
        }
        CompilerServiceError::StaleDocumentVersion { .. } => {
            Status::failed_precondition(error.to_string())
        }
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
        let service = CompilerService::spawn(config)?;
        let (compiler, exhausted) = service.into_parts();
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
