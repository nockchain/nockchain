use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerInfo};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{tool, tool_handler, tool_router, ServerHandler, ServiceExt};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::backend::Backend;
use crate::process::run_isolated;
use crate::sandbox::{SandboxKind, DEFAULT_MAX_CALLS, DEFAULT_MAX_RESULT_BYTES};

#[derive(Clone, Debug)]
pub struct SandboxConfig {
    pub process_timeout: Duration,
    pub memory_limit_mib: u64,
    pub max_calls: usize,
    pub max_result_bytes: usize,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            process_timeout: Duration::from_secs(30),
            memory_limit_mib: 512,
            max_calls: DEFAULT_MAX_CALLS,
            max_result_bytes: DEFAULT_MAX_RESULT_BYTES,
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CodeInput {
    #[schemars(
        description = "A JavaScript async arrow function. The function must return JSON-serializable data."
    )]
    pub code: String,
}

#[derive(Clone, Debug)]
pub struct NockchainMcp {
    tool_router: ToolRouter<Self>,
    backend: Backend,
    sandbox: SandboxConfig,
}

impl NockchainMcp {
    pub fn new(backend: Backend, sandbox: SandboxConfig) -> Self {
        Self {
            tool_router: Self::tool_router(),
            backend,
            sandbox,
        }
    }

    async fn run(&self, kind: SandboxKind, code: String) -> Result<CallToolResult, String> {
        run_isolated(
            kind, &self.backend, code, self.sandbox.max_calls, self.sandbox.max_result_bytes,
            self.sandbox.process_timeout, self.sandbox.memory_limit_mib,
        )
        .await
        .map(CallToolResult::structured)
        .map_err(|error| format!("{error:#}"))
    }
}

#[tool_router]
impl NockchainMcp {
    /// Run JavaScript against the in-memory, fully de-referenced Nockchain query catalog.
    /// Use `codemode.spec()` to get `{ mode, operations }`, then filter/map it and return only
    /// relevant operation definitions. No network, filesystem, timers, imports, or query calls
    /// are available in this sandbox.
    #[tool(annotations(title = "Search Nockchain query API", read_only_hint = true))]
    async fn search(
        &self,
        Parameters(input): Parameters<CodeInput>,
    ) -> Result<CallToolResult, String> {
        self.run(SandboxKind::Search, input.code).await
    }

    /// Run JavaScript that composes read-only Nockchain queries. Available functions are
    /// `codemode.request({ operation, input, format: 'json'|'native' })`,
    /// `codemode.explain({ operation, input })`, and `codemode.spec()`. Use JSON for direct
    /// evaluation or native for base64 protobuf/JAM bytes. Mutation RPCs do not exist in the
    /// catalog and are rejected by the host boundary. Block timestamps are Hoon-epoch absolute
    /// seconds, not Unix timestamps, and exceed JavaScript's safe integer range. Convert with
    /// `BigInt(timestamp) - 9223372091860848000n` before constructing a `Date`.
    #[tool(annotations(title = "Execute Nockchain queries", read_only_hint = true))]
    async fn execute(
        &self,
        Parameters(input): Parameters<CodeInput>,
    ) -> Result<CallToolResult, String> {
        self.run(SandboxKind::Execute, input.code).await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for NockchainMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("nockchain-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(format!(
                "Read-only code-mode access to the Nockchain {:?} gRPC API. Start with search, then use execute. Only search and execute are exposed; all backend operations are allowlisted queries. Block timestamps are Hoon-epoch absolute seconds, not Unix timestamps, and exceed JavaScript's safe integer range. Convert to Unix seconds with BigInt(timestamp) - 9223372091860848000n before converting to Number or Date.",
                self.backend.mode
            ))
    }
}

pub async fn serve_stdio(server: NockchainMcp) -> Result<()> {
    server
        .serve(rmcp::transport::stdio())
        .await
        .context("start stdio MCP transport")?
        .waiting()
        .await
        .context("run stdio MCP transport")?;
    Ok(())
}

#[derive(Clone, Debug)]
pub struct HttpConfig {
    pub bind: SocketAddr,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    pub bearer_token: Option<String>,
}

pub async fn serve_http(server: NockchainMcp, config: HttpConfig) -> Result<()> {
    if config.tls_cert.is_some() != config.tls_key.is_some() {
        bail!("--tls-cert and --tls-key must be provided together")
    }
    let cancellation = CancellationToken::new();
    let service_config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_cancellation_token(cancellation.child_token());
    let factory_server = server.clone();
    let service: StreamableHttpService<NockchainMcp, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(factory_server.clone()),
            Default::default(),
            service_config,
        );

    let mut mcp_router = Router::new().nest_service("/mcp", service);
    if let Some(token) = config.bearer_token.filter(|token| !token.is_empty()) {
        let token = Arc::<str>::from(token);
        mcp_router = mcp_router.layer(middleware::from_fn(move |request, next| {
            authorize(request, next, Arc::clone(&token))
        }));
    }
    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .merge(mcp_router);

    tracing::info!(bind = %config.bind, tls = config.tls_cert.is_some(), "serving Nockchain MCP at /mcp");
    match (config.tls_cert, config.tls_key) {
        (Some(cert), Some(key)) => {
            let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
                .await
                .context("load MCP HTTPS certificate and key")?;
            let handle = axum_server::Handle::new();
            let shutdown_handle = handle.clone();
            tokio::spawn(async move {
                shutdown_signal().await;
                cancellation.cancel();
                shutdown_handle.graceful_shutdown(Some(Duration::from_secs(10)));
            });
            axum_server::bind_rustls(config.bind, tls)
                .handle(handle)
                .serve(app.into_make_service())
                .await
                .context("serve HTTPS MCP")?;
        }
        (None, None) => {
            let listener = tokio::net::TcpListener::bind(config.bind)
                .await
                .with_context(|| format!("bind MCP HTTP listener at {}", config.bind))?;
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    shutdown_signal().await;
                    cancellation.cancel();
                })
                .await
                .context("serve HTTP MCP")?;
        }
        _ => unreachable!("TLS pair was validated above"),
    }
    Ok(())
}

async fn authorize(request: Request<Body>, next: Next, token: Arc<str>) -> Response {
    let expected = format!("Bearer {token}");
    let accepted = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected);
    if accepted {
        next.run(request).await
    } else {
        Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(Body::from("missing or invalid bearer token"))
            .expect("static unauthorized response")
    }
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::warn!(%error, "failed to install Ctrl-C handler");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::ApiMode;

    #[test]
    fn server_advertises_exactly_two_tools() {
        let server = NockchainMcp::new(
            Backend {
                mode: ApiMode::Public,
                endpoint: ApiMode::Public.default_backend().to_string(),
                timeout: Duration::from_secs(1),
            },
            SandboxConfig::default(),
        );
        let tools = server.tool_router.list_all();
        assert_eq!(tools.len(), 2);
        assert!(tools.iter().any(|tool| tool.name == "search"));
        let execute = tools
            .iter()
            .find(|tool| tool.name == "execute")
            .expect("execute tool");
        assert!(execute
            .description
            .as_deref()
            .is_some_and(|description| description.contains("Hoon-epoch")));
        assert!(execute
            .description
            .as_deref()
            .is_some_and(|description| description.contains("9223372091860848000")));
        assert!(tools.iter().all(|tool| {
            tool.annotations
                .as_ref()
                .and_then(|annotations| annotations.read_only_hint)
                == Some(true)
        }));

        let instructions = server.get_info().instructions.expect("server instructions");
        assert!(instructions.contains("Hoon-epoch"));
        assert!(instructions.contains("9223372091860848000"));
    }
}
