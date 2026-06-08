use std::collections::HashMap;
use std::error::Error;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bridge::core::loop_policy::BaseObserverLoopPolicy;
use clap::Parser;
use nockapp::kernel::boot;
use tracing::{error, info};
use zkvm_jetpack::hot::produce_prover_hot_state;

type SequencerJournalHandle = bridge::withdrawal::sequencer::journal::SequencerJournalHandle;
type SequencerJournalRecoveryReport =
    bridge::withdrawal::sequencer::store::SequencerJournalRecoveryReport;
type BridgeError = bridge::shared::errors::BridgeError;

const RECOVERY_CHAIN_CATCHUP_POLL_INTERVAL: Duration = Duration::from_secs(2);

fn withdrawal_sequencer_listen_addr(
    public_addr: SocketAddr,
    private_grpc_port: u16,
) -> Result<SocketAddr, Box<dyn Error>> {
    let listen_ip = match public_addr.ip() {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
    };
    let listen_port = private_grpc_port
        .checked_add(100)
        .ok_or("withdrawal sequencer port overflow")?;
    Ok(SocketAddr::new(listen_ip, listen_port))
}

fn public_nockchain_client_addr(public_addr: SocketAddr) -> SocketAddr {
    SocketAddr::new(
        match public_addr.ip() {
            IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
            ip => ip,
        },
        public_addr.port(),
    )
}

// When enabled, use jemalloc for more stable memory allocation.
#[cfg(all(feature = "jemalloc", not(feature = "tracing-heap")))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(feature = "tracing-heap")]
#[global_allocator]
static ALLOC: tracy_client::ProfiledAllocator<tikv_jemallocator::Jemalloc> =
    tracy_client::ProfiledAllocator::new(tikv_jemallocator::Jemalloc, 100);

#[derive(Parser, Debug, Clone)]
#[command(name = "nockchain-bridge-sequencer")]
struct NockchainBridgeSequencerCli {
    #[command(flatten)]
    nockchain: nockchain::NockchainCli,

    #[arg(
        long,
        help = "Base websocket URL used by the colocated sequencer watcher."
    )]
    base_ws_url: String,

    #[arg(
        long,
        default_value_t = bridge::shared::base::DEFAULT_BASE_CONFIRMATION_DEPTH,
        help = "Number of Base confirmations required before the sequencer records confirmed base height."
    )]
    base_confirmation_depth: u64,

    #[arg(
        long,
        default_value_t = bridge::withdrawal::state::WithdrawalFallbackPolicy::default().submission_timeout_blocks,
        help = "Confirmed Base blocks before the sequencer lazily hands post-canonical proposer responsibility to the next node."
    )]
    withdrawal_handoff_window_blocks: u64,

    #[arg(
        long,
        default_value_t = bridge::withdrawal::submission::WithdrawalSequencerOrphanRetryLoopPolicy::default().retry_after_base_blocks,
        help = "Confirmed Base blocks before the sequencer retries a mempool-accepted but still-unconfirmed withdrawal transaction."
    )]
    withdrawal_retry_after_base_blocks: u64,

    #[arg(
        long,
        default_value_t = bridge::withdrawal::submission::WithdrawalSequencerOrphanRetryLoopPolicy::default().retry_after_base_blocks,
        help = "Confirmed Base blocks before the sequencer retries an authorized withdrawal after submission was deferred or failed."
    )]
    authorized_submit_retry_after_base_blocks: u64,

    #[arg(
        long = "sequencer-config-path",
        alias = "bridge-config-path",
        help = "Path to the standalone withdrawal sequencer config. The deprecated --bridge-config-path alias is accepted for compatibility."
    )]
    sequencer_config_path: PathBuf,

    #[arg(
        long,
        help = "S3-compatible endpoint for the withdrawal sequencer durable journal, e.g. a Cloudflare R2 endpoint. May also be set with WITHDRAWAL_SEQUENCER_JOURNAL_OBJECT_STORE_ENDPOINT."
    )]
    sequencer_journal_object_store_endpoint: Option<String>,

    #[arg(
        long,
        help = "R2/S3 bucket for the withdrawal sequencer durable journal. May also be set with WITHDRAWAL_SEQUENCER_JOURNAL_OBJECT_STORE_BUCKET."
    )]
    sequencer_journal_object_store_bucket: Option<String>,

    #[arg(
        long,
        help = "Object-store signing region for the withdrawal sequencer durable journal. Overrides sequencer config; Cloudflare R2 commonly uses 'auto'."
    )]
    sequencer_journal_object_store_region: Option<String>,

    #[arg(
        long,
        help = "Object key prefix for the withdrawal sequencer durable journal. Overrides sequencer config."
    )]
    sequencer_journal_object_store_prefix: Option<String>,

    #[arg(
        long,
        help = "Deployment-bound withdrawal sequencer journal id. May also be set with WITHDRAWAL_SEQUENCER_JOURNAL_ID."
    )]
    sequencer_journal_id: Option<String>,

    #[arg(
        long,
        help = "R2/S3 access key id for the withdrawal sequencer durable journal. May also be set with WITHDRAWAL_SEQUENCER_JOURNAL_OBJECT_STORE_ACCESS_KEY_ID."
    )]
    sequencer_journal_object_store_access_key_id: Option<String>,

    #[arg(
        long,
        help = "R2/S3 secret access key for the withdrawal sequencer durable journal. May also be set with WITHDRAWAL_SEQUENCER_JOURNAL_OBJECT_STORE_SECRET_ACCESS_KEY."
    )]
    sequencer_journal_object_store_secret_access_key: Option<String>,
}

fn cli_or_env(value: &Option<String>, env_key: &str) -> Option<String> {
    value
        .clone()
        .or_else(|| std::env::var(env_key).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn build_sequencer_journal(
    cli: &NockchainBridgeSequencerCli,
    journal_config: &bridge::shared::config::SequencerJournalConfigToml,
) -> Result<SequencerJournalHandle, Box<dyn Error>> {
    if !journal_config.enabled {
        return Ok(SequencerJournalHandle::disabled());
    }
    let object_store = &journal_config.object_store;
    let required = |value: Option<String>, name: &str| -> Result<String, Box<dyn Error>> {
        value.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{name} is required when sequencer journal is enabled"),
            )
            .into()
        })
    };
    let endpoint = required(
        cli_or_env(
            &cli.sequencer_journal_object_store_endpoint,
            "WITHDRAWAL_SEQUENCER_JOURNAL_OBJECT_STORE_ENDPOINT",
        )
        .or_else(|| std::env::var("WITHDRAWAL_SEQUENCER_EVENT_LOG_S3_ENDPOINT").ok())
        .or_else(|| object_store.endpoint.clone()),
        "sequencer journal object-store endpoint",
    )?;
    let bucket = required(
        cli_or_env(
            &cli.sequencer_journal_object_store_bucket,
            "WITHDRAWAL_SEQUENCER_JOURNAL_OBJECT_STORE_BUCKET",
        )
        .or_else(|| std::env::var("WITHDRAWAL_SEQUENCER_EVENT_LOG_S3_BUCKET").ok())
        .or_else(|| object_store.bucket.clone()),
        "sequencer journal object-store bucket",
    )?;
    let region = cli_or_env(
        &cli.sequencer_journal_object_store_region,
        "WITHDRAWAL_SEQUENCER_JOURNAL_OBJECT_STORE_REGION",
    )
    .or_else(|| std::env::var("WITHDRAWAL_SEQUENCER_EVENT_LOG_S3_REGION").ok())
    .unwrap_or_else(|| object_store.region.clone());
    let prefix = cli_or_env(
        &cli.sequencer_journal_object_store_prefix,
        "WITHDRAWAL_SEQUENCER_JOURNAL_OBJECT_STORE_PREFIX",
    )
    .or_else(|| std::env::var("WITHDRAWAL_SEQUENCER_EVENT_LOG_S3_PREFIX").ok())
    .unwrap_or_else(|| object_store.prefix.clone());
    let journal_id = cli_or_env(&cli.sequencer_journal_id, "WITHDRAWAL_SEQUENCER_JOURNAL_ID")
        .unwrap_or_else(|| object_store.journal_id.clone());
    let verifier_address = required(
        journal_config.verifier_address.clone(),
        "sequencer journal verifier address",
    )?;
    let signing_key = required(
        std::env::var("WITHDRAWAL_SEQUENCER_JOURNAL_SIGNING_KEY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        "sequencer journal signing key",
    )?;
    let access_key_id = required(
        cli_or_env(
            &cli.sequencer_journal_object_store_access_key_id,
            "WITHDRAWAL_SEQUENCER_JOURNAL_OBJECT_STORE_ACCESS_KEY_ID",
        )
        .or_else(|| std::env::var("WITHDRAWAL_SEQUENCER_EVENT_LOG_S3_ACCESS_KEY_ID").ok())
        .or_else(|| object_store.access_key_id.clone()),
        "sequencer journal object-store access key id",
    )?;
    let secret_access_key = required(
        cli_or_env(
            &cli.sequencer_journal_object_store_secret_access_key,
            "WITHDRAWAL_SEQUENCER_JOURNAL_OBJECT_STORE_SECRET_ACCESS_KEY",
        )
        .or_else(|| std::env::var("WITHDRAWAL_SEQUENCER_EVENT_LOG_S3_SECRET_ACCESS_KEY").ok())
        .or_else(|| object_store.secret_access_key.clone()),
        "sequencer journal object-store secret access key",
    )?;
    let config = bridge::withdrawal::sequencer::journal::ObjectStoreSequencerJournalConfig {
        endpoint,
        bucket,
        region,
        prefix,
        journal_id,
        access_key_id,
        secret_access_key,
        verifier_address,
        signing_key,
    };
    Ok(SequencerJournalHandle::object_store(config)?)
}

async fn wait_for_replayed_base_bound(
    base_height_tracker: &bridge::withdrawal::sequencer::base_height::SequencerBaseHeightTracker,
    report: &SequencerJournalRecoveryReport,
) {
    let Some(required_base_height) = report.max_replayed_base_height else {
        return;
    };

    loop {
        if let Some(current_base_height) = base_height_tracker.latest_confirmed_base_height() {
            if current_base_height >= required_base_height {
                info!(
                    target: "nockchain.withdrawal_sequencer",
                    journal_id = %report.journal_id,
                    current_base_height,
                    required_base_height,
                    "sequencer Base watcher reached journal replay lower bound"
                );
                return;
            }
            info!(
                target: "nockchain.withdrawal_sequencer",
                journal_id = %report.journal_id,
                current_base_height,
                required_base_height,
                "waiting for Base watcher to catch up to replayed journal events"
            );
        } else {
            info!(
                target: "nockchain.withdrawal_sequencer",
                journal_id = %report.journal_id,
                required_base_height,
                "waiting for initial confirmed Base height before serving sequencer RPC"
            );
        }
        tokio::time::sleep(RECOVERY_CHAIN_CATCHUP_POLL_INTERVAL).await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn start_withdrawal_sequencer(
    public_addr: SocketAddr,
    private_grpc_port: u16,
    base_height_tracker: Arc<
        bridge::withdrawal::sequencer::base_height::SequencerBaseHeightTracker,
    >,
    base_withdrawal_verifier: Arc<
        dyn bridge::withdrawal::sequencer::base_verifier::SequencerBaseWithdrawalVerifier,
    >,
    handoff_window_blocks: u64,
    authorized_submit_retry_after_base_blocks: u64,
    confirmation_policy: bridge::withdrawal::submission::WithdrawalSequencerConfirmationLoopPolicy,
    orphan_retry_policy: bridge::withdrawal::submission::WithdrawalSequencerOrphanRetryLoopPolicy,
    node_pkhs: Vec<bridge::shared::types::Tip5Hash>,
    node_eth_addresses: bridge::shared::signing::BridgeNodeEthAddressMap,
    journal: SequencerJournalHandle,
) -> Result<tokio::task::JoinHandle<Result<(), BridgeError>>, Box<dyn Error>> {
    let data_dir = nockapp::system_data_dir().join("nockchain");
    tokio::fs::create_dir_all(&data_dir).await?;
    let mut withdrawal_state_store =
        bridge::withdrawal::sequencer::store::WithdrawalSequencerStore::open(
            data_dir.join("withdrawal-state-store.sqlite"),
        )
        .await?;
    let journal_enabled = journal.is_enabled();
    bridge::observability::metrics::init_metrics()
        .sequencer_withdrawal_journal_enabled
        .swap(if journal_enabled { 1.0 } else { 0.0 });
    withdrawal_state_store = withdrawal_state_store.with_journal(journal);
    if journal_enabled {
        let recovery = match withdrawal_state_store
            .recover_from_journal_on_startup()
            .await
        {
            Ok(recovery) => recovery,
            Err(err) => {
                bridge::observability::metrics::init_metrics()
                    .sequencer_withdrawal_journal_recovery_error
                    .increment();
                return Err(Box::new(err));
            }
        };
        if let Some(recovery) = recovery {
            bridge::observability::metrics::init_metrics()
                .sequencer_withdrawal_journal_recovery_events_replayed
                .swap(recovery.replayed_count as f64);
            info!(
                target: "nockchain.withdrawal_sequencer",
                journal_id = %recovery.journal_id,
                start_sequence = recovery.start_sequence,
                start_event_id = %recovery.start_event_id,
                last_sequence = recovery.last_sequence,
                last_event_id = %recovery.last_event_id,
                replayed_count = recovery.replayed_count,
                max_replayed_base_height = ?recovery.max_replayed_base_height,
                max_replayed_nockchain_height = ?recovery.max_replayed_nockchain_height,
                "withdrawal sequencer durable journal recovered"
            );
            // Replay rebuilds the sequencer projection before serving RPC. Base
            // withdrawal discovery stays with bridge/kernel projection, while
            // Nockchain inclusion and retry catch-up run in the sequencer loops
            // spawned below.
            wait_for_replayed_base_bound(&base_height_tracker, &recovery).await;
        }
        info!(
            target: "nockchain.withdrawal_sequencer",
            "withdrawal sequencer durable R2/S3-compatible journal enabled"
        );
    }
    let withdrawal_state_store = Arc::new(withdrawal_state_store);

    let sequencer_listen_addr = withdrawal_sequencer_listen_addr(public_addr, private_grpc_port)?;
    let public_client_addr = public_nockchain_client_addr(public_addr);
    let sequencer_submitter = Arc::new(
        bridge::withdrawal::submission::PublicNockchainWithdrawalSubmitter::new(format!(
            "http://{public_client_addr}"
        )),
    );

    let service_store = withdrawal_state_store.clone();
    let confirmation_store = withdrawal_state_store.clone();
    let orphan_retry_store = withdrawal_state_store.clone();
    let confirmation_submitter = sequencer_submitter.clone();
    let orphan_retry_submitter = sequencer_submitter.clone();
    let orphan_retry_base_height_tracker = base_height_tracker.clone();
    let rpc_task = tokio::spawn(async move {
        bridge::withdrawal::sequencer::rpc::serve_withdrawal_sequencer(
            sequencer_listen_addr, service_store, sequencer_submitter, base_height_tracker,
            base_withdrawal_verifier, handoff_window_blocks,
            authorized_submit_retry_after_base_blocks, node_pkhs, node_eth_addresses,
        )
        .await
    });
    tokio::spawn(async move {
        if let Err(err) =
            bridge::withdrawal::submission::run_withdrawal_sequencer_confirmation_loop(
                confirmation_store, confirmation_submitter, confirmation_policy,
            )
            .await
        {
            error!(
                target: "nockchain.withdrawal_sequencer",
                error = %err,
                "withdrawal sequencer confirmation loop exited"
            );
        }
    });
    tokio::spawn(async move {
        if let Err(err) =
            bridge::withdrawal::submission::run_withdrawal_sequencer_orphan_retry_loop(
                orphan_retry_store, orphan_retry_submitter, orphan_retry_base_height_tracker,
                orphan_retry_policy,
            )
            .await
        {
            error!(
                target: "nockchain.withdrawal_sequencer",
                error = %err,
                "withdrawal sequencer orphan retry loop exited"
            );
        }
    });

    Ok(rpc_task)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    nockvm::check_endian();
    let cli = NockchainBridgeSequencerCli::parse();
    boot::init_default_tracing(&cli.nockchain.nockapp_cli);

    if cli.base_confirmation_depth == 0 {
        return Err("base confirmation depth must be greater than 0".into());
    }

    let base_height_tracker =
        Arc::new(bridge::withdrawal::sequencer::base_height::SequencerBaseHeightTracker::default());
    let nockchain_cli = cli.nockchain.clone();
    let public_addr = nockchain_cli
        .bind_public_grpc_addr
        .ok_or("nockchain-bridge-sequencer requires --bind-public-grpc-addr to be set")?;

    let base_ws_url = cli.base_ws_url.clone();
    let verifier_base_ws_url = base_ws_url.clone();
    let base_confirmation_depth = cli.base_confirmation_depth;
    let handoff_window_blocks = cli.withdrawal_handoff_window_blocks;
    let authorized_submit_retry_after_base_blocks = cli.authorized_submit_retry_after_base_blocks;
    let orphan_retry_policy =
        bridge::withdrawal::submission::WithdrawalSequencerOrphanRetryLoopPolicy {
            retry_after_base_blocks: cli.withdrawal_retry_after_base_blocks,
            ..bridge::withdrawal::submission::WithdrawalSequencerOrphanRetryLoopPolicy::default()
        };
    let sequencer_config =
        bridge::shared::config::SequencerConfigToml::from_file(&cli.sequencer_config_path)?;
    let bridge_constants = sequencer_config.bridge_constants()?;
    let journal = build_sequencer_journal(&cli, &sequencer_config.sequencer_journal)?;
    let confirmation_policy =
        bridge::withdrawal::submission::WithdrawalSequencerConfirmationLoopPolicy {
            nockchain_confirmation_depth: sequencer_config.nockchain_confirmation_depth,
            ..bridge::withdrawal::submission::WithdrawalSequencerConfirmationLoopPolicy::default()
        };
    let sequencer_nodes = sequencer_config.validated_nodes()?;
    let withdrawal_node_pkhs: Vec<_> = sequencer_nodes
        .iter()
        .map(|node| node.nock_pkh.clone())
        .collect();
    let withdrawal_node_eth_addresses = sequencer_nodes
        .iter()
        .enumerate()
        .map(|(idx, node)| (idx as u64, node.eth_address))
        .collect::<HashMap<_, _>>();
    let watcher_base_height_tracker = base_height_tracker.clone();
    tokio::spawn(async move {
        if let Err(err) =
            bridge::withdrawal::sequencer::base_height::run_confirmed_base_height_watcher(
                base_ws_url,
                base_confirmation_depth,
                watcher_base_height_tracker,
                BaseObserverLoopPolicy::default(),
            )
            .await
        {
            error!(
                target: "nockchain.withdrawal_sequencer",
                error = %err,
                "sequencer base height watcher exited"
            );
        }
    });

    let initial_confirmed_base_height = base_height_tracker
        .wait_for_initial_confirmed_base_height()
        .await;
    info!(
        target: "nockchain.withdrawal_sequencer",
        confirmed_base_height = initial_confirmed_base_height,
        "sequencer base height watcher initialized; starting withdrawal sequencer service"
    );
    let base_withdrawal_verifier = Arc::new(
        bridge::withdrawal::sequencer::base_verifier::SequencerBaseRpcWithdrawalVerifier::connect(
            verifier_base_ws_url,
            sequencer_config.nock_contract_address()?,
            base_height_tracker.clone(),
            bridge_constants.base_blocks_chunk,
        )
        .await?,
    )
        as Arc<dyn bridge::withdrawal::sequencer::base_verifier::SequencerBaseWithdrawalVerifier>;

    let withdrawal_sequencer_rpc_task = start_withdrawal_sequencer(
        public_addr,
        nockchain_cli.bind_private_grpc_port,
        base_height_tracker.clone(),
        base_withdrawal_verifier,
        handoff_window_blocks,
        authorized_submit_retry_after_base_blocks,
        confirmation_policy,
        orphan_retry_policy,
        withdrawal_node_pkhs,
        withdrawal_node_eth_addresses,
        journal,
    )
    .await?;

    let api_config = nockchain::NockchainAPIConfig::EnablePublicServer(public_addr);
    let prover_hot_state = produce_prover_hot_state();
    let nockchain_app =
        nockchain::run_nockchain_app(nockchain_cli, prover_hot_state.as_slice(), api_config);
    tokio::pin!(nockchain_app);
    tokio::select! {
        result = &mut nockchain_app => result,
        result = withdrawal_sequencer_rpc_task => {
            match result {
                Ok(Ok(())) => {
                    error!(
                        target: "nockchain.withdrawal_sequencer",
                        "withdrawal sequencer RPC service exited unexpectedly"
                    );
                    Err("withdrawal sequencer RPC service exited unexpectedly".into())
                }
                Ok(Err(err)) => {
                    error!(
                        target: "nockchain.withdrawal_sequencer",
                        error = %err,
                        "withdrawal sequencer RPC service exited"
                    );
                    Err(format!("withdrawal sequencer RPC service exited: {err}").into())
                }
                Err(err) => {
                    error!(
                        target: "nockchain.withdrawal_sequencer",
                        error = %err,
                        "withdrawal sequencer RPC task failed"
                    );
                    Err(format!("withdrawal sequencer RPC task failed: {err}").into())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn withdrawal_sequencer_rpc_binds_loopback_for_ipv4_public_addr() {
        let public_addr: SocketAddr = "0.0.0.0:5556".parse().expect("public addr");
        let expected_addr: SocketAddr = "127.0.0.1:5655".parse().expect("expected addr");
        let listen_addr = withdrawal_sequencer_listen_addr(public_addr, 5555).expect("listen addr");

        assert_eq!(listen_addr, expected_addr);
    }

    #[test]
    fn withdrawal_sequencer_rpc_binds_loopback_for_ipv6_public_addr() {
        let public_addr: SocketAddr = "[::]:5556".parse().expect("public addr");
        let expected_addr: SocketAddr = "[::1]:5655".parse().expect("expected addr");
        let listen_addr = withdrawal_sequencer_listen_addr(public_addr, 5555).expect("listen addr");

        assert_eq!(listen_addr, expected_addr);
    }

    #[test]
    fn withdrawal_sequencer_rpc_rejects_port_overflow() {
        let public_addr: SocketAddr = "0.0.0.0:5556".parse().expect("public addr");

        assert!(withdrawal_sequencer_listen_addr(public_addr, u16::MAX).is_err());
    }

    #[test]
    fn public_nockchain_client_addr_preserves_external_public_addr() {
        let public_addr: SocketAddr = "10.1.2.3:5556".parse().expect("public addr");

        assert_eq!(public_nockchain_client_addr(public_addr), public_addr);
    }
}
