use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bridge::bridge_status::BridgeStatus;
use bridge::errors::BridgeError;
use bridge::health::{initialize_health_state, normalize_endpoint};
use bridge::stop::StopController;
use bridge::tui;
use bridge::tui::state::{new_log_buffer, TuiStatus};
use bridge::tui_client::BridgeTuiClient;
use clap::Parser;
use tokio::signal;
use tokio::time::timeout;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug, Clone)]
#[command(author, version, about = "Bridge TUI client", long_about = None)]
struct TuiCli {
    #[arg(
        long,
        help = "Bridge ingress gRPC server address (default: 127.0.0.1:8001)"
    )]
    server: Option<String>,
}

const DEFAULT_SERVER_ADDR: &str = "127.0.0.1:8001";
const INITIAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

fn init_tui_logging() -> Result<(), BridgeError> {
    let filter = EnvFilter::new(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init()
        .map_err(|e| BridgeError::Runtime(format!("failed to init tracing: {e}")))?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), BridgeError> {
    init_tui_logging()?;
    let cli = TuiCli::parse();

    let server_raw = cli
        .server
        .clone()
        .unwrap_or_else(|| DEFAULT_SERVER_ADDR.to_string());
    let server_uri = normalize_endpoint(&server_raw);

    info!("Attempting to connect to {server_uri}");
    let client = match timeout(
        INITIAL_CONNECT_TIMEOUT,
        BridgeTuiClient::new(server_uri.clone()),
    )
    .await
    {
        Ok(client) => client,
        Err(_) => {
            let timeout_secs = INITIAL_CONNECT_TIMEOUT.as_secs();
            error!("Timed out after {timeout_secs}s connecting to {server_uri}");
            return Err(BridgeError::Runtime(format!(
                "Timed out after {timeout_secs}s connecting to {server_uri}"
            )));
        }
    };

    let peers = Vec::new();
    let health_state = initialize_health_state(&peers);
    let bridge_status = BridgeStatus::new(health_state);
    let tui_status = TuiStatus::new(bridge_status, new_log_buffer());

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_signal = shutdown.clone();
    tokio::spawn(async move {
        if signal::ctrl_c().await.is_ok() {
            shutdown_signal.store(true, Ordering::Relaxed);
        }
    });

    let client_shutdown = shutdown.clone();
    let client_status = tui_status.clone();
    let client_handle =
        tokio::spawn(async move { client.run(client_status, client_shutdown).await });

    let (_stop_controller, stop_handle) = StopController::new();
    tui::run_tui(tui_status, shutdown.clone(), stop_handle).await?;

    shutdown.store(true, Ordering::Relaxed);
    let _ = client_handle.await;

    Ok(())
}
