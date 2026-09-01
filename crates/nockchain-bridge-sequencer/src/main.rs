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

type SequencerJournalHandle = bridge::withdrawal::sequencer::journal::SequencerJournalHandle;
type SequencerJournalRecoveryReport =
    bridge::withdrawal::sequencer::store::SequencerJournalRecoveryReport;
type BridgeError = bridge::shared::errors::BridgeError;

const RECOVERY_CHAIN_CATCHUP_POLL_INTERVAL: Duration = Duration::from_secs(2);
const DEFAULT_BASE_ACTIVITY_OVERLAP_BLOCKS: u64 = 100;
const DEFAULT_WITHDRAWAL_PUBLIC_DELAYED_AFTER_SECS: u64 = 24 * 60 * 60;

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
fn private_nockchain_client_addr(public_addr: SocketAddr, private_grpc_port: u16) -> SocketAddr {
    let loopback = match public_addr.ip() {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
    };
    SocketAddr::new(loopback, private_grpc_port)
}

fn withdrawal_sequencer_data_dir() -> PathBuf {
    nockapp::system_data_dir().join("nockchain")
}

fn withdrawal_public_page_token_key() -> Result<[u8; 32], BridgeError> {
    let raw = std::env::var("WITHDRAWAL_PUBLIC_PAGE_TOKEN_KEY").map_err(|_| {
        BridgeError::Config(
            "WITHDRAWAL_PUBLIC_PAGE_TOKEN_KEY must be set to a 32-byte hex secret".into(),
        )
    })?;
    let raw = raw.strip_prefix("0x").unwrap_or(&raw);
    let bytes = hex::decode(raw).map_err(|_| {
        BridgeError::Config("WITHDRAWAL_PUBLIC_PAGE_TOKEN_KEY must be valid hex".into())
    })?;
    bytes.try_into().map_err(|bytes: Vec<u8>| {
        BridgeError::Config(format!(
            "WITHDRAWAL_PUBLIC_PAGE_TOKEN_KEY must decode to 32 bytes, got {}",
            bytes.len()
        ))
    })
}

// The verifier setup releases multi-gigabyte contexts through jemalloc.
#[cfg(not(feature = "tracing-heap"))]
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
        default_value_t = DEFAULT_BASE_ACTIVITY_OVERLAP_BLOCKS,
        help = "Confirmed Base blocks rescanned on every withdrawal activity pass."
    )]
    base_activity_overlap_blocks: u64,

    #[arg(
        long,
        help = "First Base block eligible for automatic withdrawal recovery; set to the official WithdrawalWireV1 activation block."
    )]
    withdrawal_recovery_activation_block: u64,

    #[arg(
        long,
        help = "Dedicated listen address for the read-only public withdrawal query service."
    )]
    withdrawal_public_grpc_addr: SocketAddr,

    #[arg(
        long,
        default_value_t = DEFAULT_WITHDRAWAL_PUBLIC_DELAYED_AFTER_SECS,
        help = "Seconds before a still-pending public withdrawal is reported as delayed."
    )]
    withdrawal_public_delayed_after_secs: u64,

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

async fn wait_for_initial_base_recovery(
    scanner: &bridge::withdrawal::sequencer::base_verifier::SequencerBaseRpcWithdrawalVerifier,
    activity_store: &bridge::withdrawal::sequencer::base_activity::BaseActivityStore,
    sequencer_store: &bridge::withdrawal::sequencer::store::WithdrawalSequencerStore,
    overlap_blocks: u64,
    activation_block: u64,
) -> bridge::withdrawal::sequencer::base_verifier::SequencerBaseRecoveryPassReport {
    loop {
        match scanner
            .scan_and_recover_confirmed_burns(
                activity_store, sequencer_store, overlap_blocks, activation_block,
            )
            .await
        {
            Ok(report) => match &report.reconciliation {
                bridge::withdrawal::sequencer::store::BaseJournalReconciliationOutcome::Ready(
                    _,
                ) => return report,
                bridge::withdrawal::sequencer::store::BaseJournalReconciliationOutcome::ScannerBehind {
                    current_verified_block,
                    required_base_batch_end,
                } => {
                    info!(
                        target: "nockchain.withdrawal_sequencer.base_activity",
                        current_verified_block = ?current_verified_block,
                        required_base_batch_end,
                        "waiting for Base scanner to reach journal lifecycle facts"
                    );
                }
            },
            Err(err) => {
                error!(
                    target: "nockchain.withdrawal_sequencer.base_activity",
                    error = %err,
                    "initial Base burn scan/recovery failed; sequencer RPC readiness remains blocked"
                );
            }
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
    base_activity_scanner: Arc<
        bridge::withdrawal::sequencer::base_verifier::SequencerBaseRpcWithdrawalVerifier,
    >,
    base_activity_overlap_blocks: u64,
    withdrawal_recovery_activation_block: u64,
    withdrawal_public_grpc_addr: SocketAddr,
    withdrawal_public_config:
        bridge::withdrawal::sequencer::public_rpc::PublicWithdrawalQueryConfig,
    handoff_window_blocks: u64,
    authorized_submit_retry_after_base_blocks: u64,
    confirmation_policy: bridge::withdrawal::submission::WithdrawalSequencerConfirmationLoopPolicy,
    orphan_retry_policy: bridge::withdrawal::submission::WithdrawalSequencerOrphanRetryLoopPolicy,
    node_pkhs: Vec<bridge::shared::types::Tip5Hash>,
    node_eth_addresses: bridge::shared::signing::BridgeNodeEthAddressMap,
    journal: SequencerJournalHandle,
    compensated_withdrawals: bridge::shared::base::CompensatedWithdrawalRegistry,
    quote_service: Arc<dyn bridge::withdrawal::quote::WithdrawalQuotePort>,
    manual_submit_approval: bridge::withdrawal::sequencer::approval::ManualSubmitApprovalConfig,
) -> Result<tokio::task::JoinHandle<Result<(), BridgeError>>, Box<dyn Error>> {
    let data_dir = withdrawal_sequencer_data_dir();
    tokio::fs::create_dir_all(&data_dir).await?;
    if manual_submit_approval.enabled {
        tokio::fs::create_dir_all(&manual_submit_approval.approval_dir).await?;
    }
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
    let compensated_records = compensated_withdrawals
        .values()
        .map(
            |record| bridge::withdrawal::sequencer::base_incidents::CompensatedBaseWithdrawal {
                chain_id: withdrawal_public_config.base_chain_id,
                nock_contract_address: withdrawal_public_config.nock_contract_address,
                base_event_id: record.base_event_id.clone(),
                tx_hash: record.transaction_hash,
                log_index: record.log_index,
                reason: record.reason.clone(),
                evidence_reference: record.evidence_reference.clone(),
                recorded_at: record.recorded_at_unix_secs,
            },
        )
        .collect::<Vec<_>>();
    let compensated_inserted = withdrawal_state_store
        .base_activity_store()
        .incident_store()
        .record_compensated_withdrawals(compensated_records)
        .await?;
    info!(
        target: "nockchain.withdrawal_sequencer.base_activity",
        configured = compensated_withdrawals.values().count(),
        inserted = compensated_inserted,
        "loaded durable compensated withdrawal identities"
    );
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
    let base_activity_store = Arc::new(withdrawal_state_store.base_activity_store());
    let base_recovery_pass = wait_for_initial_base_recovery(
        base_activity_scanner.as_ref(),
        base_activity_store.as_ref(),
        &withdrawal_state_store,
        base_activity_overlap_blocks,
        withdrawal_recovery_activation_block,
    )
    .await;
    let base_reconciliation = match &base_recovery_pass.reconciliation {
        bridge::withdrawal::sequencer::store::BaseJournalReconciliationOutcome::Ready(report) => {
            report
        }
        bridge::withdrawal::sequencer::store::BaseJournalReconciliationOutcome::ScannerBehind {
            ..
        } => {
            return Err("Base scanner remained behind after readiness wait".into());
        }
    };
    info!(
        target: "nockchain.withdrawal_sequencer.base_activity",
        confirmed_tip = base_recovery_pass.scan.confirmed_tip,
        scan_start = base_recovery_pass.scan.scan_start,
        scan_end = base_recovery_pass.scan.scan_end,
        chunks_verified = base_recovery_pass.scan.chunks_verified,
        blocks_verified = base_recovery_pass.scan.blocks_verified,
        logs_seen = base_recovery_pass.scan.logs_seen,
        burns_inserted = base_recovery_pass.scan.burns_inserted,
        burns_rejected = base_recovery_pass.scan.burns_rejected,
        recovery_candidates = base_recovery_pass.recovery.candidates_inspected,
        recovered_pending = base_recovery_pass.recovery.recovered_pending,
        already_registered = base_recovery_pass.recovery.already_registered,
        ineligible = base_recovery_pass.recovery.ineligible,
        lifecycle_rows_validated = base_reconciliation.rows_validated,
        historical_rows_skipped = base_reconciliation.historical_rows_skipped,
        journal_sequence = base_reconciliation.journal_sequence,
        base_cursor_block = base_reconciliation.base_cursor_block,
        "initial Base burn scan and journal reconciliation completed"
    );
    let withdrawal_state_store = Arc::new(withdrawal_state_store);

    let sequencer_listen_addr = withdrawal_sequencer_listen_addr(public_addr, private_grpc_port)?;
    let public_client_addr = public_nockchain_client_addr(public_addr);
    let sequencer_submitter = Arc::new(
        bridge::withdrawal::submission::PublicNockchainWithdrawalSubmitter::new(format!(
            "http://{public_client_addr}"
        )),
    );
    let startup_reconciliation =
        bridge::withdrawal::submission::withdrawal_sequencer_startup_reconcile(
            withdrawal_state_store.as_ref(),
            sequencer_submitter.as_ref(),
            base_height_tracker.as_ref(),
            confirmation_policy.nockchain_confirmation_depth,
            orphan_retry_policy.retry_after_base_blocks,
        )
        .await?;
    info!(
        target: "nockchain.withdrawal_sequencer",
        inspected = startup_reconciliation.inspected,
        authorized_resubmitted = startup_reconciliation.authorized_resubmitted,
        mempool_accepted_observed = startup_reconciliation.mempool_accepted_observed,
        confirmed = startup_reconciliation.confirmed,
        stale_retries = startup_reconciliation.stale_retries,
        waiting = startup_reconciliation.waiting,
        lifecycle_rows_verified = startup_reconciliation.lifecycle_rows_verified,
        live_canonical_rows_verified = startup_reconciliation.live_canonical_rows_verified,
        confirmed_rows_verified = startup_reconciliation.confirmed_rows_verified,
        reservations_verified = startup_reconciliation.reservations_verified,
        "withdrawal sequencer startup reconciliation completed"
    );

    let service_store = withdrawal_state_store.clone();
    let confirmation_store = withdrawal_state_store.clone();
    let orphan_retry_store = withdrawal_state_store.clone();
    let confirmation_submitter = sequencer_submitter.clone();
    let orphan_retry_submitter = sequencer_submitter.clone();
    let orphan_retry_base_height_tracker = base_height_tracker.clone();
    let recurring_base_activity_scanner = base_activity_scanner.clone();
    let recurring_base_activity_store = base_activity_store.clone();
    let recurring_sequencer_store = withdrawal_state_store.clone();
    let public_query_service =
        bridge::withdrawal::sequencer::public_rpc::PublicWithdrawalQueryService::new(
            withdrawal_state_store.clone(),
            base_height_tracker.clone(),
            sequencer_submitter.clone(),
            quote_service,
            withdrawal_public_config,
        );
    tokio::spawn(async move {
        bridge::withdrawal::sequencer::base_verifier::run_confirmed_base_burn_tail_scanner(
            recurring_base_activity_scanner,
            recurring_base_activity_store,
            recurring_sequencer_store,
            base_activity_overlap_blocks,
            withdrawal_recovery_activation_block,
            BaseObserverLoopPolicy::default(),
        )
        .await;
    });
    let private_rpc_task = tokio::spawn(async move {
        bridge::withdrawal::sequencer::rpc::serve_withdrawal_sequencer(
            sequencer_listen_addr, service_store, sequencer_submitter, base_height_tracker,
            base_withdrawal_verifier, handoff_window_blocks,
            authorized_submit_retry_after_base_blocks, node_pkhs, node_eth_addresses,
            manual_submit_approval,
        )
        .await
    });
    let public_rpc_task = tokio::spawn(async move {
        bridge::withdrawal::sequencer::public_rpc::serve_public_withdrawal_query(
            withdrawal_public_grpc_addr, public_query_service,
        )
        .await
    });
    let rpc_task = tokio::spawn(async move {
        tokio::select! {
            result = private_rpc_task => match result {
                Ok(result) => result,
                Err(err) => Err(BridgeError::Runtime(format!(
                    "private withdrawal sequencer RPC task failed: {err}"
                ))),
            },
            result = public_rpc_task => match result {
                Ok(result) => result,
                Err(err) => Err(BridgeError::Runtime(format!(
                    "public withdrawal query RPC task failed: {err}"
                ))),
            },
        }
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
    if cli.base_activity_overlap_blocks == 0 {
        return Err("base activity overlap blocks must be greater than 0".into());
    }
    if cli.withdrawal_public_delayed_after_secs == 0 {
        return Err("withdrawal public delayed threshold must be greater than 0".into());
    }

    let base_height_tracker =
        Arc::new(bridge::withdrawal::sequencer::base_height::SequencerBaseHeightTracker::default());
    let mut nockchain_cli = cli.nockchain.clone();
    let public_addr = nockchain_cli
        .bind_public_grpc_addr
        .ok_or("nockchain-bridge-sequencer requires --bind-public-grpc-addr to be set")?;
    let prover_hot_state = nockchain::consensus::prepare_consensus_runtime(&mut nockchain_cli)?;

    let private_grpc_port = nockchain_cli.bind_private_grpc_port;
    let base_ws_url = cli.base_ws_url.clone();
    let verifier_base_ws_url = base_ws_url.clone();
    let base_confirmation_depth = cli.base_confirmation_depth;
    let base_activity_overlap_blocks = cli.base_activity_overlap_blocks;
    let withdrawal_recovery_activation_block = cli.withdrawal_recovery_activation_block;
    let withdrawal_public_grpc_addr = cli.withdrawal_public_grpc_addr;
    let withdrawal_public_delayed_after =
        Duration::from_secs(cli.withdrawal_public_delayed_after_secs);
    let withdrawal_public_page_token_key = withdrawal_public_page_token_key()?;
    let handoff_window_blocks = cli.withdrawal_handoff_window_blocks;
    let authorized_submit_retry_after_base_blocks = cli.authorized_submit_retry_after_base_blocks;
    let orphan_retry_policy =
        bridge::withdrawal::submission::WithdrawalSequencerOrphanRetryLoopPolicy {
            retry_after_base_blocks: cli.withdrawal_retry_after_base_blocks,
            ..bridge::withdrawal::submission::WithdrawalSequencerOrphanRetryLoopPolicy::default()
        };
    let sequencer_config =
        bridge::shared::config::SequencerConfigToml::from_file(&cli.sequencer_config_path)?;
    let expected_base_chain_id = sequencer_config.base_chain_id()?;
    let nock_contract_address = sequencer_config.nock_contract_address()?;
    let withdrawal_policy = sequencer_config.withdrawal_policy()?;
    let compensated_withdrawals = bridge::shared::base::CompensatedWithdrawalRegistry::from_config(
        &sequencer_config.compensated_withdrawals,
    )?;
    let sequencer_data_dir = withdrawal_sequencer_data_dir();
    let manual_submit_approval =
        bridge::withdrawal::sequencer::approval::ManualSubmitApprovalConfig {
            enabled: sequencer_config.manual_submit_approval,
            approval_dir: sequencer_config
                .manual_submit_approval_dir
                .clone()
                .unwrap_or_else(|| {
                    bridge::withdrawal::sequencer::approval::default_manual_submit_approval_dir(
                        &sequencer_data_dir,
                    )
                }),
        };
    let bridge_constants = sequencer_config.bridge_constants()?;
    let private_sequencer_port = private_grpc_port
        .checked_add(100)
        .ok_or("withdrawal sequencer port overflow")?;
    if withdrawal_public_grpc_addr.port() == public_addr.port()
        || withdrawal_public_grpc_addr.port() == private_sequencer_port
    {
        return Err(
            "withdrawal public gRPC port must differ from Nockchain public and private sequencer ports"
                .into(),
        );
    }
    if withdrawal_recovery_activation_block < bridge_constants.base_start_height {
        return Err(format!(
            "withdrawal recovery activation block {} precedes configured Base start height {}",
            withdrawal_recovery_activation_block, bridge_constants.base_start_height
        )
        .into());
    }
    let activation_offset =
        withdrawal_recovery_activation_block - bridge_constants.base_start_height;
    if activation_offset % bridge_constants.base_blocks_chunk != 0 {
        return Err(format!(
            "withdrawal recovery activation block {} must align to Base batch size {} from start {}",
            withdrawal_recovery_activation_block,
            bridge_constants.base_blocks_chunk,
            bridge_constants.base_start_height
        )
        .into());
    }
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
                expected_base_chain_id,
                base_confirmation_depth,
                nock_contract_address,
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
    let base_activity_scanner = Arc::new(
        bridge::withdrawal::sequencer::base_verifier::SequencerBaseRpcWithdrawalVerifier::connect(
            verifier_base_ws_url,
            expected_base_chain_id,
            nock_contract_address,
            base_height_tracker.clone(),
            bridge_constants.base_start_height,
            bridge_constants.base_blocks_chunk,
            base_confirmation_depth,
        )
        .await?,
    );
    let base_withdrawal_verifier: Arc<
        dyn bridge::withdrawal::sequencer::base_verifier::SequencerBaseWithdrawalVerifier,
    > = base_activity_scanner.clone();

    let (quote_spend_condition, quote_lock_root) =
        bridge::shared::config::derive_bridge_spend_authority_from_pkhs(
            bridge_constants.min_signers,
            withdrawal_node_pkhs.iter().cloned(),
        )?;
    let quote_private_addr = private_nockchain_client_addr(public_addr, private_grpc_port);
    let quote_service: Arc<dyn bridge::withdrawal::quote::WithdrawalQuotePort> = Arc::new(
        bridge::withdrawal::quote::NockchainWithdrawalQuoteService::new_private(
            format!("http://{quote_private_addr}"),
            quote_lock_root,
            quote_spend_condition,
            bridge_constants.nicks_fee_per_nock,
            sequencer_config.nockchain_confirmation_depth,
            Duration::from_secs(15),
        )?,
    );
    let api_config = nockchain::NockchainAPIConfig::EnablePublicServer(public_addr);
    let nockchain_app =
        nockchain::run_nockchain_app(nockchain_cli, prover_hot_state.as_slice(), api_config);
    tokio::pin!(nockchain_app);

    let withdrawal_public_config =
        bridge::withdrawal::sequencer::public_rpc::PublicWithdrawalQueryConfig {
            base_chain_id: expected_base_chain_id,
            nock_contract_address,
            policy_id: withdrawal_policy.id.to_string(),
            protocol_id: withdrawal_policy.wire_format.to_string(),
            page_token_key: withdrawal_public_page_token_key,
            delayed_after: withdrawal_public_delayed_after,
            base_stale_after: Duration::from_secs(60),
            admission_enabled: sequencer_config.public_withdrawal_admission_enabled,
        };

    let withdrawal_sequencer_start = start_withdrawal_sequencer(
        public_addr,
        private_grpc_port,
        base_height_tracker.clone(),
        base_withdrawal_verifier,
        base_activity_scanner,
        base_activity_overlap_blocks,
        withdrawal_recovery_activation_block,
        withdrawal_public_grpc_addr,
        withdrawal_public_config,
        handoff_window_blocks,
        authorized_submit_retry_after_base_blocks,
        confirmation_policy,
        orphan_retry_policy,
        withdrawal_node_pkhs,
        withdrawal_node_eth_addresses,
        journal,
        compensated_withdrawals,
        quote_service,
        manual_submit_approval,
    );
    tokio::pin!(withdrawal_sequencer_start);
    let withdrawal_sequencer_rpc_task = tokio::select! {
        result = &mut nockchain_app => return result,
        result = &mut withdrawal_sequencer_start => result?,
    };

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
