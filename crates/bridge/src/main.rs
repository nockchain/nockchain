use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bridge::bridge_status::{run_hourly_rotation, BridgeStatus};
use bridge::config::NonceEpochConfig;
use bridge::deposit_log::{
    create_commit_nock_deposits_driver, sync_deposit_log_from_hashchain,
    validate_deposit_log_against_chain_nonce_prefix,
};
use bridge::errors::BridgeError;
use bridge::ethereum::BaseBridge;
use bridge::health::{derive_peer_endpoints, initialize_health_state, HealthMonitorConfig};
use bridge::ingress;
use bridge::nockchain::NockchainWatcher;
use bridge::proposal_cache::ProposalCache;
use bridge::runtime::{
    run_posting_loop, run_signing_cursor_loop, BridgeRuntime, KernelCauseBuilder,
};
use bridge::signing::{extract_valid_bridge_addresses, BridgeSigner};
use bridge::status::BridgeStatusState;
use bridge::stop::create_stop_driver;
use bridge::tui::{cleanup_old_logs, init_bridge_tracing};
use bridge::types::NodeConfig;
use clap::Parser;
use kernels_open_bridge::KERNEL;
use nockapp::kernel::boot::{self, Cli as BootCli};
use nockapp::nockapp::wire::Wire;
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::{exit_driver, markdown_driver, system_data_dir};
use nockapp_grpc::services::public_nockchain::v1::driver::grpc_listener_driver;
use noun_serde::NounSerdeEncodeExt;
use tokio::{fs as tokio_fs, signal};
use tracing::info;
use zkvm_jetpack::hot::produce_prover_hot_state;

// Default to jemalloc unless opted out via `malloc` or `snmalloc`.
#[cfg(all(not(miri), not(feature = "malloc"), not(feature = "snmalloc")))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

// Opt into snmalloc as the global allocator.
#[cfg(feature = "snmalloc")]
#[global_allocator]
static ALLOC: snmalloc_rs::SnMalloc = snmalloc_rs::SnMalloc;

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
struct BridgeCli {
    #[command(flatten)]
    boot: BootCli,

    #[arg(long, short = 'c')]
    config_path: Option<PathBuf>,

    #[arg(long)]
    data_dir: Option<PathBuf>,

    #[arg(
        long,
        help = "Send a %start poke to the kernel on boot (clears stop state)"
    )]
    start: bool,

    #[arg(long, help = "Directory for log files (default: {data_dir}/logs/)")]
    log_dir: Option<PathBuf>,

    #[arg(
        long,
        help = "Number of days of logs to maintain (default: 7, disable with 0)"
    )]
    log_retention_days: Option<usize>,
}

const DEFAULT_CONFIG_TEMPLATE: &str = include_str!("../bridge-conf.example.toml");
const NETWORK_MONITOR_POLL_SECS: u64 = 15;

async fn bridge_data_dir(cli_dir: Option<PathBuf>) -> Result<PathBuf, BridgeError> {
    let bridge_data_dir = cli_dir.unwrap_or_else(|| system_data_dir().join("bridge"));
    if !bridge_data_dir.exists() {
        tokio_fs::create_dir_all(&bridge_data_dir)
            .await
            .map_err(|e| BridgeError::Config(format!("Failed to create bridge data dir: {}", e)))?;
    }
    Ok(bridge_data_dir)
}

fn ensure_config_file(path: &Path) -> Result<(), BridgeError> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            BridgeError::Config(format!("Failed to create config directory: {}", e))
        })?;
    }
    fs::write(path, DEFAULT_CONFIG_TEMPLATE).map_err(|e| {
        BridgeError::Config(format!(
            "Failed to write default config to {}: {}",
            path.display(),
            e
        ))
    })?;
    info!("wrote default config template to {}", path.display());
    Ok(())
}

fn default_ingress_listen_address(node_config: &NodeConfig) -> Result<String, BridgeError> {
    let idx = node_config.node_id as usize;
    let node = node_config.nodes.get(idx).ok_or_else(|| {
        BridgeError::Config(format!(
            "node_id {} missing from nodes list",
            node_config.node_id
        ))
    })?;
    let trimmed = node.ip.trim();
    if trimmed.is_empty() {
        return Err(BridgeError::Config(format!(
            "nodes[{}] ip must not be empty when ingress_listen_address is unset",
            idx
        )));
    }
    Ok(trimmed.to_string())
}

fn build_nonce_epoch_config(
    config_toml: &bridge::config::BridgeConfigToml,
) -> Result<NonceEpochConfig, BridgeError> {
    let nonce_epoch_base_opt = config_toml.nonce_epoch_base;
    let nonce_epoch_start_height = config_toml.nonce_epoch_start_height;
    let nonce_epoch_start_tx_id = config_toml.nonce_epoch_start_tx_id()?;

    if nonce_epoch_start_height.is_none() && nonce_epoch_start_tx_id.is_some() {
        return Err(BridgeError::Config(
            "nonce_epoch_start_tx_id_base58 requires nonce_epoch_start_height".into(),
        ));
    }

    let start_key_set = nonce_epoch_start_height.is_some() || nonce_epoch_start_tx_id.is_some();
    if let Some(base) = nonce_epoch_base_opt {
        if base == 0 {
            if start_key_set {
                return Err(BridgeError::Config(
                    "nonce_epoch_base must be non-zero when nonce_epoch_start_height or nonce_epoch_start_tx_id_base58 is set".into(),
                ));
            }
        } else {
            let Some(height) = nonce_epoch_start_height else {
                return Err(BridgeError::Config(
                    "nonce_epoch_start_height must be set when nonce_epoch_base is non-zero".into(),
                ));
            };
            if height == 0 {
                return Err(BridgeError::Config(
                    "nonce_epoch_start_height must be greater than 0 when set".into(),
                ));
            }
            if nonce_epoch_start_tx_id.is_none() {
                return Err(BridgeError::Config(
                    "nonce_epoch_start_tx_id_base58 must be set when nonce_epoch_base is non-zero"
                        .into(),
                ));
            }
        }
    } else if start_key_set {
        return Err(BridgeError::Config(
            "nonce_epoch_base must be set when nonce_epoch_start_height or nonce_epoch_start_tx_id_base58 is set".into(),
        ));
    }

    let nonce_epoch_base = nonce_epoch_base_opt.unwrap_or(0);
    let nonce_epoch_start_height = nonce_epoch_start_height.unwrap_or(1);
    Ok(NonceEpochConfig {
        base: nonce_epoch_base,
        start_height: nonce_epoch_start_height,
        start_tx_id: nonce_epoch_start_tx_id,
    })
}

#[tokio::main]
async fn main() -> Result<(), BridgeError> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let cli = BridgeCli::parse();

    // Compute data_dir first (needed for log_dir default)
    let data_dir = bridge_data_dir(cli.data_dir.clone()).await?;

    // Determine log directory: CLI override or default to {data_dir}/logs/
    let log_dir = cli.log_dir.clone().unwrap_or_else(|| data_dir.join("logs"));
    let log_retention_days = cli.log_retention_days.unwrap_or(7);

    // Initialize tracing with file logging - keep guard alive for program duration
    let _log_guard = init_bridge_tracing(&cli.boot, None, log_dir.clone(), log_retention_days)?;

    info!("Logging to directory: {}", log_dir.display());

    // Clean up old log files (best effort, don't fail startup)
    if log_retention_days > 0 {
        cleanup_old_logs(&log_dir, log_retention_days as u64);
    }

    let prover_hot_state = produce_prover_hot_state();

    let mut app = boot::setup::<NockJammer>(
        KERNEL,
        cli.boot.clone(),
        prover_hot_state.as_slice(),
        "bridge",
        Some(data_dir.clone()),
    )
    .await
    .map_err(|e| BridgeError::NockappTask(format!("Kernel setup failed: {}", e)))?;

    info!("bridge nockapp started");

    let config_path = if let Some(path) = cli.config_path.clone() {
        path
    } else {
        bridge::config::default_config_path()?
    };
    ensure_config_file(&config_path)?;

    let config_toml = bridge::config::BridgeConfigToml::from_file(&config_path)?;
    let node_config = config_toml.to_node_config()?;

    info!("loaded config from {}", config_path.display());

    info!("NodeConfig: {:?}", node_config);

    let cause_builder = Arc::new(KernelCauseBuilder);
    let (mut bridge_runtime, runtime_handle) = BridgeRuntime::new(cause_builder);
    let runtime_handle = Arc::new(runtime_handle);
    bridge_runtime.install_driver(&mut app).await?;

    let (stop_controller, stop_handle) = bridge::stop::StopController::new();

    if cli.start {
        let mut start_slab = NounSlab::new();
        let start_cause = bridge::types::BridgeCause::start();
        let start_noun = start_cause.encode(&mut start_slab);
        start_slab.set_root(start_noun);
        let start_wire = nockapp::one_punch::OnePunchWire::Poke.to_wire();
        app.poke(start_wire, start_slab)
            .await
            .map_err(|e| BridgeError::NockappTask(format!("Start poke failed: {}", e)))?;
        info!("sent %start poke to kernel");
    }

    let mut cfg_slab = NounSlab::new();
    let cfg_cause = bridge::types::BridgeCause::cfg_load(Some(node_config.clone()));
    let cfg_noun = cfg_cause.encode(&mut cfg_slab);
    cfg_slab.set_root(cfg_noun);
    let cfg_wire = nockapp::one_punch::OnePunchWire::Poke.to_wire();
    app.poke(cfg_wire, cfg_slab)
        .await
        .map_err(|e| BridgeError::NockappTask(format!("Config poke failed: {}", e)))?;

    info!("sent config to kernel");

    // Send constants to kernel
    let bridge_constants = config_toml.bridge_constants()?;
    let base_blocks_chunk = bridge_constants.base_blocks_chunk; // Extract before move
    info!(
        "sending bridge constants: min_signers={}, total_signers={}, base_start={}, nock_start={}, base_blocks_chunk={}",
        bridge_constants.min_signers,
        bridge_constants.total_signers,
        bridge_constants.base_start_height,
        bridge_constants.nockchain_start_height,
        base_blocks_chunk
    );

    let mut constants_slab = NounSlab::new();
    let constants_cause = bridge::types::BridgeCause::set_constants(bridge_constants);
    let constants_noun = constants_cause.encode(&mut constants_slab);
    constants_slab.set_root(constants_noun);
    let constants_wire = nockapp::one_punch::OnePunchWire::Poke.to_wire();
    app.poke(constants_wire, constants_slab)
        .await
        .map_err(|e| BridgeError::NockappTask(format!("Constants poke failed: {}", e)))?;

    info!("sent constants to kernel");

    let base_confirmation_depth = config_toml.base_confirmation_depth;
    if base_confirmation_depth == 0 {
        return Err(BridgeError::Config(
            "base_confirmation_depth must be greater than 0".into(),
        ));
    }

    let nockchain_confirmation_depth = config_toml.nockchain_confirmation_depth;
    if nockchain_confirmation_depth == 0 {
        return Err(BridgeError::Config(
            "nockchain_confirmation_depth must be greater than 0".into(),
        ));
    }

    let nonce_epoch = build_nonce_epoch_config(&config_toml)?;
    info!(
        "driver finality: base_confirmation_depth={}, nockchain_confirmation_depth={}",
        base_confirmation_depth, nockchain_confirmation_depth
    );

    let base_bridge = Arc::new(
        BaseBridge::new(
            config_toml.base_ws_url().to_string(),
            config_toml.inbox_contract_address()?,
            config_toml.nock_contract_address()?,
            config_toml.my_eth_key_hex().to_string(),
            runtime_handle.clone(),
            base_blocks_chunk,
            base_confirmation_depth,
            stop_handle.clone(),
        )
        .await?,
    );

    let ingress_addr_raw = if let Some(address) = config_toml.ingress_listen_address() {
        address.to_string()
    } else {
        default_ingress_listen_address(&node_config)?
    };
    let ingress_addr: SocketAddr = ingress_addr_raw
        .parse()
        .map_err(|e| BridgeError::Config(format!("invalid ingress listen address: {}", e)))?;
    let self_address = ingress_addr.to_string();
    let ingress_runtime = runtime_handle.clone();
    let node_id = node_config.node_id;

    let bridge_signer = Arc::new(BridgeSigner::new(config_toml.my_eth_key_hex().to_string())?);
    info!("Base bridge and signer initialized successfully");

    // Create proposal cache for signature aggregation
    let proposal_cache = Arc::new(ProposalCache::new());

    // Per-node deposit log for deterministic nonce assignment.
    let deposit_log_path = data_dir.join("deposit-queue.sqlite");
    let deposit_log =
        Arc::new(bridge::deposit_log::DepositLog::open(deposit_log_path.clone()).await?);
    info!("Using deposit queue log at {}", deposit_log_path.display());

    // Deposit log sync happens after the kernel action loop is running.

    // Create peers, health state, and bridge status BEFORE ingress spawn
    // so ingress can update state when receiving peer broadcasts
    let peers = derive_peer_endpoints(&node_config, node_config.node_id);
    let health_state = initialize_health_state(&peers);

    // Create BridgeStatus for drivers to update proposal state.
    let bridge_status = BridgeStatus::new(health_state.clone());
    let status_state = BridgeStatusState::new();

    let nock_watcher = NockchainWatcher::new(
        config_toml.grpc_address().to_string(),
        runtime_handle.clone(),
        nockchain_confirmation_depth,
        stop_handle.clone(),
    )
    .with_bridge_status(bridge_status.clone());
    let nock_handle = tokio::spawn(async move { nock_watcher.run().await });

    // Build address-to-node-id mapping for TUI signature display
    // node_id is derived from index in nodes array (same as derive_peer_endpoints)
    // eth_pubkey is actually the 20-byte Ethereum address (naming is misleading)
    let address_to_node_id: std::collections::HashMap<alloy::primitives::Address, u64> =
        node_config
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(idx, node)| {
                if node.eth_pubkey.0.len() == 20 {
                    let addr = alloy::primitives::Address::from_slice(&node.eth_pubkey.0);
                    Some((addr, idx as u64))
                } else {
                    None
                }
            })
            .collect();

    let ingress_signer = bridge_signer.clone();
    let ingress_cache = proposal_cache.clone();
    let ingress_tui = bridge_status.clone();
    let ingress_addr_map = address_to_node_id.clone();
    let ingress_stop_controller = stop_controller.clone();
    let ingress_peers = peers.clone();
    let ingress_status_state = status_state.clone();
    let ingress_deposit_log = deposit_log.clone();
    let ingress_nonce_epoch = nonce_epoch.clone();
    let ingress_handle = tokio::spawn(async move {
        ingress::serve_ingress(
            ingress_addr, node_id, ingress_runtime, ingress_status_state, ingress_deposit_log,
            ingress_nonce_epoch, ingress_signer, ingress_cache, ingress_tui, ingress_addr_map,
            ingress_stop_controller, ingress_peers,
        )
        .await
    });

    // core/admin drivers
    app.add_io_driver(markdown_driver()).await;
    app.add_io_driver(exit_driver()).await;

    // grpc listener driver: forwards %grpc effects to the configured gRPC endpoint
    app.add_io_driver(grpc_listener_driver(config_toml.grpc_address().to_string()))
        .await;

    // stop driver: observes STOP effects and propagates stop pokes to peers
    let stop_driver = create_stop_driver(
        runtime_handle.clone(),
        stop_controller.clone(),
        bridge_status.clone(),
        peers.clone(),
        node_config.node_id,
    );
    app.add_io_driver(stop_driver).await;

    // Note: proposal_cache was already created above (line 161) and passed to ingress.
    info!("Using shared proposal cache for signature aggregation");

    // Add commit-nock-deposits CDC driver to persist effect data.
    let propose_driver = create_commit_nock_deposits_driver(
        runtime_handle.clone(),
        stop_controller.clone(),
        bridge_status.clone(),
        None,
        stop_handle.clone(),
        deposit_log.clone(),
        nonce_epoch.clone(),
    );
    app.add_io_driver(propose_driver).await;

    let app_handle = tokio::spawn(async move { app.run().await });
    let runtime_task = tokio::spawn(async move { bridge_runtime.run().await });

    // Seed stop controller from persisted kernel stop-state so the TUI reflects it on boot.
    let stop_seed_runtime = runtime_handle.clone();
    let stop_seed_controller = stop_controller.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let stopped =
            match tokio::time::timeout(Duration::from_secs(5), stop_seed_runtime.peek_stop_state())
                .await
            {
                Ok(Ok(value)) => value,
                Ok(Err(err)) => {
                    tracing::warn!(
                        target: "bridge.stop",
                        error=%err,
                        "failed to peek stop-state on boot"
                    );
                    return;
                }
                Err(_) => {
                    tracing::warn!(
                        target: "bridge.stop",
                        "timed out peeking stop-state on boot"
                    );
                    return;
                }
            };

        if !stopped {
            return;
        }

        let last =
            match tokio::time::timeout(Duration::from_secs(5), stop_seed_runtime.peek_stop_info())
                .await
            {
                Ok(Ok(info)) => info,
                Ok(Err(err)) => {
                    tracing::warn!(
                        target: "bridge.stop",
                        error=%err,
                        "failed to peek stop-info on boot"
                    );
                    None
                }
                Err(_) => {
                    tracing::warn!(
                        target: "bridge.stop",
                        "timed out peeking stop-info on boot"
                    );
                    None
                }
            };

        let info = bridge::stop::StopInfo {
            reason: "kernel stop-state present on boot".to_string(),
            last,
            source: bridge::stop::StopSource::KernelEffect,
            at: std::time::SystemTime::now(),
        };
        let _ = stop_seed_controller.trigger(info);
    });

    // Deterministically seed/sync the per-node deposit log from the kernel hashchain
    // before we start signing/proposing.
    sync_deposit_log_from_hashchain(
        runtime_handle.clone(),
        deposit_log.clone(),
        &nonce_epoch,
        Duration::from_secs(2),
    )
    .await?;

    let tip_height = runtime_handle.nock_hashchain_tip().await?.unwrap_or(0);
    if tip_height >= nonce_epoch.start_height {
        validate_deposit_log_against_chain_nonce_prefix(
            base_bridge.clone(),
            deposit_log.clone(),
            nonce_epoch.clone(),
        )
        .await?;
    } else {
        info!(
            target: "bridge.deposit_log",
            tip_height,
            nonce_epoch_start_height = nonce_epoch.start_height,
            "skipping deposit log validation until hashchain reaches epoch start height"
        );
    }

    // Spawn signing cursor loop so nodes can deterministically (re)sign pending deposits
    // after restart without requiring re-processing the originating nock block.
    let cursor_runtime = runtime_handle.clone();
    let cursor_base = base_bridge.clone();
    let cursor_deposit_log = deposit_log.clone();
    let cursor_cache = proposal_cache.clone();
    let cursor_signer = bridge_signer.clone();
    let cursor_valid_addrs = extract_valid_bridge_addresses(&node_config);
    let cursor_tui = bridge_status.clone();
    let cursor_stop_controller = stop_controller.clone();
    let cursor_stop = stop_handle.clone();
    let cursor_peers = peers.clone();
    let cursor_node_id = node_config.node_id;
    let cursor_addr_map = address_to_node_id.clone();
    let nonce_epoch_clone = nonce_epoch.clone();
    let _cursor_handle = tokio::spawn(async move {
        run_signing_cursor_loop(
            cursor_runtime, cursor_base, cursor_deposit_log, &nonce_epoch_clone, cursor_cache,
            cursor_signer, cursor_valid_addrs, cursor_peers, cursor_node_id, cursor_tui,
            cursor_addr_map, cursor_stop_controller, cursor_stop,
        )
        .await
    });
    info!("Spawned signing cursor loop");

    // Spawn posting loop to monitor cache and post ready proposals to BASE
    // Turn-based posting is handled inside run_posting_loop using hoon_proposer()
    let posting_cache = proposal_cache.clone();
    let posting_base = base_bridge.clone();
    let posting_config = node_config.clone();
    let posting_tui = bridge_status.clone();
    let posting_stop = stop_handle.clone();
    let posting_status_state = status_state.clone();
    let _posting_handle = tokio::spawn(async move {
        run_posting_loop(
            posting_cache, posting_base, posting_config, posting_tui, posting_stop,
            posting_status_state,
        )
        .await
    });
    info!("Spawned proposal posting loop with turn-based posting");

    let base_bridge_for_ack = base_bridge.clone();
    let bridge_status_for_ack = bridge_status.clone();
    let ack_handle = tokio::spawn(async move {
        base_bridge_for_ack
            .stream_base_events(Some(bridge_status_for_ack))
            .await
    });

    let health_cfg = HealthMonitorConfig {
        self_node_id: node_config.node_id,
        self_address,
        peers: peers.clone(),
        poll_interval: Duration::from_secs(5),
        request_timeout: Duration::from_secs(2),
        bridge_status: Some(bridge_status.clone()),
    };
    let monitor_state = health_state.clone();
    let health_handle =
        tokio::spawn(
            async move { bridge::health::run_health_monitor(health_cfg, monitor_state).await },
        );

    // Network monitor: polls chain heights and updates bridge status
    let network_runtime = runtime_handle.clone();
    let network_bridge_status = bridge_status.clone();
    let _network_handle = tokio::spawn(async move {
        bridge::nockchain::run_network_monitor(
            network_runtime,
            network_bridge_status,
            Duration::from_secs(NETWORK_MONITOR_POLL_SECS),
        )
        .await
    });

    // Hourly metrics rotation: shifts hourly_tx_counts left and adds 0
    let hourly_bridge_status = bridge_status.clone();
    let _hourly_rotation_handle =
        tokio::spawn(async move { run_hourly_rotation(hourly_bridge_status).await });

    tokio::select! {
        result = app_handle => {
            match result {
                Ok(app_result) => {
                    app_result.map_err(|e| BridgeError::NockappTask(format!("App run failed: {}", e)))?;
                }
                Err(e) => {
                    return Err(BridgeError::NockappTask(format!("App task failed: {}", e)));
                }
            }
        }
        result = nock_handle => {
            match result {
                Ok(nock_result) => {
                    nock_result?;
                }
                Err(e) => {
                    return Err(BridgeError::Runtime(format!("Nock watcher failed: {}", e)));
                }
            }
        }
        result = runtime_task => {
            match result {
                Ok(runtime_result) => {
                    runtime_result?;
                }
                Err(e) => {
                    return Err(BridgeError::Runtime(format!("Runtime task failed: {}", e)));
                }
            }
        }
        result = ingress_handle => {
            match result {
                Ok(ingress_result) => {
                    ingress_result?;
                }
                Err(e) => {
                    return Err(BridgeError::Runtime(format!("Ingress server failed: {}", e)));
                }
            }
        }
        result = ack_handle => {
            match result {
                Ok(ack_result) => {
                    ack_result?;
                }
                Err(e) => {
                    return Err(BridgeError::AckTask(format!("Ack task failed: {}", e)));
                }
            }
        }
        result = health_handle => {
            match result {
                Ok(health_result) => {
                    health_result?;
                }
                Err(e) => {
                    return Err(BridgeError::Runtime(format!("Health monitor failed: {}", e)));
                }
            }
        }
        _ = signal::ctrl_c() => {
            info!("Ctrl+C received, shutting down");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Once;

    use bridge::config::{BridgeConfigToml, NodeInfoToml};
    use bridge::deposit_log::persist_commit_nock_deposits_requests;
    use bridge::tui;
    use bridge::types::{EthAddress, Tip5Hash};
    use nockapp::kernel::boot;
    use nockchain_math::belt::Belt;
    use nockchain_types::v1::Name;
    use tempfile::TempDir;

    use super::*;

    static INIT: Once = Once::new();
    const VALID_START_TX_ID: &str = "2uYre9HXRP8X6BD7w3GvgfUAU47RSmZDGkz9uJgJmD9CxN7JA69k6MF";

    fn base_config() -> BridgeConfigToml {
        BridgeConfigToml {
            node_id: 0,
            base_ws_url: "wss://example.invalid".to_string(),
            inbox_contract_address: None,
            nock_contract_address: None,
            my_eth_key: "0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318"
                .to_string(),
            my_nock_key: "5KZuFKrctV5iUburT54Z9fhpf3V3hv2sPf9GRQnjFR8T".to_string(),
            grpc_address: "http://localhost:5555".to_string(),
            base_confirmation_depth: 1,
            nockchain_confirmation_depth: 1,
            nonce_epoch_base: None,
            nonce_epoch_start_height: None,
            nonce_epoch_start_tx_id_base58: None,
            ingress_listen_address: None,
            nodes: vec![
                NodeInfoToml {
                    ip: "localhost:8001".to_string(),
                    eth_pubkey: "0x1111111111111111111111111111111111111111".to_string(),
                    nock_pkh: "2222222222222222222222222222222222222222222222222222".to_string(),
                },
                NodeInfoToml {
                    ip: "localhost:8002".to_string(),
                    eth_pubkey: "0x2222222222222222222222222222222222222222".to_string(),
                    nock_pkh: "3333333333333333333333333333333333333333333333333333".to_string(),
                },
                NodeInfoToml {
                    ip: "localhost:8003".to_string(),
                    eth_pubkey: "0x3333333333333333333333333333333333333333".to_string(),
                    nock_pkh: "4444444444444444444444444444444444444444444444444444".to_string(),
                },
                NodeInfoToml {
                    ip: "localhost:8004".to_string(),
                    eth_pubkey: "0x4444444444444444444444444444444444444444".to_string(),
                    nock_pkh: "5555555555555555555555555555555555555555555555555555".to_string(),
                },
                NodeInfoToml {
                    ip: "localhost:8005".to_string(),
                    eth_pubkey: "0x5555555555555555555555555555555555555555".to_string(),
                    nock_pkh: "6666666666666666666666666666666666666666666666666666".to_string(),
                },
            ],
            constants: None,
        }
    }

    fn init_tracing() {
        INIT.call_once(|| {
            // Set RUST_LOG for tests if not already set
            if std::env::var("RUST_LOG").is_err() {
                std::env::set_var("RUST_LOG", "debug");
            }
            let cli = boot::default_boot_cli(true);
            let temp_log_dir = std::env::temp_dir().join("bridge-test-logs");
            let _guard = init_bridge_tracing(&cli, Some(tui::new_log_buffer()), temp_log_dir, 7)
                .expect("failed to init tracing for tests");
            // Note: guard is dropped here but that's OK for tests - we just need tracing initialized
            // In production, the guard is kept alive in main()
        });
    }

    #[test]
    fn nonce_epoch_config_allows_missing_base_and_start() {
        let cfg = base_config();
        let epoch = build_nonce_epoch_config(&cfg).expect("base omitted should be ok");
        assert_eq!(epoch.base, 0);
        assert_eq!(epoch.start_height, 1);
        assert!(epoch.start_tx_id.is_none());
    }

    #[test]
    fn nonce_epoch_config_rejects_start_without_base() {
        let mut cfg = base_config();
        cfg.nonce_epoch_start_height = Some(10);
        assert!(build_nonce_epoch_config(&cfg).is_err());

        let mut cfg = base_config();
        cfg.nonce_epoch_start_tx_id_base58 = Some(VALID_START_TX_ID.to_string());
        assert!(build_nonce_epoch_config(&cfg).is_err());
    }

    #[test]
    fn nonce_epoch_config_allows_zero_base_without_anchor() {
        let mut cfg = base_config();
        cfg.nonce_epoch_base = Some(0);
        let epoch = build_nonce_epoch_config(&cfg).expect("base=0 should be ok");
        assert_eq!(epoch.base, 0);
        assert_eq!(epoch.start_height, 1);
        assert!(epoch.start_tx_id.is_none());
    }

    #[test]
    fn nonce_epoch_config_rejects_zero_base_with_anchor() {
        let mut cfg = base_config();
        cfg.nonce_epoch_base = Some(0);
        cfg.nonce_epoch_start_height = Some(10);
        assert!(build_nonce_epoch_config(&cfg).is_err());
    }

    #[test]
    fn nonce_epoch_config_rejects_nonzero_base_with_zero_height() {
        let mut cfg = base_config();
        cfg.nonce_epoch_base = Some(5);
        cfg.nonce_epoch_start_height = Some(0);
        cfg.nonce_epoch_start_tx_id_base58 = Some(VALID_START_TX_ID.to_string());
        assert!(build_nonce_epoch_config(&cfg).is_err());
    }

    #[test]
    fn nonce_epoch_config_rejects_missing_anchor_with_nonzero_base() {
        let mut cfg = base_config();
        cfg.nonce_epoch_base = Some(5);
        assert!(build_nonce_epoch_config(&cfg).is_err());

        let mut cfg = base_config();
        cfg.nonce_epoch_base = Some(5);
        cfg.nonce_epoch_start_height = Some(10);
        assert!(build_nonce_epoch_config(&cfg).is_err());
    }

    #[test]
    fn nonce_epoch_config_rejects_tx_id_without_height() {
        let mut cfg = base_config();
        cfg.nonce_epoch_base = Some(5);
        cfg.nonce_epoch_start_tx_id_base58 = Some(VALID_START_TX_ID.to_string());
        assert!(build_nonce_epoch_config(&cfg).is_err());
    }

    #[test]
    fn nonce_epoch_config_accepts_anchor_for_nonzero_base() {
        let mut cfg = base_config();
        cfg.nonce_epoch_base = Some(5);
        cfg.nonce_epoch_start_height = Some(10);
        cfg.nonce_epoch_start_tx_id_base58 = Some(VALID_START_TX_ID.to_string());
        let epoch = build_nonce_epoch_config(&cfg).expect("anchor should be accepted");
        assert_eq!(epoch.base, 5);
        assert_eq!(epoch.start_height, 10);
        assert!(epoch.start_tx_id.is_some());
    }

    #[tokio::test]
    async fn test_signature_flow() -> Result<(), BridgeError> {
        init_tracing();

        let config_toml = base_config();

        let bridge_signer = Arc::new(BridgeSigner::new(config_toml.my_eth_key_hex().to_string())?);

        let proposal_hash = [42u8; 32];
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = unsafe {
            let mut ia =
                nockvm::noun::IndirectAtom::new_raw_bytes(&mut slab, 32, proposal_hash.as_ptr());
            ia.normalize_as_atom()
        };
        let signature = bridge_signer.sign_proposal(noun.as_noun()).await?;

        assert!(!signature.r().is_zero(), "Expected valid r component");
        assert!(!signature.s().is_zero(), "Expected valid s component");
        let sig_bytes = signature.as_bytes();
        assert!(
            sig_bytes[64] == 27 || sig_bytes[64] == 28,
            "Expected valid v component"
        );

        // Verify the signer can also sign a raw hash directly
        let hash_signature = bridge_signer.sign_hash(&proposal_hash).await?;
        assert!(
            !hash_signature.r().is_zero(),
            "Expected valid r component from sign_hash"
        );
        assert!(
            !hash_signature.s().is_zero(),
            "Expected valid s component from sign_hash"
        );

        Ok(())
    }

    fn tip5(a: u64, b: u64, c: u64, d: u64, e: u64) -> Tip5Hash {
        Tip5Hash([Belt(a), Belt(b), Belt(c), Belt(d), Belt(e)])
    }

    fn addr(byte: u8) -> EthAddress {
        EthAddress([byte; 20])
    }

    #[tokio::test]
    async fn cdc_persists_epoch_requests_and_skips_pre_epoch() -> Result<(), BridgeError> {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("deposit-log.sqlite");
        let log = bridge::deposit_log::DepositLog::open(path).await?;
        let epoch = NonceEpochConfig {
            base: 100,
            start_height: 10,
            start_tx_id: None,
        };

        let req_pre = bridge::types::NockDepositRequestKernelData {
            block_height: 9,
            tx_id: tip5(1, 0, 0, 0, 0),
            as_of: tip5(9, 9, 9, 9, 9),
            name: Name::new(tip5(10, 0, 0, 0, 0), tip5(11, 0, 0, 0, 0)),
            recipient: addr(0x11),
            amount: 1,
        };
        let req_a = bridge::types::NockDepositRequestKernelData {
            block_height: 10,
            tx_id: tip5(2, 0, 0, 0, 0),
            as_of: tip5(8, 8, 8, 8, 8),
            name: Name::new(tip5(12, 0, 0, 0, 0), tip5(13, 0, 0, 0, 0)),
            recipient: addr(0x22),
            amount: 2,
        };
        let req_b = bridge::types::NockDepositRequestKernelData {
            block_height: 11,
            tx_id: tip5(3, 0, 0, 0, 0),
            as_of: tip5(7, 7, 7, 7, 7),
            name: Name::new(tip5(14, 0, 0, 0, 0), tip5(15, 0, 0, 0, 0)),
            recipient: addr(0x33),
            amount: 3,
        };

        let inserted = persist_commit_nock_deposits_requests(
            vec![req_b.clone(), req_pre, req_a.clone()],
            &log,
            &epoch,
        )
        .await?;
        assert_eq!(inserted, 2);
        assert_eq!(log.number_of_deposits_in_epoch(&epoch).await?, 2);

        let first = log
            .get_by_nonce(epoch.base + 1, &epoch)
            .await?
            .expect("expected first nonce");
        assert_eq!(first.tx_id, req_a.tx_id);

        let inserted_again =
            persist_commit_nock_deposits_requests(vec![req_a, req_b], &log, &epoch).await?;
        assert_eq!(inserted_again, 0);

        Ok(())
    }

    #[tokio::test]
    async fn cdc_orders_by_height_then_tx_id() -> Result<(), BridgeError> {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("deposit-log.sqlite");
        let log = bridge::deposit_log::DepositLog::open(path).await?;
        let epoch = NonceEpochConfig {
            base: 10,
            start_height: 1,
            start_tx_id: None,
        };

        let req_low = bridge::types::NockDepositRequestKernelData {
            block_height: 5,
            tx_id: tip5(1, 0, 0, 0, 0),
            as_of: tip5(1, 1, 1, 1, 1),
            name: Name::new(tip5(2, 0, 0, 0, 0), tip5(3, 0, 0, 0, 0)),
            recipient: addr(0x44),
            amount: 4,
        };
        let req_high = bridge::types::NockDepositRequestKernelData {
            block_height: 5,
            tx_id: tip5(2, 0, 0, 0, 0),
            as_of: tip5(2, 2, 2, 2, 2),
            name: Name::new(tip5(4, 0, 0, 0, 0), tip5(5, 0, 0, 0, 0)),
            recipient: addr(0x55),
            amount: 5,
        };

        let inserted = persist_commit_nock_deposits_requests(
            vec![req_high.clone(), req_low.clone()],
            &log,
            &epoch,
        )
        .await?;
        assert_eq!(inserted, 2);

        let first = log
            .get_by_nonce(epoch.base + 1, &epoch)
            .await?
            .expect("expected first nonce");
        let second = log
            .get_by_nonce(epoch.base + 2, &epoch)
            .await?
            .expect("expected second nonce");
        assert_eq!(first.tx_id, req_low.tx_id);
        assert_eq!(second.tx_id, req_high.tx_id);

        Ok(())
    }
}
