use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Result};
use clap::{Parser, ValueEnum};
use nockchain_mcp::backend::Backend;
use nockchain_mcp::catalog::ApiMode;
use nockchain_mcp::process::run_internal_sandbox;
use nockchain_mcp::sandbox::{SandboxKind, DEFAULT_MAX_CALLS, DEFAULT_MAX_RESULT_BYTES};
use nockchain_mcp::server::{serve_http, serve_stdio, HttpConfig, NockchainMcp, SandboxConfig};
use tracing_subscriber::EnvFilter;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Transport {
    Stdio,
    Http,
}

#[derive(Debug, Parser)]
#[command(
    name = "nockchain-mcp",
    about = "Read-only, code-mode MCP server for Nockchain gRPC APIs",
    version
)]
struct Cli {
    /// Match the node's public query API or private NockApp Peek API.
    #[arg(long, value_enum, default_value = "public", env = "NOCKCHAIN_MCP_MODE")]
    mode: ApiMode,

    /// Nockchain gRPC URI. Defaults to the wallet's public API or localhost private API.
    #[arg(long, env = "NOCKCHAIN_GRPC_BACKEND")]
    grpc_backend: Option<String>,

    /// Timeout for each gRPC connection and request.
    #[arg(long, default_value_t = 15_000, env = "NOCKCHAIN_GRPC_TIMEOUT_MS")]
    grpc_timeout_ms: u64,

    /// MCP transport: stdio for local clients, or Streamable HTTP for hosting.
    #[arg(long, value_enum, default_value = "stdio")]
    transport: Transport,

    /// Listener for Streamable HTTP/HTTPS.
    #[arg(long, default_value = "127.0.0.1:3000", env = "NOCKCHAIN_MCP_BIND")]
    bind: SocketAddr,

    /// PEM certificate chain for direct HTTPS. Must be paired with --tls-key.
    #[arg(long, env = "NOCKCHAIN_MCP_TLS_CERT")]
    tls_cert: Option<PathBuf>,

    /// PEM private key for direct HTTPS. Must be paired with --tls-cert.
    #[arg(long, env = "NOCKCHAIN_MCP_TLS_KEY")]
    tls_key: Option<PathBuf>,

    /// Optional bearer token required by the hosted /mcp endpoint.
    #[arg(long, env = "NOCKCHAIN_MCP_BEARER_TOKEN", hide_env_values = true)]
    bearer_token: Option<String>,

    /// Wall-time limit for each isolated JavaScript execution.
    #[arg(long, default_value_t = 30_000)]
    sandbox_timeout_ms: u64,

    /// Address-space limit for each isolated JavaScript process on Linux.
    #[arg(long, default_value_t = 512)]
    sandbox_memory_mib: u64,

    /// Maximum Nockchain backend calls in one execute program.
    #[arg(long, default_value_t = DEFAULT_MAX_CALLS)]
    max_calls: usize,

    /// Maximum JSON bytes returned by one search/execute program.
    #[arg(long, default_value_t = DEFAULT_MAX_RESULT_BYTES)]
    max_result_bytes: usize,

    /// Internal child-process entry point; not part of the user CLI.
    #[arg(long, value_enum, hide = true)]
    internal_sandbox_kind: Option<SandboxKind>,
}

#[tokio::main]
async fn main() -> Result<()> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    let cli = Cli::parse();
    let backend = Backend {
        mode: cli.mode,
        endpoint: cli
            .grpc_backend
            .unwrap_or_else(|| cli.mode.default_backend().to_string()),
        timeout: Duration::from_millis(cli.grpc_timeout_ms),
    };

    if let Some(kind) = cli.internal_sandbox_kind {
        return run_internal_sandbox(kind, backend, cli.max_calls, cli.max_result_bytes).await;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init()
        .map_err(|error| anyhow!("initialize tracing: {error}"))?;

    if matches!(cli.transport, Transport::Http)
        && !cli.bind.ip().is_loopback()
        && cli.bearer_token.as_deref().is_none_or(str::is_empty)
    {
        tracing::warn!(
            bind = %cli.bind,
            "hosted MCP has no bearer token; configure NOCKCHAIN_MCP_BEARER_TOKEN or put it behind an authenticated proxy"
        );
    }

    let sandbox = SandboxConfig {
        process_timeout: Duration::from_millis(cli.sandbox_timeout_ms),
        memory_limit_mib: cli.sandbox_memory_mib,
        max_calls: cli.max_calls,
        max_result_bytes: cli.max_result_bytes,
    };
    let server = NockchainMcp::new(backend, sandbox);
    match cli.transport {
        Transport::Stdio => serve_stdio(server).await,
        Transport::Http => {
            serve_http(
                server,
                HttpConfig {
                    bind: cli.bind,
                    tls_cert: cli.tls_cert,
                    tls_key: cli.tls_key,
                    bearer_token: cli.bearer_token,
                },
            )
            .await
        }
    }
}
