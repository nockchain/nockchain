use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use alloy::primitives::Address;
use async_trait::async_trait;
use backon::Retryable;
use tonic::{Request, Response, Status};
use tonic_reflection::server::Builder as ReflectionBuilder;

use crate::core::loop_policy::RetryPolicy;
use crate::observability::metrics;
use crate::shared::base::is_explicitly_refunded_withdrawal_base_event_id;
use crate::shared::errors::BridgeError;
use crate::shared::ingress::proto::withdrawal_sequencer_server::{
    WithdrawalSequencer, WithdrawalSequencerServer,
};
use crate::shared::ingress::proto::{
    CurrentLiveWithdrawalNonceRequest, CurrentLiveWithdrawalNonceResponse,
    NextPendingWithdrawalOrderingRequest, NextPendingWithdrawalOrderingResponse,
    SequencedWithdrawalStatusRequest, SequencedWithdrawalStatusResponse,
    SequencerAdvancePrecanonicalHandoffRequest, SequencerAuthorizeProposalRequest,
    SequencerCanonicalProposalArtifactsRequest, SequencerCanonicalProposalArtifactsResponse,
    SequencerFrontierAllowsWithdrawalRequest, SequencerFrontierAllowsWithdrawalResponse,
    SequencerRecordCanonicalRequest, SequencerRecordSignedProposalRequest,
    SequencerRegisterWithdrawalRequest, SequencerReservedWithdrawalInputsRequest,
    SequencerReservedWithdrawalInputsResponse, SequencerSubmitProposalRequest,
    SequencerUpdateResponse,
};
use crate::shared::proposer::withdrawal_turn_proposer;
use crate::shared::types::{zero_tip5_hash, BaseEventId, Tip5Hash};
use crate::withdrawal::proposals::TrackedWithdrawalRequest;
use crate::withdrawal::sequencer::approval::{
    check_manual_submit_approval, ManualSubmitApprovalConfig, ManualSubmitApprovalDecision,
};
use crate::withdrawal::sequencer::base_height::SequencerBaseHeightTracker;
use crate::withdrawal::sequencer::base_verifier::{
    sequencer_base_event_id_hex, SequencerBaseWithdrawalRejection as BaseWithdrawalRejection,
    SequencerBaseWithdrawalVerifier,
};
use crate::withdrawal::sequencer::store::{
    jam_transaction, validate_canonical_proposal_tx_inputs, SequencerDecision,
    WithdrawalSequencerStore,
};
use crate::withdrawal::state::{LiveWithdrawalView, WithdrawalState};
use crate::withdrawal::submission::{
    default_withdrawal_submit_retry_policy, transaction_is_fully_signed,
    WithdrawalSequencerOrphanRetryLoopPolicy, WithdrawalSubmitAttemptStatus, WithdrawalSubmitPort,
    WITHDRAWAL_SUBMIT_DEFERRED_PREFIX,
};
use crate::withdrawal::transport::{
    note_name_to_proto, proposal_from_proto, required_withdrawal_commit_signature_threshold,
    verify_withdrawal_commit_certificate, withdrawal_id_from_proto, withdrawal_id_to_proto,
};
use crate::withdrawal::types::{WithdrawalProposalData, WithdrawalSequencerProposalArtifacts};

pub struct WithdrawalSequencerRpcService {
    withdrawal_state_store: Arc<WithdrawalSequencerStore>,
    submitter: Arc<dyn WithdrawalSubmitPort>,
    submit_retry_policy: RetryPolicy,
    authorized_submit_retry_after_base_blocks: u64,
    base_height_tracker: Arc<SequencerBaseHeightTracker>,
    base_withdrawal_verifier: Arc<dyn SequencerBaseWithdrawalVerifier>,
    handoff_window_blocks: u64,
    node_pkhs: Vec<nockchain_types::tx_engine::common::Hash>,
    node_eth_addresses: HashMap<u64, Address>,
    manual_submit_approval: ManualSubmitApprovalConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BoundedSubmitOutcome {
    final_state: WithdrawalState,
    submit_attempt_count: u64,
    last_submit_attempt_base_height: u64,
    last_submit_error: Option<String>,
}

fn registered_withdrawal_matches_tracked(
    existing: &LiveWithdrawalView,
    tracked: &TrackedWithdrawalRequest,
) -> bool {
    existing.withdrawal_nonce == Some(tracked.withdrawal_nonce)
        && existing.id.base_event_id == tracked.id.base_event_id
        && existing.recipient.as_ref() == Some(&tracked.recipient)
        && existing.gross_burned_amount == Some(tracked.amount)
        && existing.base_batch_end == Some(tracked.base_batch_end)
}

fn base_event_id_from_proto(bytes: &[u8], context: &str) -> Result<BaseEventId, Status> {
    if bytes.len() != 32 {
        return Err(Status::invalid_argument(format!(
            "{context} base_event_id must be 32 bytes, got {}",
            bytes.len()
        )));
    }
    Ok(BaseEventId(bytes.to_vec()))
}

impl WithdrawalSequencerRpcService {
    /// Builds the API-node-hosted sequencer RPC service.
    pub fn new(
        withdrawal_state_store: Arc<WithdrawalSequencerStore>,
        submitter: Arc<dyn WithdrawalSubmitPort>,
        base_height_tracker: Arc<SequencerBaseHeightTracker>,
        base_withdrawal_verifier: Arc<dyn SequencerBaseWithdrawalVerifier>,
        handoff_window_blocks: u64,
        node_pkhs: Vec<nockchain_types::tx_engine::common::Hash>,
        node_eth_addresses: HashMap<u64, Address>,
    ) -> Self {
        Self::with_submit_retry_policy(
            withdrawal_state_store,
            submitter,
            base_height_tracker,
            base_withdrawal_verifier,
            handoff_window_blocks,
            node_pkhs,
            node_eth_addresses,
            default_withdrawal_submit_retry_policy(),
        )
    }

    pub fn with_authorized_submit_retry_after_base_blocks(
        mut self,
        authorized_submit_retry_after_base_blocks: u64,
    ) -> Self {
        self.authorized_submit_retry_after_base_blocks = authorized_submit_retry_after_base_blocks;
        self
    }

    pub fn with_base_withdrawal_verifier(
        mut self,
        verifier: Arc<dyn SequencerBaseWithdrawalVerifier>,
    ) -> Self {
        self.base_withdrawal_verifier = verifier;
        self
    }

    pub fn with_manual_submit_approval(mut self, config: ManualSubmitApprovalConfig) -> Self {
        self.manual_submit_approval = config;
        self
    }

    /// Builds the sequencer RPC service with an explicit bounded submit retry
    /// policy, primarily for tests that need deterministic retry timing.
    #[allow(clippy::too_many_arguments)]
    fn with_submit_retry_policy(
        withdrawal_state_store: Arc<WithdrawalSequencerStore>,
        submitter: Arc<dyn WithdrawalSubmitPort>,
        base_height_tracker: Arc<SequencerBaseHeightTracker>,
        base_withdrawal_verifier: Arc<dyn SequencerBaseWithdrawalVerifier>,
        handoff_window_blocks: u64,
        node_pkhs: Vec<nockchain_types::tx_engine::common::Hash>,
        node_eth_addresses: HashMap<u64, Address>,
        submit_retry_policy: RetryPolicy,
    ) -> Self {
        Self {
            withdrawal_state_store,
            submitter,
            submit_retry_policy,
            authorized_submit_retry_after_base_blocks:
                WithdrawalSequencerOrphanRetryLoopPolicy::default().retry_after_base_blocks,
            base_height_tracker,
            base_withdrawal_verifier,
            handoff_window_blocks,
            node_pkhs,
            node_eth_addresses,
            manual_submit_approval: ManualSubmitApprovalConfig::default(),
        }
    }

    /// Verifies that the RPC caller matches the proposer selected for the
    /// withdrawal's current `(epoch, handoff_index)` turn.
    fn require_expected_caller(
        &self,
        row: &LiveWithdrawalView,
        caller_node_id: u64,
        action: &str,
    ) -> Result<(), SequencerUpdateResponse> {
        // TODO: caller_node_id is currently trusted because bridge nodes run on
        // a VPN. If we move to a more distributed deployment, authenticate the
        // caller's node identity with a signature or stronger transport auth
        // before enforcing proposer turns server-side.
        let expected = withdrawal_turn_proposer(
            &row.id, row.current_epoch, row.handoff_index, &self.node_pkhs,
        ) as u64;
        if expected == caller_node_id {
            return Ok(());
        }
        Err(SequencerUpdateResponse {
            request_accepted: false,
            error: format!(
                "withdrawal {:?} epoch {} {action} caller {} is not the expected proposer {} for handoff index {}",
                row.id, row.current_epoch, caller_node_id, expected, row.handoff_index
            ),
        })
    }

    fn current_confirmed_base_height(&self) -> Result<u64, BridgeError> {
        self.base_height_tracker
            .latest_confirmed_base_height()
            .ok_or_else(|| {
                BridgeError::Runtime(
                    "sequencer base height watcher has not observed a confirmed Base height yet"
                        .to_string(),
                )
            })
    }

    fn deferred_submit_response(reason: impl Into<String>) -> SequencerUpdateResponse {
        SequencerUpdateResponse {
            request_accepted: false,
            error: format!("{}{}", WITHDRAWAL_SUBMIT_DEFERRED_PREFIX, reason.into()),
        }
    }

    fn authorized_submit_retry_window_remaining(
        &self,
        row: &LiveWithdrawalView,
        current_base_height: u64,
    ) -> Option<u64> {
        let last_submit_attempt_base_height = row.last_submit_attempt_base_height?;
        let elapsed = current_base_height.saturating_sub(last_submit_attempt_base_height);
        (elapsed < self.authorized_submit_retry_after_base_blocks)
            .then_some(self.authorized_submit_retry_after_base_blocks - elapsed)
    }

    async fn authorized_transaction_already_seen(
        &self,
        proposal: &WithdrawalProposalData,
        row: &LiveWithdrawalView,
    ) -> bool {
        if matches!(
            self.submitter.transaction_mempool_accepted(proposal).await,
            Ok(Some(true))
        ) {
            return true;
        }
        let Some(submitted_raw_tx_id) = row.authorized_transaction_name.as_deref() else {
            return false;
        };
        matches!(
            self.submitter
                .get_transaction_included_block(submitted_raw_tx_id)
                .await,
            Ok(Some(_))
        )
    }

    async fn advance_elapsed_precanonical_handoff(
        &self,
        id: &crate::withdrawal::types::WithdrawalId,
        epoch: u64,
    ) -> Result<SequencerUpdateResponse, Status> {
        let Some(existing) = self
            .withdrawal_state_store
            .fetch_sequenced_withdrawal(id)
            .await
            .map_err(|err| Status::internal(format!("sequencer status fetch failed: {err}")))?
        else {
            return Ok(SequencerUpdateResponse {
                request_accepted: false,
                error: format!("sequencer does not know withdrawal {id:?}"),
            });
        };
        if existing.current_epoch != epoch {
            return Ok(SequencerUpdateResponse {
                request_accepted: false,
                error: format!(
                    "withdrawal {:?} is at epoch {}, not requested epoch {}",
                    existing.id, existing.current_epoch, epoch
                ),
            });
        }
        if existing.state != WithdrawalState::Pending {
            return Ok(SequencerUpdateResponse {
                request_accepted: false,
                error: format!(
                    "withdrawal {:?} is {}, not pending",
                    existing.id,
                    existing.state.as_str()
                ),
            });
        }

        let current_base_height = match current_confirmed_base_height(&self.base_height_tracker) {
            Ok(height) => height,
            Err(response) => return Ok(response),
        };
        let turn_started_base_height = existing.turn_started_base_height.ok_or_else(|| {
            Status::failed_precondition(format!(
                "sequencer withdrawal {:?} epoch {} is missing turn_started_base_height",
                existing.id, existing.current_epoch
            ))
        })?;
        let elapsed_turns = elapsed_handoff_turns(
            turn_started_base_height, current_base_height, self.handoff_window_blocks,
        )
        .map_err(Status::failed_precondition)?;
        if elapsed_turns == 0 {
            return Ok(SequencerUpdateResponse {
                request_accepted: true,
                error: String::new(),
            });
        }

        let next_handoff_index = existing
            .handoff_index
            .checked_add(elapsed_turns)
            .ok_or_else(|| Status::internal("pre-canonical handoff index overflow"))?;
        self.withdrawal_state_store
            .record_precanonical_handoff_for_id(
                &existing.id, existing.current_epoch, next_handoff_index, current_base_height,
            )
            .await
            .map_err(|err| {
                Status::internal(format!(
                    "failed to advance pending proposer handoff for {:?}: {err}",
                    existing.id
                ))
            })?;

        Ok(SequencerUpdateResponse {
            request_accepted: true,
            error: String::new(),
        })
    }

    async fn prepare_submitted_action(
        &self,
        proposal: &WithdrawalProposalData,
        caller_node_id: Option<u64>,
        action: &str,
    ) -> Result<Result<LiveWithdrawalView, SequencerUpdateResponse>, Status> {
        let Some(status_row) = self.status_with_lazy_handoff(&proposal.id).await? else {
            return Ok(Err(SequencerUpdateResponse {
                request_accepted: false,
                error: format!("sequencer does not know withdrawal {:?}", proposal.id),
            }));
        };
        if status_row.current_epoch == proposal.epoch {
            if let Some(caller_node_id) = caller_node_id {
                if let Err(response) =
                    self.require_expected_caller(&status_row, caller_node_id, action)
                {
                    return Ok(Err(response));
                }
            }
        }
        Ok(Ok(status_row))
    }

    fn submitted_action_rejected_for_wrong_caller(response: &SequencerUpdateResponse) -> bool {
        response.error.contains("is not the expected proposer")
    }

    /// Repeatedly submits the authorized withdrawal transaction until either
    /// the node reports mempool acceptance or the bounded retry budget is
    /// exhausted. Exhaustion is recorded as an `Authorized` row with failure
    /// metadata by the caller.
    async fn submit_until_final_outcome(
        &self,
        proposal: &WithdrawalProposalData,
    ) -> Result<BoundedSubmitOutcome, BridgeError> {
        #[derive(Debug)]
        enum SubmitAttemptError {
            NotYetAccepted,
            Submit(BridgeError),
        }

        #[derive(Debug, Default)]
        struct SubmitAttemptTracker {
            submit_attempt_count: u64,
            last_submit_attempt_base_height: Option<u64>,
        }

        let tracker = Arc::new(Mutex::new(SubmitAttemptTracker::default()));
        let base_height_tracker = self.base_height_tracker.clone();
        let submitter = self.submitter.clone();
        let proposal = proposal.clone();
        let tracker_for_retry = tracker.clone();
        let retry_policy = self.submit_retry_policy;

        let submit_result = {
            // `backon` treats this closure as a single submit attempt. Each
            // call returns one attempt result, and `retry(...)` keeps invoking
            // it with exponential backoff until we either observe
            // `MempoolAccepted` (`Ok(())`) or exhaust the retry budget on
            // `Err(...)`.
            let submit = move || {
                let submitter = submitter.clone();
                let proposal = proposal.clone();
                let tracker = tracker_for_retry.clone();
                let base_height_tracker = base_height_tracker.clone();
                async move {
                    let attempt_base_height = base_height_tracker
                        .latest_confirmed_base_height()
                        .ok_or_else(|| {
                            SubmitAttemptError::Submit(BridgeError::Runtime(
                                "sequencer base height watcher has not observed a confirmed Base height yet"
                                    .to_string(),
                            ))
                        })?;
                    {
                        let mut tracker = tracker
                            .lock()
                            .expect("submit attempt tracker lock should not be poisoned");
                        tracker.submit_attempt_count =
                            tracker.submit_attempt_count.saturating_add(1);
                        tracker.last_submit_attempt_base_height = Some(attempt_base_height);
                    }

                    match submitter.submit_withdrawal(&proposal).await {
                        Ok(WithdrawalSubmitAttemptStatus::MempoolAccepted) => Ok(()),
                        Ok(WithdrawalSubmitAttemptStatus::NotYetAccepted) => {
                            Err(SubmitAttemptError::NotYetAccepted)
                        }
                        Err(err) => Err(SubmitAttemptError::Submit(err)),
                    }
                }
            };

            submit.retry(retry_policy.exponential_builder()).await
        };

        let tracker = tracker
            .lock()
            .expect("submit attempt tracker lock should not be poisoned");
        match submit_result {
            Ok(()) => Ok(BoundedSubmitOutcome {
                final_state: WithdrawalState::MempoolAccepted,
                submit_attempt_count: tracker.submit_attempt_count,
                last_submit_attempt_base_height: tracker
                    .last_submit_attempt_base_height
                    .expect("submit attempts should set last_submit_attempt_base_height"),
                last_submit_error: None,
            }),
            Err(err) => Ok(BoundedSubmitOutcome {
                final_state: WithdrawalState::Authorized,
                submit_attempt_count: tracker.submit_attempt_count,
                last_submit_attempt_base_height: tracker
                    .last_submit_attempt_base_height
                    .expect("submit attempts should set last_submit_attempt_base_height"),
                last_submit_error: Some(match err {
                    SubmitAttemptError::NotYetAccepted => {
                        "transaction was not reported mempool-accepted before the retry budget was exhausted"
                            .to_string()
                    }
                    SubmitAttemptError::Submit(err) => err.to_string(),
                }),
            }),
        }
    }
}

/// Serves the withdrawal sequencer gRPC API until the server exits.
#[allow(clippy::too_many_arguments)]
pub async fn serve_withdrawal_sequencer(
    addr: SocketAddr,
    withdrawal_state_store: Arc<WithdrawalSequencerStore>,
    submitter: Arc<dyn WithdrawalSubmitPort>,
    base_height_tracker: Arc<SequencerBaseHeightTracker>,
    base_withdrawal_verifier: Arc<dyn SequencerBaseWithdrawalVerifier>,
    handoff_window_blocks: u64,
    authorized_submit_retry_after_base_blocks: u64,
    node_pkhs: Vec<nockchain_types::tx_engine::common::Hash>,
    node_eth_addresses: HashMap<u64, Address>,
    manual_submit_approval: ManualSubmitApprovalConfig,
) -> Result<(), BridgeError> {
    let reflection_service = ReflectionBuilder::configure()
        .register_encoded_file_descriptor_set(crate::shared::grpc::FILE_DESCRIPTOR_SET)
        .build_v1()
        .map_err(|err| BridgeError::Runtime(format!("reflection build error: {}", err)))?;
    let service = WithdrawalSequencerRpcService::new(
        withdrawal_state_store, submitter, base_height_tracker, base_withdrawal_verifier,
        handoff_window_blocks, node_pkhs, node_eth_addresses,
    )
    .with_authorized_submit_retry_after_base_blocks(authorized_submit_retry_after_base_blocks)
    .with_manual_submit_approval(manual_submit_approval);

    tonic::transport::Server::builder()
        .add_service(reflection_service)
        .add_service(WithdrawalSequencerServer::new(service))
        .serve(addr)
        .await
        .map_err(|err| BridgeError::Runtime(format!("withdrawal sequencer server error: {err}")))
}

/// Serves the withdrawal sequencer gRPC API with an explicit shutdown signal.
#[allow(clippy::too_many_arguments)]
pub async fn serve_withdrawal_sequencer_with_shutdown(
    addr: SocketAddr,
    withdrawal_state_store: Arc<WithdrawalSequencerStore>,
    submitter: Arc<dyn WithdrawalSubmitPort>,
    base_height_tracker: Arc<SequencerBaseHeightTracker>,
    base_withdrawal_verifier: Arc<dyn SequencerBaseWithdrawalVerifier>,
    handoff_window_blocks: u64,
    authorized_submit_retry_after_base_blocks: u64,
    node_pkhs: Vec<nockchain_types::tx_engine::common::Hash>,
    node_eth_addresses: HashMap<u64, Address>,
    manual_submit_approval: ManualSubmitApprovalConfig,
    shutdown: tokio::sync::oneshot::Receiver<()>,
) -> Result<(), BridgeError> {
    let reflection_service = ReflectionBuilder::configure()
        .register_encoded_file_descriptor_set(crate::shared::grpc::FILE_DESCRIPTOR_SET)
        .build_v1()
        .map_err(|err| BridgeError::Runtime(format!("reflection build error: {}", err)))?;
    let service = WithdrawalSequencerRpcService::new(
        withdrawal_state_store, submitter, base_height_tracker, base_withdrawal_verifier,
        handoff_window_blocks, node_pkhs, node_eth_addresses,
    )
    .with_authorized_submit_retry_after_base_blocks(authorized_submit_retry_after_base_blocks)
    .with_manual_submit_approval(manual_submit_approval);

    tonic::transport::Server::builder()
        .add_service(reflection_service)
        .add_service(WithdrawalSequencerServer::new(service))
        .serve_with_shutdown(addr, async move {
            let _ = shutdown.await;
        })
        .await
        .map_err(|err| BridgeError::Runtime(format!("withdrawal sequencer server error: {err}")))
}

/// Converts an optional durable sequencer row into the gRPC status payload,
/// preserving the known withdrawal nonce even when no row exists yet.
fn sequenced_status_response(
    row: Option<LiveWithdrawalView>,
    withdrawal_nonce: u64,
    current_confirmed_base_height: Option<u64>,
    handoff_window_blocks: u64,
) -> SequencedWithdrawalStatusResponse {
    match row {
        Some(row) => {
            let blocks_until_handoff = sequenced_blocks_until_handoff(
                &row, current_confirmed_base_height, handoff_window_blocks,
            );
            SequencedWithdrawalStatusResponse {
                found: true,
                current_epoch: row.current_epoch,
                state: row.state.as_str().to_string(),
                proposal_hash: row.proposal_hash.unwrap_or_default(),
                authorized_transaction_name: row.authorized_transaction_name.unwrap_or_default(),
                withdrawal_nonce: row.withdrawal_nonce.unwrap_or(withdrawal_nonce),
                handoff_index: row.handoff_index,
                turn_started_base_height: row.turn_started_base_height,
                current_confirmed_base_height,
                handoff_window_blocks,
                blocks_until_handoff,
            }
        }
        None => SequencedWithdrawalStatusResponse {
            found: false,
            current_epoch: 0,
            state: String::new(),
            proposal_hash: String::new(),
            authorized_transaction_name: String::new(),
            withdrawal_nonce,
            handoff_index: 0,
            turn_started_base_height: None,
            current_confirmed_base_height,
            handoff_window_blocks,
            blocks_until_handoff: None,
        },
    }
}

fn sequenced_blocks_until_handoff(
    row: &LiveWithdrawalView,
    current_confirmed_base_height: Option<u64>,
    handoff_window_blocks: u64,
) -> Option<u64> {
    if !matches!(
        row.state,
        WithdrawalState::Pending | WithdrawalState::PeerCanonical | WithdrawalState::Authorized
    ) {
        return None;
    }
    let current_base_height = current_confirmed_base_height?;
    let turn_started_base_height = row.turn_started_base_height?;
    Some(
        handoff_window_blocks
            .saturating_sub(current_base_height.saturating_sub(turn_started_base_height)),
    )
}

/// Converts the next pending withdrawal ordering row into its gRPC response
/// shape.
fn next_pending_withdrawal_ordering_response(
    row: Option<(crate::withdrawal::types::WithdrawalId, u64)>,
) -> NextPendingWithdrawalOrderingResponse {
    match row {
        Some((id, withdrawal_nonce)) => NextPendingWithdrawalOrderingResponse {
            found: true,
            withdrawal_id: Some(withdrawal_id_to_proto(&id)),
            withdrawal_nonce,
        },
        None => NextPendingWithdrawalOrderingResponse {
            found: false,
            withdrawal_id: None,
            withdrawal_nonce: 0,
        },
    }
}

/// Converts the sequencer frontier nonce into its gRPC response shape.
fn current_live_withdrawal_nonce_response(
    withdrawal_nonce: Option<u64>,
) -> CurrentLiveWithdrawalNonceResponse {
    match withdrawal_nonce {
        Some(withdrawal_nonce) => CurrentLiveWithdrawalNonceResponse {
            found: true,
            withdrawal_nonce,
        },
        None => CurrentLiveWithdrawalNonceResponse {
            found: false,
            withdrawal_nonce: 0,
        },
    }
}

/// Converts a sequencer frontier id check into its gRPC response shape.
fn frontier_allows_withdrawal_response(
    check: crate::withdrawal::sequencer::store::WithdrawalFrontierCheck,
) -> SequencerFrontierAllowsWithdrawalResponse {
    SequencerFrontierAllowsWithdrawalResponse {
        allowed: check.allowed(),
        registered: check.registered,
        is_frontier: check.is_frontier,
    }
}

fn canonical_artifacts_response(
    artifacts: Option<WithdrawalSequencerProposalArtifacts>,
) -> Result<SequencerCanonicalProposalArtifactsResponse, BridgeError> {
    let Some(artifacts) = artifacts else {
        return Ok(SequencerCanonicalProposalArtifactsResponse {
            found: false,
            withdrawal_id: None,
            epoch: 0,
            proposal_hash: String::new(),
            amount: 0,
            base_batch_end: 0,
            snapshot: None,
            selected_inputs: Vec::new(),
            transaction_jam: Vec::new(),
            commit_certificate: Vec::new(),
            authorized_transaction_name: String::new(),
            authorized_transaction_jam: Vec::new(),
            authorized_raw_tx: Vec::new(),
        });
    };
    Ok(SequencerCanonicalProposalArtifactsResponse {
        found: true,
        withdrawal_id: Some(withdrawal_id_to_proto(&artifacts.id)),
        epoch: artifacts.epoch,
        proposal_hash: artifacts.proposal_hash,
        amount: artifacts.amount,
        base_batch_end: artifacts.base_batch_end,
        snapshot: Some(crate::withdrawal::transport::snapshot_to_proto(
            &artifacts.snapshot,
        )),
        selected_inputs: artifacts
            .selected_inputs
            .iter()
            .map(note_name_to_proto)
            .collect(),
        transaction_jam: jam_transaction(&artifacts.transaction)?,
        commit_certificate: artifacts.commit_certificate.unwrap_or_default(),
        authorized_transaction_name: artifacts.authorized_transaction_name.unwrap_or_default(),
        authorized_transaction_jam: artifacts.authorized_transaction_jam.unwrap_or_default(),
        authorized_raw_tx: artifacts.authorized_raw_tx.unwrap_or_default(),
    })
}

/// Computes how many proposer turns have elapsed since the stored Base-height
/// anchor for a canonical withdrawal.
fn elapsed_handoff_turns(
    turn_started_base_height: u64,
    current_base_height: u64,
    handoff_window_blocks: u64,
) -> Result<u64, String> {
    if current_base_height < turn_started_base_height {
        return Err(format!(
            "current confirmed Base height {current_base_height} is behind stored turn_started_base_height {turn_started_base_height}"
        ));
    }
    if handoff_window_blocks == 0 {
        return Ok(1);
    }
    Ok((current_base_height - turn_started_base_height) / handoff_window_blocks)
}

/// Loads the latest confirmed Base height observed by the sequencer's watcher,
/// rejecting requests until the watcher has initialized.
fn current_confirmed_base_height(
    tracker: &SequencerBaseHeightTracker,
) -> Result<u64, SequencerUpdateResponse> {
    tracker
        .latest_confirmed_base_height()
        .ok_or_else(|| SequencerUpdateResponse {
            request_accepted: false,
            error: "sequencer base height watcher has not observed a confirmed Base height yet"
                .to_string(),
        })
}

impl WithdrawalSequencerRpcService {
    /// Fetches sequencer status for a withdrawal and lazily advances proposer
    /// turns when the Base-height handoff window has elapsed.
    async fn status_with_lazy_handoff(
        &self,
        id: &crate::withdrawal::types::WithdrawalId,
    ) -> Result<Option<LiveWithdrawalView>, Status> {
        let mut row = self
            .withdrawal_state_store
            .fetch_sequenced_withdrawal(id)
            .await
            .map_err(|err| Status::internal(format!("sequencer status fetch failed: {err}")))?;
        let Some(existing) = row.as_ref() else {
            return Ok(None);
        };
        if !matches!(
            existing.state,
            WithdrawalState::Pending | WithdrawalState::PeerCanonical | WithdrawalState::Authorized
        ) {
            return Ok(row);
        }

        let current_base_height = current_confirmed_base_height(&self.base_height_tracker)
            .map_err(|response| Status::failed_precondition(response.error))?;
        let turn_started_base_height = existing.turn_started_base_height.ok_or_else(|| {
            Status::failed_precondition(format!(
                "sequencer withdrawal {:?} epoch {} is missing turn_started_base_height",
                existing.id, existing.current_epoch
            ))
        })?;
        let elapsed_turns = elapsed_handoff_turns(
            turn_started_base_height, current_base_height, self.handoff_window_blocks,
        )
        .map_err(Status::failed_precondition)?;
        if elapsed_turns == 0 {
            return Ok(row);
        }
        let next_handoff_index = existing
            .handoff_index
            .checked_add(elapsed_turns)
            .ok_or_else(|| Status::internal("proposer handoff index overflow"))?;
        match existing.state {
            WithdrawalState::Pending => {
                self.withdrawal_state_store
                    .record_precanonical_handoff_for_id(
                        &existing.id, existing.current_epoch, next_handoff_index,
                        current_base_height,
                    )
                    .await
                    .map_err(|err| {
                        Status::internal(format!(
                            "failed to advance lazy pending proposer handoff for {:?}: {err}",
                            existing.id
                        ))
                    })?;
            }
            WithdrawalState::PeerCanonical | WithdrawalState::Authorized => {
                self.withdrawal_state_store
                    .record_proposer_turn_expired_for_id(
                        &existing.id, existing.current_epoch, next_handoff_index,
                        current_base_height,
                    )
                    .await
                    .map_err(|err| {
                        Status::internal(format!(
                            "failed to advance lazy proposer handoff for {:?}: {err}",
                            existing.id
                        ))
                    })?;
            }
            _ => return Ok(row),
        }
        row = self
            .withdrawal_state_store
            .fetch_sequenced_withdrawal(id)
            .await
            .map_err(|err| Status::internal(format!("sequencer status refetch failed: {err}")))?;
        Ok(row)
    }
}

#[async_trait]
impl WithdrawalSequencer for WithdrawalSequencerRpcService {
    /// Registers the bridge-local withdrawal nonce with the sequencer's
    /// durable withdrawal row.
    async fn register_withdrawal(
        &self,
        request: Request<SequencerRegisterWithdrawalRequest>,
    ) -> Result<Response<SequencerUpdateResponse>, Status> {
        metrics::init_metrics()
            .sequencer_withdrawal_registration_requests
            .increment();
        let inner = request.into_inner();
        let base_event_id = base_event_id_from_proto(&inner.base_event_id, "registration")?;
        if is_explicitly_refunded_withdrawal_base_event_id(&base_event_id) {
            let base_event_id_hex = sequencer_base_event_id_hex(&base_event_id);
            let err = BaseWithdrawalRejection::ExplicitlyRefunded {
                base_event_id_hex: base_event_id_hex.clone(),
            };
            metrics::init_metrics()
                .sequencer_withdrawal_registration_rejected
                .increment();
            tracing::warn!(
                target: "bridge.withdrawal.sequencer",
                withdrawal_nonce = inner.withdrawal_nonce,
                base_batch_end = inner.base_batch_end,
                base_event_id = %base_event_id_hex,
                "ignored explicitly refunded withdrawal registration"
            );
            return Ok(Response::new(SequencerUpdateResponse {
                request_accepted: false,
                error: err.to_string(),
            }));
        }
        let id = crate::withdrawal::types::WithdrawalId {
            // Registration is sequencer ordering over the globally unique Base
            // event id. The kernel `as_of` becomes authoritative only when a
            // canonical proposal is accepted and the row is updated.
            as_of: zero_tip5_hash(),
            base_event_id,
        };
        let recipient = Tip5Hash::from_be_limb_bytes(&inner.recipient)
            .map_err(|err| Status::invalid_argument(format!("invalid recipient: {err}")))?;
        let tracked = TrackedWithdrawalRequest {
            id,
            recipient,
            amount: inner.burned_amount,
            base_batch_end: inner.base_batch_end,
            withdrawal_nonce: inner.withdrawal_nonce,
        };
        match self
            .withdrawal_state_store
            .fetch_sequenced_withdrawal(&tracked.id)
            .await
        {
            Ok(Some(existing)) if registered_withdrawal_matches_tracked(&existing, &tracked) => {
                let metrics = metrics::init_metrics();
                metrics
                    .sequencer_withdrawal_registration_accepted
                    .increment();
                metrics
                    .sequencer_withdrawal_registration_idempotent
                    .increment();
                return Ok(Response::new(SequencerUpdateResponse {
                    request_accepted: true,
                    error: String::new(),
                }));
            }
            Ok(Some(_)) => {
                metrics::init_metrics()
                    .sequencer_withdrawal_registration_rejected
                    .increment();
                return Ok(Response::new(SequencerUpdateResponse {
                    request_accepted: false,
                    error:
                        "sequencer withdrawal is already registered with different request facts"
                            .to_string(),
                }));
            }
            Ok(None) => {}
            Err(err) => {
                return Err(Status::internal(format!(
                    "sequencer registration lookup failed: {err}"
                )));
            }
        }
        let verify_started = Instant::now();
        let verify_result = self.base_withdrawal_verifier.verify(&tracked).await;
        metrics::init_metrics()
            .sequencer_withdrawal_base_verifier_verify_time
            .add_timing(&verify_started.elapsed());
        if let Err(err) = verify_result {
            metrics::init_metrics()
                .sequencer_withdrawal_base_verifier_rejected
                .increment();
            tracing::warn!(
                target: "bridge.withdrawal.sequencer",
                withdrawal_nonce = tracked.withdrawal_nonce,
                base_batch_end = tracked.base_batch_end,
                error = %err,
                "rejected withdrawal registration after Base verification"
            );
            return Ok(Response::new(SequencerUpdateResponse {
                request_accepted: false,
                error: err.to_string(),
            }));
        }
        metrics::init_metrics()
            .sequencer_withdrawal_base_verifier_accepted
            .increment();
        let current_base_height = match self.current_confirmed_base_height() {
            Ok(height) => height,
            Err(err) => {
                metrics::init_metrics()
                    .sequencer_withdrawal_registration_rejected
                    .increment();
                return Ok(Response::new(SequencerUpdateResponse {
                    request_accepted: false,
                    error: err.to_string(),
                }));
            }
        };

        match self
            .withdrawal_state_store
            .ensure_tracked_withdrawal_ordering_at_base_height(&tracked, current_base_height)
            .await
        {
            Ok(()) => {
                metrics::init_metrics()
                    .sequencer_withdrawal_registration_accepted
                    .increment();
                Ok(Response::new(SequencerUpdateResponse {
                    request_accepted: true,
                    error: String::new(),
                }))
            }
            Err(err) => {
                let metrics = metrics::init_metrics();
                metrics
                    .sequencer_withdrawal_registration_rejected
                    .increment();
                let error = err.to_string();
                if error.contains("would sort before already registered withdrawal history") {
                    metrics
                        .sequencer_withdrawal_registration_rejected_lower_than_head
                        .increment();
                }
                Ok(Response::new(SequencerUpdateResponse {
                    request_accepted: false,
                    error,
                }))
            }
        }
    }

    /// Records the fixed peer-canonical proposal before authorization begins.
    async fn record_canonical_proposal(
        &self,
        request: Request<SequencerRecordCanonicalRequest>,
    ) -> Result<Response<SequencerUpdateResponse>, Status> {
        metrics::init_metrics()
            .sequencer_withdrawal_canonicalize_requests
            .increment();
        let inner = request.into_inner();
        let envelope = inner
            .proposal
            .ok_or_else(|| Status::invalid_argument("missing proposal envelope"))?;
        let commit_certificate = inner
            .commit_certificate
            .ok_or_else(|| Status::invalid_argument("missing withdrawal commit certificate"))?;
        let proposal = proposal_from_proto(&envelope)
            .map_err(|err| Status::invalid_argument(format!("invalid proposal envelope: {err}")))?;
        let withdrawal_nonce = envelope.withdrawal_nonce;
        let caller_node_id = inner.caller_node_id;
        let proposal_hash = proposal
            .proposal_hash()
            .map_err(|err| Status::invalid_argument(format!("invalid proposal envelope: {err}")))?;
        let required_commit_signers = required_withdrawal_commit_signature_threshold(&proposal)
            .map_err(|err| {
                Status::invalid_argument(format!("invalid canonicalization threshold: {err}"))
            })?;
        verify_withdrawal_commit_certificate(
            &proposal, &proposal_hash, &commit_certificate, required_commit_signers,
            &self.node_eth_addresses,
        )
        .map_err(|err| {
            metrics::init_metrics()
                .sequencer_withdrawal_canonicalize_certificate_verify_failed
                .increment();
            Status::invalid_argument(format!("invalid commit certificate: {err}"))
        })?;
        if let Err(err) = validate_canonical_proposal_tx_inputs(&proposal) {
            metrics::init_metrics()
                .sequencer_withdrawal_canonicalize_rejected
                .increment();
            return Ok(Response::new(SequencerUpdateResponse {
                request_accepted: false,
                error: err.to_string(),
            }));
        }

        if let Err(response) = self
            .prepare_submitted_action(&proposal, Some(caller_node_id), "canonicalization")
            .await?
        {
            let metrics = metrics::init_metrics();
            metrics
                .sequencer_withdrawal_canonicalize_rejected
                .increment();
            if Self::submitted_action_rejected_for_wrong_caller(&response) {
                metrics
                    .sequencer_withdrawal_canonicalize_not_frontier
                    .increment();
            }
            return Ok(Response::new(response));
        }

        if let Err(err) = self
            .withdrawal_state_store
            .ensure_registered_proposal_ordering(&proposal, withdrawal_nonce)
            .await
        {
            let metrics = metrics::init_metrics();
            metrics
                .sequencer_withdrawal_canonicalize_rejected
                .increment();
            if err.to_string().contains("while sequencer frontier") {
                metrics
                    .sequencer_withdrawal_canonicalize_not_frontier
                    .increment();
            }
            return Ok(Response::new(SequencerUpdateResponse {
                request_accepted: false,
                error: err.to_string(),
            }));
        }
        let current_base_height = match current_confirmed_base_height(&self.base_height_tracker) {
            Ok(height) => height,
            Err(response) => return Ok(Response::new(response)),
        };
        match self
            .withdrawal_state_store
            .record_peer_canonical_proposal(
                &proposal,
                Some(&commit_certificate),
                current_base_height,
            )
            .await
        {
            Ok(()) => {
                metrics::init_metrics()
                    .sequencer_withdrawal_canonicalize_accepted
                    .increment();
                Ok(Response::new(SequencerUpdateResponse {
                    request_accepted: true,
                    error: String::new(),
                }))
            }
            Err(err) => {
                metrics::init_metrics()
                    .sequencer_withdrawal_canonicalize_rejected
                    .increment();
                Ok(Response::new(SequencerUpdateResponse {
                    request_accepted: false,
                    error: err.to_string(),
                }))
            }
        }
    }

    /// Advances the shared pre-canonical handoff index for a withdrawal epoch
    /// after a local assembly turn times out.
    async fn advance_precanonical_handoff(
        &self,
        request: Request<SequencerAdvancePrecanonicalHandoffRequest>,
    ) -> Result<Response<SequencerUpdateResponse>, Status> {
        let inner = request.into_inner();
        let id = inner
            .withdrawal_id
            .ok_or_else(|| Status::invalid_argument("missing withdrawal id"))?;
        let id = withdrawal_id_from_proto(&id)
            .map_err(|err| Status::invalid_argument(format!("invalid withdrawal id: {err}")))?;
        let response = self
            .advance_elapsed_precanonical_handoff(&id, inner.epoch)
            .await?;
        Ok(Response::new(response))
    }

    /// Records one signer witness contribution for the fixed canonical
    /// withdrawal transaction. New contributions only refresh the
    /// sequencer-owned handoff window while the tx is still peer-canonical;
    /// after authorization, extra signatures no longer extend the submit turn.
    async fn record_signed_proposal(
        &self,
        request: Request<SequencerRecordSignedProposalRequest>,
    ) -> Result<Response<SequencerUpdateResponse>, Status> {
        let inner = request.into_inner();
        let envelope = inner
            .proposal
            .ok_or_else(|| Status::invalid_argument("missing proposal envelope"))?;
        let proposal = proposal_from_proto(&envelope)
            .map_err(|err| Status::invalid_argument(format!("invalid proposal envelope: {err}")))?;
        let withdrawal_nonce = envelope.withdrawal_nonce;
        if let Err(response) = self
            .prepare_submitted_action(&proposal, None, "signed proposal")
            .await?
        {
            return Ok(Response::new(response));
        }
        let current_base_height = match current_confirmed_base_height(&self.base_height_tracker) {
            Ok(height) => height,
            Err(response) => return Ok(Response::new(response)),
        };
        if let Err(err) = self
            .withdrawal_state_store
            .ensure_registered_proposal_ordering(&proposal, withdrawal_nonce)
            .await
        {
            return Ok(Response::new(SequencerUpdateResponse {
                request_accepted: false,
                error: err.to_string(),
            }));
        }
        match self
            .withdrawal_state_store
            .record_proposal_signed(&proposal, inner.signer_node_id, current_base_height)
            .await
        {
            Ok(()) => Ok(Response::new(SequencerUpdateResponse {
                request_accepted: true,
                error: String::new(),
            })),
            Err(err) => Ok(Response::new(SequencerUpdateResponse {
                request_accepted: false,
                error: err.to_string(),
            })),
        }
    }

    /// Returns the currently reserved note-name set across all
    /// canonical-or-later sequencer-owned withdrawals.
    async fn get_reserved_withdrawal_inputs(
        &self,
        _request: Request<SequencerReservedWithdrawalInputsRequest>,
    ) -> Result<Response<SequencerReservedWithdrawalInputsResponse>, Status> {
        let reserved_inputs = self
            .withdrawal_state_store
            .list_reserved_input_names()
            .await
            .map_err(|err| {
                Status::internal(format!("sequencer reserved input fetch failed: {err}"))
            })?;
        Ok(Response::new(SequencerReservedWithdrawalInputsResponse {
            reserved_inputs: reserved_inputs.iter().map(note_name_to_proto).collect(),
        }))
    }

    /// Authorizes the fully signed proposal for the next pending withdrawal
    /// nonce, if the proposal matches the sequencer's ordering and state rules.
    async fn authorize_proposal(
        &self,
        request: Request<SequencerAuthorizeProposalRequest>,
    ) -> Result<Response<SequencerUpdateResponse>, Status> {
        metrics::init_metrics()
            .sequencer_withdrawal_authorize_requests
            .increment();
        let inner = request.into_inner();
        let caller_node_id = inner.caller_node_id;
        let envelope = inner
            .proposal
            .ok_or_else(|| Status::invalid_argument("missing proposal envelope"))?;
        let commit_certificate = inner
            .commit_certificate
            .ok_or_else(|| Status::invalid_argument("missing withdrawal commit certificate"))?;
        let proposal = proposal_from_proto(&envelope)
            .map_err(|err| Status::invalid_argument(format!("invalid proposal envelope: {err}")))?;
        let withdrawal_nonce = envelope.withdrawal_nonce;
        if !transaction_is_fully_signed(&proposal.transaction) {
            metrics::init_metrics()
                .sequencer_withdrawal_authorize_rejected
                .increment();
            return Ok(Response::new(SequencerUpdateResponse {
                request_accepted: false,
                error: format!(
                    "withdrawal {:?} epoch {} is not fully signed",
                    proposal.id, proposal.epoch
                ),
            }));
        }

        let proposal_hash = proposal
            .proposal_hash()
            .map_err(|err| Status::invalid_argument(format!("invalid proposal envelope: {err}")))?;
        let required_commit_signers = required_withdrawal_commit_signature_threshold(&proposal)
            .map_err(|err| {
                Status::invalid_argument(format!("invalid authorization threshold: {err}"))
            })?;
        verify_withdrawal_commit_certificate(
            &proposal, &proposal_hash, &commit_certificate, required_commit_signers,
            &self.node_eth_addresses,
        )
        .map_err(|err| Status::invalid_argument(format!("invalid commit certificate: {err}")))?;
        if let Err(response) = self
            .prepare_submitted_action(&proposal, Some(caller_node_id), "authorization")
            .await?
        {
            let metrics = metrics::init_metrics();
            metrics.sequencer_withdrawal_authorize_rejected.increment();
            if Self::submitted_action_rejected_for_wrong_caller(&response) {
                metrics
                    .sequencer_withdrawal_authorize_not_frontier
                    .increment();
            }
            return Ok(Response::new(response));
        }
        let current_base_height = match current_confirmed_base_height(&self.base_height_tracker) {
            Ok(height) => height,
            Err(response) => return Ok(Response::new(response)),
        };
        match self
            .withdrawal_state_store
            .sequencer_authorize_proposal(
                &proposal, withdrawal_nonce, &commit_certificate, current_base_height,
            )
            .await
        {
            Ok(SequencerDecision::Allowed) => {
                metrics::init_metrics()
                    .sequencer_withdrawal_authorize_accepted
                    .increment();
                Ok(Response::new(SequencerUpdateResponse {
                    request_accepted: true,
                    error: String::new(),
                }))
            }
            Ok(SequencerDecision::Rejected(error)) => {
                let metrics = metrics::init_metrics();
                metrics.sequencer_withdrawal_authorize_rejected.increment();
                if error.contains("while sequencer frontier") {
                    metrics
                        .sequencer_withdrawal_authorize_not_frontier
                        .increment();
                }
                Ok(Response::new(SequencerUpdateResponse {
                    request_accepted: false,
                    error,
                }))
            }
            Err(err) => {
                metrics::init_metrics()
                    .sequencer_withdrawal_authorize_rejected
                    .increment();
                Err(Status::internal(format!(
                    "failed to authorize sequenced withdrawal: {err}"
                )))
            }
        }
    }

    /// Submits the already authorized proposal for the next pending withdrawal
    /// nonce.
    async fn submit_proposal(
        &self,
        request: Request<SequencerSubmitProposalRequest>,
    ) -> Result<Response<SequencerUpdateResponse>, Status> {
        metrics::init_metrics()
            .sequencer_withdrawal_submit_attempts
            .increment();
        let inner = request.into_inner();
        let caller_node_id = inner.caller_node_id;
        let envelope = inner
            .proposal
            .ok_or_else(|| Status::invalid_argument("missing proposal envelope"))?;
        let proposal = proposal_from_proto(&envelope)
            .map_err(|err| Status::invalid_argument(format!("invalid proposal envelope: {err}")))?;
        let withdrawal_nonce = envelope.withdrawal_nonce;
        let status_row = match self
            .prepare_submitted_action(&proposal, Some(caller_node_id), "submission")
            .await?
        {
            Ok(status_row) => status_row,
            Err(response) => {
                let metrics = metrics::init_metrics();
                if Self::submitted_action_rejected_for_wrong_caller(&response) {
                    metrics.sequencer_withdrawal_submit_deferred.increment();
                } else {
                    metrics.sequencer_withdrawal_submit_error.increment();
                }
                return Ok(Response::new(response));
            }
        };

        match self
            .withdrawal_state_store
            .sequencer_can_submit_proposal(&proposal, withdrawal_nonce)
            .await
        {
            Ok(SequencerDecision::Allowed) => {}
            Ok(SequencerDecision::Rejected(error)) => {
                metrics::init_metrics()
                    .sequencer_withdrawal_submit_deferred
                    .increment();
                return Ok(Response::new(SequencerUpdateResponse {
                    request_accepted: false,
                    error,
                }));
            }
            Err(err) => {
                metrics::init_metrics()
                    .sequencer_withdrawal_submit_error
                    .increment();
                return Err(Status::internal(format!(
                    "failed to validate sequenced submission: {err}"
                )));
            }
        }

        if matches!(
            status_row.state,
            WithdrawalState::MempoolAccepted | WithdrawalState::Confirmed
        ) {
            metrics::init_metrics()
                .sequencer_withdrawal_submit_mempool_accepted
                .increment();
            return Ok(Response::new(SequencerUpdateResponse {
                request_accepted: true,
                error: String::new(),
            }));
        }

        let current_base_height = self
            .current_confirmed_base_height()
            .map_err(|err| Status::internal(format!("withdrawal submission unavailable: {err}")))?;

        if let Some(remaining) =
            self.authorized_submit_retry_window_remaining(&status_row, current_base_height)
        {
            metrics::init_metrics()
                .sequencer_withdrawal_submit_deferred
                .increment();
            return Ok(Response::new(Self::deferred_submit_response(format!(
                "authorized retry window has not elapsed for withdrawal {:?} epoch {}; {} confirmed Base blocks remaining",
                proposal.id, proposal.epoch, remaining
            ))));
        }

        if status_row.last_submit_attempt_base_height.is_some()
            && self
                .authorized_transaction_already_seen(&proposal, &status_row)
                .await
        {
            self.withdrawal_state_store
                .record_authorized_mempool_accepted(&proposal)
                .await
                .map_err(|err| {
                    Status::internal(format!(
                        "failed to record observed mempool acceptance: {err}"
                    ))
                })?;
            return Ok(Response::new(SequencerUpdateResponse {
                request_accepted: true,
                error: String::new(),
            }));
        }

        match check_manual_submit_approval(&self.manual_submit_approval, &status_row)
            .map_err(|err| Status::internal(format!("manual approval check failed: {err}")))?
        {
            ManualSubmitApprovalDecision::Approved => {}
            ManualSubmitApprovalDecision::Deferred(reason) => {
                metrics::init_metrics()
                    .sequencer_withdrawal_submit_deferred
                    .increment();
                return Ok(Response::new(Self::deferred_submit_response(reason)));
            }
        }

        if let Err(err) = self.submitter.submission_node_available().await {
            metrics::init_metrics()
                .sequencer_withdrawal_submit_deferred
                .increment();
            let error = err.to_string();
            return Ok(Response::new(Self::deferred_submit_response(format!(
                "public Nockchain gRPC unavailable for withdrawal {:?} epoch {}: {error}",
                proposal.id, proposal.epoch
            ))));
        }

        let outcome = self
            .submit_until_final_outcome(&proposal)
            .await
            .map_err(|err| Status::internal(format!("withdrawal submission failed: {err}")))?;

        self.withdrawal_state_store
            .record_submit_outcome(
                &proposal,
                outcome.final_state,
                status_row
                    .submit_attempt_count
                    .saturating_add(outcome.submit_attempt_count),
                outcome.last_submit_attempt_base_height,
                outcome.last_submit_error,
            )
            .await
            .map_err(|err| {
                Status::internal(format!(
                    "failed to record sequenced submission outcome: {err}"
                ))
            })?;

        if outcome.final_state == WithdrawalState::Authorized {
            metrics::init_metrics()
                .sequencer_withdrawal_submit_error
                .increment();
            return Err(Status::internal(format!(
                "withdrawal submission failed after bounded retry budget for {:?} epoch {}",
                proposal.id, proposal.epoch
            )));
        }

        metrics::init_metrics()
            .sequencer_withdrawal_submit_mempool_accepted
            .increment();
        Ok(Response::new(SequencerUpdateResponse {
            request_accepted: true,
            error: String::new(),
        }))
    }

    /// Returns the earliest withdrawal nonce that still blocks sequencing
    /// progress, if any.
    async fn get_next_pending_withdrawal_ordering(
        &self,
        _request: Request<NextPendingWithdrawalOrderingRequest>,
    ) -> Result<Response<NextPendingWithdrawalOrderingResponse>, Status> {
        let row = self
            .withdrawal_state_store
            .next_pending_withdrawal_ordering()
            .await
            .map_err(|err| {
                Status::internal(format!(
                    "sequencer next pending ordering fetch failed: {err}"
                ))
            })?;
        Ok(Response::new(next_pending_withdrawal_ordering_response(
            row,
        )))
    }

    /// Returns the nonce-only sequencer frontier for bridge active-work
    /// filtering.
    async fn get_current_live_withdrawal_nonce(
        &self,
        _request: Request<CurrentLiveWithdrawalNonceRequest>,
    ) -> Result<Response<CurrentLiveWithdrawalNonceResponse>, Status> {
        let withdrawal_nonce = self
            .withdrawal_state_store
            .current_live_withdrawal_nonce()
            .await
            .map_err(|err| {
                Status::internal(format!(
                    "sequencer current live withdrawal nonce fetch failed: {err}"
                ))
            })?;
        let metrics = metrics::init_metrics();
        metrics
            .sequencer_withdrawal_frontier_present
            .swap(if withdrawal_nonce.is_some() { 1.0 } else { 0.0 });
        metrics
            .sequencer_withdrawal_frontier_nonce
            .swap(withdrawal_nonce.unwrap_or_default() as f64);
        Ok(Response::new(current_live_withdrawal_nonce_response(
            withdrawal_nonce,
        )))
    }

    /// Returns whether the requested withdrawal id is registered and is the
    /// current sequencer frontier.
    async fn frontier_allows_withdrawal(
        &self,
        request: Request<SequencerFrontierAllowsWithdrawalRequest>,
    ) -> Result<Response<SequencerFrontierAllowsWithdrawalResponse>, Status> {
        let id = request
            .into_inner()
            .withdrawal_id
            .ok_or_else(|| Status::invalid_argument("missing withdrawal id"))?;
        let id = withdrawal_id_from_proto(&id)
            .map_err(|err| Status::invalid_argument(format!("invalid withdrawal id: {err}")))?;
        let check = self
            .withdrawal_state_store
            .frontier_allows_withdrawal(&id)
            .await
            .map_err(|err| {
                Status::internal(format!("sequencer frontier withdrawal check failed: {err}"))
            })?;
        Ok(Response::new(frontier_allows_withdrawal_response(check)))
    }

    /// Returns the sequencer's current durable lifecycle view for a withdrawal.
    async fn get_sequenced_withdrawal_status(
        &self,
        request: Request<SequencedWithdrawalStatusRequest>,
    ) -> Result<Response<SequencedWithdrawalStatusResponse>, Status> {
        let id = request
            .into_inner()
            .withdrawal_id
            .ok_or_else(|| Status::invalid_argument("missing withdrawal id"))?;
        let id = withdrawal_id_from_proto(&id)
            .map_err(|err| Status::invalid_argument(format!("invalid withdrawal id: {err}")))?;
        let row = self.status_with_lazy_handoff(&id).await?;
        let withdrawal_nonce = self
            .withdrawal_state_store
            .withdrawal_nonce_for(&id)
            .await
            .map_err(|err| Status::internal(format!("sequencer nonce fetch failed: {err}")))?
            .unwrap_or(0);
        Ok(Response::new(sequenced_status_response(
            row,
            withdrawal_nonce,
            self.base_height_tracker.latest_confirmed_base_height(),
            self.handoff_window_blocks,
        )))
    }

    /// Returns canonical proposal artifacts projected from sequencer journal
    /// facts.
    async fn get_canonical_proposal_artifacts(
        &self,
        request: Request<SequencerCanonicalProposalArtifactsRequest>,
    ) -> Result<Response<SequencerCanonicalProposalArtifactsResponse>, Status> {
        let id = request
            .into_inner()
            .withdrawal_id
            .ok_or_else(|| Status::invalid_argument("missing withdrawal id"))?;
        let id = withdrawal_id_from_proto(&id)
            .map_err(|err| Status::invalid_argument(format!("invalid withdrawal id: {err}")))?;
        let artifacts = self
            .withdrawal_state_store
            .load_canonical_proposal_artifacts(&id)
            .await
            .map_err(|err| {
                Status::failed_precondition(format!(
                    "sequencer canonical proposal artifacts unavailable: {err}"
                ))
            })?;
        let response = canonical_artifacts_response(artifacts).map_err(|err| {
            Status::internal(format!(
                "failed to encode canonical proposal artifacts: {err}"
            ))
        })?;
        Ok(Response::new(response))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::fs;
    use std::sync::Mutex;
    use std::time::Duration;

    use alloy::primitives::Address;
    use nockapp::noun::slab::{NockJammer, NounSlab};
    use nockchain_math::belt::Belt;
    use nockchain_types::tx_engine::common::Hash as Tip5Hash;
    use nockchain_types::v1::Name;
    use noun_serde::NounDecode;
    use prost::Message;
    use tempfile::{tempdir, TempDir};
    use tonic::Request;

    use super::*;
    use crate::shared::ingress::proto::withdrawal_sequencer_server::WithdrawalSequencer;
    use crate::shared::ingress::proto::{
        SequencedWithdrawalStatusRequest, SequencerAdvancePrecanonicalHandoffRequest,
        SequencerAuthorizeProposalRequest, SequencerRecordCanonicalRequest,
        SequencerRecordSignedProposalRequest, SequencerRegisterWithdrawalRequest,
        SequencerReservedWithdrawalInputsRequest, SequencerSubmitProposalRequest,
        WithdrawalCommitCertificate, WithdrawalCommitSignature,
    };
    use crate::shared::signing::BridgeSigner;
    use crate::shared::types::BaseEventId;
    use crate::withdrawal::sequencer::approval::{
        approval_file_path, render_approval_record, write_approval_record_atomic,
    };
    use crate::withdrawal::sequencer::base_verifier::SequencerBaseWithdrawalRejection;
    use crate::withdrawal::sequencer::store::WithdrawalSubmissionEventType;
    use crate::withdrawal::submission::{
        WithdrawalNetworkSubmitStatus, WithdrawalSubmitAttemptStatus,
    };
    use crate::withdrawal::transport::{
        compute_withdrawal_commit_digest, note_name_from_proto, proposal_from_proto,
        proposal_to_proto, withdrawal_id_to_proto,
    };
    use crate::withdrawal::types::{
        NockWithdrawalRequestKernelData, WithdrawalProposalData, WithdrawalSnapshot,
    };

    #[derive(Debug, Clone)]
    struct ScriptedSubmitAttempt {
        result: Result<WithdrawalSubmitAttemptStatus, String>,
        advance_confirmed_base_height_to: Option<u64>,
    }

    impl ScriptedSubmitAttempt {
        fn returns(status: WithdrawalSubmitAttemptStatus) -> Self {
            Self {
                result: Ok(status),
                advance_confirmed_base_height_to: None,
            }
        }

        fn then_advance_confirmed_base_height_to(mut self, height: u64) -> Self {
            self.advance_confirmed_base_height_to = Some(height);
            self
        }
    }

    #[derive(Default)]
    struct RecordingSubmitter {
        submitted: Mutex<Vec<WithdrawalProposalData>>,
        resubmitted_raw_tx_ids: Mutex<Vec<String>>,
        resubmitted_raw_txs: Mutex<Vec<nockchain_types::v1::RawTx>>,
        base_height_tracker: Mutex<Option<Arc<SequencerBaseHeightTracker>>>,
        scripted_attempts: Mutex<VecDeque<ScriptedSubmitAttempt>>,
        submission_node_error: Mutex<Option<String>>,
    }

    impl RecordingSubmitter {
        fn attach_base_height_tracker(&self, tracker: Arc<SequencerBaseHeightTracker>) {
            *self
                .base_height_tracker
                .lock()
                .expect("base height tracker lock should not be poisoned") = Some(tracker);
        }

        fn script_submit_attempts(
            &self,
            attempts: impl IntoIterator<Item = ScriptedSubmitAttempt>,
        ) {
            self.scripted_attempts
                .lock()
                .expect("scripted submit attempts lock should not be poisoned")
                .extend(attempts);
        }

        fn set_submission_node_error(&self, error: impl Into<String>) {
            *self
                .submission_node_error
                .lock()
                .expect("submission node error lock should not be poisoned") = Some(error.into());
        }

        fn pop_scripted_attempt(&self) -> ScriptedSubmitAttempt {
            self.scripted_attempts
                .lock()
                .expect("scripted submit attempts lock should not be poisoned")
                .pop_front()
                .unwrap_or_else(|| {
                    ScriptedSubmitAttempt::returns(WithdrawalSubmitAttemptStatus::MempoolAccepted)
                })
        }

        fn apply_scripted_attempt_side_effects(&self, attempt: &ScriptedSubmitAttempt) {
            if let Some(next_height) = attempt.advance_confirmed_base_height_to {
                if let Some(tracker) = self
                    .base_height_tracker
                    .lock()
                    .expect("base height tracker lock should not be poisoned")
                    .as_ref()
                    .cloned()
                {
                    tracker.record_confirmed_base_height(next_height);
                }
            }
        }
    }

    #[async_trait]
    impl WithdrawalSubmitPort for RecordingSubmitter {
        async fn submission_node_available(&self) -> Result<(), BridgeError> {
            match self
                .submission_node_error
                .lock()
                .expect("submission node error lock should not be poisoned")
                .clone()
            {
                Some(error) => Err(BridgeError::Runtime(error)),
                None => Ok(()),
            }
        }

        async fn submit_withdrawal(
            &self,
            proposal: &WithdrawalProposalData,
        ) -> Result<WithdrawalSubmitAttemptStatus, BridgeError> {
            self.submitted
                .lock()
                .expect("submitted proposals lock")
                .push(proposal.clone());
            let attempt = self.pop_scripted_attempt();
            self.apply_scripted_attempt_side_effects(&attempt);
            match attempt.result {
                Ok(status) => Ok(status),
                Err(err) => Err(BridgeError::Runtime(err)),
            }
        }

        async fn resubmit_raw_tx(
            &self,
            raw_tx: &nockchain_types::v1::RawTx,
        ) -> Result<WithdrawalNetworkSubmitStatus, BridgeError> {
            self.resubmitted_raw_tx_ids
                .lock()
                .expect("resubmitted raw tx ids lock")
                .push(crate::withdrawal::raw_tx::raw_tx_id_base58(raw_tx));
            self.resubmitted_raw_txs
                .lock()
                .expect("resubmitted raw txs lock")
                .push(raw_tx.clone());
            Ok(WithdrawalNetworkSubmitStatus::MempoolAccepted)
        }

        async fn transaction_mempool_accepted(
            &self,
            _proposal: &WithdrawalProposalData,
        ) -> Result<Option<bool>, BridgeError> {
            Ok(None)
        }

        async fn get_transaction_included_block(
            &self,
            _submitted_raw_tx_id: &str,
        ) -> Result<Option<crate::withdrawal::submission::WithdrawalIncludedBlock>, BridgeError>
        {
            Ok(None)
        }
    }

    struct ScriptedBaseWithdrawalVerifier {
        results: Mutex<VecDeque<Result<(), SequencerBaseWithdrawalRejection>>>,
        calls: Mutex<Vec<TrackedWithdrawalRequest>>,
    }

    impl ScriptedBaseWithdrawalVerifier {
        fn new(
            results: impl IntoIterator<Item = Result<(), SequencerBaseWithdrawalRejection>>,
        ) -> Self {
            Self {
                results: Mutex::new(results.into_iter().collect()),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn accepting() -> Self {
            Self::new([Ok(())])
        }

        fn calls(&self) -> Vec<TrackedWithdrawalRequest> {
            self.calls
                .lock()
                .expect("verifier calls lock should not be poisoned")
                .clone()
        }
    }

    #[async_trait]
    impl SequencerBaseWithdrawalVerifier for ScriptedBaseWithdrawalVerifier {
        async fn verify(
            &self,
            tracked: &TrackedWithdrawalRequest,
        ) -> Result<(), SequencerBaseWithdrawalRejection> {
            self.calls
                .lock()
                .expect("verifier calls lock should not be poisoned")
                .push(tracked.clone());
            self.results
                .lock()
                .expect("verifier results lock should not be poisoned")
                .pop_front()
                .unwrap_or(Ok(()))
        }
    }

    const TEST_WITHDRAWAL_OPERATOR_KEY: &str =
        "0x0000000000000000000000000000000000000000000000000000000000000001";

    fn test_withdrawal_signer() -> BridgeSigner {
        BridgeSigner::new(TEST_WITHDRAWAL_OPERATOR_KEY.to_string())
            .expect("valid withdrawal test signer")
    }

    fn sample_node_pkhs() -> Vec<Tip5Hash> {
        vec![
            Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            Tip5Hash([Belt(11), Belt(12), Belt(13), Belt(14), Belt(15)]),
            Tip5Hash([Belt(21), Belt(22), Belt(23), Belt(24), Belt(25)]),
        ]
    }

    fn sample_node_eth_addresses() -> HashMap<u64, Address> {
        let address = test_withdrawal_signer().address();
        (0_u64..3).map(|node_id| (node_id, address)).collect()
    }

    async fn open_rpc_service() -> (
        WithdrawalSequencerRpcService,
        Arc<WithdrawalSequencerStore>,
        Arc<SequencerBaseHeightTracker>,
        Arc<RecordingSubmitter>,
        TempDir,
    ) {
        open_rpc_service_with_handoff_window(
            crate::withdrawal::state::WithdrawalFallbackPolicy::default().submission_timeout_blocks,
        )
        .await
    }

    async fn open_rpc_service_with_handoff_window(
        handoff_window_blocks: u64,
    ) -> (
        WithdrawalSequencerRpcService,
        Arc<WithdrawalSequencerStore>,
        Arc<SequencerBaseHeightTracker>,
        Arc<RecordingSubmitter>,
        TempDir,
    ) {
        let dir = tempdir().expect("tempdir");
        let withdrawal_state_store = Arc::new(
            WithdrawalSequencerStore::open(dir.path().join("withdrawal-state-store.sqlite"))
                .await
                .expect("withdrawal state store"),
        );
        let base_height_tracker = Arc::new(SequencerBaseHeightTracker::default());
        base_height_tracker.record_confirmed_base_height(100);
        let submitter = Arc::new(RecordingSubmitter::default());
        submitter.attach_base_height_tracker(base_height_tracker.clone());
        let rpc = WithdrawalSequencerRpcService::with_submit_retry_policy(
            withdrawal_state_store.clone(),
            submitter.clone(),
            base_height_tracker.clone(),
            Arc::new(ScriptedBaseWithdrawalVerifier::accepting()),
            handoff_window_blocks,
            sample_node_pkhs(),
            sample_node_eth_addresses(),
            RetryPolicy {
                min_delay: Duration::ZERO,
                max_delay: Duration::ZERO,
                // 2 retries after the first attempt => 3 total attempts.
                max_times: Some(2),
                jitter: false,
            },
        );
        (
            rpc, withdrawal_state_store, base_height_tracker, submitter, dir,
        )
    }

    fn sample_base_event_id(start: u8) -> BaseEventId {
        BaseEventId((0..32).map(|offset| start.wrapping_add(offset)).collect())
    }

    fn refunded_withdrawal_base_event_id() -> BaseEventId {
        BaseEventId(
            hex::decode("45cfbf831f2abf377164f857a2bc47338fcaa8f4f12a5986a3ba9bef35afeabd")
                .expect("refunded base event id hex"),
        )
    }

    fn sample_request() -> NockWithdrawalRequestKernelData {
        NockWithdrawalRequestKernelData {
            base_event_id: sample_base_event_id(0xaa),
            recipient: Tip5Hash([Belt(101), Belt(102), Belt(103), Belt(104), Belt(105)]),
            amount: 123_456,
            base_batch_end: 777,
            as_of: Tip5Hash([Belt(11), Belt(22), Belt(33), Belt(44), Belt(55)]),
        }
    }

    fn sample_transaction() -> nockchain_types::v1::Transaction {
        const TRANSACTION_JAM: &[u8] = include_bytes!(
            "../../../test-fixtures/transactions/9MpGym52AumtwyBxYPyVsWHvcamUYwZkc1Nq7w3cFGF28u8ceVDwt3e.tx"
        );

        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = slab
            .cue_into(TRANSACTION_JAM.to_vec().into())
            .expect("failed to cue transaction fixture");

        let space = nockapp::NounAllocator::noun_space(&slab);
        nockchain_types::v1::Transaction::from_noun(&noun, &space)
            .expect("failed to decode transaction fixture")
    }

    fn sample_proposal(epoch: u64) -> WithdrawalProposalData {
        let request = sample_request();
        let transaction = sample_transaction();
        WithdrawalProposalData {
            id: request.withdrawal_id(),
            recipient: request.recipient.clone(),
            amount: request.amount.saturating_sub(111),
            burned_amount: request.amount,
            base_batch_end: request.base_batch_end,
            epoch,
            snapshot: WithdrawalSnapshot {
                height: 42 + epoch,
                block_id: Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            },
            selected_inputs: transaction.normalized_input_names(),
            transaction,
        }
    }

    fn expected_proposer(proposal: &WithdrawalProposalData, handoff_index: u64) -> u64 {
        crate::shared::proposer::withdrawal_turn_proposer(
            &proposal.id,
            proposal.epoch,
            handoff_index,
            &sample_node_pkhs(),
        ) as u64
    }

    async fn register_proposal_ordering(
        withdrawal_state_store: &WithdrawalSequencerStore,
        proposal: &WithdrawalProposalData,
        withdrawal_nonce: u64,
    ) {
        withdrawal_state_store
            .ensure_tracked_withdrawal_ordering_at_base_height(
                &TrackedWithdrawalRequest {
                    id: proposal.id.clone(),
                    recipient: proposal.recipient.clone(),
                    amount: proposal.burned_amount,
                    base_batch_end: proposal.base_batch_end,
                    withdrawal_nonce,
                },
                100,
            )
            .await
            .expect("register withdrawal ordering");
    }

    async fn authorize_for_submission(
        rpc: &WithdrawalSequencerRpcService,
        withdrawal_state_store: &WithdrawalSequencerStore,
        base_height_tracker: &SequencerBaseHeightTracker,
        proposal: &WithdrawalProposalData,
        withdrawal_nonce: u64,
    ) -> (
        crate::shared::ingress::proto::WithdrawalProposalEnvelope,
        u64,
    ) {
        register_proposal_ordering(withdrawal_state_store, proposal, withdrawal_nonce).await;
        withdrawal_state_store
            .record_proposal_canonicalized(proposal, 100)
            .await
            .expect("canonicalize proposal");
        withdrawal_state_store
            .record_proposal_signed(proposal, 1, 100)
            .await
            .expect("record signed proposal");
        base_height_tracker.record_confirmed_base_height(120);

        let mut proposal_proto = proposal_to_proto(proposal).expect("proposal proto");
        proposal_proto.withdrawal_nonce = withdrawal_nonce;
        let caller_node_id = expected_proposer(proposal, 0);
        let authorize = rpc
            .authorize_proposal(Request::new(SequencerAuthorizeProposalRequest {
                proposal: Some(proposal_proto.clone()),
                commit_certificate: Some(sample_commit_certificate(proposal).await),
                caller_node_id,
            }))
            .await
            .expect("authorize response")
            .into_inner();
        assert!(
            authorize.request_accepted,
            "authorization response: {authorize:?}"
        );

        (proposal_proto, caller_node_id)
    }

    async fn register_withdrawal_request(
        rpc: &WithdrawalSequencerRpcService,
        proposal: &WithdrawalProposalData,
        withdrawal_nonce: u64,
    ) -> SequencerUpdateResponse {
        rpc.register_withdrawal(Request::new(SequencerRegisterWithdrawalRequest {
            base_event_id: proposal.id.base_event_id.0.clone(),
            withdrawal_nonce,
            recipient: proposal.recipient.to_be_limb_bytes().to_vec(),
            burned_amount: proposal.burned_amount,
            base_batch_end: proposal.base_batch_end,
        }))
        .await
        .expect("register withdrawal")
        .into_inner()
    }

    fn incompletely_signed_transaction() -> nockchain_types::v1::Transaction {
        let mut transaction = sample_transaction();
        let nockchain_types::v1::Transaction::V1(tx) = &mut transaction;
        tx.metadata.inputs = nockchain_types::v1::InputMetadata::LegacySignatures(
            nockchain_types::v1::SignatureMap(Vec::new()),
        );
        transaction
    }

    async fn sample_commit_certificate(
        proposal: &WithdrawalProposalData,
    ) -> WithdrawalCommitCertificate {
        let proposal_hash = proposal.proposal_hash().expect("proposal hash");
        let digest = compute_withdrawal_commit_digest(&proposal.id, proposal.epoch, &proposal_hash)
            .expect("commit digest");
        let signature = test_withdrawal_signer()
            .sign_hash(&digest)
            .await
            .expect("commit signature");
        WithdrawalCommitCertificate {
            withdrawal_id: Some(withdrawal_id_to_proto(&proposal.id)),
            epoch: proposal.epoch,
            proposal_hash,
            signatures: vec![WithdrawalCommitSignature {
                signer_node_id: 1,
                withdrawal_id: Some(withdrawal_id_to_proto(&proposal.id)),
                epoch: proposal.epoch,
                proposal_hash: proposal
                    .proposal_hash()
                    .expect("proposal hash for signature"),
                signature: signature.as_bytes().to_vec(),
            }],
        }
    }

    #[test]
    fn elapsed_handoff_turns_rejects_regressing_base_height() {
        let err = elapsed_handoff_turns(200, 150, 10)
            .expect_err("regressing base height should be rejected");
        assert!(err.contains("behind stored turn_started_base_height"));
    }

    #[tokio::test]
    async fn sequencer_rpc_service_registers_pending_turn_start_base_height() {
        let (rpc, withdrawal_state_store, base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(0);
        base_height_tracker.record_confirmed_base_height(321);

        let registered = rpc
            .register_withdrawal(Request::new(SequencerRegisterWithdrawalRequest {
                base_event_id: proposal.id.base_event_id.0.clone(),
                withdrawal_nonce: 1,
                recipient: proposal.recipient.to_be_limb_bytes().to_vec(),
                burned_amount: proposal.burned_amount,
                base_batch_end: proposal.base_batch_end,
            }))
            .await
            .expect("register withdrawal")
            .into_inner();
        assert!(registered.request_accepted);

        let status = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .expect("sequenced withdrawal exists");
        assert_eq!(status.state, WithdrawalState::Pending);
        assert_eq!(status.id.as_of, zero_tip5_hash());
        assert_eq!(status.id.base_event_id, proposal.id.base_event_id);
        assert_eq!(status.handoff_index, 0);
        assert_eq!(status.turn_started_base_height, Some(321));
    }

    #[tokio::test]
    async fn sequencer_rpc_service_rejects_registration_without_base_event_id() {
        let (rpc, withdrawal_state_store, base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(0);
        base_height_tracker.record_confirmed_base_height(321);

        let err = rpc
            .register_withdrawal(Request::new(SequencerRegisterWithdrawalRequest {
                base_event_id: Vec::new(),
                withdrawal_nonce: 1,
                recipient: proposal.recipient.to_be_limb_bytes().to_vec(),
                burned_amount: proposal.burned_amount,
                base_batch_end: proposal.base_batch_end,
            }))
            .await
            .expect_err("missing base event id should be invalid");

        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("base_event_id must be 32 bytes"));
        assert!(withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .is_none());
    }

    #[tokio::test]
    async fn sequencer_rpc_service_rejects_registration_before_base_height_observed() {
        let dir = tempdir().expect("tempdir");
        let withdrawal_state_store = Arc::new(
            WithdrawalSequencerStore::open(dir.path().join("withdrawal-state-store.sqlite"))
                .await
                .expect("withdrawal state store"),
        );
        let base_height_tracker = Arc::new(SequencerBaseHeightTracker::default());
        let submitter = Arc::new(RecordingSubmitter::default());
        submitter.attach_base_height_tracker(base_height_tracker.clone());
        let rpc = WithdrawalSequencerRpcService::with_submit_retry_policy(
            withdrawal_state_store.clone(),
            submitter,
            base_height_tracker,
            Arc::new(ScriptedBaseWithdrawalVerifier::accepting()),
            crate::withdrawal::state::WithdrawalFallbackPolicy::default().submission_timeout_blocks,
            sample_node_pkhs(),
            sample_node_eth_addresses(),
            RetryPolicy {
                min_delay: Duration::ZERO,
                max_delay: Duration::ZERO,
                max_times: Some(2),
                jitter: false,
            },
        );
        let proposal = sample_proposal(0);

        let response = rpc
            .register_withdrawal(Request::new(SequencerRegisterWithdrawalRequest {
                base_event_id: proposal.id.base_event_id.0.clone(),
                withdrawal_nonce: 1,
                recipient: proposal.recipient.to_be_limb_bytes().to_vec(),
                burned_amount: proposal.burned_amount,
                base_batch_end: proposal.base_batch_end,
            }))
            .await
            .expect("register withdrawal")
            .into_inner();

        assert!(!response.request_accepted);
        assert!(response
            .error
            .contains("has not observed a confirmed Base height yet"));
        assert!(withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .is_none());
    }

    #[tokio::test]
    async fn sequencer_rpc_service_verified_registration_writes_pending() {
        let (rpc, withdrawal_state_store, _base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let verifier = Arc::new(ScriptedBaseWithdrawalVerifier::accepting());
        let rpc = rpc.with_base_withdrawal_verifier(verifier.clone());
        let proposal = sample_proposal(0);

        let response = register_withdrawal_request(&rpc, &proposal, 1).await;

        assert!(
            response.request_accepted,
            "registration should be accepted: {}",
            response.error
        );
        assert_eq!(verifier.calls().len(), 1);
        let row = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .expect("sequenced withdrawal exists");
        assert_eq!(row.state, WithdrawalState::Pending);
        assert_eq!(row.withdrawal_nonce, Some(1));
    }

    #[tokio::test]
    async fn sequencer_rpc_service_ignores_explicitly_refunded_withdrawal_registration() {
        let (rpc, withdrawal_state_store, _base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let verifier = Arc::new(ScriptedBaseWithdrawalVerifier::accepting());
        let rpc = rpc.with_base_withdrawal_verifier(verifier.clone());
        let mut proposal = sample_proposal(0);
        proposal.id.base_event_id = refunded_withdrawal_base_event_id();

        let response = register_withdrawal_request(&rpc, &proposal, 1).await;

        assert!(!response.request_accepted);
        assert!(response.error.contains("explicitly refunded"));
        assert!(response
            .error
            .contains("45cfbf831f2abf377164f857a2bc47338fcaa8f4f12a5986a3ba9bef35afeabd"));
        assert_eq!(verifier.calls().len(), 0);
        assert!(withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .is_none());
    }

    #[tokio::test]
    async fn sequencer_rpc_service_verifier_rejection_leaves_no_pending_row() {
        let (rpc, withdrawal_state_store, _base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let verifier = Arc::new(ScriptedBaseWithdrawalVerifier::new([Err(
            SequencerBaseWithdrawalRejection::MissingBaseEventId {
                base_event_id_hex: "0xdead".into(),
                batch_start: 10,
                batch_end: 19,
            },
        )]));
        let rpc = rpc.with_base_withdrawal_verifier(verifier);
        let proposal = sample_proposal(0);

        let response = register_withdrawal_request(&rpc, &proposal, 1).await;

        assert!(!response.request_accepted);
        assert!(response.error.contains("was not found"));
        assert!(withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .is_none());
    }

    #[tokio::test]
    async fn sequencer_rpc_service_verifier_rpc_failure_fails_closed() {
        let (rpc, withdrawal_state_store, _base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let verifier = Arc::new(ScriptedBaseWithdrawalVerifier::new([Err(
            SequencerBaseWithdrawalRejection::RpcFailure {
                error: "base rpc unavailable".into(),
            },
        )]));
        let rpc = rpc.with_base_withdrawal_verifier(verifier);
        let proposal = sample_proposal(0);

        let response = register_withdrawal_request(&rpc, &proposal, 1).await;

        assert!(!response.request_accepted);
        assert!(response.error.contains("Base RPC"));
        assert!(withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .is_none());
    }

    #[tokio::test]
    async fn sequencer_rpc_service_idempotent_registration_skips_verifier_and_succeeds() {
        let (rpc, withdrawal_state_store, _base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let verifier = Arc::new(ScriptedBaseWithdrawalVerifier::new([Ok(())]));
        let rpc = rpc.with_base_withdrawal_verifier(verifier.clone());
        let proposal = sample_proposal(0);

        let first = register_withdrawal_request(&rpc, &proposal, 1).await;
        let second = register_withdrawal_request(&rpc, &proposal, 1).await;

        assert!(first.request_accepted, "first accepted: {}", first.error);
        assert!(second.request_accepted, "second accepted: {}", second.error);
        assert_eq!(verifier.calls().len(), 1);
        let row = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .expect("sequenced withdrawal exists");
        assert_eq!(row.withdrawal_nonce, Some(1));
    }

    #[tokio::test]
    async fn sequencer_rpc_service_duplicate_base_event_with_different_facts_fails_before_verifier()
    {
        let (rpc, withdrawal_state_store, _base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let verifier = Arc::new(ScriptedBaseWithdrawalVerifier::new([Ok(())]));
        let rpc = rpc.with_base_withdrawal_verifier(verifier.clone());
        let proposal = sample_proposal(0);
        let mut mismatched = proposal.clone();
        mismatched.burned_amount += 1;

        let first = register_withdrawal_request(&rpc, &proposal, 1).await;
        let second = register_withdrawal_request(&rpc, &mismatched, 1).await;

        assert!(first.request_accepted, "first accepted: {}", first.error);
        assert!(!second.request_accepted);
        assert!(second.error.contains("different request facts"));
        assert_eq!(verifier.calls().len(), 1);
        let row = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .expect("sequenced withdrawal exists");
        assert_eq!(row.gross_burned_amount, Some(proposal.burned_amount));
    }

    #[tokio::test]
    async fn malicious_first_registration_cannot_block_later_valid_withdrawal() {
        let (rpc, withdrawal_state_store, _base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let verifier = Arc::new(ScriptedBaseWithdrawalVerifier::new([
            Err(SequencerBaseWithdrawalRejection::MissingBaseEventId {
                base_event_id_hex: "0xbad".into(),
                batch_start: 10,
                batch_end: 19,
            }),
            Ok(()),
        ]));
        let rpc = rpc.with_base_withdrawal_verifier(verifier);
        let bogus = sample_proposal(0);
        let mut valid = sample_proposal(1);
        valid.id.base_event_id = sample_base_event_id(0xbb);

        let rejected = register_withdrawal_request(&rpc, &bogus, 1).await;
        let accepted = register_withdrawal_request(&rpc, &valid, 1).await;

        assert!(!rejected.request_accepted);
        assert!(
            accepted.request_accepted,
            "valid accepted: {}",
            accepted.error
        );
        assert!(withdrawal_state_store
            .fetch_sequenced_withdrawal(&bogus.id)
            .await
            .expect("fetch bogus sequenced withdrawal")
            .is_none());
        let row = withdrawal_state_store
            .fetch_sequenced_withdrawal(&valid.id)
            .await
            .expect("fetch valid sequenced withdrawal")
            .expect("valid sequenced withdrawal exists");
        assert_eq!(row.withdrawal_nonce, Some(1));
        assert_eq!(
            withdrawal_state_store
                .current_live_withdrawal_nonce()
                .await
                .expect("current live nonce"),
            Some(1)
        );
    }

    #[tokio::test]
    async fn sequencer_rpc_service_lists_reserved_inputs_for_canonical_withdrawal() {
        let (rpc, _withdrawal_state_store, _base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(0);
        let commit_certificate = sample_commit_certificate(&proposal).await;

        let registered = rpc
            .register_withdrawal(Request::new(SequencerRegisterWithdrawalRequest {
                base_event_id: proposal.id.base_event_id.0.clone(),
                withdrawal_nonce: 1,
                recipient: proposal.recipient.to_be_limb_bytes().to_vec(),
                burned_amount: proposal.burned_amount,
                base_batch_end: proposal.base_batch_end,
            }))
            .await
            .expect("register withdrawal")
            .into_inner();
        assert!(registered.request_accepted);

        let mut proposal_proto = proposal_to_proto(&proposal).expect("proposal proto");
        proposal_proto.withdrawal_nonce = 1;
        let recorded = rpc
            .record_canonical_proposal(Request::new(SequencerRecordCanonicalRequest {
                proposal: Some(proposal_proto),
                commit_certificate: Some(commit_certificate),
                caller_node_id: expected_proposer(&proposal, 0),
            }))
            .await
            .expect("record canonical proposal")
            .into_inner();
        assert!(
            recorded.request_accepted,
            "canonical proposal should succeed: {}",
            recorded.error
        );

        let listed = rpc
            .get_reserved_withdrawal_inputs(Request::new(
                SequencerReservedWithdrawalInputsRequest {},
            ))
            .await
            .expect("list reserved inputs")
            .into_inner();
        let listed_inputs = listed
            .reserved_inputs
            .iter()
            .map(note_name_from_proto)
            .collect::<Result<Vec<_>, _>>()
            .expect("decode listed reserved inputs");
        assert_eq!(listed_inputs, proposal.selected_inputs);
    }

    #[tokio::test]
    async fn sequencer_rpc_service_rejects_canonical_certificate_with_invalid_signature() {
        let (rpc, withdrawal_state_store, _base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(0);
        register_proposal_ordering(withdrawal_state_store.as_ref(), &proposal, 1).await;

        let mut commit_certificate = sample_commit_certificate(&proposal).await;
        commit_certificate.signatures[0].signature[0] ^= 0x01;
        let mut proposal_proto = proposal_to_proto(&proposal).expect("proposal proto");
        proposal_proto.withdrawal_nonce = 1;

        let err = rpc
            .record_canonical_proposal(Request::new(SequencerRecordCanonicalRequest {
                proposal: Some(proposal_proto),
                commit_certificate: Some(commit_certificate),
                caller_node_id: expected_proposer(&proposal, 0),
            }))
            .await
            .expect_err("invalid commit signature should be rejected");

        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(
            err.message()
                .contains("failed Ethereum address verification"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn sequencer_rpc_service_current_live_withdrawal_nonce_returns_empty_frontier() {
        // The nonce-only frontier RPC reports found=false when no sequencer row
        // is currently unreleased.
        let (rpc, _withdrawal_state_store, _base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;

        let response = rpc
            .get_current_live_withdrawal_nonce(Request::new(CurrentLiveWithdrawalNonceRequest {}))
            .await
            .expect("current live nonce response")
            .into_inner();

        assert!(!response.found);
        assert_eq!(response.withdrawal_nonce, 0);
    }

    #[tokio::test]
    async fn sequencer_rpc_service_current_live_withdrawal_nonce_returns_frontier() {
        // The nonce-only frontier RPC exposes the lowest unreleased sequencer
        // nonce without requiring clients to key active work by withdrawal id.
        let (rpc, withdrawal_state_store, _base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(0);

        register_proposal_ordering(&withdrawal_state_store, &proposal, 1).await;

        let response = rpc
            .get_current_live_withdrawal_nonce(Request::new(CurrentLiveWithdrawalNonceRequest {}))
            .await
            .expect("current live nonce response")
            .into_inner();

        assert!(response.found);
        assert_eq!(response.withdrawal_nonce, 1);
    }

    #[tokio::test]
    async fn sequencer_rpc_service_frontier_allows_registered_frontier_withdrawal() {
        // The base-event frontier RPC answers the exact active-work question:
        // this burn event must already be registered and must be the current frontier.
        let (rpc, withdrawal_state_store, _base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(0);

        register_proposal_ordering(&withdrawal_state_store, &proposal, 1).await;

        let response = rpc
            .frontier_allows_withdrawal(Request::new(SequencerFrontierAllowsWithdrawalRequest {
                withdrawal_id: Some(withdrawal_id_to_proto(&proposal.id)),
            }))
            .await
            .expect("frontier check response")
            .into_inner();

        assert!(response.allowed);
        assert!(response.registered);
        assert!(response.is_frontier);
    }

    #[tokio::test]
    async fn sequencer_rpc_service_frontier_rejects_unregistered_withdrawal() {
        // Missing sequencer registration makes the id inactive even when the
        // client knows a local withdrawal request.
        let (rpc, _withdrawal_state_store, _base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(0);

        let response = rpc
            .frontier_allows_withdrawal(Request::new(SequencerFrontierAllowsWithdrawalRequest {
                withdrawal_id: Some(withdrawal_id_to_proto(&proposal.id)),
            }))
            .await
            .expect("frontier check response")
            .into_inner();

        assert!(!response.allowed);
        assert!(!response.registered);
        assert!(!response.is_frontier);
    }

    #[tokio::test]
    async fn sequencer_rpc_service_rejects_canonical_proposal_when_tx_inputs_do_not_match() {
        let (rpc, _withdrawal_state_store, _base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let mut proposal = sample_proposal(7);
        proposal.selected_inputs = vec![Name::new(
            Tip5Hash([Belt(901), Belt(902), Belt(903), Belt(904), Belt(905)]),
            Tip5Hash([Belt(911), Belt(912), Belt(913), Belt(914), Belt(915)]),
        )];

        let registered = rpc
            .register_withdrawal(Request::new(SequencerRegisterWithdrawalRequest {
                base_event_id: proposal.id.base_event_id.0.clone(),
                withdrawal_nonce: 1,
                recipient: proposal.recipient.to_be_limb_bytes().to_vec(),
                burned_amount: proposal.burned_amount,
                base_batch_end: proposal.base_batch_end,
            }))
            .await
            .expect("register withdrawal")
            .into_inner();
        assert!(registered.request_accepted);

        let mut proposal_proto = proposal_to_proto(&proposal).expect("proposal proto");
        proposal_proto.withdrawal_nonce = 1;
        let recorded = rpc
            .record_canonical_proposal(Request::new(SequencerRecordCanonicalRequest {
                proposal: Some(proposal_proto),
                commit_certificate: Some(sample_commit_certificate(&proposal).await),
                caller_node_id: expected_proposer(&proposal, 0),
            }))
            .await
            .expect("record canonical proposal")
            .into_inner();
        assert!(!recorded.request_accepted);
        assert!(recorded
            .error
            .contains("transaction inputs do not match proposal selected_inputs"));
    }

    #[tokio::test]
    async fn sequencer_rpc_service_rejects_authorization_when_pinned_reserved_inputs_mismatch() {
        use diesel::{Connection, ExpressionMethods, QueryDsl, RunQueryDsl};

        use crate::withdrawal::sequencer::schema::withdrawal_reserved_inputs::dsl as reserved;

        let (rpc, withdrawal_state_store, _base_height_tracker, _submitter, dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(90);
        let withdrawal_nonce = 1;
        let wrong_input = Name::new(
            Tip5Hash([Belt(991), Belt(992), Belt(993), Belt(994), Belt(995)]),
            Tip5Hash([Belt(996), Belt(997), Belt(998), Belt(999), Belt(1_000)]),
        );

        register_proposal_ordering(&withdrawal_state_store, &proposal, withdrawal_nonce).await;
        withdrawal_state_store
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("canonicalize proposal");

        let path = dir.path().join("withdrawal-state-store.sqlite");
        let path_str = path.to_str().expect("sqlite path should be valid unicode");
        let mut conn = diesel::SqliteConnection::establish(path_str)
            .expect("open sqlite connection for reservation corruption");
        diesel::delete(
            reserved::withdrawal_reserved_inputs
                .filter(
                    reserved::withdrawal_id_as_of.eq(proposal.id.as_of.to_be_limb_bytes().to_vec()),
                )
                .filter(
                    reserved::withdrawal_id_base_event_id.eq(proposal.id.base_event_id.0.clone()),
                ),
        )
        .execute(&mut conn)
        .expect("delete reserved inputs for proposal");
        diesel::insert_into(reserved::withdrawal_reserved_inputs)
            .values((
                reserved::withdrawal_id_as_of.eq(proposal.id.as_of.to_be_limb_bytes().to_vec()),
                reserved::withdrawal_id_base_event_id.eq(proposal.id.base_event_id.0.clone()),
                reserved::epoch.eq(i64::try_from(proposal.epoch).expect("epoch fits in sqlite")),
                reserved::input_first.eq(wrong_input.first.to_be_limb_bytes().to_vec()),
                reserved::input_last.eq(wrong_input.last.to_be_limb_bytes().to_vec()),
                reserved::created_at.eq(1_i64),
                reserved::updated_at.eq(1_i64),
            ))
            .execute(&mut conn)
            .expect("insert mismatched reserved input");

        let mut proposal_proto = proposal_to_proto(&proposal).expect("proposal proto");
        proposal_proto.withdrawal_nonce = withdrawal_nonce;
        let expected_caller = crate::shared::proposer::withdrawal_turn_proposer(
            &proposal.id,
            proposal.epoch,
            0,
            &sample_node_pkhs(),
        ) as u64;
        let err = rpc
            .authorize_proposal(Request::new(SequencerAuthorizeProposalRequest {
                proposal: Some(proposal_proto),
                commit_certificate: Some(sample_commit_certificate(&proposal).await),
                caller_node_id: expected_caller,
            }))
            .await
            .expect_err("authorization should fail on mismatched reserved inputs");
        assert_eq!(err.code(), tonic::Code::Internal);
        assert!(err
            .message()
            .contains("do not match pinned canonical proposal"));

        let status = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal after failed authorization")
            .expect("sequenced withdrawal exists");
        assert_eq!(status.state, WithdrawalState::PeerCanonical);
    }

    #[tokio::test]
    async fn sequencer_rpc_service_records_peer_canonical_proposal_before_authorization() {
        let (rpc, withdrawal_state_store, base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(0);
        let withdrawal_nonce = 1;
        base_height_tracker.record_confirmed_base_height(100);
        register_proposal_ordering(&withdrawal_state_store, &proposal, withdrawal_nonce).await;

        let mut proposal_proto = proposal_to_proto(&proposal).expect("proposal proto");
        proposal_proto.withdrawal_nonce = withdrawal_nonce;
        let canonical = rpc
            .record_canonical_proposal(Request::new(SequencerRecordCanonicalRequest {
                proposal: Some(proposal_proto),
                commit_certificate: Some(sample_commit_certificate(&proposal).await),
                caller_node_id: expected_proposer(&proposal, 0),
            }))
            .await
            .expect("canonical response")
            .into_inner();
        assert!(canonical.request_accepted);

        let status = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .expect("sequenced withdrawal exists");
        assert_eq!(status.state, WithdrawalState::PeerCanonical);
        assert_eq!(status.handoff_index, 0);
        assert_eq!(status.turn_started_base_height, Some(100));
    }

    #[tokio::test]
    async fn canonicalization_uses_base_event_registration_identity() {
        let (rpc, withdrawal_state_store, base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(0);
        let mut registered = proposal.clone();
        registered.id.as_of = Tip5Hash([Belt(901), Belt(902), Belt(903), Belt(904), Belt(905)]);
        let withdrawal_nonce = 1;
        base_height_tracker.record_confirmed_base_height(1_000);

        let registered_response =
            register_withdrawal_request(&rpc, &registered, withdrawal_nonce).await;
        assert!(
            registered_response.request_accepted,
            "{}",
            registered_response.error
        );
        let pending = withdrawal_state_store
            .fetch_sequenced_withdrawal(&registered.id)
            .await
            .expect("fetch pending registration")
            .expect("pending registration exists");
        assert_eq!(pending.state, WithdrawalState::Pending);
        assert_eq!(pending.id.as_of, zero_tip5_hash());
        assert_eq!(pending.id.base_event_id, proposal.id.base_event_id);
        assert_eq!(
            crate::shared::proposer::withdrawal_turn_proposer(
                &proposal.id,
                proposal.epoch,
                0,
                &sample_node_pkhs()
            ),
            crate::shared::proposer::withdrawal_turn_proposer(
                &registered.id,
                proposal.epoch,
                0,
                &sample_node_pkhs()
            )
        );

        let mut proposal_proto = proposal_to_proto(&proposal).expect("proposal proto");
        proposal_proto.withdrawal_nonce = withdrawal_nonce;
        let canonical = rpc
            .record_canonical_proposal(Request::new(SequencerRecordCanonicalRequest {
                proposal: Some(proposal_proto),
                commit_certificate: Some(sample_commit_certificate(&proposal).await),
                caller_node_id: expected_proposer(&proposal, 0),
            }))
            .await
            .expect("canonical response")
            .into_inner();
        assert!(
            canonical.request_accepted,
            "canonicalization should accept the Base-event owner: {}",
            canonical.error
        );

        let status = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .expect("sequenced withdrawal exists");
        assert_eq!(status.state, WithdrawalState::PeerCanonical);
        assert_eq!(status.id.as_of, proposal.id.as_of);
        assert_eq!(status.id.base_event_id, proposal.id.base_event_id);
    }

    #[test]
    fn sequenced_withdrawal_status_timing_fields_roundtrip() {
        let status = SequencedWithdrawalStatusResponse {
            found: true,
            current_epoch: 2,
            state: "pending".to_string(),
            proposal_hash: "proposal".to_string(),
            authorized_transaction_name: "tx".to_string(),
            withdrawal_nonce: 3,
            handoff_index: 4,
            turn_started_base_height: Some(50),
            current_confirmed_base_height: Some(61),
            handoff_window_blocks: 17,
            blocks_until_handoff: Some(6),
        };
        let mut encoded = Vec::new();
        status.encode(&mut encoded).expect("encode status");
        let decoded =
            SequencedWithdrawalStatusResponse::decode(encoded.as_slice()).expect("decode status");

        assert_eq!(decoded.current_confirmed_base_height, Some(61));
        assert_eq!(decoded.handoff_window_blocks, 17);
        assert_eq!(decoded.blocks_until_handoff, Some(6));
    }

    #[tokio::test]
    async fn sequencer_status_reports_configured_handoff_timing() {
        let handoff_window_blocks = 17;
        let (rpc, withdrawal_state_store, base_height_tracker, _submitter, _dir) =
            open_rpc_service_with_handoff_window(handoff_window_blocks).await;
        let proposal = sample_proposal(10);
        let withdrawal_nonce = 1;
        base_height_tracker.record_confirmed_base_height(200);
        withdrawal_state_store
            .ensure_tracked_withdrawal_ordering_at_base_height(
                &TrackedWithdrawalRequest {
                    id: proposal.id.clone(),
                    recipient: proposal.recipient.clone(),
                    amount: proposal.burned_amount,
                    base_batch_end: proposal.base_batch_end,
                    withdrawal_nonce,
                },
                190,
            )
            .await
            .expect("register pending withdrawal");

        let response = rpc
            .get_sequenced_withdrawal_status(Request::new(SequencedWithdrawalStatusRequest {
                withdrawal_id: Some(withdrawal_id_to_proto(&proposal.id)),
            }))
            .await
            .expect("sequenced status response")
            .into_inner();

        assert!(response.found);
        assert_eq!(response.current_confirmed_base_height, Some(200));
        assert_eq!(response.handoff_window_blocks, handoff_window_blocks);
        assert_eq!(response.turn_started_base_height, Some(190));
        assert_eq!(response.blocks_until_handoff, Some(7));
    }

    #[tokio::test]
    async fn sequencer_rpc_service_lazily_advances_stale_proposer_turn_on_status_reads() {
        let (rpc, withdrawal_state_store, base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(11);
        let withdrawal_nonce = 1;
        let commit_certificate = sample_commit_certificate(&proposal).await;

        register_proposal_ordering(&withdrawal_state_store, &proposal, withdrawal_nonce).await;
        withdrawal_state_store
            .record_peer_canonical_proposal(&proposal, Some(&commit_certificate), 100)
            .await
            .expect("record canonical proposal");
        base_height_tracker.record_confirmed_base_height(200);

        let response = rpc
            .get_sequenced_withdrawal_status(Request::new(SequencedWithdrawalStatusRequest {
                withdrawal_id: Some(withdrawal_id_to_proto(&proposal.id)),
            }))
            .await
            .expect("sequenced status response")
            .into_inner();
        assert!(response.found);
        assert_eq!(response.handoff_index, 1);
        assert_eq!(response.turn_started_base_height, Some(200));

        let status = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .expect("sequenced withdrawal exists");
        assert_eq!(status.state, WithdrawalState::PeerCanonical);
        assert_eq!(status.handoff_index, 1);
        assert_eq!(status.turn_started_base_height, Some(200));
    }

    #[tokio::test]
    async fn sequencer_rpc_service_lazily_advances_stale_pending_turn_on_status_reads() {
        let (rpc, withdrawal_state_store, base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(12);
        let withdrawal_nonce = 1;
        withdrawal_state_store
            .ensure_tracked_withdrawal_ordering_at_base_height(
                &TrackedWithdrawalRequest {
                    id: proposal.id.clone(),
                    recipient: proposal.recipient.clone(),
                    amount: proposal.burned_amount,
                    base_batch_end: proposal.base_batch_end,
                    withdrawal_nonce,
                },
                100,
            )
            .await
            .expect("register pending withdrawal");
        base_height_tracker.record_confirmed_base_height(200);

        let response = rpc
            .get_sequenced_withdrawal_status(Request::new(SequencedWithdrawalStatusRequest {
                withdrawal_id: Some(withdrawal_id_to_proto(&proposal.id)),
            }))
            .await
            .expect("sequenced status response")
            .into_inner();
        assert!(response.found);
        assert_eq!(response.handoff_index, 1);
        assert_eq!(response.turn_started_base_height, Some(200));

        let status = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .expect("sequenced withdrawal exists");
        assert_eq!(status.state, WithdrawalState::Pending);
        assert_eq!(status.handoff_index, 1);
        assert_eq!(status.turn_started_base_height, Some(200));
    }

    #[tokio::test]
    async fn advance_precanonical_handoff_ignores_client_supplied_index_and_height() {
        let (rpc, withdrawal_state_store, base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(0);
        withdrawal_state_store
            .ensure_tracked_withdrawal_ordering_at_base_height(
                &TrackedWithdrawalRequest {
                    id: proposal.id.clone(),
                    recipient: proposal.recipient.clone(),
                    amount: proposal.burned_amount,
                    base_batch_end: proposal.base_batch_end,
                    withdrawal_nonce: 1,
                },
                100,
            )
            .await
            .expect("register pending withdrawal");
        base_height_tracker.record_confirmed_base_height(200);

        let response = rpc
            .advance_precanonical_handoff(Request::new(
                SequencerAdvancePrecanonicalHandoffRequest {
                    withdrawal_id: Some(withdrawal_id_to_proto(&proposal.id)),
                    epoch: proposal.epoch,
                    next_handoff_index: 10_000,
                    turn_started_base_height: 999_999,
                },
            ))
            .await
            .expect("advance pre-canonical handoff")
            .into_inner();
        assert!(response.request_accepted, "response: {response:?}");

        let status = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .expect("sequenced withdrawal exists");
        assert_eq!(status.state, WithdrawalState::Pending);
        assert_eq!(status.handoff_index, 1);
        assert_eq!(status.turn_started_base_height, Some(200));
    }

    #[tokio::test]
    async fn advance_precanonical_handoff_does_not_double_advance_after_lazy_status_read() {
        let (rpc, withdrawal_state_store, base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(0);
        withdrawal_state_store
            .ensure_tracked_withdrawal_ordering_at_base_height(
                &TrackedWithdrawalRequest {
                    id: proposal.id.clone(),
                    recipient: proposal.recipient.clone(),
                    amount: proposal.burned_amount,
                    base_batch_end: proposal.base_batch_end,
                    withdrawal_nonce: 1,
                },
                100,
            )
            .await
            .expect("register pending withdrawal");
        base_height_tracker.record_confirmed_base_height(200);

        let status_response = rpc
            .get_sequenced_withdrawal_status(Request::new(SequencedWithdrawalStatusRequest {
                withdrawal_id: Some(withdrawal_id_to_proto(&proposal.id)),
            }))
            .await
            .expect("sequenced status response")
            .into_inner();
        assert_eq!(status_response.handoff_index, 1);

        let handoff_response = rpc
            .advance_precanonical_handoff(Request::new(
                SequencerAdvancePrecanonicalHandoffRequest {
                    withdrawal_id: Some(withdrawal_id_to_proto(&proposal.id)),
                    epoch: proposal.epoch,
                    next_handoff_index: 2,
                    turn_started_base_height: 200,
                },
            ))
            .await
            .expect("advance pre-canonical handoff")
            .into_inner();
        assert!(
            handoff_response.request_accepted,
            "response: {handoff_response:?}"
        );

        let status = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .expect("sequenced withdrawal exists");
        assert_eq!(status.state, WithdrawalState::Pending);
        assert_eq!(status.handoff_index, 1);
        assert_eq!(status.turn_started_base_height, Some(200));
    }

    #[tokio::test]
    async fn canonicalization_from_old_proposer_rejects_after_lazy_handoff() {
        let (rpc, withdrawal_state_store, base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(0);
        withdrawal_state_store
            .ensure_tracked_withdrawal_ordering_at_base_height(
                &TrackedWithdrawalRequest {
                    id: proposal.id.clone(),
                    recipient: proposal.recipient.clone(),
                    amount: proposal.burned_amount,
                    base_batch_end: proposal.base_batch_end,
                    withdrawal_nonce: 1,
                },
                100,
            )
            .await
            .expect("register pending withdrawal");
        base_height_tracker.record_confirmed_base_height(200);

        let mut proposal_proto = proposal_to_proto(&proposal).expect("proposal proto");
        proposal_proto.withdrawal_nonce = 1;
        let response = rpc
            .record_canonical_proposal(Request::new(SequencerRecordCanonicalRequest {
                proposal: Some(proposal_proto),
                commit_certificate: Some(sample_commit_certificate(&proposal).await),
                caller_node_id: expected_proposer(&proposal, 0),
            }))
            .await
            .expect("canonical response")
            .into_inner();

        assert!(!response.request_accepted, "response: {response:?}");
        assert!(
            response.error.contains("is not the expected proposer"),
            "response: {response:?}"
        );
        let status = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .expect("sequenced withdrawal exists");
        assert_eq!(status.state, WithdrawalState::Pending);
        assert_eq!(status.handoff_index, 1);
        assert!(status.proposal_hash.is_none());
    }

    #[tokio::test]
    async fn sequencer_rpc_service_refreshes_handoff_window_on_new_signed_progress_only() {
        let (rpc, withdrawal_state_store, base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(13);
        let withdrawal_nonce = 1;
        let commit_certificate = sample_commit_certificate(&proposal).await;

        register_proposal_ordering(&withdrawal_state_store, &proposal, withdrawal_nonce).await;
        withdrawal_state_store
            .record_peer_canonical_proposal(&proposal, Some(&commit_certificate), 100)
            .await
            .expect("record canonical proposal");
        base_height_tracker.record_confirmed_base_height(120);

        let mut proposal_proto = proposal_to_proto(&proposal).expect("proposal proto");
        proposal_proto.withdrawal_nonce = withdrawal_nonce;
        let response = rpc
            .record_signed_proposal(Request::new(SequencerRecordSignedProposalRequest {
                proposal: Some(proposal_proto.clone()),
                signer_node_id: 7,
            }))
            .await
            .expect("signed proposal response")
            .into_inner();
        assert!(response.request_accepted);

        let status = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .expect("sequenced withdrawal exists");
        assert_eq!(status.handoff_index, 0);
        assert_eq!(status.turn_started_base_height, Some(120));

        let handoff_height = 120
            + crate::withdrawal::state::WithdrawalFallbackPolicy::default()
                .submission_timeout_blocks;
        base_height_tracker.record_confirmed_base_height(handoff_height);
        let duplicate = rpc
            .record_signed_proposal(Request::new(SequencerRecordSignedProposalRequest {
                proposal: Some(proposal_proto),
                signer_node_id: 7,
            }))
            .await
            .expect("duplicate signed proposal response")
            .into_inner();
        assert!(duplicate.request_accepted);

        let unchanged = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .expect("sequenced withdrawal exists");
        assert_eq!(unchanged.handoff_index, 1);
        assert_eq!(unchanged.turn_started_base_height, Some(handoff_height));

        let signed_events = withdrawal_state_store
            .list_submission_events()
            .await
            .expect("list sequencer submission events")
            .into_iter()
            .filter(|event| event.event_type == WithdrawalSubmissionEventType::ProposalSigned)
            .count();
        assert_eq!(signed_events, 1);
    }

    #[tokio::test]
    async fn sequencer_rpc_service_applies_elapsed_authorized_handoff_before_late_signatures() {
        let (rpc, withdrawal_state_store, base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(13);
        let withdrawal_nonce = 1;
        let commit_certificate = sample_commit_certificate(&proposal).await;

        register_proposal_ordering(&withdrawal_state_store, &proposal, withdrawal_nonce).await;
        withdrawal_state_store
            .record_peer_canonical_proposal(&proposal, Some(&commit_certificate), 100)
            .await
            .expect("record canonical proposal");
        withdrawal_state_store
            .record_proposal_authorized(&proposal)
            .await
            .expect("authorize proposal");

        let handoff_height = 120
            + crate::withdrawal::state::WithdrawalFallbackPolicy::default()
                .submission_timeout_blocks;
        base_height_tracker.record_confirmed_base_height(handoff_height);
        let mut proposal_proto = proposal_to_proto(&proposal).expect("proposal proto");
        proposal_proto.withdrawal_nonce = withdrawal_nonce;
        let response = rpc
            .record_signed_proposal(Request::new(SequencerRecordSignedProposalRequest {
                proposal: Some(proposal_proto),
                signer_node_id: 8,
            }))
            .await
            .expect("late signed proposal response")
            .into_inner();
        assert!(response.request_accepted);

        let status = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .expect("sequenced withdrawal exists");
        assert_eq!(status.state, WithdrawalState::Authorized);
        assert_eq!(status.handoff_index, 1);
        assert_eq!(status.turn_started_base_height, Some(handoff_height));
    }

    #[tokio::test]
    async fn sequencer_rpc_service_updates_authorization_and_submission() {
        let (rpc, withdrawal_state_store, base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(0);
        let withdrawal_nonce = 1;
        register_proposal_ordering(&withdrawal_state_store, &proposal, withdrawal_nonce).await;
        withdrawal_state_store
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("canonicalize proposal");
        withdrawal_state_store
            .record_proposal_signed(&proposal, 1, 100)
            .await
            .expect("record signed proposal");

        let mut proposal_proto = proposal_to_proto(&proposal).expect("proposal proto");
        proposal_proto.withdrawal_nonce = withdrawal_nonce;

        let authorize = rpc
            .authorize_proposal(Request::new(SequencerAuthorizeProposalRequest {
                proposal: Some(proposal_proto.clone()),
                commit_certificate: Some(sample_commit_certificate(&proposal).await),
                caller_node_id: crate::shared::proposer::withdrawal_turn_proposer(
                    &proposal.id,
                    proposal.epoch,
                    0,
                    &sample_node_pkhs(),
                ) as u64,
            }))
            .await
            .expect("authorize response")
            .into_inner();
        assert!(authorize.request_accepted);

        let status = rpc
            .get_sequenced_withdrawal_status(Request::new(SequencedWithdrawalStatusRequest {
                withdrawal_id: Some(withdrawal_id_to_proto(&proposal.id)),
            }))
            .await
            .expect("status response")
            .into_inner();
        assert!(status.found);
        assert_eq!(status.state, "authorized");
        assert_eq!(status.turn_started_base_height, Some(100));
        base_height_tracker.record_confirmed_base_height(120);

        let submit = rpc
            .submit_proposal(Request::new(SequencerSubmitProposalRequest {
                proposal: Some(proposal_proto.clone()),
                caller_node_id: crate::shared::proposer::withdrawal_turn_proposer(
                    &proposal.id,
                    proposal.epoch,
                    0,
                    &sample_node_pkhs(),
                ) as u64,
            }))
            .await
            .expect("submit response")
            .into_inner();
        assert!(submit.request_accepted);

        let submitted_status = rpc
            .get_sequenced_withdrawal_status(Request::new(SequencedWithdrawalStatusRequest {
                withdrawal_id: Some(withdrawal_id_to_proto(&proposal.id)),
            }))
            .await
            .expect("status after submit")
            .into_inner();
        assert!(submitted_status.found);
        assert_eq!(submitted_status.state, "mempool_accepted");
        let reserved_before_confirm = rpc
            .get_reserved_withdrawal_inputs(Request::new(
                SequencerReservedWithdrawalInputsRequest {},
            ))
            .await
            .expect("reserved inputs before confirmation")
            .into_inner();
        let reserved_before_confirm = reserved_before_confirm
            .reserved_inputs
            .iter()
            .map(note_name_from_proto)
            .collect::<Result<Vec<_>, _>>()
            .expect("decode reserved inputs before confirmation");
        assert_eq!(reserved_before_confirm, proposal.selected_inputs);
        let sequenced = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch submitted withdrawal")
            .expect("submitted withdrawal exists");
        assert_eq!(sequenced.submit_attempt_count, 1);
        assert_eq!(sequenced.last_submit_attempt_base_height, Some(120));
    }

    #[tokio::test]
    async fn sequencer_rpc_service_refreshes_submit_turn_on_authorization() {
        let (rpc, withdrawal_state_store, base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(14);
        let withdrawal_nonce = 1;

        register_proposal_ordering(&withdrawal_state_store, &proposal, withdrawal_nonce).await;
        withdrawal_state_store
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("canonicalize proposal");
        withdrawal_state_store
            .record_proposal_signed(&proposal, 1, 100)
            .await
            .expect("record signed proposal");
        base_height_tracker.record_confirmed_base_height(120);

        let mut proposal_proto = proposal_to_proto(&proposal).expect("proposal proto");
        proposal_proto.withdrawal_nonce = withdrawal_nonce;
        let authorize = rpc
            .authorize_proposal(Request::new(SequencerAuthorizeProposalRequest {
                proposal: Some(proposal_proto),
                commit_certificate: Some(sample_commit_certificate(&proposal).await),
                caller_node_id: crate::shared::proposer::withdrawal_turn_proposer(
                    &proposal.id,
                    proposal.epoch,
                    0,
                    &sample_node_pkhs(),
                ) as u64,
            }))
            .await
            .expect("authorize response")
            .into_inner();
        assert!(authorize.request_accepted);

        let status = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .expect("sequenced withdrawal exists");
        assert_eq!(status.state, WithdrawalState::Authorized);
        assert_eq!(status.turn_started_base_height, Some(120));
    }

    #[tokio::test]
    async fn sequencer_rpc_service_rejects_authorization_certificate_with_invalid_signature() {
        let (rpc, withdrawal_state_store, base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(16);
        let withdrawal_nonce = 1;

        register_proposal_ordering(&withdrawal_state_store, &proposal, withdrawal_nonce).await;
        withdrawal_state_store
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("canonicalize proposal");
        withdrawal_state_store
            .record_proposal_signed(&proposal, 1, 100)
            .await
            .expect("record signed proposal");
        base_height_tracker.record_confirmed_base_height(120);

        let mut commit_certificate = sample_commit_certificate(&proposal).await;
        commit_certificate.signatures[0].signature[0] ^= 0x01;
        let mut proposal_proto = proposal_to_proto(&proposal).expect("proposal proto");
        proposal_proto.withdrawal_nonce = withdrawal_nonce;

        let err = rpc
            .authorize_proposal(Request::new(SequencerAuthorizeProposalRequest {
                proposal: Some(proposal_proto),
                commit_certificate: Some(commit_certificate),
                caller_node_id: crate::shared::proposer::withdrawal_turn_proposer(
                    &proposal.id,
                    proposal.epoch,
                    0,
                    &sample_node_pkhs(),
                ) as u64,
            }))
            .await
            .expect_err("invalid authorization commit signature should be rejected");

        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(
            err.message()
                .contains("failed Ethereum address verification"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn sequencer_rpc_service_rejects_authorization_from_wrong_caller_node() {
        let (rpc, withdrawal_state_store, _base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(15);
        let withdrawal_nonce = 1;

        register_proposal_ordering(&withdrawal_state_store, &proposal, withdrawal_nonce).await;
        withdrawal_state_store
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("canonicalize proposal");
        withdrawal_state_store
            .record_proposal_signed(&proposal, 1, 100)
            .await
            .expect("record signed proposal");

        let expected_caller = crate::shared::proposer::withdrawal_turn_proposer(
            &proposal.id,
            proposal.epoch,
            0,
            &sample_node_pkhs(),
        ) as u64;
        let wrong_caller = (expected_caller + 1) % (sample_node_pkhs().len() as u64);
        let mut proposal_proto = proposal_to_proto(&proposal).expect("proposal proto");
        proposal_proto.withdrawal_nonce = withdrawal_nonce;
        let authorize = rpc
            .authorize_proposal(Request::new(SequencerAuthorizeProposalRequest {
                proposal: Some(proposal_proto),
                commit_certificate: Some(sample_commit_certificate(&proposal).await),
                caller_node_id: wrong_caller,
            }))
            .await
            .expect("authorize response")
            .into_inner();
        assert!(!authorize.request_accepted);
        assert!(authorize.error.contains("not the expected proposer"));

        let status = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch sequenced withdrawal")
            .expect("sequenced withdrawal exists");
        assert_eq!(status.state, WithdrawalState::PeerCanonical);
    }

    #[tokio::test]
    async fn sequencer_rpc_service_rejects_authorization_for_incompletely_signed_proposal() {
        let (rpc, withdrawal_state_store, _base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let mut proposal = sample_proposal(0);
        proposal.transaction = incompletely_signed_transaction();
        assert!(!transaction_is_fully_signed(&proposal.transaction));
        let roundtripped_proposal = proposal_from_proto(
            &proposal_to_proto(&proposal).expect("proposal proto for incomplete transaction"),
        )
        .expect("roundtrip incomplete proposal");
        assert!(!transaction_is_fully_signed(
            &roundtripped_proposal.transaction
        ));

        let withdrawal_nonce = 1;
        register_proposal_ordering(&withdrawal_state_store, &proposal, withdrawal_nonce).await;
        withdrawal_state_store
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("canonicalize proposal");

        let mut proposal_proto = proposal_to_proto(&proposal).expect("proposal proto");
        proposal_proto.withdrawal_nonce = withdrawal_nonce;
        let authorize = rpc
            .authorize_proposal(Request::new(SequencerAuthorizeProposalRequest {
                proposal: Some(proposal_proto),
                commit_certificate: Some(sample_commit_certificate(&proposal).await),
                caller_node_id: 0,
            }))
            .await
            .expect("authorize response")
            .into_inner();
        assert!(!authorize.request_accepted);
        assert!(authorize.error.contains("not fully signed"));
    }

    #[tokio::test]
    async fn sequencer_rpc_service_rejects_submission_for_non_authorized_proposal() {
        let (rpc, withdrawal_state_store, _base_height_tracker, submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(0);
        let withdrawal_nonce = 1;
        register_proposal_ordering(&withdrawal_state_store, &proposal, withdrawal_nonce).await;
        withdrawal_state_store
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("canonicalize proposal");

        let mut proposal_proto = proposal_to_proto(&proposal).expect("proposal proto");
        proposal_proto.withdrawal_nonce = withdrawal_nonce;

        let submit = rpc
            .submit_proposal(Request::new(SequencerSubmitProposalRequest {
                proposal: Some(proposal_proto),
                caller_node_id: crate::shared::proposer::withdrawal_turn_proposer(
                    &proposal.id,
                    proposal.epoch,
                    0,
                    &sample_node_pkhs(),
                ) as u64,
            }))
            .await
            .expect("submit response")
            .into_inner();
        assert!(!submit.request_accepted);
        assert!(submit.error.contains("authorized"));
        assert!(submitter
            .submitted
            .lock()
            .expect("submitted proposals lock")
            .is_empty());
    }

    #[tokio::test]
    async fn sequencer_rpc_service_rejects_submission_from_wrong_caller_node() {
        let (rpc, withdrawal_state_store, _base_height_tracker, submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(16);
        let withdrawal_nonce = 1;

        register_proposal_ordering(&withdrawal_state_store, &proposal, withdrawal_nonce).await;
        withdrawal_state_store
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("canonicalize proposal");
        withdrawal_state_store
            .record_proposal_signed(&proposal, 1, 100)
            .await
            .expect("record signed proposal");

        let expected_caller = crate::shared::proposer::withdrawal_turn_proposer(
            &proposal.id,
            proposal.epoch,
            0,
            &sample_node_pkhs(),
        ) as u64;
        let wrong_caller = (expected_caller + 1) % (sample_node_pkhs().len() as u64);
        let mut proposal_proto = proposal_to_proto(&proposal).expect("proposal proto");
        proposal_proto.withdrawal_nonce = withdrawal_nonce;
        let authorize = rpc
            .authorize_proposal(Request::new(SequencerAuthorizeProposalRequest {
                proposal: Some(proposal_proto.clone()),
                commit_certificate: Some(sample_commit_certificate(&proposal).await),
                caller_node_id: expected_caller,
            }))
            .await
            .expect("authorize response")
            .into_inner();
        assert!(authorize.request_accepted);

        let submit = rpc
            .submit_proposal(Request::new(SequencerSubmitProposalRequest {
                proposal: Some(proposal_proto),
                caller_node_id: wrong_caller,
            }))
            .await
            .expect("submit response")
            .into_inner();
        assert!(!submit.request_accepted);
        assert!(submit.error.contains("not the expected proposer"));
        assert!(submitter
            .submitted
            .lock()
            .expect("submitted proposals lock")
            .is_empty());
    }

    #[tokio::test]
    async fn sequencer_rpc_manual_approval_disabled_preserves_existing_submit_behavior() {
        let (rpc, withdrawal_state_store, base_height_tracker, submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(30);
        let withdrawal_nonce = 1;
        let (proposal_proto, caller_node_id) = authorize_for_submission(
            &rpc, &withdrawal_state_store, &base_height_tracker, &proposal, withdrawal_nonce,
        )
        .await;

        let submit = rpc
            .submit_proposal(Request::new(SequencerSubmitProposalRequest {
                proposal: Some(proposal_proto),
                caller_node_id,
            }))
            .await
            .expect("submit response")
            .into_inner();
        assert!(submit.request_accepted, "submit response: {submit:?}");
        assert_eq!(
            submitter
                .submitted
                .lock()
                .expect("submitted proposals lock")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn sequencer_rpc_manual_approval_enabled_without_file_defers_without_submit_attempt() {
        let (rpc, withdrawal_state_store, base_height_tracker, submitter, dir) =
            open_rpc_service().await;
        let approval_dir = dir.path().join("approvals");
        let rpc = rpc.with_manual_submit_approval(ManualSubmitApprovalConfig {
            enabled: true,
            approval_dir,
        });
        let proposal = sample_proposal(31);
        let withdrawal_nonce = 1;
        let (proposal_proto, caller_node_id) = authorize_for_submission(
            &rpc, &withdrawal_state_store, &base_height_tracker, &proposal, withdrawal_nonce,
        )
        .await;

        let deferred = rpc
            .submit_proposal(Request::new(SequencerSubmitProposalRequest {
                proposal: Some(proposal_proto),
                caller_node_id,
            }))
            .await
            .expect("deferred submit response")
            .into_inner();
        assert!(!deferred.request_accepted);
        assert!(deferred
            .error
            .starts_with(WITHDRAWAL_SUBMIT_DEFERRED_PREFIX));
        assert!(deferred.error.contains("manual operator approval"));
        assert!(submitter
            .submitted
            .lock()
            .expect("submitted proposals lock")
            .is_empty());

        let sequenced = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch authorized withdrawal")
            .expect("authorized withdrawal exists");
        assert_eq!(sequenced.state, WithdrawalState::Authorized);
        assert_eq!(sequenced.submit_attempt_count, 0);
        assert_eq!(sequenced.last_submit_attempt_base_height, None);
    }

    #[tokio::test]
    async fn sequencer_rpc_manual_approval_enabled_with_malformed_file_defers() {
        let (rpc, withdrawal_state_store, base_height_tracker, submitter, dir) =
            open_rpc_service().await;
        let approval_dir = dir.path().join("approvals");
        let rpc = rpc.with_manual_submit_approval(ManualSubmitApprovalConfig {
            enabled: true,
            approval_dir: approval_dir.clone(),
        });
        let proposal = sample_proposal(32);
        let withdrawal_nonce = 1;
        let (proposal_proto, caller_node_id) = authorize_for_submission(
            &rpc, &withdrawal_state_store, &base_height_tracker, &proposal, withdrawal_nonce,
        )
        .await;
        let facts = withdrawal_state_store
            .list_pending_approval_facts()
            .await
            .expect("pending approval facts")
            .pop()
            .expect("one pending approval");
        fs::create_dir_all(&approval_dir).expect("create approval dir");
        fs::write(
            approval_file_path(&approval_dir, &facts.authorized_transaction_name)
                .expect("approval path"),
            "not-an-approval-record",
        )
        .expect("write malformed approval");

        let deferred = rpc
            .submit_proposal(Request::new(SequencerSubmitProposalRequest {
                proposal: Some(proposal_proto),
                caller_node_id,
            }))
            .await
            .expect("deferred submit response")
            .into_inner();
        assert!(!deferred.request_accepted);
        assert!(deferred.error.contains("malformed"));
        assert!(submitter
            .submitted
            .lock()
            .expect("submitted proposals lock")
            .is_empty());
    }

    #[tokio::test]
    async fn sequencer_rpc_manual_approval_enabled_with_mismatched_file_defers() {
        let (rpc, withdrawal_state_store, base_height_tracker, submitter, dir) =
            open_rpc_service().await;
        let approval_dir = dir.path().join("approvals");
        let rpc = rpc.with_manual_submit_approval(ManualSubmitApprovalConfig {
            enabled: true,
            approval_dir: approval_dir.clone(),
        });
        let proposal = sample_proposal(33);
        let withdrawal_nonce = 1;
        let (proposal_proto, caller_node_id) = authorize_for_submission(
            &rpc, &withdrawal_state_store, &base_height_tracker, &proposal, withdrawal_nonce,
        )
        .await;
        let mut facts = withdrawal_state_store
            .list_pending_approval_facts()
            .await
            .expect("pending approval facts")
            .pop()
            .expect("one pending approval");
        facts.epoch = facts.epoch.saturating_add(1);
        fs::create_dir_all(&approval_dir).expect("create approval dir");
        fs::write(
            approval_file_path(&approval_dir, &facts.authorized_transaction_name)
                .expect("approval path"),
            render_approval_record(&facts),
        )
        .expect("write mismatched approval");

        let deferred = rpc
            .submit_proposal(Request::new(SequencerSubmitProposalRequest {
                proposal: Some(proposal_proto),
                caller_node_id,
            }))
            .await
            .expect("deferred submit response")
            .into_inner();
        assert!(!deferred.request_accepted);
        assert!(deferred.error.contains("does not match"));
        assert!(submitter
            .submitted
            .lock()
            .expect("submitted proposals lock")
            .is_empty());
    }

    #[tokio::test]
    async fn sequencer_rpc_manual_approval_enabled_with_matching_file_submits() {
        let (rpc, withdrawal_state_store, base_height_tracker, submitter, dir) =
            open_rpc_service().await;
        let approval_dir = dir.path().join("approvals");
        let rpc = rpc.with_manual_submit_approval(ManualSubmitApprovalConfig {
            enabled: true,
            approval_dir: approval_dir.clone(),
        });
        let proposal = sample_proposal(34);
        let withdrawal_nonce = 1;
        let (proposal_proto, caller_node_id) = authorize_for_submission(
            &rpc, &withdrawal_state_store, &base_height_tracker, &proposal, withdrawal_nonce,
        )
        .await;
        let facts = withdrawal_state_store
            .list_pending_approval_facts()
            .await
            .expect("pending approval facts")
            .pop()
            .expect("one pending approval");
        write_approval_record_atomic(&approval_dir, &facts).expect("write approval record");

        let accepted = rpc
            .submit_proposal(Request::new(SequencerSubmitProposalRequest {
                proposal: Some(proposal_proto),
                caller_node_id,
            }))
            .await
            .expect("accepted submit response")
            .into_inner();
        assert!(accepted.request_accepted, "submit response: {accepted:?}");
        assert_eq!(
            submitter
                .submitted
                .lock()
                .expect("submitted proposals lock")
                .len(),
            1
        );

        let sequenced = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch submitted withdrawal")
            .expect("submitted withdrawal exists");
        assert_eq!(sequenced.state, WithdrawalState::MempoolAccepted);
        assert_eq!(sequenced.submit_attempt_count, 1);
        assert_eq!(sequenced.last_submit_attempt_base_height, Some(120));
    }

    #[tokio::test]
    async fn sequencer_rpc_approval_fact_introspection_lists_authorized_rows() {
        let (rpc, withdrawal_state_store, base_height_tracker, _submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(35);
        let proposal_hash = proposal.proposal_hash().expect("proposal hash");
        let withdrawal_nonce = 1;
        authorize_for_submission(
            &rpc, &withdrawal_state_store, &base_height_tracker, &proposal, withdrawal_nonce,
        )
        .await;

        let pending = withdrawal_state_store
            .list_pending_approval_facts()
            .await
            .expect("pending approval facts");
        assert_eq!(pending.len(), 1);
        let facts = &pending[0];
        assert_eq!(facts.withdrawal_id_as_of, proposal.id.as_of.to_base58());
        assert_eq!(
            facts.withdrawal_id_base_event_id,
            hex::encode(&proposal.id.base_event_id.0)
        );
        assert_eq!(facts.epoch, proposal.epoch);
        assert_eq!(facts.proposal_hash, proposal_hash);
        assert!(!facts.authorized_transaction_name.is_empty());

        let by_tx_id = withdrawal_state_store
            .load_authorized_approval_facts_by_tx_id(&facts.authorized_transaction_name)
            .await
            .expect("load approval facts by tx id")
            .expect("approval facts by tx id");
        assert_eq!(by_tx_id, *facts);
    }

    #[tokio::test]
    async fn sequencer_rpc_service_reuses_same_authorized_tx_after_handoff_rotation() {
        let (rpc, withdrawal_state_store, base_height_tracker, submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(17);
        let withdrawal_nonce = 1;
        let proposal_hash = proposal.proposal_hash().expect("proposal hash");
        assert!(transaction_is_fully_signed(&proposal.transaction));

        register_proposal_ordering(&withdrawal_state_store, &proposal, withdrawal_nonce).await;
        withdrawal_state_store
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("canonicalize proposal");

        base_height_tracker.record_confirmed_base_height(120);
        let initial_caller = crate::shared::proposer::withdrawal_turn_proposer(
            &proposal.id,
            proposal.epoch,
            0,
            &sample_node_pkhs(),
        ) as u64;
        let mut proposal_proto = proposal_to_proto(&proposal).expect("proposal proto");
        proposal_proto.withdrawal_nonce = withdrawal_nonce;
        let authorize = rpc
            .authorize_proposal(Request::new(SequencerAuthorizeProposalRequest {
                proposal: Some(proposal_proto.clone()),
                commit_certificate: Some(sample_commit_certificate(&proposal).await),
                caller_node_id: initial_caller,
            }))
            .await
            .expect("authorize response")
            .into_inner();
        assert!(authorize.request_accepted);

        let authorized = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch authorized withdrawal")
            .expect("authorized withdrawal exists");
        assert_eq!(authorized.state, WithdrawalState::Authorized);
        assert_eq!(authorized.proposal_hash, Some(proposal_hash.clone()));
        assert_eq!(authorized.turn_started_base_height, Some(120));

        let handoff_height = 120
            + crate::withdrawal::state::WithdrawalFallbackPolicy::default()
                .submission_timeout_blocks;
        base_height_tracker.record_confirmed_base_height(handoff_height);
        let rotated_caller = crate::shared::proposer::withdrawal_turn_proposer(
            &proposal.id,
            proposal.epoch,
            1,
            &sample_node_pkhs(),
        ) as u64;
        assert_ne!(initial_caller, rotated_caller);

        let stale_submit = rpc
            .submit_proposal(Request::new(SequencerSubmitProposalRequest {
                proposal: Some(proposal_proto.clone()),
                caller_node_id: initial_caller,
            }))
            .await
            .expect("stale submit response")
            .into_inner();
        assert!(!stale_submit.request_accepted);
        assert!(stale_submit.error.contains("not the expected proposer"));

        let submit = rpc
            .submit_proposal(Request::new(SequencerSubmitProposalRequest {
                proposal: Some(proposal_proto),
                caller_node_id: rotated_caller,
            }))
            .await
            .expect("rotated submit response")
            .into_inner();
        assert!(submit.request_accepted);

        {
            let submitted = submitter
                .submitted
                .lock()
                .expect("submitted proposals lock");
            assert_eq!(submitted.len(), 1);
            assert_eq!(
                submitted[0]
                    .proposal_hash()
                    .expect("submitted proposal hash"),
                proposal_hash
            );
        }

        let status = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch submitted withdrawal")
            .expect("submitted withdrawal exists");
        assert_eq!(status.state, WithdrawalState::MempoolAccepted);
        assert_eq!(status.current_epoch, proposal.epoch);
        assert_eq!(status.proposal_hash, Some(proposal_hash));
    }

    #[tokio::test]
    async fn sequencer_rpc_service_keeps_authorized_after_bounded_submit_failure_until_retry_window(
    ) {
        let (rpc, withdrawal_state_store, base_height_tracker, submitter, _dir) =
            open_rpc_service().await;
        let rpc = rpc.with_authorized_submit_retry_after_base_blocks(2);
        let proposal = sample_proposal(0);
        let withdrawal_nonce = 1;
        register_proposal_ordering(&withdrawal_state_store, &proposal, withdrawal_nonce).await;
        withdrawal_state_store
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("canonicalize proposal");
        withdrawal_state_store
            .record_proposal_signed(&proposal, 1, 100)
            .await
            .expect("record signed proposal");

        let mut proposal_proto = proposal_to_proto(&proposal).expect("proposal proto");
        proposal_proto.withdrawal_nonce = withdrawal_nonce;

        let authorize = rpc
            .authorize_proposal(Request::new(SequencerAuthorizeProposalRequest {
                proposal: Some(proposal_proto.clone()),
                commit_certificate: Some(sample_commit_certificate(&proposal).await),
                caller_node_id: crate::shared::proposer::withdrawal_turn_proposer(
                    &proposal.id,
                    proposal.epoch,
                    0,
                    &sample_node_pkhs(),
                ) as u64,
            }))
            .await
            .expect("authorize response")
            .into_inner();
        assert!(authorize.request_accepted);

        submitter.script_submit_attempts([
            ScriptedSubmitAttempt::returns(WithdrawalSubmitAttemptStatus::NotYetAccepted),
            ScriptedSubmitAttempt::returns(WithdrawalSubmitAttemptStatus::NotYetAccepted),
            ScriptedSubmitAttempt::returns(WithdrawalSubmitAttemptStatus::NotYetAccepted),
        ]);
        base_height_tracker.record_confirmed_base_height(120);

        let submit_err = rpc
            .submit_proposal(Request::new(SequencerSubmitProposalRequest {
                proposal: Some(proposal_proto.clone()),
                caller_node_id: crate::shared::proposer::withdrawal_turn_proposer(
                    &proposal.id,
                    proposal.epoch,
                    0,
                    &sample_node_pkhs(),
                ) as u64,
            }))
            .await
            .expect_err("bounded submit failure should return error");
        assert!(submit_err
            .to_string()
            .contains("withdrawal submission failed after bounded retry budget"));

        let submitted_status = rpc
            .get_sequenced_withdrawal_status(Request::new(SequencedWithdrawalStatusRequest {
                withdrawal_id: Some(withdrawal_id_to_proto(&proposal.id)),
            }))
            .await
            .expect("status after failed submit")
            .into_inner();
        assert!(submitted_status.found);
        assert_eq!(submitted_status.state, "authorized");
        assert_eq!(
            submitter
                .submitted
                .lock()
                .expect("submitted proposals lock")
                .len(),
            3
        );
        let sequenced = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch authorized withdrawal")
            .expect("authorized withdrawal exists");
        assert_eq!(sequenced.state, WithdrawalState::Authorized);
        assert_eq!(sequenced.submit_attempt_count, 3);
        assert_eq!(sequenced.last_submit_attempt_base_height, Some(120));

        base_height_tracker.record_confirmed_base_height(121);
        let deferred = rpc
            .submit_proposal(Request::new(SequencerSubmitProposalRequest {
                proposal: Some(proposal_proto.clone()),
                caller_node_id: crate::shared::proposer::withdrawal_turn_proposer(
                    &proposal.id,
                    proposal.epoch,
                    0,
                    &sample_node_pkhs(),
                ) as u64,
            }))
            .await
            .expect("cadence deferred response")
            .into_inner();
        assert!(!deferred.request_accepted);
        assert!(deferred
            .error
            .starts_with(WITHDRAWAL_SUBMIT_DEFERRED_PREFIX));
        assert_eq!(
            submitter
                .submitted
                .lock()
                .expect("submitted proposals lock")
                .len(),
            3
        );

        submitter.script_submit_attempts([ScriptedSubmitAttempt::returns(
            WithdrawalSubmitAttemptStatus::MempoolAccepted,
        )]);
        base_height_tracker.record_confirmed_base_height(122);
        let accepted = rpc
            .submit_proposal(Request::new(SequencerSubmitProposalRequest {
                proposal: Some(proposal_proto),
                caller_node_id: crate::shared::proposer::withdrawal_turn_proposer(
                    &proposal.id,
                    proposal.epoch,
                    0,
                    &sample_node_pkhs(),
                ) as u64,
            }))
            .await
            .expect("submit retry accepted")
            .into_inner();
        assert!(accepted.request_accepted);
        let sequenced = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch accepted withdrawal")
            .expect("accepted withdrawal exists");
        assert_eq!(sequenced.state, WithdrawalState::MempoolAccepted);
        assert_eq!(sequenced.submit_attempt_count, 4);
    }

    #[tokio::test]
    async fn sequencer_rpc_service_defers_initial_submission_when_grpc_node_is_down() {
        let (rpc, withdrawal_state_store, base_height_tracker, submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(28);
        let withdrawal_nonce = 1;

        register_proposal_ordering(&withdrawal_state_store, &proposal, withdrawal_nonce).await;
        withdrawal_state_store
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("canonicalize proposal");
        withdrawal_state_store
            .record_proposal_signed(&proposal, 1, 100)
            .await
            .expect("record signed proposal");

        let mut proposal_proto = proposal_to_proto(&proposal).expect("proposal proto");
        proposal_proto.withdrawal_nonce = withdrawal_nonce;
        let caller_node_id = crate::shared::proposer::withdrawal_turn_proposer(
            &proposal.id,
            proposal.epoch,
            0,
            &sample_node_pkhs(),
        ) as u64;
        let authorize = rpc
            .authorize_proposal(Request::new(SequencerAuthorizeProposalRequest {
                proposal: Some(proposal_proto.clone()),
                commit_certificate: Some(sample_commit_certificate(&proposal).await),
                caller_node_id,
            }))
            .await
            .expect("authorize response")
            .into_inner();
        assert!(authorize.request_accepted);

        submitter.set_submission_node_error("connect refused");
        base_height_tracker.record_confirmed_base_height(120);
        let events_before_submit = withdrawal_state_store
            .list_submission_events()
            .await
            .expect("list events before deferred submit")
            .len();

        let deferred = rpc
            .submit_proposal(Request::new(SequencerSubmitProposalRequest {
                proposal: Some(proposal_proto),
                caller_node_id,
            }))
            .await
            .expect("deferred submit response")
            .into_inner();
        assert!(!deferred.request_accepted);
        assert!(deferred
            .error
            .starts_with(WITHDRAWAL_SUBMIT_DEFERRED_PREFIX));
        assert!(deferred.error.contains("connect refused"));
        assert!(submitter
            .submitted
            .lock()
            .expect("submitted proposals lock")
            .is_empty());

        let sequenced = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch authorized withdrawal")
            .expect("authorized withdrawal exists");
        assert_eq!(sequenced.state, WithdrawalState::Authorized);
        assert_eq!(sequenced.submit_attempt_count, 0);
        assert_eq!(sequenced.last_submit_attempt_base_height, None);
        assert_eq!(sequenced.last_submit_error, None);
        let events_after_submit = withdrawal_state_store
            .list_submission_events()
            .await
            .expect("list events after deferred submit")
            .len();
        assert_eq!(events_after_submit, events_before_submit);
    }

    #[tokio::test]
    async fn sequencer_rpc_service_records_last_submit_attempt_base_height_from_bounded_attempts() {
        let (rpc, withdrawal_state_store, _base_height_tracker, submitter, _dir) =
            open_rpc_service().await;
        let proposal = sample_proposal(18);
        let withdrawal_nonce = 1;

        register_proposal_ordering(&withdrawal_state_store, &proposal, withdrawal_nonce).await;
        withdrawal_state_store
            .record_proposal_canonicalized(&proposal, 100)
            .await
            .expect("canonicalize proposal");
        withdrawal_state_store
            .record_proposal_signed(&proposal, 1, 100)
            .await
            .expect("record signed proposal");

        let mut proposal_proto = proposal_to_proto(&proposal).expect("proposal proto");
        proposal_proto.withdrawal_nonce = withdrawal_nonce;
        let authorize = rpc
            .authorize_proposal(Request::new(SequencerAuthorizeProposalRequest {
                proposal: Some(proposal_proto.clone()),
                commit_certificate: Some(sample_commit_certificate(&proposal).await),
                caller_node_id: crate::shared::proposer::withdrawal_turn_proposer(
                    &proposal.id,
                    proposal.epoch,
                    0,
                    &sample_node_pkhs(),
                ) as u64,
            }))
            .await
            .expect("authorize response")
            .into_inner();
        assert!(authorize.request_accepted);

        submitter.script_submit_attempts([
            ScriptedSubmitAttempt::returns(WithdrawalSubmitAttemptStatus::NotYetAccepted)
                .then_advance_confirmed_base_height_to(150),
            ScriptedSubmitAttempt::returns(WithdrawalSubmitAttemptStatus::MempoolAccepted)
                .then_advance_confirmed_base_height_to(175),
        ]);

        let submit = rpc
            .submit_proposal(Request::new(SequencerSubmitProposalRequest {
                proposal: Some(proposal_proto),
                caller_node_id: crate::shared::proposer::withdrawal_turn_proposer(
                    &proposal.id,
                    proposal.epoch,
                    0,
                    &sample_node_pkhs(),
                ) as u64,
            }))
            .await
            .expect("submit response")
            .into_inner();
        assert!(submit.request_accepted);

        let sequenced = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch submitted withdrawal")
            .expect("submitted withdrawal exists");
        assert_eq!(sequenced.state, WithdrawalState::MempoolAccepted);
        assert_eq!(sequenced.submit_attempt_count, 2);
        assert_eq!(sequenced.last_submit_attempt_base_height, Some(150));
    }
}
