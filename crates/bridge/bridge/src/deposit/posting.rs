use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::core::deposit::posting::{
    PostingCandidateDecision, PostingPlanner, PostingReadyProposal, PostingTickPlanInput,
};
use crate::core::loop_policy::PostingLoopPolicy;
use crate::deposit::cache::{ProposalCache, ProposalState};
use crate::deposit::ports::BaseContractPort;
use crate::deposit::types::{DepositId, DepositSubmission, SignatureSet};
use crate::observability::metrics;
use crate::observability::status::{BridgeStatus, BridgeStatusState, LastSubmittedDeposit};
use crate::shared::types::NodeConfig;

const SUBMIT_DEPOSIT_TIMEOUT_SECS: u64 = 60;

fn system_time_secs(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn update_proposal_cache_metrics(proposal_cache: &ProposalCache) {
    let metrics = metrics::init_metrics();
    let snapshot = match proposal_cache.metrics_snapshot() {
        Ok(snapshot) => snapshot,
        Err(_) => {
            metrics.proposal_cache_metrics_update_error.increment();
            return;
        }
    };

    metrics
        .proposal_cache_total
        .swap(snapshot.proposal_total as f64);
    metrics
        .proposal_cache_collecting
        .swap(snapshot.collecting as f64);
    metrics.proposal_cache_ready.swap(snapshot.ready as f64);
    metrics.proposal_cache_posting.swap(snapshot.posting as f64);
    metrics
        .proposal_cache_confirmed
        .swap(snapshot.confirmed as f64);
    metrics.proposal_cache_failed.swap(snapshot.failed as f64);
    metrics
        .proposal_cache_total_peer_signatures
        .swap(snapshot.total_peer_signatures as f64);
    metrics
        .proposal_cache_max_peer_signatures_per_proposal
        .swap(snapshot.max_peer_signatures_per_proposal as f64);
    metrics
        .proposal_cache_proposals_with_my_signature
        .swap(snapshot.proposals_with_my_signature as f64);
    metrics
        .proposal_cache_pending_signature_deposit_count
        .swap(snapshot.pending_signature_deposit_count as f64);
    metrics
        .proposal_cache_pending_signature_total
        .swap(snapshot.pending_signature_total as f64);
    metrics
        .proposal_cache_oldest_age_secs
        .swap(snapshot.oldest_age_secs as f64);
    metrics
        .proposal_cache_oldest_confirmed_age_secs
        .swap(snapshot.oldest_confirmed_age_secs as f64);
    metrics
        .proposal_cache_oldest_failed_age_secs
        .swap(snapshot.oldest_failed_age_secs as f64);
    metrics
        .proposal_cache_pending_oldest_age_secs
        .swap(snapshot.pending_oldest_age_secs as f64);
    metrics
        .proposal_cache_approx_state_bytes
        .swap(snapshot.approx_state_bytes as f64);
    metrics
        .proposal_cache_approx_peer_signature_bytes
        .swap(snapshot.approx_peer_signature_bytes as f64);
    metrics
        .proposal_cache_approx_my_signature_bytes
        .swap(snapshot.approx_my_signature_bytes as f64);
    metrics
        .proposal_cache_approx_pending_signature_bytes
        .swap(snapshot.approx_pending_signature_bytes as f64);
    metrics
        .proposal_cache_approx_total_bytes
        .swap(snapshot.approx_total_bytes as f64);
}

fn spawn_deposit_confirmation_broadcast(
    peers: &[(u64, String)],
    msg: &crate::shared::ingress::proto::DepositConfirmationBroadcast,
    proposal_id: &str,
) {
    use tracing::{info, warn};

    use crate::shared::ingress::proto::bridge_ingress_client::BridgeIngressClient;

    for (peer_node_id, peer_address) in peers {
        let msg = msg.clone();
        let addr = peer_address.clone();
        let peer_id = *peer_node_id;
        let prop_id = proposal_id.to_string();

        tokio::spawn(async move {
            match BridgeIngressClient::connect(addr.clone()).await {
                Ok(mut client) => match client.broadcast_deposit_confirmation(msg.clone()).await {
                    Ok(_) => {
                        info!(
                            target: "bridge.posting",
                            peer_node_id=peer_id,
                            proposal_hash=%prop_id,
                            "broadcast confirmation to peer"
                        );
                    }
                    Err(e) if e.code() == tonic::Code::Unimplemented => {
                        let legacy_msg = crate::shared::ingress::proto::ConfirmationBroadcast {
                            deposit_id: msg.deposit_id,
                            proposal_hash: msg.proposal_hash,
                            tx_hash: msg.tx_hash,
                            block_number: msg.block_number,
                            timestamp: msg.timestamp,
                        };

                        match client.broadcast_confirmation(legacy_msg).await {
                            Ok(_) => {
                                info!(
                                    target: "bridge.posting",
                                    peer_node_id=peer_id,
                                    proposal_hash=%prop_id,
                                    "broadcast confirmation to peer via legacy RPC"
                                );
                            }
                            Err(e) => {
                                warn!(
                                    target: "bridge.posting",
                                    peer_node_id=peer_id,
                                    error=%e,
                                    "failed to broadcast confirmation to peer via legacy RPC"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            target: "bridge.posting",
                            peer_node_id=peer_id,
                            error=%e,
                            "failed to broadcast confirmation to peer"
                        );
                    }
                },
                Err(e) => {
                    warn!(
                        target: "bridge.posting",
                        peer_node_id=peer_id,
                        peer_address=%addr,
                        error=%e,
                        "failed to connect to peer for confirmation broadcast"
                    );
                }
            }
        });
    }
}

#[derive(Clone, Debug, Default)]
pub struct PostingTickState {
    pub ticks_executed: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct PostingTickInput {
    pub now: SystemTime,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PostingTickOutcome {
    pub ready_proposals: usize,
    pub submitted: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PostingSubmissionExecutionResult {
    Submitted,
    ProposalNoLongerReady,
    SignatureFetchFailed,
    MarkPostingFailed,
    SubmitFailed,
    SubmitTimedOut,
}

#[derive(Clone)]
pub struct PostingTickPorts<B: BaseContractPort> {
    proposal_cache: Arc<ProposalCache>,
    base_bridge: Arc<B>,
}

impl<B: BaseContractPort> PostingTickPorts<B> {
    pub fn new(proposal_cache: Arc<ProposalCache>, base_bridge: Arc<B>) -> Self {
        Self {
            proposal_cache,
            base_bridge,
        }
    }
}

#[derive(Clone)]
pub struct PostingTickNodeState {
    node_config: NodeConfig,
    peers: Arc<Vec<(u64, String)>>,
    my_node_id: usize,
}

impl PostingTickNodeState {
    pub fn new(node_config: NodeConfig) -> Self {
        let my_node_id = node_config.node_id as usize;
        let peers: Vec<(u64, String)> = node_config
            .nodes
            .iter()
            .enumerate()
            .filter(|(idx, _)| *idx != my_node_id)
            .map(|(idx, node)| {
                (
                    idx as u64,
                    crate::observability::health::normalize_endpoint(&node.ip),
                )
            })
            .collect();
        Self {
            node_config,
            peers: Arc::new(peers),
            my_node_id,
        }
    }
}

#[derive(Clone)]
pub struct PostingTickControl {
    bridge_status: BridgeStatus,
    status_state: BridgeStatusState,
}

impl PostingTickControl {
    pub fn new(bridge_status: BridgeStatus, status_state: BridgeStatusState) -> Self {
        Self {
            bridge_status,
            status_state,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PostingTickConfig {
    failover_backoff_secs: u64,
}

impl PostingTickConfig {
    pub fn new(failover_backoff_secs: u64) -> Self {
        Self {
            failover_backoff_secs,
        }
    }
}

#[derive(Clone)]
pub struct PostingTickContext<B: BaseContractPort> {
    ports: PostingTickPorts<B>,
    node: PostingTickNodeState,
    control: PostingTickControl,
    config: PostingTickConfig,
}

impl<B: BaseContractPort> PostingTickContext<B> {
    pub fn new(
        ports: PostingTickPorts<B>,
        node: PostingTickNodeState,
        control: PostingTickControl,
        config: PostingTickConfig,
    ) -> Self {
        Self {
            ports,
            node,
            control,
            config,
        }
    }

    async fn execute_submission(
        &self,
        deposit_id: &DepositId,
        proposal_state: &ProposalState,
        proposal_hash: [u8; 32],
        proposal_id: &str,
        now: SystemTime,
        now_secs: u64,
    ) -> PostingSubmissionExecutionResult {
        use serde_bytes::ByteBuf;
        use tracing::{error, info, warn};

        use crate::observability::tui::types::{AlertSeverity, BatchStatus, ProposalStatus};
        use crate::shared::ingress::proto::DepositConfirmationBroadcast;

        let signatures = match self
            .ports
            .proposal_cache
            .get_signatures_for_posting(deposit_id)
        {
            Ok(Some(sigs)) => sigs,
            Ok(None) => {
                warn!(
                    target: "bridge.posting",
                    proposal_hash=%proposal_id,
                    "proposal no longer ready for posting"
                );
                return PostingSubmissionExecutionResult::ProposalNoLongerReady;
            }
            Err(e) => {
                error!(
                    target: "bridge.posting",
                    error=%e,
                    proposal_hash=%proposal_id,
                    "failed to get signatures for posting"
                );
                let _ = self.ports.proposal_cache.mark_failed(deposit_id);
                return PostingSubmissionExecutionResult::SignatureFetchFailed;
            }
        };

        if let Err(e) = self.ports.proposal_cache.mark_posting(deposit_id) {
            error!(
                target: "bridge.posting",
                error=%e,
                proposal_hash=%proposal_id,
                "failed to mark proposal as posting"
            );
            return PostingSubmissionExecutionResult::MarkPostingFailed;
        }

        self.control
            .bridge_status
            .update_batch_status(BatchStatus::Submitting {
                batch_id: proposal_state.proposal.nonce,
            });

        info!(
            target: "bridge.posting",
            proposal_hash=%proposal_id,
            "posting proposal to BASE"
        );

        let req = &proposal_state.proposal;
        let mut recipient_bytes = [0u8; 20];
        recipient_bytes.copy_from_slice(&req.recipient.0);

        let submission = DepositSubmission {
            tx_id: req.tx_id.clone(),
            name_first: req.name.first.clone(),
            name_last: req.name.last.clone(),
            recipient: recipient_bytes,
            amount: req.amount as u128,
            block_height: req.block_height,
            as_of: req.as_of.clone(),
            nonce: req.nonce,
            signatures: SignatureSet {
                eth_signatures: signatures.into_iter().map(ByteBuf::from).collect(),
                nock_signatures: vec![],
            },
        };

        if let Some(mut proposal) = self.control.bridge_status.find_proposal(proposal_id) {
            proposal.status = ProposalStatus::Submitted;
            proposal.submitted_at = Some(now);
            if let Ok(duration) = now.duration_since(proposal.created_at) {
                proposal.time_to_submit_ms = Some(duration.as_millis() as u64);
            }
            self.control.bridge_status.update_proposal(proposal);
        }

        match tokio::time::timeout(
            Duration::from_secs(SUBMIT_DEPOSIT_TIMEOUT_SECS),
            self.ports.base_bridge.submit_deposit(submission),
        )
        .await
        {
            Ok(Ok(result)) => {
                info!(
                    target: "bridge.posting",
                    proposal_hash=%proposal_id,
                    tx_hash=%result.tx_hash,
                    block_number=%result.block_number,
                    "successfully posted deposit to BASE"
                );

                self.control
                    .status_state
                    .update_last_submitted_deposit(LastSubmittedDeposit {
                        deposit: proposal_state.proposal.clone(),
                        base_tx_hash: result.tx_hash.clone(),
                        base_block_number: result.block_number,
                    });

                let _ = self.ports.proposal_cache.mark_confirmed(deposit_id);

                let confirmation_msg = DepositConfirmationBroadcast {
                    deposit_id: deposit_id.to_bytes(),
                    proposal_hash: proposal_hash.to_vec(),
                    tx_hash: result.tx_hash.as_bytes().to_vec(),
                    block_number: result.block_number,
                    timestamp: now_secs,
                };
                spawn_deposit_confirmation_broadcast(
                    self.node.peers.as_ref(),
                    &confirmation_msg,
                    proposal_id,
                );

                if let Some(mut proposal) = self.control.bridge_status.find_proposal(proposal_id) {
                    proposal.status = ProposalStatus::Executed;
                    proposal.tx_hash = Some(result.tx_hash);
                    proposal.submitted_at_block = Some(result.block_number);
                    proposal.executed_at_block = Some(result.block_number);
                    self.control.bridge_status.update_proposal(proposal);
                }

                self.control
                    .bridge_status
                    .update_batch_status(BatchStatus::Idle);
                PostingSubmissionExecutionResult::Submitted
            }
            Ok(Err(e)) => {
                error!(
                    target: "bridge.posting",
                    error=%e,
                    proposal_hash=%proposal_id,
                    "failed to post deposit to BASE"
                );

                let _ = self.ports.proposal_cache.mark_failed(deposit_id);

                if let Some(mut proposal) = self.control.bridge_status.find_proposal(proposal_id) {
                    proposal.status = ProposalStatus::Failed {
                        reason: format!("BASE submission failed: {}", e),
                    };
                    self.control.bridge_status.update_proposal(proposal);
                }

                self.control.bridge_status.push_alert(
                    AlertSeverity::Error,
                    "Proposal Failed".to_string(),
                    format!("Failed to post deposit {}: {}", proposal_id, e),
                    "posting-loop".to_string(),
                );

                self.control
                    .bridge_status
                    .update_batch_status(BatchStatus::Idle);
                PostingSubmissionExecutionResult::SubmitFailed
            }
            Err(_) => {
                error!(
                    target: "bridge.posting",
                    proposal_hash=%proposal_id,
                    timeout_secs=SUBMIT_DEPOSIT_TIMEOUT_SECS,
                    "posting to BASE timed out"
                );

                let _ = self.ports.proposal_cache.mark_failed(deposit_id);

                if let Some(mut proposal) = self.control.bridge_status.find_proposal(proposal_id) {
                    proposal.status = ProposalStatus::Failed {
                        reason: format!(
                            "BASE submission timed out after {}s",
                            SUBMIT_DEPOSIT_TIMEOUT_SECS
                        ),
                    };
                    self.control.bridge_status.update_proposal(proposal);
                }

                self.control.bridge_status.push_alert(
                    AlertSeverity::Error,
                    "Proposal Failed".to_string(),
                    format!(
                        "Failed to post deposit {}: timed out after {}s",
                        proposal_id, SUBMIT_DEPOSIT_TIMEOUT_SECS
                    ),
                    "posting-loop".to_string(),
                );

                self.control
                    .bridge_status
                    .update_batch_status(BatchStatus::Idle);
                PostingSubmissionExecutionResult::SubmitTimedOut
            }
        }
    }

    pub async fn tick_once(
        &self,
        state: &mut PostingTickState,
        input: PostingTickInput,
    ) -> PostingTickOutcome {
        use tracing::{debug, error, info};

        use crate::shared::proposer::active_proposer;

        state.ticks_executed = state.ticks_executed.saturating_add(1);
        let mut outcome = PostingTickOutcome::default();

        let ready_proposals = match self.ports.proposal_cache.ready_proposals() {
            Ok(proposals) => proposals,
            Err(e) => {
                error!(target: "bridge.posting", error=%e, "failed to fetch ready proposals");
                return outcome;
            }
        };

        if ready_proposals.is_empty() {
            return outcome;
        }
        outcome.ready_proposals = ready_proposals.len();

        let last_chain_nonce = match self.ports.base_bridge.get_last_deposit_nonce().await {
            Ok(n) => n,
            Err(e) => {
                error!(target: "bridge.posting", error=%e, "failed to query lastDepositNonce from chain");
                return outcome;
            }
        };
        self.control
            .bridge_status
            .update_last_deposit_nonce(last_chain_nonce);
        let next_nonce = last_chain_nonce + 1;

        debug!(
            target: "bridge.posting",
            last_chain_nonce=last_chain_nonce,
            next_nonce=next_nonce,
            "queried chain for deposit nonce"
        );

        let now_secs = system_time_secs(input.now);
        let current_height = ready_proposals
            .first()
            .map(|(_, proposal_state)| proposal_state.proposal.block_height)
            .unwrap_or(0);
        let node_pkhs: Vec<_> = self
            .node
            .node_config
            .nodes
            .iter()
            .map(|node| node.nock_pkh.clone())
            .collect();
        let num_nodes = node_pkhs.len();
        let current_proposer = active_proposer(current_height, &node_pkhs);

        debug!(
            target: "bridge.posting",
            ready_count=ready_proposals.len(),
            current_height=current_height,
            current_proposer=current_proposer,
            my_node_id=self.node.my_node_id,
            "checking ready proposals"
        );

        let decisions = PostingPlanner::plan_tick(
            PostingTickPlanInput {
                next_nonce,
                my_node_id: self.node.my_node_id,
                current_proposer,
                num_nodes,
                now_secs,
                failover_backoff_secs: self.config.failover_backoff_secs,
            },
            &ready_proposals
                .iter()
                .map(|(_, proposal_state)| PostingReadyProposal {
                    nonce: proposal_state.proposal.nonce,
                    ready_at: proposal_state.ready_at,
                })
                .collect::<Vec<_>>(),
        );

        for ((deposit_id, proposal_state), decision) in ready_proposals.into_iter().zip(decisions) {
            let proposal_hash = proposal_state.proposal_hash;
            let proposal_id = hex::encode(proposal_hash);

            let is_proposer = match decision {
                PostingCandidateDecision::MarkConfirmedOnChain => {
                    debug!(
                        target: "bridge.posting",
                        proposal_hash=%proposal_id,
                        nonce=proposal_state.proposal.nonce,
                        last_chain_nonce=last_chain_nonce,
                        "proposal already confirmed on chain, marking confirmed"
                    );
                    let _ = self.ports.proposal_cache.mark_confirmed(&deposit_id);
                    continue;
                }
                PostingCandidateDecision::WaitForEarlierNonce => {
                    debug!(
                        target: "bridge.posting",
                        proposal_hash=%proposal_id,
                        nonce=proposal_state.proposal.nonce,
                        next_nonce=next_nonce,
                        "waiting for nonce {} to be ready before posting {}",
                        next_nonce,
                        proposal_state.proposal.nonce
                    );
                    continue;
                }
                PostingCandidateDecision::NotMyTurn => {
                    debug!(
                        target: "bridge.posting",
                        proposal_hash=%proposal_id,
                        current_proposer=current_proposer,
                        my_node_id=self.node.my_node_id,
                        "not my turn to post, waiting for proposer or failover"
                    );
                    continue;
                }
                PostingCandidateDecision::Submit { is_proposer } => is_proposer,
            };

            info!(
                target: "bridge.posting",
                proposal_hash=%proposal_id,
                current_proposer=current_proposer,
                my_node_id=self.node.my_node_id,
                is_proposer=is_proposer,
                "posting proposal to BASE"
            );

            if matches!(
                self.execute_submission(
                    &deposit_id, &proposal_state, proposal_hash, &proposal_id, input.now, now_secs,
                )
                .await,
                PostingSubmissionExecutionResult::Submitted
            ) {
                outcome.submitted += 1;
            }
        }

        outcome
    }
}

pub async fn posting_tick_once<B: BaseContractPort>(
    context: &PostingTickContext<B>,
    state: &mut PostingTickState,
    input: PostingTickInput,
) -> PostingTickOutcome {
    context.tick_once(state, input).await
}

pub async fn run_posting_loop<B: BaseContractPort>(
    proposal_cache: Arc<ProposalCache>,
    base_bridge: Arc<B>,
    node_config: NodeConfig,
    bridge_status: BridgeStatus,
    stop: crate::shared::stop::StopHandle,
    status_state: BridgeStatusState,
) {
    run_posting_loop_with_policy(
        proposal_cache,
        base_bridge,
        node_config,
        bridge_status,
        stop,
        status_state,
        PostingLoopPolicy::default(),
    )
    .await
}

pub async fn run_posting_loop_with_policy<B: BaseContractPort>(
    proposal_cache: Arc<ProposalCache>,
    base_bridge: Arc<B>,
    node_config: NodeConfig,
    bridge_status: BridgeStatus,
    stop: crate::shared::stop::StopHandle,
    status_state: BridgeStatusState,
    policy: PostingLoopPolicy,
) {
    use tracing::info;

    info!("Starting proposal posting loop");
    let context = PostingTickContext::new(
        PostingTickPorts::new(proposal_cache, base_bridge),
        PostingTickNodeState::new(node_config),
        PostingTickControl::new(bridge_status, status_state),
        PostingTickConfig::new(policy.failover_backoff_secs),
    );
    let mut state = PostingTickState::default();

    loop {
        tokio::time::sleep(policy.tick_interval).await;
        update_proposal_cache_metrics(&context.ports.proposal_cache);
        if stop.is_stopped() {
            continue;
        }

        let _ = posting_tick_once(
            &context,
            &mut state,
            PostingTickInput {
                now: SystemTime::now(),
            },
        )
        .await;
    }
}
