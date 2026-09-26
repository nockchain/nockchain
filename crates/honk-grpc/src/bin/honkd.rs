use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use honk::workspace::WorkspaceConfig;
use honk_grpc::{
    bind_loopback, DaemonConfig, HonkServer, DEFAULT_MAX_COMPILES, DEFAULT_WORKER_STACK_BYTES,
    PROTOCOL_NAME, PROTOCOL_VERSION,
};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(about = "Persistent loopback gRPC server for the honk Hoon compiler")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:0")]
    listen: SocketAddr,
    #[arg(long)]
    prelude: PathBuf,
    #[arg(long = "deps-dir")]
    dependencies: PathBuf,
    #[arg(long)]
    sut_jam: Option<PathBuf>,
    #[arg(long)]
    no_dbug: bool,
    #[arg(long)]
    no_vet: bool,
    #[arg(long, default_value_t = DEFAULT_MAX_COMPILES)]
    max_compiles: u64,
    #[arg(
        long,
        env = "HONK_WORKER_STACK_BYTES",
        default_value_t = DEFAULT_WORKER_STACK_BYTES
    )]
    worker_stack_bytes: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn,honk=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();

    let listener = bind_loopback(args.listen).await?;
    let local_addr = listener
        .local_addr()
        .context("failed to read honkd listening address")?;
    let server = HonkServer::new(DaemonConfig {
        workspace: WorkspaceConfig {
            prelude: args.prelude,
            dependencies: args.dependencies,
            subject_type_jam: args.sut_jam,
            dbug: !args.no_dbug,
            vet: !args.no_vet,
        },
        max_compiles: args.max_compiles,
        worker_stack_bytes: args.worker_stack_bytes,
    })?;

    // Machine-readable readiness stays on stdout; tracing stays on stderr.
    println!(
        "{{\"address\":\"{local_addr}\",\"protocol\":\"{PROTOCOL_NAME}\",\"protocol_version\":{PROTOCOL_VERSION}}}"
    );
    server
        .serve(listener, async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
}
