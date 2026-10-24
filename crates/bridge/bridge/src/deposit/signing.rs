use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::Address;
use tracing::{debug, info, warn};

use crate::core::deposit::signing::{
    SigningCandidatePrecheckDecision, SigningCandidatePrecheckInput, SigningEpochBoundsDecision,
    SigningEpochBoundsDecisionInput, SigningPlanner, SigningProcessedDecision,
    SigningProcessedDecisionInput, SigningTickPlanAction, SigningTickPlanInput,
};
use crate::core::loop_policy::SigningLoopPolicy;
use crate::deposit::cache::{ProposalCache, ProposalStatus, SIGNATURE_THRESHOLD};
use crate::deposit::log::DepositLog;
use crate::deposit::ports::BaseContractPort;
use crate::deposit::types::{DepositId, NockDepositRequestData};
use crate::observability::health::PeerEndpoint;
use crate::observability::metrics;
use crate::observability::status::BridgeStatus;
use crate::shared::config::NonceEpochConfig;
use crate::shared::runtime::BridgeRuntimeHandle;
use crate::shared::signing::BridgeSigner;
use crate::shared::stop::{StopController, StopHandle};
use crate::shared::types::Tip5Hash;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DepositSignatureBroadcastReason {
    Initial,
    Regossip,
}

impl DepositSignatureBroadcastReason {
    fn as_str(self) -> &'static str {
        match self {
            DepositSignatureBroadcastReason::Initial => "initial",
            DepositSignatureBroadcastReason::Regossip => "regossip",
        }
    }
}

fn system_time_secs(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn format_source_tx_id(tx_id: &Tip5Hash) -> String {
    tx_id.to_base58()
}

fn spawn_deposit_signature_broadcast(
    peers: &[PeerEndpoint],
    msg: &crate::shared::ingress::proto::DepositSignatureBroadcast,
    prop_id: &str,
    reason: DepositSignatureBroadcastReason,
) {
    use crate::shared::ingress::proto::bridge_ingress_client::BridgeIngressClient;

    for peer in peers {
        let msg = msg.clone();
        let addr = peer.address.clone();
        let peer_id = peer.node_id;
        let prop_id = prop_id.to_string();

        tokio::spawn(async move {
            match BridgeIngressClient::connect(addr.clone()).await {
                Ok(mut client) => match client.broadcast_deposit_signature(msg.clone()).await {
                    Ok(_) => {
                        debug!(
                            target: "bridge.cursor",
                            peer_node_id=peer_id,
                            proposal_hash=%prop_id,
                            reason=reason.as_str(),
                            "broadcast signature to peer"
                        );
                    }
                    Err(e) if e.code() == tonic::Code::Unimplemented => {
                        let legacy_msg = crate::shared::ingress::proto::SignatureBroadcast {
                            deposit_id: msg.deposit_id,
                            proposal_hash: msg.proposal_hash,
                            signature: msg.signature,
                            signer_address: msg.signer_address,
                            timestamp: msg.timestamp,
                        };

                        match client.broadcast_signature(legacy_msg).await {
                            Ok(_) => {
                                debug!(
                                    target: "bridge.cursor",
                                    peer_node_id=peer_id,
                                    proposal_hash=%prop_id,
                                    reason=reason.as_str(),
                                    "broadcast signature to peer via legacy RPC"
                                );
                            }
                            Err(e) => {
                                warn!(
                                    target: "bridge.cursor",
                                    peer_node_id=peer_id,
                                    error=%e,
                                    reason=reason.as_str(),
                                    "failed to broadcast signature to peer via legacy RPC"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            target: "bridge.cursor",
                            peer_node_id=peer_id,
                            error=%e,
                            reason=reason.as_str(),
                            "failed to broadcast signature to peer"
                        );
                    }
                },
                Err(e) => {
                    warn!(
                        target: "bridge.cursor",
                        peer_node_id=peer_id,
                        peer_address=%addr,
                        error=%e,
                        reason=reason.as_str(),
                        "failed to connect to peer for signature broadcast"
                    );
                }
            }
        });
    }
}

#[derive(Clone, Debug)]
pub struct SigningTickState {
    pub logged_epoch_ready: bool,
    pub last_regossip_at: SystemTime,
}

impl SigningTickState {
    pub fn new(now: SystemTime) -> Self {
        Self {
            logged_epoch_ready: false,
            last_regossip_at: now,
        }
    }
}

impl Default for SigningTickState {
    fn default() -> Self {
        Self::new(UNIX_EPOCH)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SigningTickInput {
    pub now: SystemTime,
    pub tip_height: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SigningTickOutcome {
    pub regossip_broadcasts: usize,
    pub initial_broadcasts: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SigningCandidateExecutionResult {
    Broadcasted,
    SignFailed,
    DuplicateSignature,
    ProposalStale,
    InvalidOwnSignature,
    CacheUpdateFailed,
}

#[derive(Clone)]
pub struct SigningTickPorts<B: BaseContractPort> {
    runtime: Arc<BridgeRuntimeHandle>,
    base_bridge: Arc<B>,
    deposit_log: Arc<DepositLog>,
    proposal_cache: Arc<ProposalCache>,
}

impl<B: BaseContractPort> SigningTickPorts<B> {
    pub fn new(
        runtime: Arc<BridgeRuntimeHandle>,
        base_bridge: Arc<B>,
        deposit_log: Arc<DepositLog>,
        proposal_cache: Arc<ProposalCache>,
    ) -> Self {
        Self {
            runtime,
            base_bridge,
            deposit_log,
            proposal_cache,
        }
    }
}

#[derive(Clone)]
pub struct SigningTickNodeState {
    signer: Arc<BridgeSigner>,
    valid_addresses: Arc<HashSet<Address>>,
    peers: Arc<Vec<PeerEndpoint>>,
    self_node_id: u64,
    address_to_node_id: Arc<HashMap<Address, u64>>,
}

impl SigningTickNodeState {
    pub fn new(
        signer: Arc<BridgeSigner>,
        valid_addresses: HashSet<Address>,
        peers: Vec<PeerEndpoint>,
        self_node_id: u64,
        address_to_node_id: HashMap<Address, u64>,
    ) -> Self {
        Self {
            signer,
            valid_addresses: Arc::new(valid_addresses),
            peers: Arc::new(peers),
            self_node_id,
            address_to_node_id: Arc::new(address_to_node_id),
        }
    }
}

#[derive(Clone)]
pub struct SigningTickControl {
    bridge_status: BridgeStatus,
    stop_controller: StopController,
    stop: StopHandle,
    local_stop_mode: SigningLocalStopMode,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SigningLocalStopMode {
    #[default]
    RuntimeProbeAndBroadcast,
    LocalTriggerOnly,
}

impl SigningTickControl {
    pub fn new(
        bridge_status: BridgeStatus,
        stop_controller: StopController,
        stop: StopHandle,
    ) -> Self {
        Self {
            bridge_status,
            stop_controller,
            stop,
            local_stop_mode: SigningLocalStopMode::RuntimeProbeAndBroadcast,
        }
    }

    pub fn with_local_stop_mode(mut self, local_stop_mode: SigningLocalStopMode) -> Self {
        self.local_stop_mode = local_stop_mode;
        self
    }
}

#[derive(Clone)]
pub struct SigningTickConfig {
    nonce_epoch: NonceEpochConfig,
    policy: SigningLoopPolicy,
}

impl SigningTickConfig {
    pub fn new(nonce_epoch: &NonceEpochConfig, policy: SigningLoopPolicy) -> Self {
        Self {
            nonce_epoch: nonce_epoch.clone(),
            policy,
        }
    }
}

#[derive(Clone)]
pub struct SigningTickContext<B: BaseContractPort> {
    ports: SigningTickPorts<B>,
    node: SigningTickNodeState,
    control: SigningTickControl,
    config: SigningTickConfig,
}

impl<B: BaseContractPort> SigningTickContext<B> {
    pub fn new(
        ports: SigningTickPorts<B>,
        node: SigningTickNodeState,
        control: SigningTickControl,
        config: SigningTickConfig,
    ) -> Self {
        Self {
            ports,
            node,
            control,
            config,
        }
    }

    async fn execute_signing_candidate(
        &self,
        req: &NockDepositRequestData,
        deposit_id: &DepositId,
        now: SystemTime,
        now_secs: u64,
    ) -> SigningCandidateExecutionResult {
        use crate::shared::ingress::proto::DepositSignatureBroadcast;
        use crate::shared::signing::verify_bridge_signature;

        let my_eth_address = self.node.signer.address();
        let proposal_hash = req.compute_proposal_hash();
        let proposal_id = hex::encode(proposal_hash);

        self.control
            .bridge_status
            .update_proposal(crate::observability::tui::types::Proposal {
                id: proposal_id.clone(),
                proposal_type: "deposit".to_string(),
                description: format!(
                    "Deposit {} wei to {} (nonce {})",
                    req.amount,
                    hex::encode(req.recipient.0),
                    req.nonce
                ),
                signatures_collected: 0,
                signatures_required: SIGNATURE_THRESHOLD as u8,
                signers: vec![],
                created_at: now,
                status: crate::observability::tui::types::ProposalStatus::Pending,
                data_hash: proposal_id.clone(),
                submitted_at_block: None,
                submitted_at: None,
                tx_hash: None,
                time_to_submit_ms: None,
                executed_at_block: None,
                source_block: Some(req.block_height),
                amount: Some(req.amount as u128),
                recipient: Some(format!("0x{}", hex::encode(req.recipient.0))),
                nonce: Some(req.nonce),
                source_tx_id: Some(format_source_tx_id(&req.tx_id)),
                current_proposer: None,
                is_my_turn: false,
                time_until_takeover: None,
            });

        let signature = match self.node.signer.sign_hash(&proposal_hash).await {
            Ok(sig) => sig.as_bytes().to_vec(),
            Err(e) => {
                tracing::error!(
                    target: "bridge.cursor",
                    error=%e,
                    proposal_hash=%proposal_id,
                    "failed to sign proposal"
                );
                return SigningCandidateExecutionResult::SignFailed;
            }
        };

        let add_result = self.ports.proposal_cache.add_signature(
            deposit_id,
            crate::deposit::cache::SignatureData {
                signer_address: my_eth_address,
                signature: signature.clone(),
                proposal_hash,
                is_mine: true,
            },
            Some(req.clone()),
            |hash, sig| verify_bridge_signature(hash, sig, &self.node.valid_addresses),
        );

        if let Ok(report) = self
            .ports
            .proposal_cache
            .apply_pending_signatures(deposit_id, |hash, sig| {
                verify_bridge_signature(hash, sig, &self.node.valid_addresses)
            })
        {
            if report.applied > 0 {
                debug!(
                    target: "bridge.cursor",
                    proposal_hash=%proposal_id,
                    applied_count=report.applied,
                    "applied pending signatures from peers"
                );
            }
            if let Some(first) = report.mismatched.first() {
                let deposit_id_hex = hex::encode(deposit_id.to_bytes());
                let expected_hex = hex::encode(first.expected_hash);
                let received_hex = hex::encode(first.received_hash);
                warn!(
                    target: "bridge.cursor",
                    deposit_id=%deposit_id_hex,
                    expected_hash=%expected_hex,
                    received_hash=%received_hex,
                    signer=%first.signer_address,
                    mismatch_count=report.mismatched.len(),
                    "peer signature proposal hash mismatch, possible nonce divergence"
                );
                self.control.bridge_status.push_alert(
                    crate::observability::tui::types::AlertSeverity::Error,
                    "Nonce Divergence Suspected".to_string(),
                    format!(
                        "Deposit {} has {} peer signature(s) for a different proposal hash. expected={}, received={}, signer={}",
                        deposit_id_hex,
                        report.mismatched.len(),
                        expected_hex,
                        received_hex,
                        first.signer_address
                    ),
                    "nonce-divergence".to_string(),
                );
            }
        }

        if let Ok(Some(proposal_state)) = self.ports.proposal_cache.get_state(deposit_id) {
            self.control
                .bridge_status
                .sync_proposal_signatures_from_cache(
                    &proposal_id, &proposal_state, &self.node.address_to_node_id,
                    self.node.self_node_id,
                );
        }

        match add_result {
            Ok(crate::deposit::cache::SignatureAddResult::Added)
            | Ok(crate::deposit::cache::SignatureAddResult::ThresholdReached) => {}
            Ok(crate::deposit::cache::SignatureAddResult::Duplicate) => {
                debug!(
                    target: "bridge.cursor",
                    proposal_hash=%proposal_id,
                    "duplicate signature, skipping broadcast"
                );
                return SigningCandidateExecutionResult::DuplicateSignature;
            }
            Ok(crate::deposit::cache::SignatureAddResult::Stale) => {
                info!(
                    target: "bridge.cursor",
                    proposal_hash=%proposal_id,
                    "proposal already confirmed, skipping broadcast"
                );
                return SigningCandidateExecutionResult::ProposalStale;
            }
            Ok(crate::deposit::cache::SignatureAddResult::Invalid(msg)) => {
                warn!(
                    target: "bridge.cursor",
                    proposal_hash=%proposal_id,
                    error=%msg,
                    "own signature invalid, skipping broadcast"
                );
                return SigningCandidateExecutionResult::InvalidOwnSignature;
            }
            Err(e) => {
                warn!(
                    target: "bridge.cursor",
                    proposal_hash=%proposal_id,
                    error=%e,
                    "failed to add own signature to cache, skipping broadcast"
                );
                return SigningCandidateExecutionResult::CacheUpdateFailed;
            }
        }

        let broadcast_msg = DepositSignatureBroadcast {
            deposit_id: deposit_id.to_bytes(),
            proposal_hash: proposal_hash.to_vec(),
            signature,
            signer_address: my_eth_address.as_slice().to_vec(),
            timestamp: now_secs,
        };

        spawn_deposit_signature_broadcast(
            self.node.peers.as_ref(),
            &broadcast_msg,
            &proposal_id,
            DepositSignatureBroadcastReason::Initial,
        );
        SigningCandidateExecutionResult::Broadcasted
    }

    fn execute_regossip(&self, now_secs: u64) -> usize {
        use crate::shared::ingress::proto::DepositSignatureBroadcast;

        let my_eth_address = self.node.signer.address();
        let mut broadcasts = 0;
        match self.ports.proposal_cache.collecting_with_my_sig() {
            Ok(pending) => {
                for (deposit_id, proposal_state) in pending {
                    let Some(sig) = proposal_state.my_signature.clone() else {
                        continue;
                    };

                    let broadcast_msg = DepositSignatureBroadcast {
                        deposit_id: deposit_id.to_bytes(),
                        proposal_hash: proposal_state.proposal_hash.to_vec(),
                        signature: sig,
                        signer_address: my_eth_address.as_slice().to_vec(),
                        timestamp: now_secs,
                    };

                    let prop_id = hex::encode(proposal_state.proposal_hash);
                    spawn_deposit_signature_broadcast(
                        self.node.peers.as_ref(),
                        &broadcast_msg,
                        &prop_id,
                        DepositSignatureBroadcastReason::Regossip,
                    );
                    broadcasts += 1;
                }
            }
            Err(err) => {
                warn!(
                    target: "bridge.cursor",
                    error=%err,
                    "failed to gather proposals for signature re-gossip"
                );
            }
        }
        if broadcasts > 0 {
            debug!(
                target: "bridge.cursor",
                regossip_broadcasts=broadcasts,
                "re-gossiped collecting signatures"
            );
        }
        broadcasts
    }

    async fn trigger_local_stop(&self, reason: String) {
        use std::time::SystemTime;

        use crate::observability::tui::types::AlertSeverity;
        use crate::shared::stop::{trigger_local_stop, StopInfo, StopSource};

        info!(
            target: "bridge.cursor",
            mode = ?self.control.local_stop_mode,
            reason=%reason,
            "signing requested local stop"
        );

        match self.control.local_stop_mode {
            SigningLocalStopMode::RuntimeProbeAndBroadcast => {
                trigger_local_stop(
                    self.ports.runtime.clone(),
                    self.control.stop_controller.clone(),
                    self.control.bridge_status.clone(),
                    reason,
                )
                .await;
            }
            SigningLocalStopMode::LocalTriggerOnly => {
                let metrics = metrics::init_metrics();
                metrics.stop_local_requests.increment();

                let info = StopInfo {
                    reason: reason.clone(),
                    last: None,
                    source: StopSource::Local,
                    at: SystemTime::now(),
                };
                if !self.control.stop_controller.trigger(info) {
                    metrics.stop_local_duplicate.increment();
                    info!(
                        target: "bridge.cursor",
                        reason=%reason,
                        "local stop already active, skipping duplicate local-only trigger"
                    );
                    return;
                }
                metrics.stop_local_triggered.increment();
                info!(
                    target: "bridge.cursor",
                    reason=%reason,
                    "local-only stop activated"
                );
                self.control.bridge_status.push_alert(
                    AlertSeverity::Error,
                    "Bridge Stopped".to_string(),
                    reason,
                    "local-stop".to_string(),
                );
            }
        }
    }

    pub async fn tick_once(
        &self,
        state: &mut SigningTickState,
        input: SigningTickInput,
    ) -> SigningTickOutcome {
        let mut outcome = SigningTickOutcome::default();
        let now = input.now;
        let now_secs = system_time_secs(now);

        if now
            .duration_since(state.last_regossip_at)
            .unwrap_or_default()
            >= self.config.policy.regossip_interval
        {
            outcome.regossip_broadcasts += self.execute_regossip(now_secs);
            state.last_regossip_at = now;
        }

        let last_chain_nonce = match self.ports.base_bridge.get_last_deposit_nonce().await {
            Ok(nonce) => {
                self.control.bridge_status.update_last_deposit_nonce(nonce);
                Some(nonce)
            }
            Err(e) => {
                warn!(
                    target: "bridge.cursor",
                    error=%e,
                    "failed to query lastDepositNonce from chain"
                );
                None
            }
        };

        let nonce_epoch_base = self.config.nonce_epoch.base;
        let first_epoch_nonce = self.config.nonce_epoch.first_epoch_nonce();
        let mut epoch_ready_logged = false;
        let mut maybe_mark_epoch_ready = |tip_height: u64, state: &mut SigningTickState| {
            if epoch_ready_logged {
                return;
            }
            state.logged_epoch_ready = true;
            epoch_ready_logged = true;
            info!(
                target: "bridge.cursor",
                tip_height,
                nonce_epoch_start_height = self.config.nonce_epoch.start_height,
                "hashchain reached nonce epoch start height, signing enabled"
            );
        };
        let mut plan = SigningPlanner::plan_tick(SigningTickPlanInput {
            tip_height: input.tip_height,
            nonce_epoch_start_height: self.config.nonce_epoch.start_height,
            logged_epoch_ready: state.logged_epoch_ready,
            last_chain_nonce,
            nonce_epoch_base,
            first_epoch_nonce,
            log_len: None,
        });
        let next_nonce = loop {
            match plan {
                SigningTickPlanAction::WaitForTip => {
                    debug!(
                        target: "bridge.cursor",
                        "no nock hashchain tip yet, waiting before signing"
                    );
                    return outcome;
                }
                SigningTickPlanAction::WaitForEpochStart { tip_height } => {
                    debug!(
                        target: "bridge.cursor",
                        tip_height,
                        nonce_epoch_start_height = self.config.nonce_epoch.start_height,
                        "hashchain behind nonce epoch start height, waiting to sign"
                    );
                    return outcome;
                }
                SigningTickPlanAction::NeedLastChainNonce {
                    tip_height,
                    reached_epoch_start,
                } => {
                    if reached_epoch_start {
                        maybe_mark_epoch_ready(tip_height, state);
                    }
                    return outcome;
                }
                SigningTickPlanAction::StopNonceEpochMismatch {
                    tip_height,
                    reached_epoch_start,
                    last_chain_nonce,
                    nonce_epoch_base,
                } => {
                    if reached_epoch_start {
                        maybe_mark_epoch_ready(tip_height, state);
                    }
                    let reason = format!(
                        "nonce epoch mismatch: nonce_epoch_base ({nonce_epoch_base}) is greater than on-chain lastDepositNonce ({last_chain_nonce}); check config"
                    );
                    self.trigger_local_stop(reason).await;
                    return outcome;
                }
                SigningTickPlanAction::NeedLogLen {
                    tip_height,
                    reached_epoch_start,
                    last_chain_nonce,
                } => {
                    if reached_epoch_start {
                        maybe_mark_epoch_ready(tip_height, state);
                    }

                    let log_len = match self
                        .ports
                        .deposit_log
                        .number_of_deposits_in_epoch(&self.config.nonce_epoch)
                        .await
                    {
                        Ok(v) => v,
                        Err(err) => {
                            warn!(
                                target: "bridge.cursor",
                                error=%err,
                                "failed to count deposits in sqlite"
                            );
                            let reason = format!("failed to count deposits in sqlite: {err}");
                            self.trigger_local_stop(reason).await;
                            return outcome;
                        }
                    };

                    plan = SigningPlanner::plan_tick(SigningTickPlanInput {
                        tip_height: input.tip_height,
                        nonce_epoch_start_height: self.config.nonce_epoch.start_height,
                        logged_epoch_ready: state.logged_epoch_ready,
                        last_chain_nonce: Some(last_chain_nonce),
                        nonce_epoch_base,
                        first_epoch_nonce,
                        log_len: Some(log_len),
                    });
                }
                SigningTickPlanAction::WaitForLogCatchup {
                    tip_height,
                    reached_epoch_start,
                    nonce_epoch_base,
                    log_len,
                    spent_epoch_nonces,
                    ..
                } => {
                    if reached_epoch_start {
                        maybe_mark_epoch_ready(tip_height, state);
                    }
                    debug!(
                        target: "bridge.cursor",
                        log_len,
                        spent_epoch_nonces,
                        nonce_epoch_base,
                        "deposit log behind chain prefix, waiting for log to catch up"
                    );
                    return outcome;
                }
                SigningTickPlanAction::Continue {
                    tip_height,
                    reached_epoch_start,
                    next_nonce,
                    ..
                } => {
                    if reached_epoch_start {
                        maybe_mark_epoch_ready(tip_height, state);
                    }
                    break next_nonce;
                }
            }
        };

        let candidates = match self
            .ports
            .deposit_log
            .records_from_nonce(
                next_nonce, self.config.policy.pipeline_depth, &self.config.nonce_epoch,
            )
            .await
        {
            Ok(v) => v,
            Err(err) => {
                warn!(
                    target: "bridge.cursor",
                    error=%err,
                    "failed to query candidate deposits from sqlite"
                );
                return outcome;
            }
        };

        if candidates.is_empty() {
            return outcome;
        }

        for (nonce, record) in candidates {
            if self.control.stop.is_stopped() {
                break;
            }

            if matches!(
                SigningPlanner::plan_epoch_bounds(SigningEpochBoundsDecisionInput {
                    is_before_start_key: self
                        .config
                        .nonce_epoch
                        .is_before_start_key(record.block_height, &record.tx_id),
                }),
                SigningEpochBoundsDecision::StopRecordBeforeStart
            ) {
                let reason = format!(
                    "signing candidate is before nonce_epoch start key (record_height={}, start_height={}); candidate should have been filtered before signing",
                    record.block_height, self.config.nonce_epoch.start_height
                );
                self.trigger_local_stop(reason).await;
                break;
            }

            let req = NockDepositRequestData {
                tx_id: record.tx_id.clone(),
                name: record.name.clone(),
                recipient: record.recipient,
                amount: record.amount_to_mint,
                block_height: record.block_height,
                as_of: record.as_of.clone(),
                nonce,
            };

            let deposit_id = DepositId::from_effect_payload(&req);

            let existing_state = self
                .ports
                .proposal_cache
                .get_state(&deposit_id)
                .ok()
                .flatten();
            let precheck = SigningPlanner::plan_candidate_precheck(SigningCandidatePrecheckInput {
                is_confirmed: existing_state
                    .as_ref()
                    .map(|proposal_state| proposal_state.status == ProposalStatus::Confirmed)
                    .unwrap_or(false),
                has_my_signature: existing_state
                    .as_ref()
                    .and_then(|proposal_state| proposal_state.my_signature.as_ref())
                    .is_some(),
            });
            match precheck {
                SigningCandidatePrecheckDecision::SkipConfirmed
                | SigningCandidatePrecheckDecision::SkipAlreadySigned => {
                    continue;
                }
                SigningCandidatePrecheckDecision::CheckProcessedOnChain => {}
            }

            match self
                .ports
                .base_bridge
                .is_deposit_processed(&req.tx_id)
                .await
            {
                Ok(processed_on_chain) => {
                    if matches!(
                        SigningPlanner::plan_processed(SigningProcessedDecisionInput {
                            processed_on_chain,
                        }),
                        SigningProcessedDecision::SkipProcessed
                    ) {
                        debug!(
                            target: "bridge.cursor",
                            nonce,
                            "deposit already processed on-chain, skipping signature"
                        );
                        continue;
                    }
                }
                Err(e) => {
                    warn!(
                        target: "bridge.cursor",
                        nonce=req.nonce,
                        error=%e,
                        "failed to query processedDeposits, proceeding to sign anyway"
                    );
                }
            }

            if matches!(
                self.execute_signing_candidate(&req, &deposit_id, now, now_secs)
                    .await,
                SigningCandidateExecutionResult::Broadcasted
            ) {
                outcome.initial_broadcasts += 1;
            }
        }

        outcome
    }
}

pub async fn signing_tick_once<B: BaseContractPort>(
    context: &SigningTickContext<B>,
    state: &mut SigningTickState,
    input: SigningTickInput,
) -> SigningTickOutcome {
    context.tick_once(state, input).await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_signing_cursor_loop<B: BaseContractPort>(
    runtime: Arc<BridgeRuntimeHandle>,
    base_bridge: Arc<B>,
    deposit_log: Arc<DepositLog>,
    nonce_epoch: &NonceEpochConfig,
    proposal_cache: Arc<ProposalCache>,
    signer: Arc<BridgeSigner>,
    valid_addresses: HashSet<Address>,
    peers: Vec<PeerEndpoint>,
    self_node_id: u64,
    bridge_status: BridgeStatus,
    address_to_node_id: HashMap<Address, u64>,
    stop_controller: StopController,
    stop: StopHandle,
) {
    run_signing_cursor_loop_with_policy(
        runtime,
        base_bridge,
        deposit_log,
        nonce_epoch,
        proposal_cache,
        signer,
        valid_addresses,
        peers,
        self_node_id,
        bridge_status,
        address_to_node_id,
        stop_controller,
        stop,
        SigningLoopPolicy::default(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_signing_cursor_loop_with_policy<B: BaseContractPort>(
    runtime: Arc<BridgeRuntimeHandle>,
    base_bridge: Arc<B>,
    deposit_log: Arc<DepositLog>,
    nonce_epoch: &NonceEpochConfig,
    proposal_cache: Arc<ProposalCache>,
    signer: Arc<BridgeSigner>,
    valid_addresses: HashSet<Address>,
    peers: Vec<PeerEndpoint>,
    self_node_id: u64,
    bridge_status: BridgeStatus,
    address_to_node_id: HashMap<Address, u64>,
    stop_controller: StopController,
    stop: StopHandle,
    policy: SigningLoopPolicy,
) {
    use tokio::time::{interval, MissedTickBehavior};

    info!(
        target: "bridge.cursor",
        poll_interval_secs=policy.poll_interval.as_secs(),
        pipeline_depth=policy.pipeline_depth,
        regossip_interval_secs=policy.regossip_interval.as_secs(),
        "starting signing cursor loop"
    );

    let context = SigningTickContext::new(
        SigningTickPorts::new(runtime.clone(), base_bridge, deposit_log, proposal_cache),
        SigningTickNodeState::new(
            signer, valid_addresses, peers, self_node_id, address_to_node_id,
        ),
        SigningTickControl::new(bridge_status, stop_controller, stop.clone()),
        SigningTickConfig::new(nonce_epoch, policy),
    );

    let mut ticker = interval(policy.poll_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut state = SigningTickState::new(SystemTime::now());

    loop {
        ticker.tick().await;

        if stop.is_stopped() {
            continue;
        }

        let tip_height = match runtime.nock_hashchain_tip().await {
            Ok(height) => height,
            Err(err) => {
                warn!(
                    target: "bridge.cursor",
                    error=%err,
                    "failed to peek nock hashchain tip height"
                );
                continue;
            }
        };
        let _ = signing_tick_once(
            &context,
            &mut state,
            SigningTickInput {
                now: SystemTime::now(),
                tip_height,
            },
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_source_tx_id_uses_base58() {
        let tx_id = Tip5Hash::from_limbs(&[1, 2, 3, 4, 5]);
        let expected = tx_id.to_base58();
        assert_eq!(format_source_tx_id(&tx_id), expected);
        assert_ne!(format!("{:?}", tx_id), expected);
    }
}
