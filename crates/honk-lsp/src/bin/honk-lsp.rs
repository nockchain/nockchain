use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use honk_lsp::{run_stdio, LspConfig};
use honk_service::{DEFAULT_MAX_COMPILES, DEFAULT_WORKER_STACK_BYTES};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(about = "Hoon language server backed directly by honk")]
struct Args {
    /// Hoon compiler prelude. Defaults to <workspace>/hoon/common/hoon.hoon.
    #[arg(long)]
    prelude: Option<PathBuf>,
    /// Dependency root. Defaults to <workspace>/hoon when present, otherwise the workspace.
    #[arg(long = "deps-dir")]
    dependencies: Option<PathBuf>,
    /// Stable entry to check after any open document changes.
    #[arg(long)]
    entry: Option<PathBuf>,
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
    #[arg(long, default_value_t = 150)]
    check_delay_ms: u64,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn,honk_lsp=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .compact()
        .init();

    run_stdio(LspConfig {
        prelude: args.prelude,
        dependencies: args.dependencies,
        entry: args.entry,
        subject_type_jam: args.sut_jam,
        dbug: !args.no_dbug,
        vet: !args.no_vet,
        max_compiles: args.max_compiles,
        worker_stack_bytes: args.worker_stack_bytes,
        check_delay_ms: args.check_delay_ms,
    })
}
