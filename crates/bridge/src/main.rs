use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bridge::core::loop_policy::NockObserverLoopPolicy;
use bridge::deposit::cache::ProposalCache;
use bridge::deposit::log::create_commit_nock_deposits_driver;
use bridge::deposit::runtime::{
    bootstrap_runtime as bootstrap_deposit_runtime,
    spawn_runtime_loops as spawn_deposit_runtime_loops, DepositRuntimeContext,
};
use bridge::observability::health::{
    derive_peer_endpoints, initialize_health_state, HealthMonitorConfig,
};
use bridge::observability::status::{run_hourly_rotation, BridgeStatus, BridgeStatusState};
use bridge::observability::tui::{cleanup_old_logs, init_bridge_tracing};
use bridge::observability::tui_api::WithdrawalTuiSource;
use bridge::shared::base::BaseBridge;
use bridge::shared::config::{
    canonical_testing_bridge_lock_root, derive_bridge_spend_authority_from_nodes, NonceEpochConfig,
};
use bridge::shared::errors::BridgeError;
use bridge::shared::ingress;
use bridge::shared::nockchain::{
    bootstrap_blockchain_constants, validate_blockchain_constants_match, NockchainWatcher,
    BLOCKCHAIN_CONSTANTS_PATH,
};
use bridge::shared::runtime::{
    spawn_base_observer, spawn_kernel_runtime, BridgeRuntime, KernelCauseBuilder,
};
use bridge::shared::signing::{
    extract_bridge_node_eth_addresses, extract_valid_bridge_addresses, BridgeSigner,
};
use bridge::shared::stop::create_stop_driver;
use bridge::shared::types::{BridgeConstants, NodeConfig, Tip5Hash};
use bridge::withdrawal::assembly::{
    create_withdrawal_execution_driver, WithdrawalAssemblyContext, WithdrawalAssemblyPlannerConfig,
    WithdrawalExecutionDriverContext, WithdrawalFatalStopContext, WithdrawalSigningContext,
};
use bridge::withdrawal::proposals::{WithdrawalProjectionStore, WithdrawalProposalRegistry};
use bridge::withdrawal::runtime::{
    bootstrap_runtime as bootstrap_withdrawal_runtime,
    spawn_runtime_loops as spawn_withdrawal_runtime_loops, WithdrawalRuntimeContext,
};
use bridge::withdrawal::sequencer::client::GrpcWithdrawalSequencerClient;
use bridge::withdrawal::snapshot::{BridgeNoteSnapshotService, BridgeOwnedNoteSelectors};
use bridge::withdrawal::state::WithdrawalFallbackPolicy;
use bridge::withdrawal::submission::WithdrawalSubmissionContext;
use bridge::withdrawal::transport::WithdrawalProposalTransport;
use bridge::withdrawal::validation::WithdrawalTransactionBodyValidator;
use clap::Parser;
use kernels_open_bridge::KERNEL;
use nockapp::kernel::boot::{self, Cli as BootCli};
use nockapp::nockapp::wire::Wire;
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::{exit_driver, markdown_driver, system_data_dir, NounAllocator};
use nockapp_grpc::services::public_nockchain::v1::driver::grpc_listener_driver;
use nockchain_types::v1::{FirstName, Lock, SpendCondition};
use nockchain_types::BlockchainConstants;
use noun_serde::{NounDecode, NounSerdeEncodeExt};
use tokio::{fs as tokio_fs, signal};
use tracing::info;
use wallet_tx_builder::types::RawNoteDataEntry;
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
const NOCK_OBSERVER_POLL_MILLIS_ENV: &str = "BRIDGE_NOCK_OBSERVER_POLL_MILLIS";
const DEFAULT_BRIDGE_WITHDRAWAL_LOCK_ROOT_B58: &str =
    "AcsPkuhXQoGeEsF91yynpm1kcW17PQ2Z1MEozgx7YnDPkZwrtzLuuqd";

/// Resolves the bridge-local data directory. This does not create the
/// directory because nockapp's `--new` check must inspect the kernel boot
/// directory before bridge-local logs or SQLite files are materialized.
fn bridge_data_dir(cli_dir: Option<PathBuf>) -> PathBuf {
    cli_dir.unwrap_or_else(|| system_data_dir().join("bridge"))
}

async fn ensure_bridge_data_dir(bridge_data_dir: &Path) -> Result<(), BridgeError> {
    if !bridge_data_dir.exists() {
        tokio_fs::create_dir_all(bridge_data_dir)
            .await
            .map_err(|e| BridgeError::Config(format!("Failed to create bridge data dir: {}", e)))?;
    }
    Ok(())
}

/// Ensures that a config file exists at `path`, materializing the example
/// template on first boot.
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

fn parse_optional_duration_millis_env_value(
    key: &str,
    raw: Option<&str>,
) -> Result<Option<Duration>, BridgeError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let millis = trimmed.parse::<u64>().map_err(|err| {
        BridgeError::Config(format!("{key} must be a u64 millisecond value: {err}"))
    })?;
    if millis == 0 {
        return Err(BridgeError::Config(format!("{key} must be greater than 0")));
    }
    Ok(Some(Duration::from_millis(millis)))
}

fn nock_observer_loop_policy_from_env() -> Result<NockObserverLoopPolicy, BridgeError> {
    let mut policy = NockObserverLoopPolicy::default();
    if let Some(poll_interval) = parse_optional_duration_millis_env_value(
        NOCK_OBSERVER_POLL_MILLIS_ENV,
        std::env::var(NOCK_OBSERVER_POLL_MILLIS_ENV).ok().as_deref(),
    )? {
        policy.poll_interval = poll_interval;
    }
    Ok(policy)
}

/// Derives the ingress listen address from the local node entry when the config
/// omits an explicit ingress endpoint.
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

/// Validates and normalizes the deposit nonce-epoch activation settings into the
/// runtime structure used by deposit processing.
fn build_deposit_nonce_epoch_config(
    config_toml: &bridge::shared::config::BridgeConfigToml,
) -> Result<NonceEpochConfig, BridgeError> {
    let deposit_nonce_epoch_base_opt = config_toml.deposit_nonce_epoch_base;
    let deposit_nonce_epoch_start_height = config_toml.deposit_nonce_epoch_start_height;
    let nonce_epoch_start_tx_id = config_toml.deposit_nonce_epoch_start_tx_id()?;

    if deposit_nonce_epoch_start_height.is_none() && nonce_epoch_start_tx_id.is_some() {
        return Err(BridgeError::Config(
            "deposit_nonce_epoch_start_tx_id_base58 requires deposit_nonce_epoch_start_height"
                .into(),
        ));
    }

    let start_key_set =
        deposit_nonce_epoch_start_height.is_some() || nonce_epoch_start_tx_id.is_some();
    if let Some(base) = deposit_nonce_epoch_base_opt {
        if base == 0 {
            if start_key_set {
                return Err(BridgeError::Config(
                    "deposit_nonce_epoch_base must be non-zero when deposit_nonce_epoch_start_height or deposit_nonce_epoch_start_tx_id_base58 is set".into(),
                ));
            }
        } else {
            let Some(height) = deposit_nonce_epoch_start_height else {
                return Err(BridgeError::Config(
                    "deposit_nonce_epoch_start_height must be set when deposit_nonce_epoch_base is non-zero".into(),
                ));
            };
            if height == 0 {
                return Err(BridgeError::Config(
                    "deposit_nonce_epoch_start_height must be greater than 0 when set".into(),
                ));
            }
            if nonce_epoch_start_tx_id.is_none() {
                return Err(BridgeError::Config(
                    "deposit_nonce_epoch_start_tx_id_base58 must be set when deposit_nonce_epoch_base is non-zero"
                        .into(),
                ));
            }
        }
    } else if start_key_set {
        return Err(BridgeError::Config(
            "deposit_nonce_epoch_base must be set when deposit_nonce_epoch_start_height or deposit_nonce_epoch_start_tx_id_base58 is set".into(),
        ));
    }

    let deposit_nonce_epoch_base = deposit_nonce_epoch_base_opt.unwrap_or(0);
    let deposit_nonce_epoch_start_height = deposit_nonce_epoch_start_height.unwrap_or(1);
    Ok(NonceEpochConfig {
        base: deposit_nonce_epoch_base,
        start_height: deposit_nonce_epoch_start_height,
        start_tx_id: nonce_epoch_start_tx_id,
    })
}

/// Derives the first-name selector for the bridge-owned withdrawal lock root.
fn bridge_multisig_first_name(bridge_lock_root: &Tip5Hash) -> Result<String, BridgeError> {
    let first_name = FirstName::from_lock_root(bridge_lock_root).map_err(|err| {
        BridgeError::Config(format!(
            "failed to derive bridge multisig first-name from withdrawal lock root: {err}"
        ))
    })?;
    Ok(first_name.into_hash().to_base58())
}

/// Builds the selector set used to query bridge-owned notes from the private
/// Nockchain balance snapshot APIs.
fn bridge_owned_note_selectors(
    bridge_lock_root: &Tip5Hash,
) -> Result<BridgeOwnedNoteSelectors, BridgeError> {
    Ok(BridgeOwnedNoteSelectors {
        first_names: vec![bridge_multisig_first_name(bridge_lock_root)?],
    })
}

fn canonical_mainnet_bridge_lock_root() -> Result<Tip5Hash, BridgeError> {
    Tip5Hash::from_base58(DEFAULT_BRIDGE_WITHDRAWAL_LOCK_ROOT_B58).map_err(|err| {
        BridgeError::Config(format!(
            "invalid default bridge withdrawal lock root {}: {err}",
            DEFAULT_BRIDGE_WITHDRAWAL_LOCK_ROOT_B58
        ))
    })
}

fn expected_bridge_lock_root_for_environment(
    blockchain_constants: &BlockchainConstants,
) -> Result<Tip5Hash, BridgeError> {
    if blockchain_constants == &BlockchainConstants::default() {
        canonical_mainnet_bridge_lock_root()
    } else {
        canonical_testing_bridge_lock_root()
    }
}

fn resolve_validated_bridge_spend_authority(
    node_config: &NodeConfig,
    bridge_constants: &BridgeConstants,
    blockchain_constants: &BlockchainConstants,
) -> Result<(Tip5Hash, SpendCondition), BridgeError> {
    let declared_root = node_config.bridge_lock_root.clone();
    let (spend_authority_spend_condition, derived_root) =
        derive_bridge_spend_authority_from_nodes(&node_config.nodes, bridge_constants.min_signers)?;
    if declared_root != derived_root {
        return Err(BridgeError::Config(format!(
            "configured bridge_lock_root {} does not match signer-derived root {}",
            declared_root.to_base58(),
            derived_root.to_base58()
        )));
    }
    let expected_root = expected_bridge_lock_root_for_environment(blockchain_constants)?;
    if declared_root != expected_root {
        let environment = if blockchain_constants == &BlockchainConstants::default() {
            "mainnet"
        } else {
            "testing/fakenet"
        };
        return Err(BridgeError::Config(format!(
            "configured bridge_lock_root {} does not match canonical {} bridge root {}",
            declared_root.to_base58(),
            environment,
            expected_root.to_base58()
        )));
    }
    Ok((declared_root, spend_authority_spend_condition))
}

/// Converts the connected blockchain constants into the planner configuration
/// used when assembling withdrawal transactions.
fn withdrawal_planner_config(
    bridge_lock_root: Tip5Hash,
    spend_authority_spend_condition: SpendCondition,
    bridge_constants: &BridgeConstants,
    blockchain_constants: &BlockchainConstants,
) -> Result<WithdrawalAssemblyPlannerConfig, BridgeError> {
    Ok(WithdrawalAssemblyPlannerConfig {
        spend_authority_lock_root: bridge_lock_root.clone(),
        spend_authority_spend_condition: spend_authority_spend_condition.clone(),
        refund_lock_root: bridge_lock_root,
        refund_note_data: vec![RawNoteDataEntry::from_lock(Lock::SpendCondition(
            spend_authority_spend_condition,
        ))],
        nicks_fee_per_nock: bridge_constants.nicks_fee_per_nock,
        blockchain_constants: blockchain_constants.clone(),
        bythos_phase: blockchain_constants.bythos_phase,
        base_fee: blockchain_constants.base_fee,
        input_fee_divisor: blockchain_constants.input_fee_divisor,
        min_fee: blockchain_constants.note_data.min_fee,
    })
}

#[cfg(test)]
mod planner_fee_tests {
    use nockapp::utils::NOCK_STACK_SIZE;
    use nockapp::NounExt;
    use nockchain_types::v1::note::NOTE_DATA_KEY_BRIDGE_WITHDRAWAL;
    use nockchain_types::v1::{LockPrimitive, Pkh};
    use nockvm::mem::NockStack;
    use nockvm::noun::Noun;
    use noun_serde::NounDecode;
    use wallet_tx_builder::fee::{compute_minimum_fee, FeeInputs};
    use wallet_tx_builder::types::PlannedOutput;
    use wallet_tx_builder::word_count::{
        estimate_seed_words, estimate_witness_words, WitnessWordInput,
    };

    use super::*;

    #[derive(Debug, Clone, PartialEq, NounDecode)]
    struct WithdrawalTxFixtureEntry {
        case: String,
        transaction: nockchain_types::v1::Transaction,
        height: u64,
        min_fee: u64,
        seed_words: u64,
        witness_words: u64,
    }

    fn decode_withdrawal_tx_fixtures() -> Vec<WithdrawalTxFixtureEntry> {
        let fixture_bytes =
            include_bytes!("../../wallet-tx-builder/tests/fixtures/withdrawal_tx_fixtures.jam");
        let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);
        let noun = Noun::cue_bytes_slice(&mut stack, fixture_bytes).expect("fixture jam must cue");
        let space = stack.noun_space();
        Vec::<WithdrawalTxFixtureEntry>::from_noun(&noun, &space).expect("fixture noun must decode")
    }

    fn withdrawal_tx_fixture(case: &str) -> WithdrawalTxFixtureEntry {
        decode_withdrawal_tx_fixtures()
            .into_iter()
            .find(|fixture| fixture.case.trim_start_matches('%') == case)
            .unwrap_or_else(|| panic!("missing fixture case: {case}"))
    }

    fn outputs_from_fixture_transaction(
        transaction: nockchain_types::v1::Transaction,
    ) -> Vec<PlannedOutput> {
        let nockchain_types::v1::Transaction::V1(tx) = transaction;
        tx.spends
            .0
            .into_iter()
            .flat_map(|(_, spend)| match spend {
                nockchain_types::tx_engine::v1::tx::Spend::Legacy(spend) => spend.seeds.0,
                nockchain_types::tx_engine::v1::tx::Spend::Witness(spend) => spend.seeds.0,
            })
            .map(|seed| PlannedOutput {
                lock_root: seed.lock_root,
                amount: seed.gift.0 as u64,
                note_data: seed
                    .note_data
                    .iter()
                    .map(|entry| RawNoteDataEntry {
                        key: entry.key.clone(),
                        blob: entry.raw_blob(),
                    })
                    .collect(),
            })
            .collect()
    }

    #[test]
    fn withdrawal_planner_config_models_multisig_refund_note_data() {
        let spend_authority = SpendCondition::new(vec![LockPrimitive::Pkh(Pkh::new(
            3,
            vec![
                Tip5Hash::from_limbs(&[1, 0, 0, 0, 0]),
                Tip5Hash::from_limbs(&[2, 0, 0, 0, 0]),
                Tip5Hash::from_limbs(&[3, 0, 0, 0, 0]),
            ],
        ))]);
        let bridge_lock_root = Lock::SpendCondition(spend_authority.clone())
            .hash()
            .expect("lock root");
        let planner = withdrawal_planner_config(
            bridge_lock_root.clone(),
            spend_authority.clone(),
            &BridgeConstants::default(),
            &BlockchainConstants::default(),
        )
        .expect("planner config");

        assert_eq!(planner.refund_lock_root, bridge_lock_root);
        assert_eq!(planner.refund_note_data.len(), 1);
        assert_eq!(planner.refund_note_data[0].key, "lock");
    }

    #[test]
    fn bridge_multisig_withdrawal_fixture_min_fee_matches_planner_with_refund_note_data() {
        let fixture = withdrawal_tx_fixture("bridge-multisig-withdrawal-with-change");
        let outputs = outputs_from_fixture_transaction(fixture.transaction.clone());
        let refund_outputs = outputs
            .iter()
            .filter(|output| {
                output
                    .note_data
                    .iter()
                    .all(|entry| entry.key != NOTE_DATA_KEY_BRIDGE_WITHDRAWAL)
            })
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            refund_outputs.len(),
            1,
            "fixture should contain exactly one refund output"
        );
        let refund_output = &refund_outputs[0];
        let nockchain_types::v1::Transaction::V1(tx) = fixture.transaction.clone();
        let nockchain_types::tx_engine::v1::tx::InputMetadata::SpendConditions(input_metadata) =
            tx.metadata.inputs
        else {
            panic!("fixture must carry spend-condition metadata");
        };
        let spend_authority = input_metadata
            .0
            .into_iter()
            .next()
            .map(|(_, spend_condition)| spend_condition)
            .expect("fixture metadata for witness spend");
        let bridge_lock_root = Lock::SpendCondition(spend_authority.clone())
            .hash()
            .expect("fixture lock root");
        let planner = withdrawal_planner_config(
            bridge_lock_root.clone(),
            spend_authority.clone(),
            &BridgeConstants::default(),
            &BlockchainConstants::default(),
        )
        .expect("planner config");

        assert_eq!(refund_output.lock_root, planner.refund_lock_root);
        assert_eq!(refund_output.note_data, planner.refund_note_data);

        let default_blockchain_constants = BlockchainConstants::default();
        let blockchain_constants = BlockchainConstants {
            bythos_phase: 10,
            base_fee: 256,
            input_fee_divisor: 4,
            note_data: nockchain_types::NoteDataConstraints {
                min_fee: 0,
                ..default_blockchain_constants.note_data
            },
            ..default_blockchain_constants
        };
        let chain_context = wallet_tx_builder::types::ChainContext {
            height: nockchain_types::tx_engine::common::BlockHeight(nockchain_math::belt::Belt(
                fixture.height,
            )),
            bythos_phase: nockchain_types::tx_engine::common::BlockHeight(
                nockchain_math::belt::Belt(blockchain_constants.bythos_phase),
            ),
            base_fee: blockchain_constants.base_fee,
            input_fee_divisor: blockchain_constants.input_fee_divisor,
            min_fee: blockchain_constants.note_data.min_fee,
        };
        let seed_words = estimate_seed_words(&outputs, &chain_context);
        let witness_words = estimate_witness_words(
            &[WitnessWordInput {
                spend_condition: spend_authority,
                input_origin_page: nockchain_types::tx_engine::common::BlockHeight(
                    nockchain_math::belt::Belt(17),
                ),
                spend_condition_count: Some(1),
            }],
            &chain_context,
        );
        let minimum_fee = compute_minimum_fee(FeeInputs {
            seed_words,
            witness_words,
            base_fee: chain_context.base_fee,
            input_fee_divisor: chain_context.input_fee_divisor,
            min_fee: chain_context.min_fee,
            height: chain_context.height,
            bythos_phase: chain_context.bythos_phase,
        })
        .minimum_fee;

        assert_eq!(
            minimum_fee, fixture.min_fee,
            "seed_words={seed_words} witness_words={witness_words}"
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KernelBlockchainConstantsBootAction {
    InitializeKernel,
    UseExistingKernel,
}

fn resolve_effective_blockchain_constants(
    kernel_blockchain_constants: Option<BlockchainConstants>,
    connected_blockchain_constants: BlockchainConstants,
) -> Result<(BlockchainConstants, KernelBlockchainConstantsBootAction), BridgeError> {
    match kernel_blockchain_constants {
        Some(kernel_blockchain_constants) => {
            validate_blockchain_constants_match(
                &kernel_blockchain_constants, &connected_blockchain_constants,
            )?;
            Ok((
                kernel_blockchain_constants,
                KernelBlockchainConstantsBootAction::UseExistingKernel,
            ))
        }
        None => Ok((
            connected_blockchain_constants,
            KernelBlockchainConstantsBootAction::InitializeKernel,
        )),
    }
}

async fn peek_kernel_blockchain_constants(
    app: &mut nockapp::NockApp<NockJammer>,
) -> Result<Option<BlockchainConstants>, BridgeError> {
    let mut path_slab = NounSlab::<NockJammer>::new();
    let path_noun = vec![BLOCKCHAIN_CONSTANTS_PATH.to_string()].encode(&mut path_slab);
    path_slab.set_root(path_noun);

    let Some(response_slab) = app.peek_handle(path_slab).await? else {
        return Ok(None);
    };

    let noun = unsafe { response_slab.root() };
    let space = response_slab.noun_space();
    BlockchainConstants::from_noun(noun, &space)
        .map(Some)
        .map_err(|err| {
            BridgeError::Runtime(format!(
                "failed to decode kernel blockchain-constants: {err}"
            ))
        })
}

async fn set_kernel_blockchain_constants(
    app: &mut nockapp::NockApp<NockJammer>,
    blockchain_constants: &BlockchainConstants,
) -> Result<(), BridgeError> {
    let mut blockchain_constants_slab = NounSlab::new();
    let blockchain_constants_cause =
        bridge::shared::types::BridgeCause::set_blockchain_constants(blockchain_constants.clone());
    let blockchain_constants_noun =
        blockchain_constants_cause.encode(&mut blockchain_constants_slab);
    blockchain_constants_slab.set_root(blockchain_constants_noun);
    let blockchain_constants_wire = nockapp::one_punch::OnePunchWire::Poke.to_wire();
    app.poke(blockchain_constants_wire, blockchain_constants_slab)
        .await
        .map_err(|e| {
            BridgeError::NockappTask(format!("Blockchain-constants poke failed: {}", e))
        })?;
    Ok(())
}

#[tokio::main]
/// Boots the bridge runtime and wires together config, chain clients, ingress,
/// and withdrawal coordination services.
async fn main() -> Result<(), BridgeError> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let cli = BridgeCli::parse();

    // Compute data_dir first (needed for log_dir default). This is the
    // bridge-local directory. Do not create it until after nockapp boot:
    // `--new` must see a fresh kernel boot directory before bridge-local logs
    // or SQLite files are materialized.
    let data_dir = bridge_data_dir(cli.data_dir.clone());
    let boot_cli = cli.boot.clone();

    // Determine log directory: CLI override or default to {data_dir}/logs/
    let log_dir = cli.log_dir.clone().unwrap_or_else(|| data_dir.join("logs"));
    let log_retention_days = cli.log_retention_days.unwrap_or(7);

    let prover_hot_state = produce_prover_hot_state();

    let mut app = boot::setup::<NockJammer>(
        KERNEL,
        boot_cli.clone(),
        prover_hot_state.as_slice(),
        "bridge",
        Some(data_dir.clone()),
    )
    .await
    .map_err(|e| BridgeError::NockappTask(format!("Kernel setup failed: {}", e)))?;

    ensure_bridge_data_dir(&data_dir).await?;

    // Initialize tracing with file logging - keep guard alive for program duration.
    // This intentionally happens after nockapp boot so fresh `--new` starts are
    // not rejected because bridge-local logs were created first.
    let _log_guard = init_bridge_tracing(&boot_cli, None, log_dir.clone(), log_retention_days)?;

    info!("Logging to directory: {}", log_dir.display());

    // Clean up old log files (best effort, don't fail startup)
    if log_retention_days > 0 {
        cleanup_old_logs(&log_dir, log_retention_days as u64);
    }

    info!("bridge nockapp started");

    let config_path = if let Some(path) = cli.config_path.clone() {
        path
    } else {
        bridge::shared::config::default_config_path()?
    };
    ensure_config_file(&config_path)?;

    let config_toml = bridge::shared::config::BridgeConfigToml::from_file(&config_path)?;
    let node_config = config_toml.to_node_config()?;

    info!("loaded config from {}", config_path.display());

    info!(
        node_id = node_config.node_id,
        node_count = node_config.nodes.len(),
        "loaded bridge node config"
    );

    let cause_builder = Arc::new(KernelCauseBuilder);
    let (mut bridge_runtime, runtime_handle) = BridgeRuntime::new(cause_builder);
    let runtime_handle = Arc::new(runtime_handle);
    bridge_runtime.install_driver(&mut app).await?;

    let (stop_controller, stop_handle) = bridge::shared::stop::StopController::new();

    if cli.start {
        let mut start_slab = NounSlab::new();
        let start_cause = bridge::shared::types::BridgeCause::start();
        let start_noun = start_cause.encode(&mut start_slab);
        start_slab.set_root(start_noun);
        let start_wire = nockapp::one_punch::OnePunchWire::Poke.to_wire();
        app.poke(start_wire, start_slab)
            .await
            .map_err(|e| BridgeError::NockappTask(format!("Start poke failed: {}", e)))?;
        info!("sent %start poke to kernel");
    }

    let mut cfg_slab = NounSlab::new();
    let cfg_cause = bridge::shared::types::BridgeCause::cfg_load(Some(node_config.clone()));
    let cfg_noun = cfg_cause.encode(&mut cfg_slab);
    cfg_slab.set_root(cfg_noun);
    let cfg_wire = nockapp::one_punch::OnePunchWire::Poke.to_wire();
    app.poke(cfg_wire, cfg_slab)
        .await
        .map_err(|e| BridgeError::NockappTask(format!("Config poke failed: {}", e)))?;

    info!("sent config to kernel");

    // Send constants to kernel
    let bridge_constants = config_toml.bridge_constants()?;
    let withdrawal_min_signers = usize::try_from(bridge_constants.min_signers)
        .map_err(|err| BridgeError::Config(format!("min_signers does not fit in usize: {err}")))?;
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
    let constants_cause =
        bridge::shared::types::BridgeCause::set_constants(bridge_constants.clone());
    let constants_noun = constants_cause.encode(&mut constants_slab);
    constants_slab.set_root(constants_noun);
    let constants_wire = nockapp::one_punch::OnePunchWire::Poke.to_wire();
    app.poke(constants_wire, constants_slab)
        .await
        .map_err(|e| BridgeError::NockappTask(format!("Constants poke failed: {}", e)))?;

    info!("sent constants to kernel");

    let base_confirmation_depth = config_toml.base_confirmation_depth;
    let nockchain_confirmation_depth = config_toml.nockchain_confirmation_depth;

    let nonce_epoch = build_deposit_nonce_epoch_config(&config_toml)?;
    let withdrawal_activation_cutoff = config_toml.withdrawal_activation_cutoff()?;
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

    let withdrawal_projection_store = Arc::new(
        WithdrawalProjectionStore::open(data_dir.join("withdrawal-local-state.sqlite")).await?,
    );
    let connected_blockchain_constants =
        bootstrap_blockchain_constants(config_toml.grpc_address()).await?;
    let existing_kernel_blockchain_constants = peek_kernel_blockchain_constants(&mut app).await?;
    let (effective_blockchain_constants, boot_action) = resolve_effective_blockchain_constants(
        existing_kernel_blockchain_constants, connected_blockchain_constants,
    )?;
    if boot_action == KernelBlockchainConstantsBootAction::InitializeKernel {
        set_kernel_blockchain_constants(&mut app, &effective_blockchain_constants).await?;
        info!(
            target: "bridge.withdrawal",
            "initialized bridge kernel blockchain-constants from connected private nockchain node"
        );
    } else {
        info!(
            target: "bridge.withdrawal",
            "confirmed bridge kernel blockchain-constants match the connected private nockchain node"
        );
    }
    let (active_bridge_lock_root, spend_authority_spend_condition) =
        resolve_validated_bridge_spend_authority(
            &node_config, &bridge_constants, &effective_blockchain_constants,
        )?;
    let withdrawal_planner = withdrawal_planner_config(
        active_bridge_lock_root.clone(),
        spend_authority_spend_condition,
        &bridge_constants,
        &effective_blockchain_constants,
    )?;
    let withdrawal_registry = Arc::new(WithdrawalProposalRegistry::new(
        withdrawal_projection_store,
        WithdrawalTransactionBodyValidator::new(active_bridge_lock_root.clone()),
    ));

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

    let confirmed_snapshot_service = Arc::new(
        BridgeNoteSnapshotService::new_private(
            config_toml.grpc_address().to_string(),
            bridge_owned_note_selectors(&active_bridge_lock_root)?,
            Duration::from_secs(30),
        )
        .with_nockchain_confirmation_depth(nockchain_confirmation_depth),
    );
    let withdrawal_node_pkhs: Vec<_> = node_config
        .nodes
        .iter()
        .map(|node| node.nock_pkh.clone())
        .collect();
    let withdrawal_node_eth_addresses = extract_bridge_node_eth_addresses(&node_config);
    let withdrawal_sequencer_client = Arc::new(GrpcWithdrawalSequencerClient::new(
        config_toml.nockchain_sequencer_api_address()?,
    ));

    // Create peers, health state, and bridge status BEFORE ingress spawn
    // so ingress can update state when receiving peer broadcasts.
    let peers = derive_peer_endpoints(&node_config, node_config.node_id);
    let health_state = initialize_health_state(&peers);

    // Create BridgeStatus for drivers to update proposal state.
    let bridge_status = BridgeStatus::new(health_state.clone());
    let status_state = BridgeStatusState::new();

    let withdrawal_transport = Arc::new(
        WithdrawalProposalTransport::new(
            node_config.node_id,
            withdrawal_node_pkhs.clone(),
            withdrawal_node_eth_addresses.clone(),
            withdrawal_min_signers,
            bridge_signer.clone(),
            withdrawal_registry.clone(),
            WithdrawalFallbackPolicy::default(),
        )
        .with_sequencer(withdrawal_sequencer_client.clone())
        .with_bridge_status(bridge_status.clone()),
    );

    // Per-node deposit log for deterministic nonce assignment.
    let deposit_log_path = data_dir.join("deposit-queue.sqlite");
    let deposit_log =
        Arc::new(bridge::deposit::log::DepositLog::open(deposit_log_path.clone()).await?);
    info!("Using deposit queue log at {}", deposit_log_path.display());
    bridge::observability::metrics::update_deposit_log_max_nonce(
        deposit_log.max_nonce_in_epoch(&nonce_epoch).await?,
    );

    // Deposit cursor restore happens after the kernel action loop is running.

    let nock_watcher = NockchainWatcher::with_policy(
        config_toml.grpc_address().to_string(),
        runtime_handle.clone(),
        nockchain_confirmation_depth,
        stop_handle.clone(),
        nock_observer_loop_policy_from_env()?,
    )
    .with_bridge_status(bridge_status.clone())
    .with_confirmed_snapshot_service(confirmed_snapshot_service.clone());
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
    let ingress_stop_handle = stop_handle.clone();
    let ingress_peers = peers.clone();
    let ingress_status_state = status_state.clone();
    let ingress_deposit_log = deposit_log.clone();
    let ingress_nonce_epoch = nonce_epoch.clone();
    let ingress_withdrawal_transport = Some(withdrawal_transport.clone());
    let ingress_withdrawal_tui_source = Some(WithdrawalTuiSource {
        registry: withdrawal_registry.clone(),
        sequencer: Some(withdrawal_sequencer_client.clone()),
        activation_cutoff: withdrawal_activation_cutoff,
        local_node_id: node_config.node_id,
        node_pkhs: withdrawal_node_pkhs.clone(),
    });
    let ingress_handle = tokio::spawn(async move {
        ingress::serve_ingress(
            ingress_addr, node_id, ingress_runtime, ingress_status_state, ingress_deposit_log,
            ingress_nonce_epoch, ingress_signer, ingress_cache, ingress_tui, ingress_addr_map,
            ingress_stop_controller, ingress_stop_handle, ingress_peers,
            ingress_withdrawal_transport, ingress_withdrawal_tui_source,
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

    let withdrawal_execution_driver = create_withdrawal_execution_driver(
        WithdrawalExecutionDriverContext {
            runtime: runtime_handle.clone(),
            stop_controller: stop_controller.clone(),
            bridge_status: bridge_status.clone(),
            stop: stop_handle.clone(),
            proposal_registry: withdrawal_registry.clone(),
            withdrawal_transport: withdrawal_transport.clone(),
            peers: peers.clone(),
            activation_cutoff: withdrawal_activation_cutoff,
        },
        node_config.node_id,
    );
    app.add_io_driver(withdrawal_execution_driver).await;

    let app_handle = tokio::spawn(async move { app.run().await });
    let runtime_task = spawn_kernel_runtime(bridge_runtime);

    info!(
        target: "bridge.withdrawal",
        "restoring tracked withdrawal requests from durable state"
    );
    let restored_withdrawals = bootstrap_withdrawal_runtime(
        runtime_handle.as_ref(),
        withdrawal_registry.as_ref(),
        withdrawal_activation_cutoff,
    )
    .await?;
    info!(
        target: "bridge.withdrawal",
        restored_withdrawals,
        "restored tracked withdrawal requests from durable state"
    );

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

        let info = bridge::shared::stop::StopInfo {
            reason: "kernel stop-state present on boot".to_string(),
            last,
            source: bridge::shared::stop::StopSource::KernelEffect,
            at: std::time::SystemTime::now(),
        };
        let _ = stop_seed_controller.trigger(info);
    });

    let deposit_runtime = DepositRuntimeContext {
        runtime: runtime_handle.clone(),
        base_bridge: base_bridge.clone(),
        deposit_log: deposit_log.clone(),
        nonce_epoch: nonce_epoch.clone(),
        proposal_cache: proposal_cache.clone(),
        signer: bridge_signer.clone(),
        valid_addresses: extract_valid_bridge_addresses(&node_config),
        peers: peers.clone(),
        self_node_id: node_config.node_id,
        bridge_status: bridge_status.clone(),
        address_to_node_id: address_to_node_id.clone(),
        stop_controller: stop_controller.clone(),
        stop: stop_handle.clone(),
        status_state: status_state.clone(),
        node_config: node_config.clone(),
    };
    bootstrap_deposit_runtime(&deposit_runtime).await?;
    let _deposit_runtime_handles = spawn_deposit_runtime_loops(deposit_runtime);
    info!("Spawned deposit runtime loops");

    let local_signer_pkh = node_config
        .nodes
        .get(node_config.node_id as usize)
        .ok_or_else(|| {
            BridgeError::Config(format!(
                "node_id {} missing from nodes list for withdrawal signer pkh",
                node_config.node_id
            ))
        })?
        .nock_pkh
        .clone();
    let withdrawal_assembly_context = WithdrawalAssemblyContext {
        kernel: runtime_handle.clone(),
        snapshot_service: confirmed_snapshot_service.clone(),
        sequencer: withdrawal_sequencer_client.clone(),
        proposal_registry: withdrawal_registry.clone(),
        bridge_status: bridge_status.clone(),
        planner: withdrawal_planner,
        fallback_policy: WithdrawalFallbackPolicy::default(),
        local_node_id: node_config.node_id,
        node_pkhs: withdrawal_node_pkhs.clone(),
    };
    let withdrawal_signing_context = WithdrawalSigningContext {
        kernel: runtime_handle.clone(),
        sequencer: withdrawal_sequencer_client.clone(),
        proposal_registry: withdrawal_registry.clone(),
        local_node_id: node_config.node_id,
        local_signer_pkh,
        node_eth_addresses: withdrawal_node_eth_addresses,
        fatal_stop: Some(WithdrawalFatalStopContext {
            runtime: runtime_handle.clone(),
            stop_controller: stop_controller.clone(),
            bridge_status: bridge_status.clone(),
        }),
    };
    let withdrawal_submission_context = WithdrawalSubmissionContext {
        sequencer: withdrawal_sequencer_client,
        proposal_registry: withdrawal_registry.clone(),
        bridge_status: bridge_status.clone(),
        fallback_policy: WithdrawalFallbackPolicy::default(),
        local_node_id: node_config.node_id,
        node_pkhs: withdrawal_node_pkhs.clone(),
    };
    let withdrawal_runtime = WithdrawalRuntimeContext {
        assembly: withdrawal_assembly_context,
        signing: withdrawal_signing_context,
        submission: withdrawal_submission_context,
        activation_cutoff: withdrawal_activation_cutoff,
        stop: stop_handle.clone(),
        assembly_policy: Default::default(),
        signing_policy: Default::default(),
        submission_policy: Default::default(),
    };
    let _withdrawal_runtime_handles = spawn_withdrawal_runtime_loops(withdrawal_runtime);
    info!("Spawned withdrawal runtime loops");

    let ack_handle = spawn_base_observer(base_bridge.clone(), bridge_status.clone());

    let health_cfg = HealthMonitorConfig {
        self_node_id: node_config.node_id,
        self_address,
        peers: peers.clone(),
        poll_interval: Duration::from_secs(5),
        request_timeout: Duration::from_secs(2),
        bridge_status: Some(bridge_status.clone()),
    };
    let monitor_state = health_state.clone();
    let health_handle = tokio::spawn(async move {
        bridge::observability::health::run_health_monitor(health_cfg, monitor_state).await
    });

    // Network monitor: polls chain heights and updates bridge status
    let network_runtime = runtime_handle.clone();
    let network_bridge_status = bridge_status.clone();
    let _network_handle = tokio::spawn(async move {
        bridge::shared::nockchain::run_network_monitor(
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
    #![allow(clippy::unwrap_used)]

    use std::sync::Once;

    use bridge::deposit::log::persist_commit_nock_deposits_requests;
    use bridge::observability::tui;
    use bridge::shared::config::{BridgeConfigToml, NodeInfoToml};
    use bridge::shared::types::{EthAddress, Tip5Hash};
    use nockapp::kernel::boot;
    use nockchain_math::belt::Belt;
    use nockchain_types::default_fakenet_blockchain_constants;
    use nockchain_types::v1::Name;
    use nockvm::noun::NounAllocator;
    use tempfile::TempDir;

    use super::*;

    static INIT: Once = Once::new();
    const VALID_START_TX_ID: &str = "2uYre9HXRP8X6BD7w3GvgfUAU47RSmZDGkz9uJgJmD9CxN7JA69k6MF";

    #[test]
    fn bridge_data_dir_resolution_does_not_create_directory() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("bridge-local");

        assert_eq!(bridge_data_dir(Some(path.clone())), path);
        assert!(
            !path.exists(),
            "bridge-local data dir must not be created before nockapp boot"
        );
    }

    fn base_config() -> BridgeConfigToml {
        BridgeConfigToml {
            node_id: 0,
            base_ws_url: "wss://example.invalid".to_string(),
            bridge_lock_root: DEFAULT_BRIDGE_WITHDRAWAL_LOCK_ROOT_B58.to_string(),
            inbox_contract_address: None,
            nock_contract_address: None,
            my_eth_key: "0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318"
                .to_string(),
            my_nock_key: "5KZuFKrctV5iUburT54Z9fhpf3V3hv2sPf9GRQnjFR8T".to_string(),
            grpc_address: "http://localhost:5555".to_string(),
            nockchain_sequencer_api_address: None,
            base_confirmation_depth: 1,
            nockchain_confirmation_depth: 1,
            deposit_nonce_epoch_base: None,
            deposit_nonce_epoch_start_height: None,
            deposit_nonce_epoch_start_tx_id_base58: None,
            withdrawal_activation_nock_next_height: Some(200),
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
            let cli = boot::ephemeral_test_boot_cli(true);
            let temp_log_dir = std::env::temp_dir().join("bridge-test-logs");
            let _guard = init_bridge_tracing(&cli, Some(tui::new_log_buffer()), temp_log_dir, 7)
                .expect("failed to init tracing for tests");
            // Note: guard is dropped here but that's OK for tests - we just need tracing initialized
            // In production, the guard is kept alive in main()
        });
    }

    #[test]
    fn deposit_nonce_epoch_config_allows_missing_base_and_start() {
        let cfg = base_config();
        let epoch = build_deposit_nonce_epoch_config(&cfg).expect("base omitted should be ok");
        assert_eq!(epoch.base, 0);
        assert_eq!(epoch.start_height, 1);
        assert!(epoch.start_tx_id.is_none());
    }

    #[test]
    fn nock_observer_poll_interval_env_parses_optional_millis() {
        assert_eq!(
            parse_optional_duration_millis_env_value(NOCK_OBSERVER_POLL_MILLIS_ENV, None).unwrap(),
            None
        );
        assert_eq!(
            parse_optional_duration_millis_env_value(NOCK_OBSERVER_POLL_MILLIS_ENV, Some("250"))
                .unwrap(),
            Some(Duration::from_millis(250))
        );
        assert!(
            parse_optional_duration_millis_env_value(NOCK_OBSERVER_POLL_MILLIS_ENV, Some("0"))
                .is_err()
        );
        assert!(parse_optional_duration_millis_env_value(
            NOCK_OBSERVER_POLL_MILLIS_ENV,
            Some("bad")
        )
        .is_err());
    }

    #[test]
    fn withdrawal_activation_cutoff_requires_nock_height() {
        let mut cfg = base_config();
        cfg.withdrawal_activation_nock_next_height = None;
        assert!(cfg.withdrawal_activation_cutoff().is_err());
    }

    #[test]
    fn withdrawal_activation_cutoff_accepts_nock_next_height() {
        let cfg = base_config();
        let cutoff = cfg.withdrawal_activation_cutoff().unwrap();
        assert_eq!(cutoff.nock_next_height, 200);
    }

    #[test]
    fn deposit_nonce_epoch_config_rejects_start_without_base() {
        let mut cfg = base_config();
        cfg.deposit_nonce_epoch_start_height = Some(10);
        assert!(build_deposit_nonce_epoch_config(&cfg).is_err());

        let mut cfg = base_config();
        cfg.deposit_nonce_epoch_start_tx_id_base58 = Some(VALID_START_TX_ID.to_string());
        assert!(build_deposit_nonce_epoch_config(&cfg).is_err());
    }

    #[test]
    fn bridge_owned_note_selectors_include_bridge_multisig_first_name() {
        let bridge_lock_root = canonical_mainnet_bridge_lock_root().expect("mainnet bridge root");
        let selectors = bridge_owned_note_selectors(&bridge_lock_root).expect("selectors");

        assert_eq!(
            selectors.first_names,
            vec![bridge_multisig_first_name(&bridge_lock_root).expect("bridge first-name")]
        );
    }

    #[test]
    fn resolve_effective_blockchain_constants_initializes_kernel_when_missing() {
        let connected = default_fakenet_blockchain_constants();

        let (effective, action) = resolve_effective_blockchain_constants(None, connected.clone())
            .expect("missing kernel constants should initialize");

        assert_eq!(effective, connected);
        assert_eq!(
            action,
            KernelBlockchainConstantsBootAction::InitializeKernel
        );
    }

    #[test]
    fn resolve_effective_blockchain_constants_uses_matching_kernel_value() {
        let kernel = default_fakenet_blockchain_constants();
        let connected = kernel.clone();

        let (effective, action) =
            resolve_effective_blockchain_constants(Some(kernel.clone()), connected)
                .expect("matching kernel constants should be accepted");

        assert_eq!(effective, kernel);
        assert_eq!(
            action,
            KernelBlockchainConstantsBootAction::UseExistingKernel
        );
    }

    #[test]
    fn resolve_effective_blockchain_constants_rejects_mismatch() {
        let kernel = default_fakenet_blockchain_constants();
        let mut connected = kernel.clone();
        connected.coinbase_timelock_min += 1;

        let err = resolve_effective_blockchain_constants(Some(kernel), connected)
            .expect_err("mismatched kernel constants should fail");
        assert!(err.to_string().contains("bridge kernel state"));
    }

    #[test]
    fn deposit_nonce_epoch_config_allows_zero_base_without_anchor() {
        let mut cfg = base_config();
        cfg.deposit_nonce_epoch_base = Some(0);
        let epoch = build_deposit_nonce_epoch_config(&cfg).expect("base=0 should be ok");
        assert_eq!(epoch.base, 0);
        assert_eq!(epoch.start_height, 1);
        assert!(epoch.start_tx_id.is_none());
    }

    #[test]
    fn deposit_nonce_epoch_config_rejects_zero_base_with_anchor() {
        let mut cfg = base_config();
        cfg.deposit_nonce_epoch_base = Some(0);
        cfg.deposit_nonce_epoch_start_height = Some(10);
        assert!(build_deposit_nonce_epoch_config(&cfg).is_err());
    }

    #[test]
    fn deposit_nonce_epoch_config_rejects_nonzero_base_with_zero_height() {
        let mut cfg = base_config();
        cfg.deposit_nonce_epoch_base = Some(5);
        cfg.deposit_nonce_epoch_start_height = Some(0);
        cfg.deposit_nonce_epoch_start_tx_id_base58 = Some(VALID_START_TX_ID.to_string());
        assert!(build_deposit_nonce_epoch_config(&cfg).is_err());
    }

    #[test]
    fn deposit_nonce_epoch_config_rejects_missing_anchor_with_nonzero_base() {
        let mut cfg = base_config();
        cfg.deposit_nonce_epoch_base = Some(5);
        assert!(build_deposit_nonce_epoch_config(&cfg).is_err());

        let mut cfg = base_config();
        cfg.deposit_nonce_epoch_base = Some(5);
        cfg.deposit_nonce_epoch_start_height = Some(10);
        assert!(build_deposit_nonce_epoch_config(&cfg).is_err());
    }

    #[test]
    fn deposit_nonce_epoch_config_rejects_tx_id_without_height() {
        let mut cfg = base_config();
        cfg.deposit_nonce_epoch_base = Some(5);
        cfg.deposit_nonce_epoch_start_tx_id_base58 = Some(VALID_START_TX_ID.to_string());
        assert!(build_deposit_nonce_epoch_config(&cfg).is_err());
    }

    #[test]
    fn deposit_nonce_epoch_config_accepts_anchor_for_nonzero_base() {
        let mut cfg = base_config();
        cfg.deposit_nonce_epoch_base = Some(5);
        cfg.deposit_nonce_epoch_start_height = Some(10);
        cfg.deposit_nonce_epoch_start_tx_id_base58 = Some(VALID_START_TX_ID.to_string());
        let epoch = build_deposit_nonce_epoch_config(&cfg).expect("anchor should be accepted");
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
            let space = slab.noun_space();
            ia.normalize_as_atom(&space)
        };
        let space = slab.noun_space();
        let signature = bridge_signer.sign_proposal(noun.as_noun(), &space).await?;

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
        let log = bridge::deposit::log::DepositLog::open(path).await?;
        let epoch = NonceEpochConfig {
            base: 100,
            start_height: 10,
            start_tx_id: None,
        };

        let req_pre = bridge::deposit::types::NockDepositRequestKernelData {
            block_height: 9,
            tx_id: tip5(1, 0, 0, 0, 0),
            as_of: tip5(9, 9, 9, 9, 9),
            name: Name::new(tip5(10, 0, 0, 0, 0), tip5(11, 0, 0, 0, 0)),
            recipient: addr(0x11),
            amount: 1,
        };
        let req_a = bridge::deposit::types::NockDepositRequestKernelData {
            block_height: 10,
            tx_id: tip5(2, 0, 0, 0, 0),
            as_of: tip5(8, 8, 8, 8, 8),
            name: Name::new(tip5(12, 0, 0, 0, 0), tip5(13, 0, 0, 0, 0)),
            recipient: addr(0x22),
            amount: 2,
        };
        let req_b = bridge::deposit::types::NockDepositRequestKernelData {
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
        let log = bridge::deposit::log::DepositLog::open(path).await?;
        let epoch = NonceEpochConfig {
            base: 10,
            start_height: 1,
            start_tx_id: None,
        };

        let req_low = bridge::deposit::types::NockDepositRequestKernelData {
            block_height: 5,
            tx_id: tip5(1, 0, 0, 0, 0),
            as_of: tip5(1, 1, 1, 1, 1),
            name: Name::new(tip5(2, 0, 0, 0, 0), tip5(3, 0, 0, 0, 0)),
            recipient: addr(0x44),
            amount: 4,
        };
        let req_high = bridge::deposit::types::NockDepositRequestKernelData {
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
