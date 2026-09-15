use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, B256, U256};
use async_trait::async_trait;
use bridge::observability::tui_api::proto as tui_proto;
use bridge::observability::tui_api::proto::bridge_tui_client::BridgeTuiClient;
use bridge::shared::base::burn_for_withdrawal_signature_hash;
use bridge::shared::config::{derive_bridge_spend_authority_from_pkhs, BridgeConfigToml};
use bridge::shared::ingress::proto as ingress_proto;
use bridge::shared::types::{WithdrawalPolicy, WITHDRAWAL_POLICY_V1_BASE_UNITS_PER_NICK};
use bridge::withdrawal::sequencer::client::GrpcWithdrawalSequencerClient;
use bridge::withdrawal::snapshot::{
    BridgeNoteSnapshotService, BridgeOwnedNoteSelectors, ConfirmedBridgeNoteSnapshot,
};
use bridge::withdrawal::submission::WithdrawalSequencerPort;
use bridge::withdrawal::transport::withdrawal_id_from_proto;
use bridge::withdrawal::types::{WithdrawalId, WithdrawalSequencerProposalArtifacts};
use nockchain_types::common::Hash as NockHash;
use nockchain_types::tx_engine::common::{FirstName, Version};
use nockchain_types::tx_engine::v1::tx::{Lock, LockPrimitive, Pkh, SpendCondition};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::time::{sleep, Instant};
use tonic::Request;

use crate::anvil::{AnvilBackend, AnvilConfig};
use crate::anvil_fork::{PinnedAnvilFork, PinnedForkConfig};
use crate::browser_driver::{
    run_browser_driver, run_browser_failure_driver, run_browser_recovery_matrix_driver,
    terminal_proof_sha256, verify_browser_backend_parity, write_browser_manifest_new,
    BrowserBackendEvidenceV1, BrowserDriverLaunch, BrowserDriverManifestV2, BrowserDriverMode,
    BrowserWithdrawalResultV2, BROWSER_DRIVER_CHAIN_ID, BROWSER_DRIVER_SCHEMA_VERSION,
};
use crate::client_driver::{
    SelectedWithdrawalClient, WithdrawalClientDriver, WithdrawalClientMode, WithdrawalClientRequest,
};
use crate::cluster_config::{
    deterministic_cluster_nodes, BRIDGE_DEV_IRIS_SDK_VERSION_ENV, BRIDGE_ETH_KEYS,
    BRIDGE_NOCK_KEYS, BRIDGE_NOCK_PKHS,
};
use crate::e2e::{
    normalize_core_withdrawal_evidence, CoreWithdrawalEvidence, CoreWithdrawalPhase,
    CoreWithdrawalProgress, E2eBaseMode, E2eClientMode, E2eRunContext, E2eScenarioExecutor,
    NormalizedCoreWithdrawalFacts, ScenarioExecution,
};
use crate::environment::BaseE2eEnvironment;
use crate::evidence::{
    EvidenceArtifacts, EvidenceAssertion, EvidenceCollector, EvidenceDeploymentFacts,
    EvidenceEnvironmentFacts, EvidenceEnvironmentMode, EvidenceKernelFacts, EvidenceNockchainFacts,
    EvidenceRunFacts, EvidenceRunStatus, EvidenceSequencerFacts, EvidenceStep,
    EvidenceTerminalFacts, ExternalArtifactReference, RedactionDeclaration,
    WithdrawalEvidenceCapsuleV1,
};
use crate::fork_seeder::{
    ForkBalanceSeedRequest, ForkBalanceSeeder, ForkContractState, ForkSeeder,
};
use crate::hermetic_deploy::{HermeticDeployConfig, HermeticDeployment};
use crate::iris_artifact::{IrisArtifact, IrisArtifactResolver};
use crate::iris_driver::{observe_withdrawal_burn, submit_withdrawal_burn, BurnSubmissionProof};
use crate::nockchain_probe::{
    wait_for_nockchain_transaction, LiveNockchainProbeSource, NockchainInputSnapshotFacts,
    NockchainProbeRequest, NockchainTransactionFacts, NoteNameFacts, SelectedInputNoteFacts,
};
use crate::redaction::{SecretRedactor, SecretValue};
use crate::scenario::{
    core_withdrawal_amount_nicks, core_withdrawal_amount_nocks, LocalBaseEnvironment,
    ScenarioHarness,
};
use crate::settlement_oracle::{
    wait_for_terminal_withdrawal, BridgeKernelTerminalFacts, KernelFrontierFacts,
    PublicWithdrawalState, PublicWithdrawalTerminalFacts, ReservationTerminalFacts,
    SequencerTerminalFacts, SequencerTerminalState, SettlementConservationProof, SettlementOracle,
    TerminalChainSource, TerminalKernelSource, TerminalOracleSources, TerminalPublicSource,
    TerminalReservationSource, TerminalSequencerSource, TerminalWithdrawalProof,
    TerminalWithdrawalTarget, TimedTerminalFact,
};

const ANVIL_ACCOUNT: &str = "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266";
const ANVIL_PRIVATE_KEY: &str =
    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const DESTINATION_V1_PKH: &str = "9phXGACnW4238oqgvn2gpwaUjG3RAqcxq2Ash2vaKp8KjzSd3MQ56Jt";
const BASE_ENVIRONMENT_MANIFEST: &str = "crates/bridge/e2e/environments/base-sepolia.json";
const RECEIPT_TIMEOUT: Duration = Duration::from_secs(60);
const CLUSTER_PROGRESS_TIMEOUT: Duration = Duration::from_secs(720);
const NOCKCHAIN_PROBE_TIMEOUT: Duration = Duration::from_secs(360);
const BROWSER_TERMINAL_PROOF_SCHEMA_VERSION: u64 = 1;

#[derive(Debug, Clone)]
pub struct LiveE2eOptions {
    pub bridge_dev_binary: PathBuf,
    pub nockswap_checkout: Option<PathBuf>,
    pub browser_timeout: Duration,
}

pub struct LiveE2eExecutor {
    options: LiveE2eOptions,
    base: Option<ProvisionedBase>,
    deployment: Option<ProvisionedDeployment>,
    scenario: Option<ScenarioHarness>,
    initial_snapshot: Option<ConfirmedBridgeNoteSnapshot>,
    iris: Option<IrisArtifact>,
}

impl LiveE2eExecutor {
    pub fn new(options: LiveE2eOptions) -> Self {
        Self {
            options,
            base: None,
            deployment: None,
            scenario: None,
            initial_snapshot: None,
            iris: None,
        }
    }

    fn base(&self) -> Result<&AnvilBackend, String> {
        self.base
            .as_ref()
            .map(ProvisionedBase::backend)
            .ok_or_else(|| "live E2E Base backend is not provisioned".to_owned())
    }

    fn deployment(&self) -> Result<&ProvisionedDeployment, String> {
        self.deployment
            .as_ref()
            .ok_or_else(|| "live E2E deployment is not provisioned".to_owned())
    }

    fn scenario(&self) -> Result<&ScenarioHarness, String> {
        self.scenario
            .as_ref()
            .ok_or_else(|| "live E2E cluster is not provisioned".to_owned())
    }
}

#[async_trait]
impl E2eScenarioExecutor for LiveE2eExecutor {
    async fn provision(&mut self, context: &E2eRunContext) -> Result<(), String> {
        let environment =
            BaseE2eEnvironment::from_path(context.workspace_root.join(BASE_ENVIRONMENT_MANIFEST))
                .map_err(|error| error.to_string())?;
        let holder = Address::from_str(ANVIL_ACCOUNT).map_err(|error| error.to_string())?;
        let deterministic_nodes = deterministic_cluster_nodes();
        let deterministic_signers = deterministic_nodes.clone().map(|node| {
            Address::from_str(&node.eth_address).expect("checked deterministic signer")
        });
        let required_nicks = core_withdrawal_amount_nicks(&WithdrawalPolicy::v1(), 1)
            .map_err(|error| error.to_string())?;
        let balance_request = ForkBalanceSeedRequest {
            holder,
            required_nicks,
            headroom_nicks: required_nicks,
            gas_accounts: deterministic_signers.to_vec(),
            gas_balance_wei: U256::from_str("1000000000000000000000")
                .map_err(|error| error.to_string())?,
        };

        let (base, deployment) = match context.base {
            E2eBaseMode::Hermetic => {
                let backend = AnvilBackend::start(AnvilConfig::empty(), &environment)
                    .await
                    .map_err(|error| error.to_string())?;
                let mut deploy_config =
                    HermeticDeployConfig::discover(&context.workspace_root, deterministic_signers);
                deploy_config.deployment_dir = context.run_dir.join("base-deployments");
                let deployed = HermeticDeployment::deploy(&backend, deploy_config)
                    .await
                    .map_err(|error| error.to_string())?;
                let state = deployed.facts().bridge_state.clone();
                ForkBalanceSeeder::seed(&backend, &state, balance_request)
                    .await
                    .map_err(|error| error.to_string())?;
                let start_height = backend
                    .block_number()
                    .await
                    .map_err(|error| error.to_string())?;
                let runtime_code_hashes = deployed
                    .facts()
                    .runtime_artifacts
                    .iter()
                    .map(|artifact| {
                        (
                            artifact.contract_name.clone(),
                            format!("{:#x}", artifact.runtime_keccak256),
                        )
                    })
                    .collect();
                let facts = ProvisionedDeployment {
                    environment_id: deployed.facts().environment_id.clone(),
                    inbox: deployed.facts().addresses.message_inbox_proxy,
                    nock: deployed.facts().addresses.nock,
                    runtime_code_hashes,
                    state,
                    start_height,
                };
                (ProvisionedBase::Hermetic(backend), facts)
            }
            E2eBaseMode::BaseSepoliaFork => {
                let source_rpc_url = context
                    .archive_rpc_url
                    .clone()
                    .ok_or_else(|| "fork E2E is missing its archive RPC URL".to_owned())?;
                let fork =
                    PinnedAnvilFork::start(PinnedForkConfig::new(source_rpc_url), &environment)
                        .await
                        .map_err(|error| error.to_string())?;
                let runtime_code_hashes = BTreeMap::from([
                    (
                        "ERC1967Proxy".to_owned(),
                        fork.evidence()
                            .pristine
                            .message_inbox_proxy
                            .keccak256
                            .clone(),
                    ),
                    (
                        "MessageInbox".to_owned(),
                        fork.evidence()
                            .pristine
                            .message_inbox_implementation
                            .keccak256
                            .clone(),
                    ),
                    (
                        "Nock".to_owned(),
                        fork.evidence().pristine.nock.keccak256.clone(),
                    ),
                ]);
                let seed = ForkSeeder::seed(fork.backend(), fork.pristine(), deterministic_signers)
                    .await
                    .map_err(|error| error.to_string())?;
                ForkBalanceSeeder::seed(fork.backend(), &seed.after, balance_request)
                    .await
                    .map_err(|error| error.to_string())?;
                let start_height = fork
                    .backend()
                    .block_number()
                    .await
                    .map_err(|error| error.to_string())?;
                let facts = ProvisionedDeployment {
                    environment_id: "base-sepolia-fork".to_owned(),
                    inbox: Address::from_str(&seed.after.nock_inbox)
                        .map_err(|error| error.to_string())?,
                    nock: Address::from_str(&seed.after.message_inbox_nock)
                        .map_err(|error| error.to_string())?,
                    runtime_code_hashes,
                    state: seed.after,
                    start_height,
                };
                (ProvisionedBase::Fork(fork), facts)
            }
        };
        self.base = Some(base);
        self.deployment = Some(deployment.clone());
        // The sequencer verifies only confirmed Base blocks. Mine one empty block
        // beyond the configured start so an empty deployment can establish its
        // initial activity cursor before the first withdrawal burn.
        self.base()?
            .mine(1)
            .await
            .map_err(|error| error.to_string())?;

        if context.client == E2eClientMode::Iris {
            let input = context.iris_artifact.clone().ok_or_else(|| {
                "Iris client selected without immutable artifact input".to_owned()
            })?;
            self.iris = Some(
                IrisArtifactResolver::default()
                    .resolve(input, &context.run_dir)
                    .await
                    .map_err(|error| error.to_string())?,
            );
        }

        let mut scenario = ScenarioHarness::for_e2e_run(
            &context.run_id,
            context.workspace_root.clone(),
            self.options.bridge_dev_binary.clone(),
            &context.run_dir,
        )
        .map_err(|error| error.to_string())?;
        scenario.extend_env_overrides(context.artifacts.environment_overrides());
        scenario.extend_env_overrides([
            (
                "TENDERLY_PUBLIC_ADDRESS".to_owned(),
                ANVIL_ACCOUNT.to_owned(),
            ),
            (
                "TENDERLY_TEST_PRIVATE_KEY".to_owned(),
                ANVIL_PRIVATE_KEY.to_owned(),
            ),
            (
                "BRIDGE_DEV_WITHDRAWAL_ACTIVATION_NOCK_NEXT_HEIGHT".to_owned(),
                "1".to_owned(),
            ),
        ]);
        if let Some(iris) = &self.iris {
            scenario.extend_env_overrides([(
                BRIDGE_DEV_IRIS_SDK_VERSION_ENV.to_owned(),
                iris.facts.package_version.clone(),
            )]);
        }
        scenario
            .write_local_base_environment(&LocalBaseEnvironment {
                http_url: self.base()?.http_url().as_url().to_string(),
                ws_url: self.base()?.ws_url().to_owned(),
                chain_id: BROWSER_DRIVER_CHAIN_ID,
                start_height: deployment.start_height,
                inbox_contract: format!("{:#x}", deployment.inbox),
                nock_contract: format!("{:#x}", deployment.nock),
            })
            .map_err(|error| error.to_string())?;
        scenario
            .spawn_local_cluster()
            .map_err(|error| error.to_string())?;
        let liquidity_nicks = required_nicks
            .checked_mul(2)
            .ok_or_else(|| "bridge liquidity amount overflow".to_owned())?;
        scenario
            .complete_deposit_on_all_nodes_with_amount_after(&liquidity_nicks.to_string(), None)
            .map_err(|error| error.to_string())?;
        scenario
            .assert_no_stop_conditions_in_logs()
            .map_err(|error| error.to_string())?;

        let bridge_lock_root = bridge_lock_root()?;
        let first_name = FirstName::from_lock_root(&bridge_lock_root)
            .map_err(|error| error.to_string())?
            .into_hash()
            .to_base58();
        let snapshot_service = BridgeNoteSnapshotService::new_private(
            scenario
                .private_nockchain_endpoint()
                .map_err(|error| error.to_string())?,
            BridgeOwnedNoteSelectors {
                first_names: vec![first_name],
            },
            Duration::ZERO,
        )
        .with_nockchain_confirmation_depth(1);
        self.initial_snapshot =
            Some(wait_for_snapshot(&snapshot_service, Duration::from_secs(120)).await?);
        self.scenario = Some(scenario);
        Ok(())
    }

    async fn execute(&mut self, context: &E2eRunContext) -> Result<ScenarioExecution, String> {
        let deployment = self.deployment()?.clone();
        let holder = Address::from_str(ANVIL_ACCOUNT).map_err(|error| error.to_string())?;
        let destination_lock_root = destination_lock_root(DESTINATION_V1_PKH)?;
        let amount_nocks = core_withdrawal_amount_nocks(&WithdrawalPolicy::v1(), 1)
            .map_err(|error| error.to_string())?;
        let amount_nicks = amount_nocks
            .checked_mul(WithdrawalPolicy::v1().nicks_per_nock)
            .ok_or_else(|| "withdrawal nick amount overflow".to_owned())?;
        let amount_base_units = U256::from(amount_nicks)
            .checked_mul(U256::from(WITHDRAWAL_POLICY_V1_BASE_UNITS_PER_NICK))
            .ok_or_else(|| "withdrawal Base-unit amount overflow".to_owned())?;
        let request = WithdrawalClientRequest {
            nock_token: deployment.nock,
            burner: holder,
            amount_base_units,
            destination_kind: "v1_pkh".to_owned(),
            destination_value: DESTINATION_V1_PKH.to_owned(),
            expected_lock_root: destination_lock_root,
        };
        let selected = SelectedWithdrawalClient::select(
            WithdrawalClientMode::from(context.client),
            self.iris.clone(),
        )
        .map_err(|error| error.to_string())?;
        let encoded = selected
            .encode(&request)
            .await
            .map_err(|error| error.to_string())?;
        let require_official_iris = context.client == E2eClientMode::Iris;
        if require_official_iris {
            // Hermetic/forked Anvil has no autonomous Base block production.
            // Advance once immediately before browser admission so the
            // production freshness gate observes real post-provision progress.
            self.base()?
                .mine(1)
                .await
                .map_err(|error| error.to_string())?;
        }
        let base_start = self
            .base()?
            .block_number()
            .await
            .map_err(|error| error.to_string())?;
        let mut browser_task = None;
        let mut browser_failure_launch = None;
        let burn = if require_official_iris {
            let iris = self
                .iris
                .clone()
                .ok_or_else(|| "resolved Iris artifact is unavailable".to_owned())?;
            let nockswap_checkout = self.options.nockswap_checkout.clone().ok_or_else(|| {
                "--nockswap-checkout is required for the live Iris E2E".to_owned()
            })?;
            let public_status_url = self
                .scenario()?
                .public_withdrawal_http_endpoint()
                .map_err(|error| error.to_string())?;
            let browser = prepare_browser_launch(
                context,
                self.scenario()?,
                &deployment,
                self.base()?.http_url().as_url().to_string(),
                public_status_url,
                &iris,
                amount_nocks,
                nockswap_checkout,
                self.options.browser_timeout,
            )?;
            browser_failure_launch = Some(browser.clone());
            write_browser_manifest_new(&context.run_dir, &browser.manifest_path, &browser.manifest)
                .map_err(|error| error.to_string())?;
            let mut task = tokio::task::spawn_blocking(move || {
                run_browser_driver(&context_run_dir(&browser.manifest_path), &browser)
            });
            let transaction_hash = tokio::select! {
                result = wait_for_browser_burn(
                    self.base()?.backend(),
                    base_start,
                    deployment.nock,
                    self.options.browser_timeout,
                ) => result?,
                result = &mut task => {
                    match result
                        .map_err(|error| format!("browser driver task failed: {error}"))?
                    {
                        Ok(_) => {
                            return Err(
                                "browser driver exited before its Base burn was observed".to_owned(),
                            );
                        }
                        Err(error) => return Err(error.to_string()),
                    }
                }
            };
            browser_task = Some(task);
            observe_withdrawal_burn(
                self.base()?.backend(),
                &request,
                encoded,
                true,
                transaction_hash,
                RECEIPT_TIMEOUT,
            )
            .await
            .map_err(|error| error.to_string())?
        } else {
            submit_withdrawal_burn(
                self.base()?.backend(),
                &request,
                encoded,
                false,
                RECEIPT_TIMEOUT,
            )
            .await
            .map_err(|error| error.to_string())?
        };
        self.base()?
            .mine(2)
            .await
            .map_err(|error| error.to_string())?;

        let mut progress = CoreWithdrawalProgress::default();
        let lifecycle = wait_for_lifecycle(
            self.base()?,
            self.scenario()?,
            &burn,
            deployment.nock,
            &mut progress,
            CLUSTER_PROGRESS_TIMEOUT,
        )
        .await?;
        let selected_inputs = selected_input_facts(
            self.initial_snapshot
                .as_ref()
                .ok_or_else(|| "initial Nockchain note snapshot is unavailable".to_owned())?,
            &lifecycle.artifacts.selected_inputs,
        )?;
        let transaction_id = lifecycle
            .artifacts
            .authorized_transaction_name
            .clone()
            .ok_or_else(|| "confirmed proposal has no authorized transaction name".to_owned())?;
        let mut chain_source = LiveNockchainProbeSource::connect(
            &self
                .scenario()?
                .public_nockchain_endpoint()
                .map_err(|error| error.to_string())?,
            &self
                .scenario()?
                .private_nockchain_endpoint()
                .map_err(|error| error.to_string())?,
        )
        .await
        .map_err(|error| error.to_string())?;
        let chain_request = NockchainProbeRequest {
            transaction_id: transaction_id.clone(),
            confirmation_depth: 1,
            recipient_lock_root: request.expected_lock_root.to_base58(),
            input_snapshot: NockchainInputSnapshotFacts {
                height: lifecycle.artifacts.snapshot.height,
                block_id: lifecycle.artifacts.snapshot.block_id.to_base58(),
            },
            selected_inputs,
        };
        let transaction = wait_for_nockchain_transaction(
            &mut chain_source,
            &chain_request,
            NOCKCHAIN_PROBE_TIMEOUT,
            Duration::from_millis(250),
        )
        .await
        .map_err(|error| error.to_string())?;
        let settlement = SettlementOracle::prove(&burn.event, &transaction)
            .map_err(|error| error.to_string())?;
        let target = TerminalWithdrawalTarget {
            withdrawal_id: withdrawal_id_label(&lifecycle.id),
            withdrawal_nonce: lifecycle.nonce,
            base_event_id: burn.event.base_event_id.clone(),
            transaction_id,
            confirmation_depth: 1,
            reserved_inputs: lifecycle
                .artifacts
                .selected_inputs
                .iter()
                .map(note_name_facts)
                .collect(),
        };
        let terminal = collect_terminal_proof(
            self.scenario()?,
            &target,
            &settlement,
            chain_source,
            chain_request,
            deployment.nock,
            &lifecycle.id,
            lifecycle.nonce,
            &lifecycle.artifacts.selected_inputs,
        )
        .await?;
        progress
            .record(CoreWithdrawalPhase::Terminal)
            .map_err(|error| error.to_string())?;
        let terminal_path = context.run_dir.join("browser-terminal-proof.json");
        write_terminal_proof(&terminal_path, context, &terminal)?;

        let browser = if let Some(task) = browser_task.take() {
            let result = task
                .await
                .map_err(|error| format!("browser driver task failed: {error}"))?
                .map_err(|error| error.to_string())?;
            let block_hash = self
                .base()?
                .block_hash(burn.block_number)
                .await
                .map_err(|error| error.to_string())?;
            let backend = BrowserBackendEvidenceV1 {
                calldata_hex: burn.mined_input_hex.clone(),
                transaction_hash: format!("{:#x}", burn.transaction_hash),
                block_number: burn.block_number.to_string(),
                block_hash: format!("{block_hash:#x}"),
                log_index: burn.event.log_index,
                base_event_id: burn.event.base_event_id.clone(),
                nock_transaction_id: terminal.target.transaction_id.clone(),
                nock_block_id: terminal.chain.facts.inclusion.block_id.clone(),
                burn_count: 1,
                payout_count: 1,
                terminal_proof_sha256: terminal_proof_sha256(&terminal_path)
                    .map_err(|error| error.to_string())?,
            };
            verify_browser_backend_parity(&result, &backend).map_err(|error| error.to_string())?;
            Some(result)
        } else {
            None
        };
        if let Some(failure_launch) = browser_failure_launch {
            let recovery_launch = failure_launch.clone();
            refresh_browser_readiness(
                self.base()?,
                &failure_launch.manifest.public_status_url,
                Duration::from_secs(300),
            )
            .await?;
            let before_burn_count = self
                .base()?
                .backend()
                .log_count(
                    deployment.nock,
                    burn_for_withdrawal_signature_hash(),
                    base_start,
                )
                .await
                .map_err(|error| error.to_string())?;
            let before_destination =
                destination_note_fingerprint(self.scenario()?, &request.expected_lock_root).await?;
            let failure_run_dir = context_run_dir(&failure_launch.manifest_path);
            tokio::task::spawn_blocking(move || {
                run_browser_failure_driver(&failure_run_dir, &failure_launch)
            })
            .await
            .map_err(|error| format!("real browser failure task failed: {error}"))?
            .map_err(|error| error.to_string())?;
            let recovery_run_dir = context_run_dir(&recovery_launch.manifest_path);
            tokio::task::spawn_blocking(move || {
                run_browser_recovery_matrix_driver(&recovery_run_dir, &recovery_launch)
            })
            .await
            .map_err(|error| format!("browser recovery matrix task failed: {error}"))?
            .map_err(|error| error.to_string())?;
            let after_burn_count = self
                .base()?
                .backend()
                .log_count(
                    deployment.nock,
                    burn_for_withdrawal_signature_hash(),
                    base_start,
                )
                .await
                .map_err(|error| error.to_string())?;
            if after_burn_count != before_burn_count {
                return Err(format!(
                    "real browser failure lane changed Base burn count from {before_burn_count} to {after_burn_count}"
                ));
            }
            let after_destination =
                destination_note_fingerprint(self.scenario()?, &request.expected_lock_root).await?;
            if after_destination != before_destination {
                return Err(
                    "real browser failure lane changed destination Nockchain payout notes"
                        .to_owned(),
                );
            }
            self.scenario()?
                .assert_no_stop_conditions_in_logs()
                .map_err(|error| error.to_string())?;
        }
        let core = CoreWithdrawalEvidence { burn, terminal };
        let normalized =
            normalize_core_withdrawal_evidence(&core).map_err(|error| error.to_string())?;
        write_live_evidence(
            context,
            &deployment,
            &core,
            &lifecycle,
            self.iris.as_ref(),
            browser.as_ref(),
        )?;
        let finished = progress
            .finish(core.clone())
            .map_err(|error| error.to_string())?;
        Ok(ScenarioExecution {
            steps_executed: finished.steps_executed + u64::from(browser.is_some()),
            facts: serde_json::to_value(LiveWithdrawalFacts {
                core,
                normalized,
                browser,
            })
            .map_err(|error| error.to_string())?,
        })
    }

    async fn shutdown(&mut self, _context: &E2eRunContext) -> Result<(), String> {
        let mut errors = Vec::new();
        if let Some(mut scenario) = self.scenario.take() {
            scenario.stop();
        }
        if let Some(base) = self.base.take() {
            if let Err(error) = base.shutdown().await {
                errors.push(error);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

#[derive(Debug, Clone)]
struct ProvisionedDeployment {
    inbox: Address,
    environment_id: String,
    runtime_code_hashes: BTreeMap<String, String>,
    nock: Address,
    state: ForkContractState,
    start_height: u64,
}

enum ProvisionedBase {
    Hermetic(AnvilBackend),
    Fork(PinnedAnvilFork),
}

impl ProvisionedBase {
    fn backend(&self) -> &AnvilBackend {
        match self {
            Self::Hermetic(backend) => backend,
            Self::Fork(fork) => fork.backend(),
        }
    }

    async fn shutdown(self) -> Result<(), String> {
        match self {
            Self::Hermetic(backend) => backend.shutdown().await.map_err(|error| error.to_string()),
            Self::Fork(fork) => fork.shutdown().await.map_err(|error| error.to_string()),
        }
    }
}

#[derive(Debug, Serialize)]
struct LiveWithdrawalFacts {
    core: CoreWithdrawalEvidence,
    normalized: NormalizedCoreWithdrawalFacts,
    browser: Option<BrowserWithdrawalResultV2>,
}

struct LifecycleResult {
    id: WithdrawalId,
    nonce: u64,
    artifacts: WithdrawalSequencerProposalArtifacts,
}

async fn wait_for_snapshot(
    service: &BridgeNoteSnapshotService,
    timeout: Duration,
) -> Result<ConfirmedBridgeNoteSnapshot, String> {
    let deadline = Instant::now() + timeout;
    loop {
        match service.refresh().await {
            Ok(Some(snapshot)) if !snapshot.normalized.candidates.is_empty() => {
                return Ok(snapshot)
            }
            Ok(_) => {}
            Err(error) if Instant::now() >= deadline => return Err(error.to_string()),
            Err(_) => {}
        }
        if Instant::now() >= deadline {
            return Err("timed out waiting for a non-empty bridge note snapshot".to_owned());
        }
        sleep(Duration::from_millis(250)).await;
    }
}

async fn destination_note_fingerprint(
    scenario: &ScenarioHarness,
    lock_root: &NockHash,
) -> Result<BTreeMap<String, (u64, u64)>, String> {
    let first_name = FirstName::from_lock_root(lock_root)
        .map_err(|error| error.to_string())?
        .into_hash()
        .to_base58();
    let service = BridgeNoteSnapshotService::new_private(
        scenario
            .private_nockchain_endpoint()
            .map_err(|error| error.to_string())?,
        BridgeOwnedNoteSelectors {
            first_names: vec![first_name],
        },
        Duration::ZERO,
    );
    let snapshot = wait_for_snapshot(&service, Duration::from_secs(120)).await?;
    snapshot
        .normalized
        .candidates
        .iter()
        .map(|candidate| {
            let identity = candidate.identity();
            let name = format!(
                "{}/{}",
                identity.name.first.to_base58(),
                identity.name.last.to_base58()
            );
            let assets = u64::try_from(candidate.assets().0)
                .map_err(|_| "destination note amount exceeds u64".to_owned())?;
            Ok((name, (assets, identity.origin_page.0 .0)))
        })
        .collect()
}

fn bridge_lock_root() -> Result<NockHash, String> {
    let pkhs = BRIDGE_NOCK_PKHS
        .iter()
        .map(|value| NockHash::from_base58(value).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    derive_bridge_spend_authority_from_pkhs(3, pkhs)
        .map(|(_, root)| root)
        .map_err(|error| error.to_string())
}

fn destination_lock_root(destination: &str) -> Result<NockHash, String> {
    let pkh = NockHash::from_base58(destination).map_err(|error| error.to_string())?;
    Lock::SpendCondition(SpendCondition::new(vec![LockPrimitive::Pkh(Pkh::new(
        1,
        vec![pkh],
    ))]))
    .hash()
    .map_err(|error| error.to_string())
}

fn prepare_browser_launch(
    context: &E2eRunContext,
    scenario: &ScenarioHarness,
    deployment: &ProvisionedDeployment,
    rpc_url: String,
    public_status_url: String,
    iris: &IrisArtifact,
    amount_nocks: u64,
    nockswap_checkout: PathBuf,
    timeout: Duration,
) -> Result<BrowserDriverLaunch, String> {
    if timeout.is_zero() || !nockswap_checkout.join("package.json").is_file() {
        return Err("browser timeout or NockSwap checkout is invalid".to_owned());
    }
    let run_dir = &context.run_dir;
    let manifest_path = run_dir.join("browser-driver-manifest.json");
    let terminal_proof_path = run_dir.join("browser-terminal-proof.json");
    let result_path = run_dir.join("browser-result.json");
    let artifact_dir = run_dir.join("browser-artifacts");
    fs::create_dir_all(&artifact_dir).map_err(|error| error.to_string())?;
    let web_port = 3_000_u16
        .checked_add(scenario.paths().port_offset)
        .ok_or_else(|| "browser web port overflow".to_owned())?;
    let mode = match context.base {
        E2eBaseMode::Hermetic => BrowserDriverMode::Hermetic,
        E2eBaseMode::BaseSepoliaFork => BrowserDriverMode::BaseSepoliaFork,
    };
    let nockswap_git_revision = tracked_git_revision(&nockswap_checkout)?;
    Ok(BrowserDriverLaunch {
        manifest: BrowserDriverManifestV2 {
            schema_version: BROWSER_DRIVER_SCHEMA_VERSION,
            run_id: context.run_id.clone(),
            mode,
            base_url: format!("http://127.0.0.1:{web_port}"),
            rpc_url,
            chain_id: BROWSER_DRIVER_CHAIN_ID,
            account: ANVIL_ACCOUNT.to_owned(),
            contracts: BTreeMap::from([
                ("nock".to_owned(), format!("{:#x}", deployment.nock)),
                (
                    "message_inbox".to_owned(),
                    format!("{:#x}", deployment.inbox),
                ),
            ]),
            bridge_signer_pkhs: BRIDGE_NOCK_PKHS.map(str::to_owned).to_vec(),
            bridge_threshold: deployment.state.threshold,
            bridge_lock_root: bridge_lock_root()?.to_base58(),
            nockswap_git_revision,
            iris_git_revision: iris.facts.git_revision.clone(),
            iris_package_version: iris.facts.package_version.clone(),
            iris_tarball_sha256: iris.facts.tarball_sha256.clone(),
            amount_nocks: amount_nocks.to_string(),
            destination_v1_pkh: DESTINATION_V1_PKH.to_owned(),
            public_status_url,
            readiness_path: "/withdrawal-status".to_owned(),
            terminal_proof_path,
            result_path,
            artifact_dir,
        },
        manifest_path,
        nockswap_checkout,
        private_key: ANVIL_PRIVATE_KEY.to_owned(),
        timeout,
    })
}

fn tracked_git_revision(checkout: &Path) -> Result<String, String> {
    for args in [&["diff", "--quiet", "--"][..], &["diff", "--cached", "--quiet", "--"][..]] {
        let status = Command::new("git")
            .args(args)
            .current_dir(checkout)
            .status()
            .map_err(|error| format!("failed to inspect NockSwap tracked files: {error}"))?;
        if !status.success() {
            return Err(
                "NockSwap tracked source must be clean before browser evidence collection"
                    .to_owned(),
            );
        }
    }
    let output = Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(checkout)
        .output()
        .map_err(|error| format!("failed to resolve NockSwap revision: {error}"))?;
    if !output.status.success() {
        return Err("failed to resolve NockSwap HEAD revision".to_owned());
    }
    let revision = String::from_utf8(output.stdout)
        .map_err(|error| format!("NockSwap revision is not UTF-8: {error}"))?;
    let revision = revision.trim();
    if revision.len() != 40 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("NockSwap revision is not a 40-character Git hash".to_owned());
    }
    Ok(revision.to_owned())
}

fn context_run_dir(manifest_path: &Path) -> PathBuf {
    manifest_path
        .parent()
        .expect("browser manifest must have a run directory")
        .to_path_buf()
}

async fn refresh_browser_readiness(
    base: &AnvilBackend,
    endpoint: &str,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let client = reqwest::Client::new();
    let mut next_mine = Instant::now();
    let mut ready_since = None;
    loop {
        let now = Instant::now();
        let observation = browser_readiness_observation(&client, endpoint).await;
        if observation.as_ref().is_some_and(|(ready, _)| *ready) {
            let since = ready_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= Duration::from_secs(5) {
                return Ok(());
            }
        } else {
            ready_since = None;
            let current_unix_ms = unix_ms()?;
            let observation_stale = observation
                .and_then(|(_, observed_at)| observed_at)
                .is_none_or(|observed_at| current_unix_ms.saturating_sub(observed_at) >= 45_000);
            if observation_stale && now >= next_mine {
                base.mine(1).await.map_err(|error| error.to_string())?;
                next_mine = now + Duration::from_secs(45);
            }
        }
        if Instant::now() >= deadline {
            return Err("timed out refreshing browser withdrawal readiness".to_owned());
        }
        sleep(Duration::from_millis(250)).await;
    }
}

async fn browser_readiness_observation(
    client: &reqwest::Client,
    endpoint: &str,
) -> Option<(bool, Option<u64>)> {
    let response = client
        .get(endpoint)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let value = response.json::<serde_json::Value>().await.ok()?;
    Some((
        value.get("ready").and_then(serde_json::Value::as_bool)?,
        value
            .get("baseObservedAt")
            .and_then(serde_json::Value::as_u64),
    ))
}

async fn wait_for_browser_burn(
    backend: &crate::base_backend::BaseBackend,
    start_block: u64,
    nock: Address,
    timeout: Duration,
) -> Result<B256, String> {
    let deadline = Instant::now() + timeout;
    let mut next_block = start_block.saturating_add(1);
    loop {
        let tip = backend
            .block_number()
            .await
            .map_err(|error| error.to_string())?;
        while next_block <= tip {
            for hash in backend
                .block_transactions(next_block)
                .await
                .map_err(|error| error.to_string())?
            {
                let transaction = backend
                    .transaction(hash)
                    .await
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| format!("Base block {next_block} transaction disappeared"))?;
                if transaction.to == Some(nock) && transaction.input.len() == 116 {
                    return Ok(hash);
                }
            }
            next_block = next_block.saturating_add(1);
        }
        if Instant::now() >= deadline {
            return Err("timed out waiting for the browser's Base burn".to_owned());
        }
        sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_for_lifecycle(
    backend: &AnvilBackend,
    scenario: &ScenarioHarness,
    burn: &BurnSubmissionProof,
    nock: Address,
    progress: &mut CoreWithdrawalProgress,
    timeout: Duration,
) -> Result<LifecycleResult, String> {
    let base_event_id = decode_hex(&burn.event.base_event_id, 32)?;
    let public_endpoint = scenario
        .public_withdrawal_endpoint()
        .map_err(|error| error.to_string())?;
    let config_path = scenario
        .bridge_config_path(0)
        .map_err(|error| error.to_string())?;
    let sequencer = sequencer_client(&config_path).await?;
    let deadline = Instant::now() + timeout;
    let mut id = None;
    let mut artifacts: Option<WithdrawalSequencerProposalArtifacts> = None;
    let mut recorded = 0_usize;
    loop {
        if id.is_none() {
            if let Some(record) =
                get_public_withdrawal(&public_endpoint, &base_event_id, nock).await?
            {
                if let Some(value) = record.withdrawal_id.as_ref() {
                    id = Some(withdrawal_id_from_proto(value).map_err(|error| error.to_string())?);
                }
            }
        }
        if let Some(withdrawal_id) = id.as_ref() {
            let status = sequencer
                .get_sequenced_withdrawal_status(withdrawal_id)
                .await
                .map_err(|error| error.to_string())?;
            if status.found {
                if recorded == 0 {
                    progress
                        .record(CoreWithdrawalPhase::Pending)
                        .map_err(|error| error.to_string())?;
                    recorded = 1;
                }
                if artifacts
                    .as_ref()
                    .and_then(|current| current.authorized_transaction_name.as_ref())
                    .is_none()
                {
                    if let Some(current) = sequencer
                        .load_canonical_proposal_artifacts(withdrawal_id)
                        .await
                        .map_err(|error| error.to_string())?
                    {
                        artifacts = Some(current);
                    }
                }
                let ready = artifacts.is_some()
                    || matches!(
                        status.state.as_str(),
                        "peer_canonical" | "authorized" | "mempool_accepted" | "confirmed"
                    );
                if ready && recorded == 1 {
                    progress
                        .record(CoreWithdrawalPhase::Ready)
                        .map_err(|error| error.to_string())?;
                    recorded = 2;
                }
                if matches!(status.state.as_str(), "mempool_accepted" | "confirmed")
                    && recorded == 2
                {
                    progress
                        .record(CoreWithdrawalPhase::Submitted)
                        .map_err(|error| error.to_string())?;
                    recorded = 3;
                }
                if status.state == "confirmed" && recorded == 3 {
                    progress
                        .record(CoreWithdrawalPhase::SequencerConfirmed)
                        .map_err(|error| error.to_string())?;
                    recorded = 4;
                }
                if recorded == 4 {
                    if let Some(artifacts) = artifacts.clone() {
                        if artifacts.authorized_transaction_name.is_some() {
                            return Ok(LifecycleResult {
                                id: withdrawal_id.clone(),
                                nonce: status.withdrawal_nonce,
                                artifacts,
                            });
                        }
                    }
                }
            }
        }
        if Instant::now() >= deadline {
            return Err("timed out waiting for the live withdrawal lifecycle".to_owned());
        }
        backend.mine(1).await.map_err(|error| error.to_string())?;
        sleep(Duration::from_millis(250)).await;
    }
}

fn selected_input_facts(
    snapshot: &ConfirmedBridgeNoteSnapshot,
    selected: &[nockchain_types::v1::Name],
) -> Result<Vec<SelectedInputNoteFacts>, String> {
    selected
        .iter()
        .map(|name| {
            let candidate = snapshot
                .normalized
                .candidates
                .iter()
                .find(|candidate| candidate.identity().name == *name)
                .ok_or_else(|| {
                    format!(
                        "selected input {}/{} is absent from the pre-burn snapshot",
                        name.first.to_base58(),
                        name.last.to_base58()
                    )
                })?;
            let note_version = match candidate.version() {
                Version::V0 => 0,
                Version::V1 => 1,
                version => return Err(format!("unsupported selected input version {version:?}")),
            };
            Ok(SelectedInputNoteFacts {
                name: note_name_facts(name),
                note_version,
                assets_nicks: u64::try_from(candidate.assets().0)
                    .map_err(|_| "selected input amount exceeds u64".to_owned())?,
                origin_height: candidate.identity().origin_page.0 .0,
                origin_transaction_id: None,
                origin_is_coinbase: None,
            })
        })
        .collect()
}

fn note_name_facts(name: &nockchain_types::v1::Name) -> NoteNameFacts {
    NoteNameFacts {
        first: name.first.to_base58(),
        last: name.last.to_base58(),
    }
}

async fn sequencer_client(config_path: &Path) -> Result<GrpcWithdrawalSequencerClient, String> {
    let config = BridgeConfigToml::from_file(config_path).map_err(|error| error.to_string())?;
    Ok(GrpcWithdrawalSequencerClient::new(
        config
            .nockchain_sequencer_api_address()
            .map_err(|error| error.to_string())?,
    ))
}

async fn get_public_withdrawal(
    endpoint: &str,
    base_event_id: &[u8],
    nock: Address,
) -> Result<Option<ingress_proto::PublicWithdrawalRecord>, String> {
    let mut client =
        ingress_proto::withdrawal_public_query_client::WithdrawalPublicQueryClient::connect(
            endpoint.to_owned(),
        )
        .await
        .map_err(|error| error.to_string())?;
    let response = client
        .get_withdrawal(Request::new(ingress_proto::GetPublicWithdrawalRequest {
            lookup: Some(ingress_proto::PublicWithdrawalLookupKey {
                deployment: Some(ingress_proto::PublicWithdrawalDeployment {
                    base_chain_id: BROWSER_DRIVER_CHAIN_ID,
                    nock_contract_address: nock.as_slice().to_vec(),
                    policy_id: "withdrawal-policy-v1".to_owned(),
                    protocol_id: "WithdrawalWireV1".to_owned(),
                }),
                key: Some(
                    ingress_proto::public_withdrawal_lookup_key::Key::BaseEventId(
                        base_event_id.to_vec(),
                    ),
                ),
            }),
        }))
        .await
        .map_err(|error| error.to_string())?
        .into_inner();
    if response.found {
        Ok(response.withdrawal)
    } else {
        Ok(None)
    }
}

async fn collect_terminal_proof(
    scenario: &ScenarioHarness,
    target: &TerminalWithdrawalTarget,
    settlement: &SettlementConservationProof,
    chain_source: LiveNockchainProbeSource,
    chain_request: NockchainProbeRequest,
    nock: Address,
    withdrawal_id: &WithdrawalId,
    withdrawal_nonce: u64,
    selected_inputs: &[nockchain_types::v1::Name],
) -> Result<TerminalWithdrawalProof, String> {
    let mut chain = LiveChainSource {
        source: chain_source,
        request: chain_request,
    };
    let mut kernels = LiveKernelSource {
        endpoints: (0..5)
            .map(|node_id| {
                scenario
                    .bridge_ingress_endpoint(node_id)
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?,
        target_withdrawal_id: target.withdrawal_id.clone(),
        target_as_of: withdrawal_id.as_of.to_be_limb_bytes().to_vec(),
        target_base_event: withdrawal_id.base_event_id.0.clone(),
    };
    let mut sequencer = LiveSequencerSource {
        config_path: scenario
            .bridge_config_path(0)
            .map_err(|error| error.to_string())?,
        withdrawal_id: withdrawal_id.clone(),
        withdrawal_nonce,
    };
    let mut reservations = LiveReservationSource {
        config_path: scenario
            .bridge_config_path(0)
            .map_err(|error| error.to_string())?,
        withdrawal_id: withdrawal_id.clone(),
        tracked_inputs: selected_inputs.to_vec(),
    };
    let mut public = LivePublicSource {
        endpoint: scenario
            .public_withdrawal_endpoint()
            .map_err(|error| error.to_string())?,
        nock,
        base_event_id: decode_hex(&target.base_event_id, 32)?,
        withdrawal_id: withdrawal_id.clone(),
        withdrawal_nonce,
    };
    wait_for_terminal_withdrawal(
        target,
        settlement,
        &mut TerminalOracleSources {
            chain: &mut chain,
            kernels: &mut kernels,
            sequencer: &mut sequencer,
            reservations: &mut reservations,
            public: &mut public,
        },
        CLUSTER_PROGRESS_TIMEOUT,
        Duration::from_millis(250),
    )
    .await
    .map_err(|error| error.to_string())
}

struct LiveChainSource {
    source: LiveNockchainProbeSource,
    request: NockchainProbeRequest,
}

#[async_trait]
impl TerminalChainSource for LiveChainSource {
    async fn observe_chain(
        &mut self,
    ) -> Result<TimedTerminalFact<NockchainTransactionFacts>, String> {
        let transaction = wait_for_nockchain_transaction(
            &mut self.source,
            &self.request,
            Duration::from_secs(5),
            Duration::from_millis(250),
        )
        .await
        .map_err(|error| error.to_string())?;
        timed(
            "nockchain-public-private", "nockchain-live-cluster", transaction,
        )
    }
}

struct LiveKernelSource {
    endpoints: Vec<String>,
    target_withdrawal_id: String,
    target_as_of: Vec<u8>,
    target_base_event: Vec<u8>,
}

#[async_trait]
impl TerminalKernelSource for LiveKernelSource {
    async fn observe_kernels(
        &mut self,
    ) -> Result<TimedTerminalFact<Vec<BridgeKernelTerminalFacts>>, String> {
        let mut facts = Vec::with_capacity(self.endpoints.len());
        for (node_id, endpoint) in self.endpoints.iter().enumerate() {
            let mut client = BridgeTuiClient::connect(endpoint.clone())
                .await
                .map_err(|error| error.to_string())?;
            let response = client
                .get_snapshot(Request::new(tui_proto::GetSnapshotRequest {
                    deposit_log_view: Some(tui_proto::DepositLogView {
                        offset: 0,
                        limit: 0,
                    }),
                    alert_view: Some(tui_proto::AlertView { limit: 0 }),
                    withdrawal_target: Some(tui_proto::WithdrawalKernelTarget {
                        as_of: self.target_as_of.clone(),
                        base_event_id: self.target_base_event.clone(),
                    }),
                }))
                .await
                .map_err(|error| error.to_string())?
                .into_inner();
            let observed_target_id = response
                .target_withdrawal_id
                .as_deref()
                .ok_or_else(|| "bridge TUI snapshot omitted target withdrawal id".to_owned())?;
            let observed_base_event = response
                .target_base_event_id
                .as_deref()
                .ok_or_else(|| "bridge TUI snapshot omitted target Base event".to_owned())?;
            if observed_target_id != self.target_withdrawal_id
                || observed_base_event != self.target_base_event.as_slice()
            {
                return Err("bridge TUI snapshot returned another withdrawal identity".to_owned());
            }
            let matching_unsettled_withdrawal = response
                .target_withdrawal_unsettled
                .ok_or_else(|| "bridge TUI snapshot omitted target kernel state".to_owned())?;
            let network = response
                .network_state
                .ok_or_else(|| "bridge TUI snapshot is missing network state".to_owned())?;
            let frontier = network
                .nockchain
                .ok_or_else(|| "bridge TUI snapshot is missing Nockchain state".to_owned())?;
            let running = tui_proto::RunningState::try_from(response.running_state)
                .is_ok_and(|state| state != tui_proto::RunningState::Stopped);
            let hold_reason = if response.nock_hold {
                Some("nock_hold".to_owned())
            } else if response.base_hold {
                Some("base_hold".to_owned())
            } else {
                None
            };
            facts.push(BridgeKernelTerminalFacts {
                node_id: u64::try_from(node_id).map_err(|error| error.to_string())?,
                available: true,
                running,
                target_withdrawal_id: observed_target_id.to_owned(),
                target_base_event_id: format!("0x{}", hex::encode(observed_base_event)),
                hold_reason,
                frontier: KernelFrontierFacts {
                    height: frontier.height,
                    block_id: frontier.tip_hash,
                },
                matching_unsettled_withdrawal,
                other_unsettled_withdrawals: network.unsettled_withdrawal_count,
            });
        }
        timed("bridge-tui-snapshots", "bridge-live-cluster", facts)
    }
}

struct LiveSequencerSource {
    config_path: PathBuf,
    withdrawal_id: WithdrawalId,
    withdrawal_nonce: u64,
}

#[async_trait]
impl TerminalSequencerSource for LiveSequencerSource {
    async fn observe_sequencer(
        &mut self,
    ) -> Result<TimedTerminalFact<SequencerTerminalFacts>, String> {
        let client = sequencer_client(&self.config_path).await?;
        let status = client
            .get_sequenced_withdrawal_status(&self.withdrawal_id)
            .await
            .map_err(|error| error.to_string())?;
        let state = sequencer_state(&status.state);
        let confirmed = state == SequencerTerminalState::Confirmed;
        let confirmed_block_id = status
            .confirmed_block_id
            .as_deref()
            .map(NockHash::from_be_limb_bytes)
            .transpose()
            .map_err(|error| error.to_string())?
            .map(|hash| hash.to_base58());
        timed(
            "private-sequencer-status",
            "sequencer-live-journal",
            SequencerTerminalFacts {
                withdrawal_id: withdrawal_id_label(&self.withdrawal_id),
                withdrawal_nonce: self.withdrawal_nonce,
                transaction_id: (!status.authorized_transaction_name.is_empty())
                    .then_some(status.authorized_transaction_name),
                state,
                confirmation_event_id: confirmed.then_some(status.confirmation_event_id).flatten(),
                confirmed_height: confirmed.then_some(status.confirmed_height).flatten(),
                confirmed_block_id: confirmed.then_some(confirmed_block_id).flatten(),
            },
        )
    }
}

struct LiveReservationSource {
    config_path: PathBuf,
    withdrawal_id: WithdrawalId,
    tracked_inputs: Vec<nockchain_types::v1::Name>,
}

#[async_trait]
impl TerminalReservationSource for LiveReservationSource {
    async fn observe_reservations(
        &mut self,
    ) -> Result<TimedTerminalFact<ReservationTerminalFacts>, String> {
        let client = sequencer_client(&self.config_path).await?;
        let status = client
            .get_sequenced_withdrawal_status(&self.withdrawal_id)
            .await
            .map_err(|error| error.to_string())?;
        let reserved = client
            .get_reserved_withdrawal_inputs()
            .await
            .map_err(|error| error.to_string())?;
        let currently_reserved_inputs = self
            .tracked_inputs
            .iter()
            .filter(|input| reserved.contains(input))
            .map(note_name_facts)
            .collect::<Vec<_>>();
        let released = status.state == "confirmed" && currently_reserved_inputs.is_empty();
        let release_event_ids = released
            .then_some(status.reservation_release_event_id)
            .flatten()
            .into_iter()
            .collect();
        timed(
            "private-reservation-projection",
            "sequencer-live-journal",
            ReservationTerminalFacts {
                withdrawal_id: withdrawal_id_label(&self.withdrawal_id),
                tracked_inputs: self.tracked_inputs.iter().map(note_name_facts).collect(),
                release_event_ids,
                currently_reserved_inputs,
                release_count: u64::from(released),
            },
        )
    }
}

struct LivePublicSource {
    endpoint: String,
    base_event_id: Vec<u8>,
    nock: Address,
    withdrawal_id: WithdrawalId,
    withdrawal_nonce: u64,
}

#[async_trait]
impl TerminalPublicSource for LivePublicSource {
    async fn observe_public(
        &mut self,
    ) -> Result<TimedTerminalFact<PublicWithdrawalTerminalFacts>, String> {
        let record = get_public_withdrawal(&self.endpoint, &self.base_event_id, self.nock)
            .await?
            .ok_or_else(|| "public withdrawal record is not available".to_owned())?;
        let observed_id = record
            .withdrawal_id
            .as_ref()
            .ok_or_else(|| "public withdrawal record is missing its withdrawal id".to_owned())
            .and_then(|value| withdrawal_id_from_proto(value).map_err(|error| error.to_string()))?;
        if observed_id != self.withdrawal_id {
            return Err("public withdrawal identity diverged".to_owned());
        }
        let status = ingress_proto::PublicWithdrawalStatus::try_from(record.status)
            .unwrap_or(ingress_proto::PublicWithdrawalStatus::Unspecified);
        let state = match status {
            ingress_proto::PublicWithdrawalStatus::Confirmed => PublicWithdrawalState::Confirmed,
            ingress_proto::PublicWithdrawalStatus::Failure => PublicWithdrawalState::Failed,
            _ => PublicWithdrawalState::Pending,
        };
        let confirmed_block_id = record
            .nock_confirmed_block_id
            .as_deref()
            .map(NockHash::from_be_limb_bytes)
            .transpose()
            .map_err(|error| error.to_string())?
            .map(|hash| hash.to_base58());
        timed(
            "public-withdrawal-grpc",
            "sequencer-live-journal",
            PublicWithdrawalTerminalFacts {
                withdrawal_id: withdrawal_id_label(&observed_id),
                withdrawal_nonce: self.withdrawal_nonce,
                state,
                base_event_id: format!("0x{}", hex::encode(&record.base_event_id)),
                transaction_id: record.nock_transaction_name,
                confirmed_height: record.nock_confirmed_height,
                confirmed_block_id,
            },
        )
    }
}

fn sequencer_state(value: &str) -> SequencerTerminalState {
    match value {
        "confirmed" => SequencerTerminalState::Confirmed,
        "mempool_accepted" => SequencerTerminalState::MempoolAccepted,
        "reorg_hold" => SequencerTerminalState::ReorgHold,
        "failed" => SequencerTerminalState::Failed,
        _ => SequencerTerminalState::Pending,
    }
}

fn timed<T>(
    source_name: &str,
    correlation_group: &str,
    facts: T,
) -> Result<TimedTerminalFact<T>, String> {
    Ok(TimedTerminalFact {
        observed_unix_ms: unix_ms()?,
        source_name: source_name.to_owned(),
        correlation_group: correlation_group.to_owned(),
        facts,
    })
}

fn unix_ms() -> Result<u64, String> {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_millis(),
    )
    .map_err(|error| error.to_string())
}

fn withdrawal_id_label(id: &WithdrawalId) -> String {
    format!(
        "{}:0x{}",
        id.as_of.to_base58(),
        hex::encode(id.base_event_id.as_slice())
    )
}

fn decode_hex(value: &str, expected_bytes: usize) -> Result<Vec<u8>, String> {
    let bytes = hex::decode(value.strip_prefix("0x").unwrap_or(value))
        .map_err(|error| error.to_string())?;
    if bytes.len() != expected_bytes {
        return Err(format!(
            "expected {expected_bytes} bytes, decoded {}",
            bytes.len()
        ));
    }
    Ok(bytes)
}

#[derive(Serialize)]
struct BrowserTerminalEnvelope<'a> {
    schema_version: u64,
    run_id: &'a str,
    terminal: bool,
    nock_transaction_id: &'a str,
    nock_block_id: &'a str,
    burn_count: u64,
    payout_count: u64,
    payout_nicks: &'a str,
    proof: &'a TerminalWithdrawalProof,
}

fn write_terminal_proof(
    path: &Path,
    context: &E2eRunContext,
    proof: &TerminalWithdrawalProof,
) -> Result<(), String> {
    let temp_path = path.with_extension("json.pending");
    let envelope = BrowserTerminalEnvelope {
        schema_version: BROWSER_TERMINAL_PROOF_SCHEMA_VERSION,
        run_id: &context.run_id,
        terminal: true,
        nock_transaction_id: &proof.target.transaction_id,
        nock_block_id: &proof.chain.facts.inclusion.block_id,
        burn_count: 1,
        payout_count: 1,
        payout_nicks: &proof.settlement.recipient_payout_nicks.0,
        proof,
    };
    let bytes = serde_json::to_vec_pretty(&envelope).map_err(|error| error.to_string())?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp_path)
        .map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    fs::rename(&temp_path, path).map_err(|error| error.to_string())
}

fn write_live_evidence(
    context: &E2eRunContext,
    deployment: &ProvisionedDeployment,
    core: &CoreWithdrawalEvidence,
    lifecycle: &LifecycleResult,
    iris: Option<&IrisArtifact>,
    browser: Option<&BrowserWithdrawalResultV2>,
) -> Result<(), String> {
    let now = unix_ms()?;
    let mode = match context.base {
        E2eBaseMode::Hermetic => EvidenceEnvironmentMode::Hermetic,
        E2eBaseMode::BaseSepoliaFork => EvidenceEnvironmentMode::BaseSepoliaFork,
    };
    let source_manifest_path = context.workspace_root.join(BASE_ENVIRONMENT_MANIFEST);
    let mut capsule = WithdrawalEvidenceCapsuleV1::new(
        EvidenceRunFacts {
            run_id: context.run_id.clone(),
            scenario: "withdrawal".to_owned(),
            seed: context.seed,
            status: EvidenceRunStatus::Passed,
            error: None,
            started_at_unix_ms: now,
            finished_at_unix_ms: Some(now),
        },
        EvidenceEnvironmentFacts {
            mode,
            environment_id: deployment.environment_id.clone(),
            source_manifest_sha256: Some(file_sha256(&source_manifest_path)?),
            source_chain_id: (context.base == E2eBaseMode::BaseSepoliaFork).then_some(84_532),
            source_block_number: None,
            source_block_hash: None,
            local_chain_id: BROWSER_DRIVER_CHAIN_ID,
            rpc_endpoint_class: "loopback_anvil".to_owned(),
        },
        RedactionDeclaration {
            policy: "withdrawal-e2e-redaction-v1".to_owned(),
            removed_secret_classes: Vec::new(),
            raw_logs_embedded: false,
            external_artifacts_only: true,
        },
    );
    capsule.artifacts = Some(EvidenceArtifacts {
        bridge_runtime: context.artifacts.clone(),
        iris: iris.map(|artifact| artifact.facts.clone()),
        nockswap_bundle: None,
    });
    capsule.deployment = Some(EvidenceDeploymentFacts {
        environment_id: deployment.environment_id.clone(),
        addresses: BTreeMap::from([
            (
                "message_inbox".to_owned(),
                format!("{:#x}", deployment.inbox),
            ),
            ("nock".to_owned(), format!("{:#x}", deployment.nock)),
        ]),
        runtime_code_hashes: deployment.runtime_code_hashes.clone(),
        pristine_state: Some(json_object_map(&deployment.state)?),
        overrides: Vec::new(),
    });
    for (index, action) in [
        "provision", "submit_burn", "pending", "ready", "submitted", "sequencer_confirmed",
        "terminal", "assert_terminal",
    ]
    .into_iter()
    .enumerate()
    {
        capsule.steps.push(EvidenceStep {
            index: u64::try_from(index).map_err(|error| error.to_string())?,
            action: action.to_owned(),
            status: "passed".to_owned(),
            started_at_unix_ms: now,
            finished_at_unix_ms: now,
            duration_ms: 0,
            frontier_before: None,
            frontier_after: None,
            detail: None,
        });
    }
    capsule.assertions = vec![
        EvidenceAssertion {
            assertion: "exact_116_byte_base_calldata".to_owned(),
            status: "passed".to_owned(),
            detail: Some(core.burn.mined_input_hex.clone()),
        },
        EvidenceAssertion {
            assertion: "single_burn_single_payout".to_owned(),
            status: "passed".to_owned(),
            detail: Some("one verified Base event and one recipient output".to_owned()),
        },
        EvidenceAssertion {
            assertion: "multisource_terminal_stability".to_owned(),
            status: "passed".to_owned(),
            detail: Some(format!(
                "{} stable observations",
                core.terminal.stable_observations
            )),
        },
    ];
    if browser.is_some() {
        capsule.assertions.push(EvidenceAssertion {
            assertion: "browser_backend_evidence_parity".to_owned(),
            status: "passed".to_owned(),
            detail: Some("NockSwap result matched backend terminal evidence".to_owned()),
        });
    }
    capsule.base = Some(core.burn.clone());
    capsule.sequencer = Some(EvidenceSequencerFacts {
        proposal_hash: Some(lifecycle.artifacts.proposal_hash.clone()),
        journal_id: None,
        sequencer: core.terminal.sequencer.clone(),
        reservations: core.terminal.reservations.clone(),
    });
    capsule.nockchain = Some(EvidenceNockchainFacts::from(&core.terminal.chain.facts));
    capsule.kernels = Some(EvidenceKernelFacts {
        observed_unix_ms: core.terminal.kernels.observed_unix_ms,
        nodes: core.terminal.kernels.facts.clone(),
    });
    capsule.public = Some(core.terminal.public.clone());
    capsule.conservation = Some(core.terminal.settlement.clone());
    capsule.terminal = Some(EvidenceTerminalFacts::from(&core.terminal));
    if browser.is_some() {
        capsule.external_artifacts.push(external_file_reference(
            "nockswap_browser_result",
            &context.run_dir.join("browser-result.json"),
            "application/json",
        )?);
    }

    let mut secrets = vec![SecretValue {
        category: "anvil_private_key".to_owned(),
        value: ANVIL_PRIVATE_KEY.to_owned(),
    }];
    secrets.extend(
        BRIDGE_ETH_KEYS
            .iter()
            .enumerate()
            .map(|(index, value)| SecretValue {
                category: format!("bridge_eth_private_key_{index}"),
                value: (*value).to_owned(),
            }),
    );
    secrets.extend(
        BRIDGE_NOCK_KEYS
            .iter()
            .enumerate()
            .map(|(index, value)| SecretValue {
                category: format!("bridge_nock_private_key_{index}"),
                value: (*value).to_owned(),
            }),
    );
    let redactor = SecretRedactor::new(secrets).map_err(|error| error.to_string())?;
    let mut collector =
        EvidenceCollector::new(&context.run_dir, redactor).map_err(|error| error.to_string())?;
    collector
        .checkpoint("terminal", &capsule)
        .map_err(|error| error.to_string())?;
    collector
        .finish(&capsule)
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn json_object_map<T: Serialize>(value: &T) -> Result<BTreeMap<String, serde_json::Value>, String> {
    match serde_json::to_value(value).map_err(|error| error.to_string())? {
        serde_json::Value::Object(values) => Ok(values.into_iter().collect()),
        _ => Err("evidence value is not a JSON object".to_owned()),
    }
}

fn external_file_reference(
    kind: &str,
    path: &Path,
    media_type: &str,
) -> Result<ExternalArtifactReference, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    Ok(ExternalArtifactReference {
        kind: kind.to_owned(),
        path: path.display().to_string(),
        sha256: hex::encode(Sha256::digest(&bytes)),
        size_bytes: bytes.len().to_string(),
        media_type: media_type.to_owned(),
    })
}

fn file_sha256(path: &Path) -> Result<String, String> {
    fs::read(path)
        .map(|bytes| hex::encode(Sha256::digest(bytes)))
        .map_err(|error| error.to_string())
}
