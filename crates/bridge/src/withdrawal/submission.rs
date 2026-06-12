use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use backon::Retryable;
use nockapp_grpc::pb::common::v1::Base58Hash;
use nockapp_grpc::pb::public::v2::transaction_accepted_response;
use nockapp_grpc::services::public_nockchain::v2::client::PublicNockchainGrpcClient;
use prost::Message;
use thiserror::Error;
use tokio::time::{interval, MissedTickBehavior};
use tracing::{info, warn};

use crate::core::loop_policy::RetryPolicy;
use crate::core::withdrawal::submission::{
    plan_authorization_status, plan_submission_status,
    select_frontier_authorize_or_submit_candidate, WithdrawalAuthorizationStatusDecision,
    WithdrawalSubmissionCandidateKind, WithdrawalSubmissionStatusDecision,
};
use crate::observability::metrics;
use crate::observability::status::BridgeStatus;
use crate::observability::tui::types::AlertSeverity;
use crate::shared::errors::BridgeError;
use crate::shared::ingress::proto::{
    SequencedWithdrawalStatusResponse, WithdrawalCommitCertificate,
};
use crate::shared::stop::StopHandle;
use crate::shared::types::Tip5Hash;
use crate::withdrawal::proposals::{
    reconstruct_withdrawal_proposal, TrackedWithdrawalRequest, WithdrawalProposalRegistry,
};
use crate::withdrawal::raw_tx as withdrawal_raw_tx;
use crate::withdrawal::sequencer::base_height::SequencerBaseHeightTracker;
use crate::withdrawal::sequencer::store::{cue_transaction, WithdrawalSequencerStore};
use crate::withdrawal::state::{LiveWithdrawalView, WithdrawalFallbackPolicy, WithdrawalState};
use crate::withdrawal::types::{
    WithdrawalId, WithdrawalProposalData, WithdrawalSequencerProposalArtifacts,
};

pub(crate) const WITHDRAWAL_SUBMIT_DEFERRED_PREFIX: &str = "withdrawal submission deferred: ";

pub(crate) fn is_withdrawal_submit_deferred_error(error: &str) -> bool {
    error.starts_with(WITHDRAWAL_SUBMIT_DEFERRED_PREFIX)
}

pub(crate) fn sequenced_withdrawal_released(status: &SequencedWithdrawalStatusResponse) -> bool {
    status.found && matches!(status.state.as_str(), "mempool_accepted" | "confirmed")
}

#[async_trait]
pub trait WithdrawalSubmitPort: Send + Sync {
    /// Checks whether the public Nockchain submission endpoint is reachable
    /// before entering a bounded submit loop.
    async fn submission_node_available(&self) -> Result<(), BridgeError> {
        Ok(())
    }

    /// Submits a fully prepared withdrawal proposal to the underlying Nockchain
    /// transaction API.
    async fn submit_withdrawal(
        &self,
        proposal: &WithdrawalProposalData,
    ) -> Result<WithdrawalSubmitAttemptStatus, BridgeError>;

    /// Resubmits an already authorized raw transaction. This is intended for
    /// sequencer-owned orphan retry.
    async fn resubmit_raw_tx(
        &self,
        raw_tx: &nockchain_types::v1::RawTx,
    ) -> Result<WithdrawalNetworkSubmitStatus, BridgeError>;

    /// Returns whether the underlying Nockchain node currently reports the
    /// proposal's transaction as mempool-accepted.
    async fn transaction_mempool_accepted(
        &self,
        _proposal: &WithdrawalProposalData,
    ) -> Result<Option<bool>, BridgeError> {
        Ok(None)
    }

    /// Returns the current block containing the submitted raw transaction id, if
    /// the public API has indexed it into a block.
    async fn get_transaction_included_block(
        &self,
        _submitted_raw_tx_id: &str,
    ) -> Result<Option<WithdrawalIncludedBlock>, BridgeError> {
        Ok(None)
    }

    /// Returns the public Nockchain tip height used to evaluate withdrawal
    /// confirmation depth.
    async fn current_nockchain_tip_height(&self) -> Result<Option<u64>, BridgeError> {
        Ok(None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithdrawalIncludedBlock {
    pub height: u64,
    pub block_id: Tip5Hash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithdrawalSubmitAttemptStatus {
    /// The submit call succeeded, but this attempt did not positively observe
    /// the transaction in the public mempool yet.
    NotYetAccepted,
    /// The transaction was submitted and this attempt positively observed it in
    /// the public mempool.
    MempoolAccepted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithdrawalNetworkSubmitStatus {
    /// The bounded submit loop positively observed the transaction in the
    /// public mempool.
    MempoolAccepted,
    /// A preflight mempool check found the transaction already present, so no
    /// resend was necessary.
    AlreadyMempoolAccepted,
    /// A bounded retry loop exhausted its budget without positively observing
    /// mempool acceptance.
    RetryExhausted,
}

#[derive(Debug, Error)]
pub enum WithdrawalSequencerCanonicalizationError {
    #[error("sequencer rejected canonical withdrawal: {0}")]
    Rejected(String),

    #[error(transparent)]
    Bridge(#[from] BridgeError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithdrawalSequencerSubmitOutcome {
    MempoolAccepted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextPendingWithdrawalOrdering {
    pub id: WithdrawalId,
    pub withdrawal_nonce: u64,
}

#[derive(Clone)]
pub struct WithdrawalSubmissionContext<S> {
    pub sequencer: Arc<S>,
    pub proposal_registry: Arc<WithdrawalProposalRegistry>,
    pub bridge_status: BridgeStatus,
    pub fallback_policy: WithdrawalFallbackPolicy,
    pub local_node_id: u64,
    pub node_pkhs: Vec<nockchain_types::tx_engine::common::Hash>,
}

#[async_trait]
pub trait WithdrawalSequencerPort: Send + Sync {
    /// Registers the bridge-local withdrawal nonce with the sequencer so
    /// ordering decisions can be enforced consistently.
    async fn register_withdrawal(
        &self,
        tracked: &TrackedWithdrawalRequest,
    ) -> Result<(), BridgeError>;

    /// Advances the shared pre-canonical handoff index for the active
    /// withdrawal epoch, allowing assembly ownership to move off a stalled
    /// assembler without creating a new tx attempt.
    async fn advance_precanonical_handoff(
        &self,
        _id: &WithdrawalId,
        _epoch: u64,
        _next_handoff_index: u64,
        _turn_started_base_height: u64,
    ) -> Result<(), BridgeError> {
        Ok(())
    }

    /// Tells the sequencer which peer-canonical proposal and commit
    /// certificate now define the fixed transaction body for this withdrawal
    /// epoch.
    async fn record_peer_canonical_proposal(
        &self,
        _proposal: &WithdrawalProposalData,
        _withdrawal_nonce: u64,
        _commit_certificate: &WithdrawalCommitCertificate,
        _caller_node_id: u64,
    ) -> Result<(), WithdrawalSequencerCanonicalizationError> {
        Ok(())
    }

    /// Tells the sequencer that one signer contributed witness data to the
    /// fixed canonical transaction body. New contributions only refresh the
    /// handoff window while the tx is still peer-canonical.
    async fn record_signed_proposal(
        &self,
        _proposal: &WithdrawalProposalData,
        _withdrawal_nonce: u64,
        _signer_node_id: u64,
    ) -> Result<(), BridgeError> {
        Ok(())
    }

    /// Returns the currently reserved note-name set across all
    /// canonical-or-later sequencer-owned withdrawals.
    async fn get_reserved_withdrawal_inputs(
        &self,
    ) -> Result<Vec<nockchain_types::v1::Name>, BridgeError> {
        Ok(Vec::new())
    }

    /// Tells the sequencer to authorize a fully signed proposal for the
    /// supplied withdrawal nonce.
    async fn authorize_proposal(
        &self,
        proposal: &WithdrawalProposalData,
        withdrawal_nonce: u64,
        commit_certificate: &WithdrawalCommitCertificate,
        caller_node_id: u64,
    ) -> Result<(), BridgeError>;

    /// Tells the sequencer to submit the already authorized proposal for the
    /// supplied withdrawal nonce.
    async fn submit_proposal(
        &self,
        proposal: &WithdrawalProposalData,
        withdrawal_nonce: u64,
        caller_node_id: u64,
    ) -> Result<Option<WithdrawalSequencerSubmitOutcome>, BridgeError>;

    /// Returns the earliest withdrawal nonce that still blocks ordering
    /// progress, if any.
    async fn get_next_pending_withdrawal_ordering(
        &self,
    ) -> Result<Option<NextPendingWithdrawalOrdering>, BridgeError>;

    /// Returns the sequencer's current withdrawal ordering frontier: the
    /// lowest live withdrawal nonce that has not yet been released.
    async fn current_live_withdrawal_nonce(&self) -> Result<Option<u64>, BridgeError> {
        Ok(self
            .get_next_pending_withdrawal_ordering()
            .await?
            .map(|ordering| ordering.withdrawal_nonce))
    }

    /// Returns whether the sequencer has this withdrawal registered as the
    /// current live frontier.
    async fn frontier_allows_withdrawal(&self, id: &WithdrawalId) -> Result<bool, BridgeError> {
        Ok(self
            .get_next_pending_withdrawal_ordering()
            .await?
            .map(|ordering| ordering.id == *id)
            .unwrap_or(false))
    }

    /// Returns the sequencer's current lifecycle status for a withdrawal.
    async fn get_sequenced_withdrawal_status(
        &self,
        id: &WithdrawalId,
    ) -> Result<SequencedWithdrawalStatusResponse, BridgeError>;

    /// Loads canonical proposal artifacts from the sequencer projection. These
    /// artifacts are replayable from the sequencer journal and are used to
    /// hydrate local proposal cache misses.
    async fn load_canonical_proposal_artifacts(
        &self,
        _id: &WithdrawalId,
    ) -> Result<Option<WithdrawalSequencerProposalArtifacts>, BridgeError> {
        Ok(None)
    }
}

pub(crate) fn alert_withdrawal_registration_failure(
    bridge_status: &BridgeStatus,
    tracked: &TrackedWithdrawalRequest,
    err: &BridgeError,
) {
    bridge_status.push_alert(
        AlertSeverity::Error,
        "Withdrawal Registration Failed".to_string(),
        format!(
            "failed to register withdrawal {:?} nonce {} with sequencer: {err}",
            tracked.id, tracked.withdrawal_nonce
        ),
        "withdrawal-sequencer".to_string(),
    );
}

pub(crate) async fn register_withdrawal_or_alert<S: WithdrawalSequencerPort + ?Sized>(
    sequencer: &S,
    bridge_status: &BridgeStatus,
    tracked: &TrackedWithdrawalRequest,
) -> Result<(), BridgeError> {
    metrics::init_metrics()
        .withdrawal_registration_attempts
        .increment();
    match sequencer.register_withdrawal(tracked).await {
        Ok(()) => {
            metrics::init_metrics()
                .withdrawal_registration_accepted
                .increment();
            Ok(())
        }
        Err(err) => {
            let metrics = metrics::init_metrics();
            metrics.withdrawal_registration_error.increment();
            metrics.withdrawal_registration_stalled.increment();
            alert_withdrawal_registration_failure(bridge_status, tracked, &err);
            Err(err)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WithdrawalSubmissionTickOutcome {
    Idle,
    Authorized { id: WithdrawalId, epoch: u64 },
    MempoolAccepted { id: WithdrawalId, epoch: u64 },
    MempoolAcceptedObserved { id: WithdrawalId, epoch: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WithdrawalSubmissionLoopPolicy {
    pub poll_interval: Duration,
}

impl Default for WithdrawalSubmissionLoopPolicy {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WithdrawalSequencerConfirmationLoopPolicy {
    pub poll_interval: Duration,
    pub nockchain_confirmation_depth: u64,
}

impl Default for WithdrawalSequencerConfirmationLoopPolicy {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(1),
            nockchain_confirmation_depth: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WithdrawalSequencerOrphanRetryLoopPolicy {
    pub poll_interval: Duration,
    /// Retry orphaned mempool-accepted withdrawals after this many confirmed
    /// Base blocks have elapsed since the last submit attempt.
    pub retry_after_base_blocks: u64,
}

impl Default for WithdrawalSequencerOrphanRetryLoopPolicy {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(1),
            // Roughly one hour at ~2 second Base blocks.
            retry_after_base_blocks: 1_800,
        }
    }
}

pub(crate) fn default_withdrawal_submit_retry_policy() -> RetryPolicy {
    RetryPolicy {
        min_delay: Duration::from_secs(1),
        max_delay: Duration::from_secs(30),
        // `backon` counts retries after the first attempt, so `5` preserves
        // the total budget of 6 attempts.
        max_times: Some(5),
        jitter: false,
    }
}

#[derive(Clone, Debug)]
pub struct PublicNockchainWithdrawalSubmitter {
    endpoint: String,
}

impl PublicNockchainWithdrawalSubmitter {
    /// Builds a submitter that talks directly to the public Nockchain API at
    /// `endpoint`.
    pub fn new(endpoint: String) -> Self {
        Self { endpoint }
    }

    async fn submit_transaction_inner(
        &self,
        transaction: &nockchain_types::v1::Transaction,
    ) -> Result<WithdrawalSubmitAttemptStatus, BridgeError> {
        let raw_tx = raw_tx_from_transaction(transaction)?;
        let submitted_raw_tx_id = withdrawal_raw_tx::raw_tx_id_base58(&raw_tx);
        self.submit_raw_tx_inner(&submitted_raw_tx_id, &raw_tx)
            .await
    }

    async fn submit_raw_tx_inner(
        &self,
        submitted_raw_tx_id: &str,
        raw_tx: &nockchain_types::v1::RawTx,
    ) -> Result<WithdrawalSubmitAttemptStatus, BridgeError> {
        let mut client = PublicNockchainGrpcClient::connect(self.endpoint.clone())
            .await
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to connect public nockchain client for withdrawal submission at {}: {err}",
                    self.endpoint
                ))
            })?;
        client
            .wallet_send_transaction(raw_tx.clone())
            .await
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to submit withdrawal transaction through {}: {err}",
                    self.endpoint
                ))
            })?;
        match self
            .raw_tx_mempool_accepted_by_id(submitted_raw_tx_id)
            .await
        {
            Ok(Some(true)) => Ok(WithdrawalSubmitAttemptStatus::MempoolAccepted),
            Ok(Some(false) | None) | Err(_) => Ok(WithdrawalSubmitAttemptStatus::NotYetAccepted),
        }
    }

    async fn transaction_mempool_accepted_by_transaction(
        &self,
        transaction: &nockchain_types::v1::Transaction,
    ) -> Result<Option<bool>, BridgeError> {
        self.raw_tx_mempool_accepted_by_id(&transaction_id_base58(transaction)?)
            .await
    }

    async fn raw_tx_mempool_accepted_by_id(
        &self,
        submitted_raw_tx_id: &str,
    ) -> Result<Option<bool>, BridgeError> {
        let mut client = PublicNockchainGrpcClient::connect(self.endpoint.clone())
            .await
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to connect public nockchain client for accepted check at {}: {err}",
                    self.endpoint
                ))
            })?;
        let response = client
            .transaction_accepted(Base58Hash {
                hash: submitted_raw_tx_id.to_string(),
            })
            .await
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to query withdrawal transaction accepted state through {}: {err}",
                    self.endpoint
                ))
            })?;
        match response.result {
            Some(transaction_accepted_response::Result::Accepted(accepted)) => Ok(Some(accepted)),
            Some(transaction_accepted_response::Result::Error(err)) => {
                Err(BridgeError::Runtime(format!(
                    "withdrawal transaction accepted query through {} returned error: {}",
                    self.endpoint, err.message
                )))
            }
            None => Ok(None),
        }
    }

    async fn resubmit_raw_tx_with_retry(
        &self,
        raw_tx: &nockchain_types::v1::RawTx,
    ) -> Result<WithdrawalNetworkSubmitStatus, BridgeError> {
        #[derive(Debug)]
        enum ResubmitAttemptError {
            NotYetAccepted,
            Submit(BridgeError),
        }

        let submitted_raw_tx_id = withdrawal_raw_tx::raw_tx_id_base58(raw_tx);
        if matches!(
            self.raw_tx_mempool_accepted_by_id(&submitted_raw_tx_id)
                .await,
            Ok(Some(true))
        ) {
            return Ok(WithdrawalNetworkSubmitStatus::AlreadyMempoolAccepted);
        }

        let submitter = self.clone();
        let raw_tx = raw_tx.clone();
        let submit = move || {
            let submitter = submitter.clone();
            let submitted_raw_tx_id = submitted_raw_tx_id.clone();
            let raw_tx = raw_tx.clone();
            async move {
                match submitter
                    .submit_raw_tx_inner(&submitted_raw_tx_id, &raw_tx)
                    .await
                {
                    Ok(WithdrawalSubmitAttemptStatus::MempoolAccepted) => Ok(()),
                    Ok(WithdrawalSubmitAttemptStatus::NotYetAccepted) => {
                        Err(ResubmitAttemptError::NotYetAccepted)
                    }
                    Err(err) => Err(ResubmitAttemptError::Submit(err)),
                }
            }
        };

        match submit
            .retry(default_withdrawal_submit_retry_policy().exponential_builder())
            .await
        {
            Ok(()) => Ok(WithdrawalNetworkSubmitStatus::MempoolAccepted),
            Err(ResubmitAttemptError::NotYetAccepted) => {
                Ok(WithdrawalNetworkSubmitStatus::RetryExhausted)
            }
            Err(ResubmitAttemptError::Submit(err)) => Err(err),
        }
    }
}

#[async_trait]
impl WithdrawalSubmitPort for PublicNockchainWithdrawalSubmitter {
    async fn submission_node_available(&self) -> Result<(), BridgeError> {
        PublicNockchainGrpcClient::connect(self.endpoint.clone())
            .await
            .map(|_| ())
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to connect public nockchain client for withdrawal submission at {}: {err}",
                    self.endpoint
                ))
            })
    }

    async fn submit_withdrawal(
        &self,
        proposal: &WithdrawalProposalData,
    ) -> Result<WithdrawalSubmitAttemptStatus, BridgeError> {
        self.submit_transaction_inner(&proposal.transaction).await
    }

    async fn resubmit_raw_tx(
        &self,
        raw_tx: &nockchain_types::v1::RawTx,
    ) -> Result<WithdrawalNetworkSubmitStatus, BridgeError> {
        self.resubmit_raw_tx_with_retry(raw_tx).await
    }

    async fn transaction_mempool_accepted(
        &self,
        proposal: &WithdrawalProposalData,
    ) -> Result<Option<bool>, BridgeError> {
        self.transaction_mempool_accepted_by_transaction(&proposal.transaction)
            .await
    }

    async fn get_transaction_included_block(
        &self,
        submitted_raw_tx_id: &str,
    ) -> Result<Option<WithdrawalIncludedBlock>, BridgeError> {
        let mut client = PublicNockchainGrpcClient::connect(self.endpoint.clone())
            .await
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to connect public nockchain client for transaction inclusion lookup at {}: {err}",
                    self.endpoint
                ))
            })?;
        client
            .get_transaction_block(Base58Hash {
                hash: submitted_raw_tx_id.to_string(),
            })
            .await
            .map(|result| {
                result.map(|(height, block_id)| WithdrawalIncludedBlock { height, block_id })
            })
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to query transaction inclusion through {}: {err}",
                    self.endpoint
                ))
            })
    }

    async fn current_nockchain_tip_height(&self) -> Result<Option<u64>, BridgeError> {
        let mut client = PublicNockchainGrpcClient::connect(self.endpoint.clone())
            .await
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to connect public nockchain client for tip height lookup at {}: {err}",
                    self.endpoint
                ))
            })?;
        client
            .explorer_heaviest_height()
            .await
            .map(Some)
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to query nockchain tip height through {}: {err}",
                    self.endpoint
                ))
            })
    }
}

/// Ensures a sequencer frontier exists by registering only the earliest local
/// tracked withdrawal when the sequencer currently has no live frontier.
async fn ensure_frontier_withdrawal_registered<S: WithdrawalSequencerPort>(
    context: &WithdrawalSubmissionContext<S>,
) -> Result<Option<u64>, BridgeError> {
    if let Some(frontier_nonce) = context.sequencer.current_live_withdrawal_nonce().await? {
        return Ok(Some(frontier_nonce));
    }
    for tracked in context
        .proposal_registry
        .load_sorted_tracked_withdrawal_requests()
        .await?
    {
        let status = context
            .sequencer
            .get_sequenced_withdrawal_status(&tracked.id)
            .await?;
        if sequenced_withdrawal_released(&status) {
            continue;
        }
        register_withdrawal_or_alert(context.sequencer.as_ref(), &context.bridge_status, &tracked)
            .await?;
        if let Some(frontier_nonce) = context.sequencer.current_live_withdrawal_nonce().await? {
            return Ok(Some(frontier_nonce));
        }
        let status = context
            .sequencer
            .get_sequenced_withdrawal_status(&tracked.id)
            .await?;
        if sequenced_withdrawal_released(&status) {
            continue;
        }
        return Ok(None);
    }
    Ok(None)
}

async fn local_frontier_row<S: WithdrawalSequencerPort>(
    context: &WithdrawalSubmissionContext<S>,
    frontier_nonce: u64,
) -> Result<Option<LiveWithdrawalView>, BridgeError> {
    context
        .proposal_registry
        .fetch_live_withdrawal_by_nonce(frontier_nonce)
        .await
}

/// Runs one bridge-side submission tick.
///
/// The tick keeps sequencer nonce registrations up to date, expires stale local
/// pre-canonical state locally, asks the sequencer for the current frontier
/// withdrawal status, authorizes fully signed local peer-canonical proposals
/// when this node owns the sequencer-reported handoff turn, and then asks the
/// sequencer to submit authorized proposals.
pub async fn withdrawal_submission_tick_once<S: WithdrawalSequencerPort>(
    context: &WithdrawalSubmissionContext<S>,
) -> Result<WithdrawalSubmissionTickOutcome, BridgeError> {
    // Reconcile already-advanced sequencer states before asking for the
    // frontier nonce. Once the sequencer marks a withdrawal mempool-accepted,
    // that nonce is released from the current live frontier, so a
    // pure frontier-driven tick would otherwise never update the local row.
    if let Some(outcome) = observe_advanced_submission_statuses(context).await? {
        return Ok(outcome);
    }

    let Some(frontier_nonce) = ensure_frontier_withdrawal_registered(context).await? else {
        metrics::init_metrics()
            .withdrawal_frontier_present
            .swap(0.0);
        return Ok(WithdrawalSubmissionTickOutcome::Idle);
    };
    metrics::init_metrics()
        .withdrawal_frontier_present
        .swap(1.0);
    metrics::init_metrics()
        .withdrawal_frontier_nonce
        .swap(frontier_nonce as f64);
    let Some(row) = local_frontier_row(context, frontier_nonce).await? else {
        metrics::init_metrics()
            .withdrawal_frontier_local_row_present
            .swap(0.0);
        return Ok(WithdrawalSubmissionTickOutcome::Idle);
    };
    metrics::init_metrics()
        .withdrawal_frontier_local_row_present
        .swap(1.0);
    let status = context
        .sequencer
        .get_sequenced_withdrawal_status(&row.id)
        .await?;
    ensure_sequencer_nonce_matches(&row.id, frontier_nonce, &status)?;
    let Some(candidate) = select_frontier_authorize_or_submit_candidate(
        &row,
        if status.found {
            status.handoff_index
        } else {
            0
        },
        context.local_node_id,
        &context.node_pkhs,
    )?
    else {
        return Ok(WithdrawalSubmissionTickOutcome::Idle);
    };

    if matches!(
        candidate.kind,
        WithdrawalSubmissionCandidateKind::AuthorizePeerCanonical
    ) {
        if matches!(
            plan_authorization_status(status.found, &status.state),
            WithdrawalAuthorizationStatusDecision::SkipAlreadyAdvanced
        ) {
            return Ok(WithdrawalSubmissionTickOutcome::Idle);
        }
        let Some(signed_proposal) = load_fully_signed_proposal_for_context(context, &row).await?
        else {
            return Ok(WithdrawalSubmissionTickOutcome::Idle);
        };
        let commit_certificate = load_peer_commit_certificate(&row)?;
        context
            .sequencer
            .authorize_proposal(
                &signed_proposal, frontier_nonce, &commit_certificate, context.local_node_id,
            )
            .await?;
        context
            .proposal_registry
            .mark_proposal_authorized(&signed_proposal)
            .await?;
        return Ok(WithdrawalSubmissionTickOutcome::Authorized {
            id: signed_proposal.id,
            epoch: signed_proposal.epoch,
        });
    }

    if !matches!(
        candidate.kind,
        WithdrawalSubmissionCandidateKind::SubmitAuthorized
    ) {
        return Ok(WithdrawalSubmissionTickOutcome::Idle);
    }
    if let Some(outcome) = observe_advanced_submission_status(context, &row, &status).await? {
        return Ok(outcome);
    }
    if matches!(
        plan_submission_status(status.found, &status.state),
        WithdrawalSubmissionStatusDecision::SkipAlreadyAdvanced
    ) {
        return Ok(WithdrawalSubmissionTickOutcome::Idle);
    }
    if !status.found
        || status.state != "authorized"
        || status.current_epoch != row.current_epoch
        || status.proposal_hash != row.proposal_hash.clone().unwrap_or_default()
    {
        return Err(BridgeError::Runtime(format!(
            "sequencer status for {:?} is not the expected authorized proposal (found={} state={} epoch={} hash={})",
            row.id,
            status.found,
            status.state,
            status.current_epoch,
            status.proposal_hash
        )));
    }
    let proposal = load_authorized_proposal(context, &row).await?;
    let outcome = context
        .sequencer
        .submit_proposal(&proposal, frontier_nonce, context.local_node_id)
        .await?;
    match outcome {
        None => Ok(WithdrawalSubmissionTickOutcome::Idle),
        Some(WithdrawalSequencerSubmitOutcome::MempoolAccepted) => {
            context
                .proposal_registry
                .mark_proposal_mempool_accepted(&proposal)
                .await?;
            Ok(WithdrawalSubmissionTickOutcome::MempoolAccepted {
                id: proposal.id,
                epoch: proposal.epoch,
            })
        }
    }
}

/// Runs the long-lived bridge-side submission loop until the bridge stops.
pub async fn run_withdrawal_submission_loop<S: WithdrawalSequencerPort>(
    context: WithdrawalSubmissionContext<S>,
    stop: StopHandle,
    policy: WithdrawalSubmissionLoopPolicy,
) {
    let mut ticker = interval(policy.poll_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;
        if stop.is_stopped() {
            continue;
        }

        let metrics = metrics::init_metrics();
        metrics.withdrawal_submission_ticks.increment();
        let started = Instant::now();
        match withdrawal_submission_tick_once(&context).await {
            Ok(outcome) => {
                metrics
                    .withdrawal_submission_tick_time
                    .add_timing(&started.elapsed());
                match outcome {
                    WithdrawalSubmissionTickOutcome::Idle => {
                        metrics.withdrawal_submission_idle.increment();
                    }
                    WithdrawalSubmissionTickOutcome::Authorized { .. } => {
                        metrics.withdrawal_submission_authorized.increment();
                    }
                    WithdrawalSubmissionTickOutcome::MempoolAccepted { .. }
                    | WithdrawalSubmissionTickOutcome::MempoolAcceptedObserved { .. } => {
                        metrics.withdrawal_submission_mempool_accepted.increment();
                    }
                }
            }
            Err(err) => {
                metrics
                    .withdrawal_submission_tick_time
                    .add_timing(&started.elapsed());
                metrics.withdrawal_submission_error.increment();
                warn!(
                    target: "bridge.withdrawal",
                    error = %err,
                    "withdrawal submission tick failed"
                );
            }
        }
    }
}

fn nockchain_inclusion_has_depth(
    tip_height: u64,
    included_height: u64,
    confirmation_depth: u64,
) -> bool {
    if tip_height == 0 || included_height == 0 || tip_height < included_height {
        return false;
    }
    tip_height.saturating_sub(included_height) >= confirmation_depth
}

/// Runs one sequencer-owned confirmation poll over accepted withdrawals using
/// the colocated public Nockchain API.
pub async fn withdrawal_sequencer_confirmation_tick_once<S>(
    withdrawal_state_store: &WithdrawalSequencerStore,
    submitter: &S,
    nockchain_confirmation_depth: u64,
) -> Result<u64, BridgeError>
where
    S: WithdrawalSubmitPort + ?Sized,
{
    metrics::init_metrics()
        .sequencer_withdrawal_confirmation_polls
        .increment();
    let mut observed = 0u64;
    let sequenced = withdrawal_state_store.list_sequenced_withdrawals().await?;
    for row in sequenced
        .into_iter()
        .filter(|row| row.state == WithdrawalState::MempoolAccepted)
    {
        let Some(submitted_raw_tx_id) = row.authorized_transaction_name.as_deref() else {
            continue;
        };
        let Some(block) = submitter
            .get_transaction_included_block(submitted_raw_tx_id)
            .await?
        else {
            continue;
        };
        let Some(tip_height) = submitter.current_nockchain_tip_height().await? else {
            continue;
        };
        if !nockchain_inclusion_has_depth(tip_height, block.height, nockchain_confirmation_depth) {
            continue;
        }
        if withdrawal_state_store
            .record_tx_confirmed_by_id(&row.id, block.height, block.block_id.clone())
            .await?
        {
            metrics::init_metrics()
                .sequencer_withdrawal_confirmation_confirmed
                .increment();
            observed = observed.saturating_add(1);
        }
    }
    Ok(observed)
}

/// Runs the sequencer-owned confirmation loop that turns tx-to-block sightings
/// into durable `confirmed` lifecycle transitions.
pub async fn run_withdrawal_sequencer_confirmation_loop<S>(
    withdrawal_state_store: Arc<WithdrawalSequencerStore>,
    submitter: Arc<S>,
    policy: WithdrawalSequencerConfirmationLoopPolicy,
) -> Result<(), BridgeError>
where
    S: WithdrawalSubmitPort + ?Sized + 'static,
{
    let mut ticker = interval(policy.poll_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;
        match withdrawal_sequencer_confirmation_tick_once(
            withdrawal_state_store.as_ref(),
            submitter.as_ref(),
            policy.nockchain_confirmation_depth,
        )
        .await
        {
            Ok(0) => {}
            Ok(observed) => {
                info!(
                    target: "bridge.withdrawal_sequencer",
                    observed,
                    "recorded confirmed withdrawal transactions from nockchain API"
                );
            }
            Err(err) => {
                metrics::init_metrics()
                    .sequencer_withdrawal_confirmation_error
                    .increment();
                warn!(
                    target: "bridge.withdrawal_sequencer",
                    error=%err,
                    "failed to observe confirmed withdrawal transactions"
                );
            }
        }
    }
}

/// Runs one sequencer-owned orphan retry pass over mempool-accepted
/// withdrawals using the colocated public Nockchain API.
pub async fn withdrawal_sequencer_orphan_retry_tick_once<S>(
    withdrawal_state_store: &WithdrawalSequencerStore,
    submitter: &S,
    base_height_tracker: &SequencerBaseHeightTracker,
    retry_after_base_blocks: u64,
) -> Result<u64, BridgeError>
where
    S: WithdrawalSubmitPort + ?Sized,
{
    let Some(current_base_height) = base_height_tracker.latest_confirmed_base_height() else {
        return Ok(0);
    };

    let mut processed = 0u64;
    let sequenced = withdrawal_state_store.list_sequenced_withdrawals().await?;
    for row in sequenced
        .into_iter()
        .filter(|row| row.state == WithdrawalState::MempoolAccepted)
    {
        let Some(last_submit_attempt_base_height) = row.last_submit_attempt_base_height else {
            continue;
        };
        if current_base_height.saturating_sub(last_submit_attempt_base_height)
            < retry_after_base_blocks
        {
            continue;
        }

        let Some(submitted_raw_tx_id) = row.authorized_transaction_name.as_deref() else {
            continue;
        };
        if submitter
            .get_transaction_included_block(submitted_raw_tx_id)
            .await?
            .is_some()
        {
            continue;
        }

        let Some(payload) = withdrawal_state_store
            .load_authorized_transaction_for_retry(&row.id)
            .await?
        else {
            continue;
        };

        let raw_tx = withdrawal_raw_tx::decode_raw_tx(payload.raw_tx_bytes)?;
        metrics::init_metrics()
            .sequencer_withdrawal_orphan_retry_attempts
            .increment();
        let retry_error = match submitter.resubmit_raw_tx(&raw_tx).await {
            Ok(WithdrawalNetworkSubmitStatus::MempoolAccepted)
            | Ok(WithdrawalNetworkSubmitStatus::AlreadyMempoolAccepted) => None,
            Ok(WithdrawalNetworkSubmitStatus::RetryExhausted) => Some(
                "transaction was not reported mempool-accepted before the retry budget was exhausted"
                    .to_string(),
            ),
            Err(err) => Some(err.to_string()),
        };
        if retry_error.is_some() {
            metrics::init_metrics()
                .sequencer_withdrawal_orphan_retry_error
                .increment();
        }
        withdrawal_state_store
            .record_mempool_retry_attempt(
                &payload.id, payload.epoch, &payload.proposal_hash, current_base_height,
                retry_error,
            )
            .await
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to record orphan retry metadata for {:?}: {err}",
                    payload.id
                ))
            })?;
        processed = processed.saturating_add(1);
    }

    Ok(processed)
}

/// Runs the sequencer-owned orphan retry loop that resubmits exact authorized
/// transactions for stale mempool-accepted withdrawals.
pub async fn run_withdrawal_sequencer_orphan_retry_loop<S>(
    withdrawal_state_store: Arc<WithdrawalSequencerStore>,
    submitter: Arc<S>,
    base_height_tracker: Arc<SequencerBaseHeightTracker>,
    policy: WithdrawalSequencerOrphanRetryLoopPolicy,
) -> Result<(), BridgeError>
where
    S: WithdrawalSubmitPort + ?Sized + 'static,
{
    let mut ticker = interval(policy.poll_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;
        match withdrawal_sequencer_orphan_retry_tick_once(
            withdrawal_state_store.as_ref(),
            submitter.as_ref(),
            base_height_tracker.as_ref(),
            policy.retry_after_base_blocks,
        )
        .await
        {
            Ok(0) => {}
            Ok(processed) => {
                info!(
                    target: "bridge.withdrawal_sequencer",
                    processed,
                    "processed stale mempool-accepted withdrawals for orphan retry"
                );
            }
            Err(err) => {
                warn!(
                    target: "bridge.withdrawal_sequencer",
                    error=%err,
                    "failed to resubmit orphaned withdrawal transactions"
                );
            }
        }
    }
}

/// Marks one sequenced withdrawal as confirmed using a concrete block
/// observation.
pub async fn observe_confirmed_withdrawal(
    proposal_registry: &WithdrawalProposalRegistry,
    id: &WithdrawalId,
    confirmed_height: u64,
    confirmed_block_id: Tip5Hash,
) -> Result<bool, BridgeError> {
    let Some(row) = proposal_registry.fetch_live_withdrawal(id).await? else {
        return Ok(false);
    };
    let proposal = load_sequenced_proposal(proposal_registry, &row).await?;
    proposal_registry
        .mark_proposal_confirmed(&proposal, confirmed_height, confirmed_block_id.clone())
        .await?;
    Ok(true)
}

/// Loads the currently cached proposal body for the sequenced row.
pub(crate) async fn load_sequenced_proposal(
    proposal_registry: &WithdrawalProposalRegistry,
    row: &LiveWithdrawalView,
) -> Result<WithdrawalProposalData, BridgeError> {
    proposal_registry
        .fetch_cached_proposal(row.id.clone(), row.current_epoch)
        .await?
        .ok_or_else(|| {
            BridgeError::Runtime(format!(
                "missing cached proposal for withdrawal {:?} epoch {}",
                row.id, row.current_epoch
            ))
        })
}

async fn load_or_hydrate_sequenced_proposal<S: WithdrawalSequencerPort>(
    context: &WithdrawalSubmissionContext<S>,
    row: &LiveWithdrawalView,
) -> Result<WithdrawalProposalData, BridgeError> {
    if let Some(proposal) = context
        .proposal_registry
        .fetch_cached_proposal(row.id.clone(), row.current_epoch)
        .await?
    {
        return Ok(proposal);
    }
    let artifacts = context
        .sequencer
        .load_canonical_proposal_artifacts(&row.id)
        .await?
        .ok_or_else(|| {
            BridgeError::Runtime(format!(
                "missing sequencer canonical artifacts for withdrawal {:?} epoch {}",
                row.id, row.current_epoch
            ))
        })?;
    let tracked = TrackedWithdrawalRequest::from_live_withdrawal(row)?;
    let proposal = reconstruct_withdrawal_proposal(&tracked, artifacts)?;
    if proposal.epoch != row.current_epoch {
        return Err(BridgeError::Runtime(format!(
            "hydrated proposal epoch {} does not match live row epoch {} for {:?}",
            proposal.epoch, row.current_epoch, row.id
        )));
    }
    context
        .proposal_registry
        .cache_reconstructed_proposal(proposal.clone())
        .await?;
    Ok(proposal)
}

/// Reconstructs the fully signed proposal for a peer-canonical row by merging
/// signature contributions into the base proposal until the transaction reaches
/// exact-threshold completeness.
#[cfg(test)]
pub(crate) async fn load_fully_signed_proposal(
    proposal_registry: &WithdrawalProposalRegistry,
    row: &LiveWithdrawalView,
) -> Result<Option<WithdrawalProposalData>, BridgeError> {
    let base_proposal = load_sequenced_proposal(proposal_registry, row).await?;
    let Some(proposal_hash) = row.proposal_hash.as_deref() else {
        return Ok(None);
    };
    let signed_transactions = proposal_registry
        .load_signed_transactions(&row.id, row.current_epoch, proposal_hash)
        .await?;
    if signed_transactions.is_empty() {
        return Ok(None);
    }

    let mut merged = base_proposal.clone();
    for record in &signed_transactions {
        merge_signed_transaction(&mut merged.transaction, &record.transaction)?;
        if transaction_is_fully_signed(&merged.transaction) {
            return Ok(Some(merged));
        }
    }
    Ok(None)
}

async fn load_fully_signed_proposal_for_context<S: WithdrawalSequencerPort>(
    context: &WithdrawalSubmissionContext<S>,
    row: &LiveWithdrawalView,
) -> Result<Option<WithdrawalProposalData>, BridgeError> {
    let base_proposal = load_or_hydrate_sequenced_proposal(context, row).await?;
    let Some(proposal_hash) = row.proposal_hash.as_deref() else {
        return Ok(None);
    };
    let signed_transactions = context
        .proposal_registry
        .load_signed_transactions(&row.id, row.current_epoch, proposal_hash)
        .await?;
    if signed_transactions.is_empty() {
        return Ok(None);
    }

    let mut merged = base_proposal;
    for record in &signed_transactions {
        merge_signed_transaction(&mut merged.transaction, &record.transaction)?;
        if transaction_is_fully_signed(&merged.transaction) {
            return Ok(Some(merged));
        }
    }
    Ok(None)
}

/// Loads the exact proposal transaction that was durably authorized for
/// submission.
async fn load_authorized_proposal<S: WithdrawalSequencerPort>(
    context: &WithdrawalSubmissionContext<S>,
    row: &LiveWithdrawalView,
) -> Result<WithdrawalProposalData, BridgeError> {
    let Some(proposal_hash) = row.proposal_hash.as_deref() else {
        return Err(BridgeError::Runtime(format!(
            "missing authorized proposal hash for withdrawal {:?}",
            row.id
        )));
    };
    let mut proposal = load_or_hydrate_sequenced_proposal(context, row).await?;
    match context
        .sequencer
        .load_canonical_proposal_artifacts(&row.id)
        .await?
    {
        Some(artifacts) => {
            let Some(authorized_transaction_jam) = artifacts.authorized_transaction_jam else {
                metrics::init_metrics()
                    .withdrawal_submission_missing_authorized_artifacts
                    .increment();
                return Err(BridgeError::Runtime(format!(
                    "missing authorized transaction artifacts for withdrawal {:?} epoch {}",
                    row.id, row.current_epoch
                )));
            };
            proposal.transaction = cue_transaction(authorized_transaction_jam)?;
        }
        None => {
            if let Some(signed_proposal) =
                load_fully_signed_proposal_for_context(context, row).await?
            {
                proposal = signed_proposal;
            } else {
                metrics::init_metrics()
                    .withdrawal_submission_missing_authorized_artifacts
                    .increment();
                return Err(BridgeError::Runtime(format!(
                    "missing fully signed authorized transaction for withdrawal {:?} epoch {}",
                    row.id, row.current_epoch
                )));
            }
        }
    }
    let computed_hash = proposal.proposal_hash()?;
    if computed_hash != proposal_hash {
        return Err(BridgeError::Runtime(format!(
            "authorized proposal hash mismatch for withdrawal {:?}: expected {} got {}",
            row.id, proposal_hash, computed_hash
        )));
    }
    Ok(proposal)
}

/// Scans locally authorized withdrawals for a sequencer state that has already
/// advanced past "authorized" and applies the first matching local transition.
///
/// This helper intentionally returns only one outcome because
/// `withdrawal_submission_tick_once` models a single meaningful transition per
/// tick. We still scan *all* authorized rows in nonce order so the earliest
/// blocking withdrawal is reconciled first, but we stop after the first local
/// state change and let the next tick continue from there.
async fn observe_advanced_submission_statuses<S: WithdrawalSequencerPort>(
    context: &WithdrawalSubmissionContext<S>,
) -> Result<Option<WithdrawalSubmissionTickOutcome>, BridgeError> {
    for row in context
        .proposal_registry
        .list_live_withdrawals_in_state(WithdrawalState::Authorized)
        .await?
    {
        let status = context
            .sequencer
            .get_sequenced_withdrawal_status(&row.id)
            .await?;
        let Some(local_nonce) = row.withdrawal_nonce else {
            continue;
        };
        ensure_sequencer_nonce_matches(&row.id, local_nonce, &status)?;
        if let Some(outcome) = observe_advanced_submission_status(context, &row, &status).await? {
            return Ok(Some(outcome));
        }
    }

    Ok(None)
}

/// Reconciles one locally authorized withdrawal against an already-advanced
/// sequencer submission state.
///
/// We only treat the status as authoritative when it still refers to the same
/// `(withdrawal id, epoch, authorized proposal hash)` that the local row is
/// tracking. That protects us from incorrectly mutating a stale local row when
/// the sequencer has already moved on to a different attempt for the same
/// withdrawal id.
async fn observe_advanced_submission_status<S: WithdrawalSequencerPort>(
    context: &WithdrawalSubmissionContext<S>,
    row: &LiveWithdrawalView,
    status: &SequencedWithdrawalStatusResponse,
) -> Result<Option<WithdrawalSubmissionTickOutcome>, BridgeError> {
    if !status.found
        || !matches!(
            plan_submission_status(status.found, &status.state),
            WithdrawalSubmissionStatusDecision::SkipAlreadyAdvanced
        )
        || status.current_epoch != row.current_epoch
        || status.proposal_hash != row.proposal_hash.clone().unwrap_or_default()
    {
        return Ok(None);
    }

    let proposal = load_authorized_proposal(context, row).await?;
    match status.state.as_str() {
        "mempool_accepted" => {
            context
                .proposal_registry
                .mark_proposal_mempool_accepted(&proposal)
                .await?;
            Ok(Some(
                WithdrawalSubmissionTickOutcome::MempoolAcceptedObserved {
                    id: proposal.id,
                    epoch: proposal.epoch,
                },
            ))
        }
        _ => Ok(None),
    }
}

/// Decodes the peer-canonical commit certificate attached to a sequenced
/// withdrawal row.
fn load_peer_commit_certificate(
    row: &LiveWithdrawalView,
) -> Result<WithdrawalCommitCertificate, BridgeError> {
    let Some(bytes) = row.peer_commit_certificate.as_ref() else {
        return Err(BridgeError::Runtime(format!(
            "missing peer commit certificate for peer-canonical withdrawal {:?}",
            row.id
        )));
    };
    WithdrawalCommitCertificate::decode(bytes.as_slice()).map_err(|err| {
        BridgeError::Runtime(format!(
            "failed to decode peer commit certificate for {:?}: {err}",
            row.id
        ))
    })
}

/// Treats sequencer/local nonce disagreement as a hard consistency failure.
fn ensure_sequencer_nonce_matches(
    id: &WithdrawalId,
    local_nonce: u64,
    status: &SequencedWithdrawalStatusResponse,
) -> Result<(), BridgeError> {
    if status.withdrawal_nonce != local_nonce {
        return Err(BridgeError::Runtime(format!(
            "sequencer withdrawal nonce mismatch for {:?}: local {} sequencer {}",
            id, local_nonce, status.withdrawal_nonce
        )));
    }
    Ok(())
}

/// Merges one signed transaction contribution into the canonical base
/// transaction while rejecting any non-witness divergence.
fn merge_signed_transaction(
    base: &mut nockchain_types::v1::Transaction,
    signed: &nockchain_types::v1::Transaction,
) -> Result<(), BridgeError> {
    match (base, signed) {
        (
            nockchain_types::v1::Transaction::V1(base_tx),
            nockchain_types::v1::Transaction::V1(signed_tx),
        ) => {
            if base_tx.name != signed_tx.name
                || base_tx.spends != signed_tx.spends
                || base_tx.metadata != signed_tx.metadata
            {
                return Err(BridgeError::Runtime(
                    "signed withdrawal transaction diverged from canonical proposal".into(),
                ));
            }
            base_tx.witness_data =
                merge_witness_data(&base_tx.witness_data, &signed_tx.witness_data)?;
            Ok(())
        }
    }
}

/// Merges witness maps from multiple signer contributions into a single witness
/// payload.
fn merge_witness_data(
    base: &nockchain_types::v1::WitnessData,
    signed: &nockchain_types::v1::WitnessData,
) -> Result<nockchain_types::v1::WitnessData, BridgeError> {
    match (base, signed) {
        (
            nockchain_types::v1::WitnessData::Witnesses(base_map),
            nockchain_types::v1::WitnessData::Witnesses(signed_map),
        ) => {
            let mut merged = base_map.0.clone();
            for (name, signed_witness) in &signed_map.0 {
                if let Some((_, existing)) = merged
                    .iter_mut()
                    .find(|(existing_name, _)| existing_name == name)
                {
                    *existing = merge_witness(existing, signed_witness)?;
                } else {
                    merged.push((name.clone(), signed_witness.clone()));
                }
            }
            Ok(nockchain_types::v1::WitnessData::Witnesses(
                nockchain_types::v1::WitnessMap(merged),
            ))
        }
        _ => Err(BridgeError::Runtime(
            "withdrawal signing requires witness-based transactions".into(),
        )),
    }
}

/// Merges one input witness contribution, preserving proof data while adding
/// non-conflicting signer entries.
fn merge_witness(
    base: &nockchain_types::v1::Witness,
    signed: &nockchain_types::v1::Witness,
) -> Result<nockchain_types::v1::Witness, BridgeError> {
    if base.lock_merkle_proof != signed.lock_merkle_proof
        || base.hax != signed.hax
        || base.tim != signed.tim
    {
        return Err(BridgeError::Runtime(
            "signed withdrawal witness diverged from canonical proof data".into(),
        ));
    }

    let mut merged_entries = base.pkh_signature.0.clone();
    for entry in &signed.pkh_signature.0 {
        if let Some(existing) = merged_entries
            .iter()
            .find(|existing| existing.pkh == entry.pkh)
        {
            if existing != entry {
                return Err(BridgeError::Runtime(
                    "conflicting withdrawal witness signature for same signer".into(),
                ));
            }
        } else {
            merged_entries.push(entry.clone());
        }
    }

    Ok(nockchain_types::v1::Witness {
        lock_merkle_proof: base.lock_merkle_proof.clone(),
        pkh_signature: nockchain_types::v1::PkhSignature::new(merged_entries),
        hax: base.hax.clone(),
        tim: base.tim,
    })
}

/// Returns whether every threshold-controlled input in the transaction now
/// carries exactly the required PKH witness set for chain-valid submission.
pub(crate) fn transaction_is_fully_signed(transaction: &nockchain_types::v1::Transaction) -> bool {
    let nockchain_types::v1::Transaction::V1(transaction) = transaction;
    let nockchain_types::v1::InputMetadata::SpendConditions(input_metadata) =
        &transaction.metadata.inputs
    else {
        return false;
    };
    let nockchain_types::v1::WitnessData::Witnesses(witness_map) = &transaction.witness_data else {
        return false;
    };

    input_metadata.0.iter().all(|(name, spend_condition)| {
        let Some(required) = spend_condition.required_pkh_policy() else {
            return false;
        };
        let Some((_, witness)) = witness_map
            .0
            .iter()
            .find(|(witness_name, _)| witness_name == name)
        else {
            return false;
        };
        if witness.pkh_signature.0.len() != required.threshold {
            return false;
        }
        let mut seen = HashSet::new();
        witness
            .pkh_signature
            .0
            .iter()
            .all(|entry| required.contains(&entry.pkh) && seen.insert(entry.pkh.clone()))
    })
}

/// Returns the actual submitted raw transaction id, which is recomputed from
/// the finalized raw transaction contents instead of reusing the stable
/// envelope transaction name.
pub(crate) fn transaction_id_base58(
    transaction: &nockchain_types::v1::Transaction,
) -> Result<String, BridgeError> {
    withdrawal_raw_tx::submitted_raw_tx_id_base58(transaction)
}

/// Converts a fully merged transaction envelope into the raw transaction shape
/// expected by the submission RPC.
pub(crate) fn raw_tx_from_transaction(
    transaction: &nockchain_types::v1::Transaction,
) -> Result<nockchain_types::v1::RawTx, BridgeError> {
    withdrawal_raw_tx::raw_tx_from_transaction(transaction)
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::sync::{Arc, Mutex};

    use diesel::prelude::*;
    use diesel::sqlite::SqliteConnection;
    use nockapp::noun::slab::{NockJammer, NounSlab};
    use nockchain_math::belt::Belt;
    use nockchain_types::tx_engine::common::Hash as Tip5Hash;
    use nockchain_types::v1::Name;
    use noun_serde::NounDecode;
    use tempfile::tempdir;

    use super::*;
    use crate::observability::tui::types::AlertSeverity;
    use crate::shared::types::AtomBytes;
    use crate::withdrawal::proposals::{WithdrawalProjectionStore, WithdrawalProposalRegistry};
    use crate::withdrawal::sequencer::schema::sequencer_withdrawals;
    use crate::withdrawal::transport::withdrawal_id_to_proto;
    use crate::withdrawal::types::{
        NockWithdrawalRequestKernelData, WithdrawalProposalData, WithdrawalSnapshot,
    };

    struct RecordingSequencerPort {
        registered: Mutex<Vec<(WithdrawalId, u64)>>,
        authorized: Mutex<Vec<(WithdrawalId, u64)>>,
        submitted: Mutex<Vec<WithdrawalProposalData>>,
        reserved_inputs: Mutex<HashMap<WithdrawalId, Vec<Name>>>,
        submit_outcome: Mutex<Option<WithdrawalSequencerSubmitOutcome>>,
        statuses: Mutex<HashMap<WithdrawalId, SequencedWithdrawalStatusResponse>>,
        register_error: Mutex<Option<String>>,
    }

    impl Default for RecordingSequencerPort {
        fn default() -> Self {
            Self {
                registered: Mutex::new(Vec::new()),
                authorized: Mutex::new(Vec::new()),
                submitted: Mutex::new(Vec::new()),
                reserved_inputs: Mutex::new(HashMap::new()),
                submit_outcome: Mutex::new(Some(WithdrawalSequencerSubmitOutcome::MempoolAccepted)),
                statuses: Mutex::new(HashMap::new()),
                register_error: Mutex::new(None),
            }
        }
    }

    #[derive(Default)]
    struct RecordingSubmitter {
        included_blocks: Mutex<HashMap<String, WithdrawalIncludedBlock>>,
        nockchain_tip_height: Mutex<Option<u64>>,
        resubmitted_raw_tx_ids: Mutex<Vec<String>>,
        resubmitted_raw_txs: Mutex<Vec<nockchain_types::v1::RawTx>>,
        scripted_resubmit_results: Mutex<VecDeque<Result<WithdrawalNetworkSubmitStatus, String>>>,
    }

    impl RecordingSubmitter {
        fn script_resubmit_results(
            &self,
            results: impl IntoIterator<Item = Result<WithdrawalNetworkSubmitStatus, String>>,
        ) {
            self.scripted_resubmit_results
                .lock()
                .expect("scripted resubmit results lock")
                .extend(results);
        }

        fn set_nockchain_tip_height(&self, height: u64) {
            *self
                .nockchain_tip_height
                .lock()
                .expect("nockchain tip height lock") = Some(height);
        }

        fn set_included_block(&self, submitted_raw_tx_id: String, block: WithdrawalIncludedBlock) {
            self.included_blocks
                .lock()
                .expect("included blocks lock")
                .insert(submitted_raw_tx_id, block);
        }
    }

    #[async_trait]
    impl WithdrawalSubmitPort for RecordingSubmitter {
        async fn submit_withdrawal(
            &self,
            _proposal: &WithdrawalProposalData,
        ) -> Result<WithdrawalSubmitAttemptStatus, BridgeError> {
            Ok(WithdrawalSubmitAttemptStatus::MempoolAccepted)
        }

        async fn resubmit_raw_tx(
            &self,
            raw_tx: &nockchain_types::v1::RawTx,
        ) -> Result<WithdrawalNetworkSubmitStatus, BridgeError> {
            self.resubmitted_raw_tx_ids
                .lock()
                .expect("resubmitted raw tx ids lock")
                .push(withdrawal_raw_tx::raw_tx_id_base58(raw_tx));
            self.resubmitted_raw_txs
                .lock()
                .expect("resubmitted raw txs lock")
                .push(raw_tx.clone());
            match self
                .scripted_resubmit_results
                .lock()
                .expect("scripted resubmit results lock")
                .pop_front()
                .unwrap_or(Ok(WithdrawalNetworkSubmitStatus::MempoolAccepted))
            {
                Ok(status) => Ok(status),
                Err(message) => Err(BridgeError::Runtime(message)),
            }
        }

        async fn get_transaction_included_block(
            &self,
            submitted_raw_tx_id: &str,
        ) -> Result<Option<WithdrawalIncludedBlock>, BridgeError> {
            Ok(self
                .included_blocks
                .lock()
                .expect("included blocks lock")
                .get(submitted_raw_tx_id)
                .cloned())
        }

        async fn current_nockchain_tip_height(&self) -> Result<Option<u64>, BridgeError> {
            Ok(*self
                .nockchain_tip_height
                .lock()
                .expect("nockchain tip height lock"))
        }
    }

    #[async_trait]
    impl WithdrawalSequencerPort for RecordingSequencerPort {
        async fn register_withdrawal(
            &self,
            tracked: &TrackedWithdrawalRequest,
        ) -> Result<(), BridgeError> {
            if let Some(err) = self
                .register_error
                .lock()
                .expect("register error lock")
                .clone()
            {
                return Err(BridgeError::Runtime(err));
            }
            self.registered
                .lock()
                .expect("registered withdrawals lock")
                .push((tracked.id.clone(), tracked.withdrawal_nonce));
            self.statuses
                .lock()
                .expect("sequencer statuses lock")
                .entry(tracked.id.clone())
                .and_modify(|status| status.withdrawal_nonce = tracked.withdrawal_nonce)
                .or_insert(SequencedWithdrawalStatusResponse {
                    found: true,
                    current_epoch: 0,
                    state: "pending".to_string(),
                    proposal_hash: String::new(),
                    authorized_transaction_name: String::new(),
                    withdrawal_nonce: tracked.withdrawal_nonce,
                    handoff_index: 0,
                    turn_started_base_height: None,

                    current_confirmed_base_height: None,

                    handoff_window_blocks: 0,

                    blocks_until_handoff: None,
                });
            Ok(())
        }

        async fn record_signed_proposal(
            &self,
            _proposal: &WithdrawalProposalData,
            _withdrawal_nonce: u64,
            _signer_node_id: u64,
        ) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn get_reserved_withdrawal_inputs(
            &self,
        ) -> Result<Vec<nockchain_types::v1::Name>, BridgeError> {
            Ok(self
                .reserved_inputs
                .lock()
                .expect("reserved inputs lock")
                .values()
                .flat_map(|inputs| inputs.clone())
                .collect())
        }

        async fn authorize_proposal(
            &self,
            proposal: &WithdrawalProposalData,
            withdrawal_nonce: u64,
            _commit_certificate: &WithdrawalCommitCertificate,
            _caller_node_id: u64,
        ) -> Result<(), BridgeError> {
            let proposal_hash = proposal
                .proposal_hash()
                .expect("proposal hash for authorized status");
            self.authorized
                .lock()
                .expect("authorized proposals lock")
                .push((proposal.id.clone(), withdrawal_nonce));
            self.reserved_inputs
                .lock()
                .expect("reserved inputs lock")
                .insert(proposal.id.clone(), proposal.selected_inputs.clone());
            self.statuses
                .lock()
                .expect("sequencer statuses lock")
                .insert(
                    proposal.id.clone(),
                    SequencedWithdrawalStatusResponse {
                        found: true,
                        current_epoch: proposal.epoch,
                        state: "authorized".to_string(),
                        proposal_hash,
                        authorized_transaction_name: transaction_id_base58(&proposal.transaction)
                            .expect("authorized transaction name"),
                        withdrawal_nonce,
                        handoff_index: 0,
                        turn_started_base_height: None,

                        current_confirmed_base_height: None,

                        handoff_window_blocks: 0,

                        blocks_until_handoff: None,
                    },
                );
            Ok(())
        }

        async fn submit_proposal(
            &self,
            proposal: &WithdrawalProposalData,
            withdrawal_nonce: u64,
            _caller_node_id: u64,
        ) -> Result<Option<WithdrawalSequencerSubmitOutcome>, BridgeError> {
            self.submitted
                .lock()
                .expect("submitted proposals lock")
                .push(proposal.clone());
            let proposal_hash = proposal
                .proposal_hash()
                .expect("proposal hash for submitted status");
            let outcome = *self.submit_outcome.lock().expect("submit outcome lock");
            if outcome.is_some() {
                self.statuses
                    .lock()
                    .expect("sequencer statuses lock")
                    .insert(
                        proposal.id.clone(),
                        SequencedWithdrawalStatusResponse {
                            found: true,
                            current_epoch: proposal.epoch,
                            state: "mempool_accepted".to_string(),
                            proposal_hash,
                            authorized_transaction_name: transaction_id_base58(
                                &proposal.transaction,
                            )
                            .expect("submitted transaction name"),
                            withdrawal_nonce,
                            handoff_index: 0,
                            turn_started_base_height: None,

                            current_confirmed_base_height: None,

                            handoff_window_blocks: 0,

                            blocks_until_handoff: None,
                        },
                    );
            }
            Ok(outcome)
        }

        async fn get_next_pending_withdrawal_ordering(
            &self,
        ) -> Result<Option<NextPendingWithdrawalOrdering>, BridgeError> {
            let registered = self
                .registered
                .lock()
                .expect("registered withdrawals lock")
                .clone();
            let statuses = self
                .statuses
                .lock()
                .expect("sequencer statuses lock")
                .clone();
            let mut candidates = registered;
            candidates.extend(
                statuses
                    .iter()
                    .filter(|(_, status)| status.withdrawal_nonce > 0)
                    .map(|(id, status)| (id.clone(), status.withdrawal_nonce)),
            );
            candidates.sort_by_key(|(_, withdrawal_nonce)| *withdrawal_nonce);
            candidates.dedup();
            for (id, withdrawal_nonce) in candidates {
                let released = statuses
                    .get(&id)
                    .map(|status| matches!(status.state.as_str(), "mempool_accepted" | "confirmed"))
                    .unwrap_or(false);
                if !released {
                    return Ok(Some(NextPendingWithdrawalOrdering {
                        id,
                        withdrawal_nonce,
                    }));
                }
            }
            Ok(None)
        }

        async fn get_sequenced_withdrawal_status(
            &self,
            id: &WithdrawalId,
        ) -> Result<SequencedWithdrawalStatusResponse, BridgeError> {
            Ok(self
                .statuses
                .lock()
                .expect("sequencer statuses lock")
                .get(id)
                .cloned()
                .unwrap_or(SequencedWithdrawalStatusResponse {
                    found: false,
                    current_epoch: 0,
                    state: String::new(),
                    proposal_hash: String::new(),
                    authorized_transaction_name: String::new(),
                    withdrawal_nonce: 0,
                    handoff_index: 0,
                    turn_started_base_height: None,

                    current_confirmed_base_height: None,

                    handoff_window_blocks: 0,

                    blocks_until_handoff: None,
                }))
        }
    }

    async fn open_context() -> (
        Arc<WithdrawalProposalRegistry>,
        Arc<WithdrawalSequencerStore>,
        tempfile::TempDir,
    ) {
        let dir = tempdir().expect("tempdir");
        let projection_store = Arc::new(
            WithdrawalProjectionStore::open(dir.path().join("withdrawal-local-state.sqlite"))
                .await
                .expect("withdrawal projection store"),
        );
        let registry = Arc::new(
            WithdrawalProposalRegistry::new_without_transaction_body_validator_for_tests(
                projection_store,
            ),
        );
        let withdrawal_state_store = Arc::new(
            WithdrawalSequencerStore::open(dir.path().join("withdrawal-state-store.sqlite"))
                .await
                .expect("withdrawal state store"),
        );
        (registry, withdrawal_state_store, dir)
    }

    fn sample_bridge_status() -> BridgeStatus {
        BridgeStatus::new(Arc::new(std::sync::RwLock::new(Vec::new())))
    }

    #[tokio::test]
    async fn submission_tick_alerts_when_withdrawal_registration_fails() {
        let (registry, _withdrawal_state_store, _dir) = open_context().await;
        let request = sample_request();
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        let bridge_status = sample_bridge_status();
        let sequencer = Arc::new(RecordingSequencerPort {
            register_error: Mutex::new(Some("sequencer unavailable".to_string())),
            ..Default::default()
        });
        let context = WithdrawalSubmissionContext {
            sequencer,
            proposal_registry: registry,
            bridge_status: bridge_status.clone(),
            fallback_policy: WithdrawalFallbackPolicy::default(),
            local_node_id: 0,
            node_pkhs: sample_node_pkhs(),
        };

        let err = withdrawal_submission_tick_once(&context)
            .await
            .expect_err("registration failure should fail submission tick");
        assert!(err.to_string().contains("sequencer unavailable"));
        let alerts = bridge_status.alerts();
        assert!(alerts.alerts.iter().any(|alert| {
            alert.severity == AlertSeverity::Error
                && alert.title == "Withdrawal Registration Failed"
                && alert.source == "withdrawal-sequencer"
                && alert.message.contains("nonce 1")
        }));
    }

    #[tokio::test]
    async fn submission_tick_does_not_register_released_sequencer_rows() {
        let (registry, _withdrawal_state_store, _dir) = open_context().await;
        let request = sample_request();
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        let sequencer = Arc::new(RecordingSequencerPort::default());
        sequencer
            .statuses
            .lock()
            .expect("sequencer statuses lock")
            .insert(
                request.withdrawal_id(),
                SequencedWithdrawalStatusResponse {
                    found: true,
                    current_epoch: 0,
                    state: WithdrawalState::MempoolAccepted.as_str().to_string(),
                    proposal_hash: String::new(),
                    authorized_transaction_name: String::new(),
                    withdrawal_nonce: 1,
                    handoff_index: 0,
                    turn_started_base_height: None,
                    current_confirmed_base_height: None,
                    handoff_window_blocks: 0,
                    blocks_until_handoff: None,
                },
            );
        let context = WithdrawalSubmissionContext {
            sequencer: sequencer.clone(),
            proposal_registry: registry,
            bridge_status: sample_bridge_status(),
            fallback_policy: WithdrawalFallbackPolicy::default(),
            local_node_id: 0,
            node_pkhs: sample_node_pkhs(),
        };

        let outcome = withdrawal_submission_tick_once(&context)
            .await
            .expect("submission tick");
        assert_eq!(outcome, WithdrawalSubmissionTickOutcome::Idle);
        assert!(
            sequencer
                .registered
                .lock()
                .expect("registered withdrawals lock")
                .is_empty(),
            "released sequencer rows should not be re-registered"
        );
    }

    fn sample_base_event_id(start: u8) -> AtomBytes {
        AtomBytes((0..32).map(|offset| start.wrapping_add(offset)).collect())
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

    fn sample_request_with_seed(seed: u8) -> NockWithdrawalRequestKernelData {
        NockWithdrawalRequestKernelData {
            base_event_id: sample_base_event_id(seed),
            recipient: Tip5Hash([
                Belt(100 + u64::from(seed)),
                Belt(101 + u64::from(seed)),
                Belt(102 + u64::from(seed)),
                Belt(103 + u64::from(seed)),
                Belt(104 + u64::from(seed)),
            ]),
            amount: 123_456 + u64::from(seed),
            base_batch_end: 777 + u64::from(seed),
            as_of: Tip5Hash([
                Belt(10 + u64::from(seed)),
                Belt(20 + u64::from(seed)),
                Belt(30 + u64::from(seed)),
                Belt(40 + u64::from(seed)),
                Belt(50 + u64::from(seed)),
            ]),
        }
    }

    fn sample_node_pkhs() -> Vec<Tip5Hash> {
        vec![
            Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            Tip5Hash([Belt(11), Belt(12), Belt(13), Belt(14), Belt(15)]),
            Tip5Hash([Belt(21), Belt(22), Belt(23), Belt(24), Belt(25)]),
        ]
    }

    fn sample_transaction() -> nockchain_types::v1::Transaction {
        const TRANSACTION_JAM: &[u8] = include_bytes!(
            "../../test-fixtures/transactions/9MpGym52AumtwyBxYPyVsWHvcamUYwZkc1Nq7w3cFGF28u8ceVDwt3e.tx"
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
            amount: request.amount.saturating_sub(222),
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

    async fn register_proposal_ordering(
        withdrawal_state_store: &WithdrawalSequencerStore,
        proposal: &WithdrawalProposalData,
        withdrawal_nonce: u64,
    ) {
        withdrawal_state_store
            .ensure_tracked_withdrawal_ordering(&TrackedWithdrawalRequest {
                id: proposal.id.clone(),
                recipient: proposal.recipient.clone(),
                amount: proposal.burned_amount,
                base_batch_end: proposal.base_batch_end,
                withdrawal_nonce,
            })
            .await
            .expect("register withdrawal ordering");
    }

    fn sample_multisig_signer_pkhs() -> Vec<Tip5Hash> {
        vec![
            Tip5Hash([Belt(301), Belt(302), Belt(303), Belt(304), Belt(305)]),
            Tip5Hash([Belt(311), Belt(312), Belt(313), Belt(314), Belt(315)]),
            Tip5Hash([Belt(321), Belt(322), Belt(323), Belt(324), Belt(325)]),
            Tip5Hash([Belt(331), Belt(332), Belt(333), Belt(334), Belt(335)]),
            Tip5Hash([Belt(341), Belt(342), Belt(343), Belt(344), Belt(345)]),
        ]
    }

    fn configure_first_input_as_multisig(
        transaction: &mut nockchain_types::v1::Transaction,
        threshold: u64,
        allowed_hashes: &[Tip5Hash],
        signer_hashes: &[Tip5Hash],
    ) {
        let nockchain_types::v1::Transaction::V1(transaction_v1) = transaction;
        let spend_condition = nockchain_types::v1::SpendCondition::new(vec![
            nockchain_types::v1::LockPrimitive::Pkh(nockchain_types::v1::Pkh::new(
                threshold,
                allowed_hashes.iter().cloned(),
            )),
        ]);

        let template_entry = match &transaction_v1.witness_data {
            nockchain_types::v1::WitnessData::Witnesses(witness_map) => witness_map
                .0
                .first()
                .and_then(|(_, witness)| witness.pkh_signature.0.first())
                .cloned()
                .expect("fixture transaction should contain a witness signature entry"),
            _ => panic!("fixture transaction must use witness-based data"),
        };
        let entries = signer_hashes
            .iter()
            .cloned()
            .map(|pkh| {
                let mut entry = template_entry.clone();
                entry.pkh = pkh;
                entry
            })
            .collect::<Vec<_>>();

        let nockchain_types::v1::InputMetadata::SpendConditions(input_metadata) =
            &mut transaction_v1.metadata.inputs
        else {
            panic!("fixture transaction must use spend-condition metadata");
        };
        let (_, existing_spend_condition) = input_metadata
            .0
            .first_mut()
            .expect("fixture transaction should contain input metadata");
        *existing_spend_condition = spend_condition.clone();

        let nockchain_types::v1::WitnessData::Witnesses(witness_map) =
            &mut transaction_v1.witness_data
        else {
            panic!("fixture transaction must use witness-based data");
        };
        let (_, witness) = witness_map
            .0
            .first_mut()
            .expect("fixture transaction should contain a witness entry");
        witness.pkh_signature = nockchain_types::v1::PkhSignature::new(entries.clone());
    }

    fn sample_multisig_proposal(epoch: u64) -> (WithdrawalProposalData, Vec<Tip5Hash>) {
        let request = sample_request();
        let mut transaction = sample_transaction();
        let signer_pkhs = sample_multisig_signer_pkhs();
        configure_first_input_as_multisig(&mut transaction, 3, &signer_pkhs, &signer_pkhs[2..3]);
        (
            WithdrawalProposalData {
                id: request.withdrawal_id(),
                recipient: request.recipient.clone(),
                amount: request.amount.saturating_sub(222),
                burned_amount: request.amount,
                base_batch_end: request.base_batch_end,
                epoch,
                snapshot: WithdrawalSnapshot {
                    height: 42 + epoch,
                    block_id: Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
                },
                selected_inputs: transaction.normalized_input_names(),
                transaction,
            },
            signer_pkhs,
        )
    }

    fn signed_contribution(
        proposal: &WithdrawalProposalData,
        allowed_hashes: &[Tip5Hash],
        signer_hashes: &[Tip5Hash],
    ) -> WithdrawalProposalData {
        let mut signed = proposal.clone();
        configure_first_input_as_multisig(
            &mut signed.transaction, 3, allowed_hashes, signer_hashes,
        );
        signed
    }

    fn first_witness_pkhs(transaction: &nockchain_types::v1::Transaction) -> Vec<Tip5Hash> {
        let nockchain_types::v1::Transaction::V1(transaction_v1) = transaction;
        let nockchain_types::v1::WitnessData::Witnesses(witness_map) = &transaction_v1.witness_data
        else {
            panic!("transaction must use witness data");
        };
        witness_map
            .0
            .first()
            .expect("transaction should contain a witness entry")
            .1
            .pkh_signature
            .0
            .iter()
            .map(|entry| entry.pkh.clone())
            .collect()
    }

    fn proposal_for_request(
        request: &NockWithdrawalRequestKernelData,
        epoch: u64,
        seed: u64,
    ) -> WithdrawalProposalData {
        let transaction = sample_transaction();
        WithdrawalProposalData {
            id: request.withdrawal_id(),
            recipient: request.recipient.clone(),
            amount: request.amount.saturating_sub(222),
            burned_amount: request.amount,
            base_batch_end: request.base_batch_end,
            epoch,
            snapshot: WithdrawalSnapshot {
                height: 42 + epoch,
                block_id: Tip5Hash([
                    Belt(seed + 1),
                    Belt(seed + 2),
                    Belt(seed + 3),
                    Belt(seed + 4),
                    Belt(seed + 5),
                ]),
            },
            selected_inputs: transaction.normalized_input_names(),
            transaction,
        }
    }

    fn sample_commit_certificate(proposal: &WithdrawalProposalData) -> WithdrawalCommitCertificate {
        WithdrawalCommitCertificate {
            withdrawal_id: Some(withdrawal_id_to_proto(&proposal.id)),
            epoch: proposal.epoch,
            proposal_hash: proposal.proposal_hash().expect("proposal hash"),
            signatures: Vec::new(),
        }
    }

    fn sample_included_block(height: u64) -> WithdrawalIncludedBlock {
        WithdrawalIncludedBlock {
            height,
            block_id: Tip5Hash([
                Belt(height + 1),
                Belt(height + 2),
                Belt(height + 3),
                Belt(height + 4),
                Belt(height + 5),
            ]),
        }
    }

    async fn seed_sequenced_submit_state(
        withdrawal_state_store: &WithdrawalSequencerStore,
        proposal: &WithdrawalProposalData,
        final_state: WithdrawalState,
        last_submit_attempt_base_height: u64,
        last_submit_error: Option<String>,
    ) {
        register_proposal_ordering(withdrawal_state_store, proposal, 1).await;
        withdrawal_state_store
            .record_proposal_canonicalized_with_certificate(
                proposal,
                Some(&sample_commit_certificate(proposal)),
                100,
            )
            .await
            .expect("record canonicalized");
        withdrawal_state_store
            .record_proposal_authorized(proposal)
            .await
            .expect("record authorized");
        withdrawal_state_store
            .record_submit_outcome(
                proposal, final_state, 1, last_submit_attempt_base_height, last_submit_error,
            )
            .await
            .expect("record submit outcome");
    }

    async fn seed_confirmed_sequenced_withdrawal(
        withdrawal_state_store: &WithdrawalSequencerStore,
        proposal: &WithdrawalProposalData,
        last_submit_attempt_base_height: u64,
    ) {
        seed_sequenced_submit_state(
            withdrawal_state_store,
            proposal,
            WithdrawalState::MempoolAccepted,
            last_submit_attempt_base_height,
            None,
        )
        .await;
        assert!(
            withdrawal_state_store
                .record_tx_confirmed_by_id(
                    &proposal.id,
                    7_777,
                    Tip5Hash([Belt(771), Belt(772), Belt(773), Belt(774), Belt(775)]),
                )
                .await
                .expect("record confirmed by id"),
            "seed confirmation should transition the sequenced withdrawal"
        );
    }

    fn clear_last_submit_attempt_base_height(dir: &tempfile::TempDir, id: &WithdrawalId) {
        use crate::withdrawal::sequencer::schema::sequencer_withdrawals::dsl as sequenced;

        let db_path = dir.path().join("withdrawal-state-store.sqlite");
        let mut conn = SqliteConnection::establish(db_path.to_string_lossy().as_ref())
            .expect("open sequencer sqlite for test mutation");
        diesel::update(
            sequencer_withdrawals::table
                .filter(sequenced::withdrawal_id_as_of.eq(id.as_of.to_be_limb_bytes().to_vec()))
                .filter(sequenced::withdrawal_id_base_event_id.eq(id.base_event_id.0.clone())),
        )
        .set(sequenced::last_submit_attempt_base_height.eq(Option::<i64>::None))
        .execute(&mut conn)
        .expect("clear last_submit_attempt_base_height");
    }

    #[tokio::test]
    async fn submission_tick_authorizes_then_submits_and_records_mempool_acceptance() {
        let (registry, _withdrawal_state_store, _dir) = open_context().await;
        let request = sample_request();
        let proposal = sample_proposal(0);
        let node_pkhs = sample_node_pkhs();
        let local_node_id =
            crate::shared::proposer::withdrawal_active_proposer(&proposal.id, 0, &node_pkhs) as u64;
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        registry
            .validate_and_cache_prepared(&proposal)
            .await
            .expect("persist proposal");
        registry
            .mark_proposal_prepared(&proposal)
            .await
            .expect("mark prepared");
        registry
            .mark_proposal_canonical_with_certificate(
                &proposal,
                &sample_commit_certificate(&proposal),
            )
            .await
            .expect("mark canonicalized");
        registry
            .record_proposal_signed(&proposal, 7)
            .await
            .expect("record signed proposal in authoritative store");

        let sequencer = Arc::new(RecordingSequencerPort {
            registered: Mutex::new(Vec::new()),
            authorized: Mutex::new(Vec::new()),
            submitted: Mutex::new(Vec::new()),
            reserved_inputs: Mutex::new(HashMap::new()),
            submit_outcome: Mutex::new(Some(WithdrawalSequencerSubmitOutcome::MempoolAccepted)),
            statuses: Mutex::new(HashMap::new()),
            register_error: Mutex::new(None),
        });
        let context = WithdrawalSubmissionContext {
            sequencer: sequencer.clone(),
            proposal_registry: registry.clone(),
            bridge_status: sample_bridge_status(),
            fallback_policy: WithdrawalFallbackPolicy::default(),
            local_node_id,
            node_pkhs,
        };

        let authorize = withdrawal_submission_tick_once(&context)
            .await
            .expect("authorization tick");
        assert_eq!(
            authorize,
            WithdrawalSubmissionTickOutcome::Authorized {
                id: proposal.id.clone(),
                epoch: 0,
            }
        );

        let outcome = withdrawal_submission_tick_once(&context)
            .await
            .expect("submission tick");
        assert_eq!(
            outcome,
            WithdrawalSubmissionTickOutcome::MempoolAccepted {
                id: proposal.id.clone(),
                epoch: 0,
            }
        );

        let live = registry
            .fetch_live_withdrawal(&proposal.id)
            .await
            .expect("live row")
            .expect("live mempool accepted withdrawal");
        assert_eq!(live.state, WithdrawalState::MempoolAccepted);
        assert_eq!(
            sequencer
                .submitted
                .lock()
                .expect("submitted proposals")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn submission_tick_observes_mempool_accepted_status_after_submission() {
        let (registry, _withdrawal_state_store, _dir) = open_context().await;
        let request = sample_request();
        let proposal = sample_proposal(0);
        let node_pkhs = sample_node_pkhs();
        let local_node_id =
            crate::shared::proposer::withdrawal_active_proposer(&proposal.id, 0, &node_pkhs) as u64;
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        registry
            .validate_and_cache_prepared(&proposal)
            .await
            .expect("persist proposal");
        registry
            .mark_proposal_prepared(&proposal)
            .await
            .expect("mark prepared");
        registry
            .mark_proposal_canonical_with_certificate(
                &proposal,
                &sample_commit_certificate(&proposal),
            )
            .await
            .expect("mark canonicalized");
        registry
            .record_proposal_signed(&proposal, 7)
            .await
            .expect("record signed proposal in authoritative store");

        let sequencer = Arc::new(RecordingSequencerPort::default());
        let context = WithdrawalSubmissionContext {
            sequencer: sequencer.clone(),
            proposal_registry: registry.clone(),
            bridge_status: sample_bridge_status(),
            fallback_policy: WithdrawalFallbackPolicy::default(),
            local_node_id,
            node_pkhs,
        };

        registry
            .mark_proposal_authorized(&proposal)
            .await
            .expect("mark authorized");
        let proposal_hash = proposal.proposal_hash().expect("proposal hash");
        sequencer
            .statuses
            .lock()
            .expect("sequencer statuses lock")
            .insert(
                proposal.id.clone(),
                SequencedWithdrawalStatusResponse {
                    found: true,
                    current_epoch: proposal.epoch,
                    state: "mempool_accepted".to_string(),
                    proposal_hash,
                    authorized_transaction_name: transaction_id_base58(&proposal.transaction)
                        .expect("tx id"),
                    withdrawal_nonce: 1,
                    handoff_index: 0,
                    turn_started_base_height: None,

                    current_confirmed_base_height: None,

                    handoff_window_blocks: 0,

                    blocks_until_handoff: None,
                },
            );

        let outcome = withdrawal_submission_tick_once(&context)
            .await
            .expect("acceptance observation tick");
        assert_eq!(
            outcome,
            WithdrawalSubmissionTickOutcome::MempoolAcceptedObserved {
                id: proposal.id.clone(),
                epoch: proposal.epoch,
            }
        );

        let live = registry
            .fetch_live_withdrawal(&proposal.id)
            .await
            .expect("live row")
            .expect("live mempool-accepted withdrawal");
        assert_eq!(live.state, WithdrawalState::MempoolAccepted);
    }

    #[tokio::test]
    async fn submission_tick_returns_idle_when_sequencer_defers_submission() {
        let (registry, _withdrawal_state_store, _dir) = open_context().await;
        let request = sample_request();
        let proposal = sample_proposal(0);
        let node_pkhs = sample_node_pkhs();
        let local_node_id =
            crate::shared::proposer::withdrawal_active_proposer(&proposal.id, 0, &node_pkhs) as u64;
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        registry
            .validate_and_cache_prepared(&proposal)
            .await
            .expect("persist proposal");
        registry
            .mark_proposal_prepared(&proposal)
            .await
            .expect("mark prepared");
        registry
            .mark_proposal_canonical_with_certificate(
                &proposal,
                &sample_commit_certificate(&proposal),
            )
            .await
            .expect("mark canonicalized");
        registry
            .record_proposal_signed(&proposal, 7)
            .await
            .expect("record signed proposal in authoritative store");

        let sequencer = Arc::new(RecordingSequencerPort::default());
        *sequencer
            .submit_outcome
            .lock()
            .expect("submit outcome lock") = None;
        let context = WithdrawalSubmissionContext {
            sequencer: sequencer.clone(),
            proposal_registry: registry.clone(),
            bridge_status: sample_bridge_status(),
            fallback_policy: WithdrawalFallbackPolicy::default(),
            local_node_id,
            node_pkhs,
        };

        withdrawal_submission_tick_once(&context)
            .await
            .expect("authorization tick");
        let outcome = withdrawal_submission_tick_once(&context)
            .await
            .expect("deferred submission tick");
        assert_eq!(outcome, WithdrawalSubmissionTickOutcome::Idle);

        let live = registry
            .fetch_live_withdrawal(&proposal.id)
            .await
            .expect("live row")
            .expect("live authorized withdrawal");
        assert_eq!(live.state, WithdrawalState::Authorized);
        assert_eq!(
            sequencer
                .get_reserved_withdrawal_inputs()
                .await
                .expect("reserved inputs after deferred submit"),
            proposal.selected_inputs
        );
    }

    #[tokio::test]
    async fn submission_tick_blocks_later_nonce_while_prior_nonce_authorized() {
        let (registry, _withdrawal_state_store, _dir) = open_context().await;
        let earlier = sample_request_with_seed(0xaa);
        let later = sample_request_with_seed(0xbb);
        let earlier_proposal = proposal_for_request(&earlier, 0, 1_000);
        let later_proposal = proposal_for_request(&later, 0, 2_000);
        let node_pkhs = sample_node_pkhs();
        let earlier_submitter = crate::shared::proposer::withdrawal_active_proposer(
            &earlier_proposal.id, 0, &node_pkhs,
        );
        let local_node_id = (0..node_pkhs.len())
            .find(|candidate| *candidate != earlier_submitter)
            .expect("alternate node for blocked nonce test") as u64;

        for request in [&earlier, &later] {
            registry
                .track_withdrawal_request(request)
                .await
                .expect("track request");
        }
        for proposal in [&earlier_proposal, &later_proposal] {
            registry
                .validate_and_cache_prepared(proposal)
                .await
                .expect("persist proposal");
            registry
                .mark_proposal_prepared(proposal)
                .await
                .expect("mark prepared");
            registry
                .mark_proposal_canonical_with_certificate(
                    proposal,
                    &sample_commit_certificate(proposal),
                )
                .await
                .expect("mark canonicalized");
        }
        registry
            .record_proposal_signed(&earlier_proposal, 7)
            .await
            .expect("record earlier signed proposal");
        registry
            .record_proposal_signed(&later_proposal, 7)
            .await
            .expect("record later signed proposal");
        registry
            .mark_proposal_authorized(&earlier_proposal)
            .await
            .expect("authorize earlier");
        registry
            .mark_proposal_authorized(&later_proposal)
            .await
            .expect("authorize later");

        let sequencer = Arc::new(RecordingSequencerPort::default());
        let earlier_hash = earlier_proposal
            .proposal_hash()
            .expect("earlier proposal hash");
        let later_hash = later_proposal.proposal_hash().expect("later proposal hash");
        sequencer
            .statuses
            .lock()
            .expect("sequencer statuses lock")
            .insert(
                earlier_proposal.id.clone(),
                SequencedWithdrawalStatusResponse {
                    found: true,
                    current_epoch: earlier_proposal.epoch,
                    state: "authorized".to_string(),
                    proposal_hash: earlier_hash,
                    authorized_transaction_name: transaction_id_base58(
                        &earlier_proposal.transaction,
                    )
                    .expect("earlier tx id"),
                    withdrawal_nonce: 1,
                    handoff_index: 0,
                    turn_started_base_height: None,

                    current_confirmed_base_height: None,

                    handoff_window_blocks: 0,

                    blocks_until_handoff: None,
                },
            );
        sequencer
            .statuses
            .lock()
            .expect("sequencer statuses lock")
            .insert(
                later_proposal.id.clone(),
                SequencedWithdrawalStatusResponse {
                    found: true,
                    current_epoch: later_proposal.epoch,
                    state: "authorized".to_string(),
                    proposal_hash: later_hash,
                    authorized_transaction_name: transaction_id_base58(&later_proposal.transaction)
                        .expect("later tx id"),
                    withdrawal_nonce: 2,
                    handoff_index: 0,
                    turn_started_base_height: None,

                    current_confirmed_base_height: None,

                    handoff_window_blocks: 0,

                    blocks_until_handoff: None,
                },
            );
        let context = WithdrawalSubmissionContext {
            sequencer: sequencer.clone(),
            proposal_registry: registry,
            bridge_status: sample_bridge_status(),
            fallback_policy: WithdrawalFallbackPolicy::default(),
            local_node_id,
            node_pkhs,
        };

        let outcome = withdrawal_submission_tick_once(&context)
            .await
            .expect("submission tick");
        assert_eq!(outcome, WithdrawalSubmissionTickOutcome::Idle);
        assert!(sequencer
            .submitted
            .lock()
            .expect("submitted proposals lock")
            .is_empty());
    }

    #[tokio::test]
    async fn submission_tick_submits_later_nonce_after_prior_nonce_mempool_accepted() {
        let (registry, _withdrawal_state_store, _dir) = open_context().await;
        let earlier = sample_request_with_seed(0xaa);
        let later = sample_request_with_seed(0xbb);
        let earlier_proposal = proposal_for_request(&earlier, 0, 1_000);
        let later_proposal = proposal_for_request(&later, 0, 2_000);
        let node_pkhs = sample_node_pkhs();
        let local_node_id =
            crate::shared::proposer::withdrawal_active_proposer(&later_proposal.id, 0, &node_pkhs)
                as u64;

        for request in [&earlier, &later] {
            registry
                .track_withdrawal_request(request)
                .await
                .expect("track request");
        }
        for proposal in [&earlier_proposal, &later_proposal] {
            registry
                .validate_and_cache_prepared(proposal)
                .await
                .expect("persist proposal");
            registry
                .mark_proposal_prepared(proposal)
                .await
                .expect("mark prepared");
            registry
                .mark_proposal_canonical_with_certificate(
                    proposal,
                    &sample_commit_certificate(proposal),
                )
                .await
                .expect("mark canonicalized");
        }
        registry
            .record_proposal_signed(&later_proposal, 7)
            .await
            .expect("record later signed proposal");
        registry
            .mark_proposal_authorized(&earlier_proposal)
            .await
            .expect("authorize earlier");
        registry
            .mark_proposal_mempool_accepted(&earlier_proposal)
            .await
            .expect("mark earlier mempool accepted");
        registry
            .mark_proposal_authorized(&later_proposal)
            .await
            .expect("authorize later");

        let sequencer = Arc::new(RecordingSequencerPort::default());
        let earlier_hash = earlier_proposal
            .proposal_hash()
            .expect("earlier proposal hash");
        let later_hash = later_proposal.proposal_hash().expect("later proposal hash");
        sequencer
            .statuses
            .lock()
            .expect("sequencer statuses lock")
            .insert(
                earlier_proposal.id.clone(),
                SequencedWithdrawalStatusResponse {
                    found: true,
                    current_epoch: earlier_proposal.epoch,
                    state: "mempool_accepted".to_string(),
                    proposal_hash: earlier_hash,
                    authorized_transaction_name: transaction_id_base58(
                        &earlier_proposal.transaction,
                    )
                    .expect("earlier tx id"),
                    withdrawal_nonce: 1,
                    handoff_index: 0,
                    turn_started_base_height: None,

                    current_confirmed_base_height: None,

                    handoff_window_blocks: 0,

                    blocks_until_handoff: None,
                },
            );
        sequencer
            .statuses
            .lock()
            .expect("sequencer statuses lock")
            .insert(
                later_proposal.id.clone(),
                SequencedWithdrawalStatusResponse {
                    found: true,
                    current_epoch: later_proposal.epoch,
                    state: "authorized".to_string(),
                    proposal_hash: later_hash,
                    authorized_transaction_name: transaction_id_base58(&later_proposal.transaction)
                        .expect("later tx id"),
                    withdrawal_nonce: 2,
                    handoff_index: 0,
                    turn_started_base_height: None,

                    current_confirmed_base_height: None,

                    handoff_window_blocks: 0,

                    blocks_until_handoff: None,
                },
            );
        let context = WithdrawalSubmissionContext {
            sequencer: sequencer.clone(),
            proposal_registry: registry,
            bridge_status: sample_bridge_status(),
            fallback_policy: WithdrawalFallbackPolicy::default(),
            local_node_id,
            node_pkhs,
        };

        let outcome = withdrawal_submission_tick_once(&context)
            .await
            .expect("submission tick");
        assert_eq!(
            outcome,
            WithdrawalSubmissionTickOutcome::MempoolAccepted {
                id: later_proposal.id.clone(),
                epoch: later_proposal.epoch,
            }
        );
        let submitted = sequencer
            .submitted
            .lock()
            .expect("submitted proposals lock");
        assert_eq!(submitted.len(), 1);
        assert_eq!(submitted[0].id, later_proposal.id);
    }

    #[tokio::test]
    async fn confirmation_observation_marks_operator_rows_confirmed() {
        let (registry, _withdrawal_state_store, _dir) = open_context().await;
        let request = sample_request();
        let proposal = sample_proposal(0);
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        registry
            .validate_and_cache_prepared(&proposal)
            .await
            .expect("persist proposal");
        registry
            .mark_proposal_prepared(&proposal)
            .await
            .expect("mark prepared");
        registry
            .mark_proposal_canonical_with_certificate(
                &proposal,
                &sample_commit_certificate(&proposal),
            )
            .await
            .expect("mark canonicalized");
        registry
            .mark_proposal_authorized(&proposal)
            .await
            .expect("mark authorized");
        registry
            .mark_proposal_mempool_accepted(&proposal)
            .await
            .expect("mark mempool accepted");

        let confirmed = observe_confirmed_withdrawal(
            registry.as_ref(),
            &proposal.id,
            999,
            Tip5Hash([Belt(901), Belt(902), Belt(903), Belt(904), Belt(905)]),
        )
        .await
        .expect("observe confirmed");
        assert!(confirmed);
        assert!(
            registry
                .fetch_live_withdrawal(&proposal.id)
                .await
                .expect("fetch live withdrawal")
                .is_none(),
            "confirmed withdrawals should be absent from the active tracked view"
        );
        assert!(
            registry
                .fetch_live_withdrawal(&proposal.id)
                .await
                .expect("fetch live withdrawal after confirmation")
                .is_none(),
            "confirmed withdrawals should no longer be exposed as live operator attempts"
        );
    }

    #[test]
    fn fixture_transaction_id_matches_recomputed_raw_tx_id() {
        let transaction = sample_transaction();
        let raw_tx = raw_tx_from_transaction(&transaction).expect("raw tx from transaction");
        let tx_id = transaction_id_base58(&transaction).expect("recomputed submitted tx id");
        let stable_name = match &transaction {
            nockchain_types::v1::Transaction::V1(tx) => tx.name.clone(),
        };

        assert_eq!(raw_tx.id.to_base58(), tx_id);
        assert_ne!(
            stable_name, tx_id,
            "submitted raw tx id must be recomputed from finalized spends rather than reusing the stable transaction name"
        );
        assert_eq!(
            raw_tx.version,
            nockchain_types::tx_engine::common::Version::V1
        );
        assert_eq!(
            stable_name,
            match &transaction {
                nockchain_types::v1::Transaction::V1(tx) => tx.name.clone(),
            }
        );
    }

    #[test]
    fn raw_tx_id_base58_reads_stored_raw_tx_id() {
        let transaction = sample_transaction();
        let mut raw_tx = raw_tx_from_transaction(&transaction).expect("raw tx from transaction");
        let computed_id = raw_tx.compute_id().expect("raw tx id should compute");

        assert_eq!(raw_tx.id, computed_id);
        raw_tx.id = Tip5Hash::from_limbs(&[1, 2, 3, 4, 5]);
        assert_ne!(raw_tx.id, computed_id);
        assert_eq!(
            withdrawal_raw_tx::raw_tx_id_base58(&raw_tx),
            raw_tx.id.to_base58()
        );
    }

    #[test]
    fn raw_tx_from_transaction_applies_witness_data_to_spends() {
        let transaction = sample_transaction();
        let (target_name, target_witness) = {
            let nockchain_types::v1::Transaction::V1(tx) = &transaction;
            let nockchain_types::v1::WitnessData::Witnesses(witness_map) = &tx.witness_data else {
                panic!("fixture transaction must use witness data");
            };

            witness_map
                .0
                .iter()
                .find_map(|(witness_name, witness)| {
                    let spend_witness = match &tx
                        .spends
                        .0
                        .iter()
                        .find(|(spend_name, _)| spend_name == witness_name)?
                        .1
                    {
                        nockchain_types::v1::Spend::Witness(spend) => &spend.witness,
                        _ => return None,
                    };
                    (spend_witness != witness).then(|| (witness_name.clone(), witness.clone()))
                })
                .expect("fixture transaction should contain a spend whose witness data differs from the spend")
        };

        let raw_tx = raw_tx_from_transaction(&transaction).expect("raw tx from transaction");
        let applied_spend_witness = match &raw_tx
            .spends
            .0
            .iter()
            .find(|(spend_name, _)| *spend_name == target_name)
            .expect("matching raw spend")
            .1
        {
            nockchain_types::v1::Spend::Witness(spend) => &spend.witness,
            _ => panic!("fixture raw tx must use witness spends"),
        };
        assert_eq!(*applied_spend_witness, target_witness);
    }

    #[tokio::test]
    async fn load_fully_signed_proposal_stops_after_threshold_minus_one_contributions() {
        let (registry, _withdrawal_state_store, _dir) = open_context().await;
        let request = sample_request();
        let (proposal, signer_pkhs) = sample_multisig_proposal(0);
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        registry
            .validate_and_cache_prepared(&proposal)
            .await
            .expect("persist proposal");
        registry
            .mark_proposal_prepared(&proposal)
            .await
            .expect("mark prepared");
        registry
            .mark_proposal_canonical_with_certificate(
                &proposal,
                &sample_commit_certificate(&proposal),
            )
            .await
            .expect("mark canonicalized");

        for (signer_node_id, signer_hash) in [(0_u64, 0_usize), (1, 1), (3, 3), (4, 4)] {
            registry
                .record_proposal_signed(
                    &signed_contribution(
                        &proposal,
                        &signer_pkhs,
                        &[signer_pkhs[2].clone(), signer_pkhs[signer_hash].clone()],
                    ),
                    signer_node_id,
                )
                .await
                .expect("record signed contribution");
        }

        let row = registry
            .fetch_live_withdrawal(&proposal.id)
            .await
            .expect("fetch live row")
            .expect("peer-canonical live row");
        let Some(reconstructed) = load_fully_signed_proposal(registry.as_ref(), &row)
            .await
            .expect("reconstruct fully signed proposal")
        else {
            panic!("multisig proposal should reconstruct");
        };
        assert!(
            transaction_is_fully_signed(&reconstructed.transaction),
            "reconstructed transaction should be exactly threshold-complete"
        );
        assert_eq!(
            first_witness_pkhs(&reconstructed.transaction),
            vec![
                signer_pkhs[2].clone(),
                signer_pkhs[0].clone(),
                signer_pkhs[1].clone(),
            ],
            "reconstruction should stop after the first two stored contributions in deterministic order"
        );
    }

    #[tokio::test]
    async fn load_fully_signed_proposal_returns_none_before_threshold_is_met() {
        let (registry, _withdrawal_state_store, _dir) = open_context().await;
        let request = sample_request();
        let (proposal, signer_pkhs) = sample_multisig_proposal(0);
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        registry
            .validate_and_cache_prepared(&proposal)
            .await
            .expect("persist proposal");
        registry
            .mark_proposal_prepared(&proposal)
            .await
            .expect("mark prepared");
        registry
            .mark_proposal_canonical_with_certificate(
                &proposal,
                &sample_commit_certificate(&proposal),
            )
            .await
            .expect("mark canonicalized");
        registry
            .record_proposal_signed(
                &signed_contribution(
                    &proposal,
                    &signer_pkhs,
                    &[signer_pkhs[2].clone(), signer_pkhs[0].clone()],
                ),
                0,
            )
            .await
            .expect("record one signed contribution");

        let row = registry
            .fetch_live_withdrawal(&proposal.id)
            .await
            .expect("fetch live row")
            .expect("peer-canonical live row");

        assert!(
            load_fully_signed_proposal(registry.as_ref(), &row)
                .await
                .expect("reconstruct proposal")
                .is_none(),
            "base signer plus one stored contribution should still be below the 3-of-5 threshold"
        );
    }

    #[test]
    fn transaction_is_fully_signed_rejects_over_signed_witnesses() {
        let mut transaction = sample_transaction();
        assert!(transaction_is_fully_signed(&transaction));

        let nockchain_types::v1::Transaction::V1(transaction_v1) = &mut transaction;
        let nockchain_types::v1::WitnessData::Witnesses(witness_map) =
            &mut transaction_v1.witness_data
        else {
            panic!("fixture transaction must use witness data");
        };

        let witness = witness_map
            .0
            .iter_mut()
            .find_map(|(_, witness)| (!witness.pkh_signature.0.is_empty()).then_some(witness))
            .expect("fixture transaction should contain signer entries");
        let extra = witness
            .pkh_signature
            .0
            .first()
            .cloned()
            .expect("fixture witness entry");
        witness.pkh_signature.0.push(extra);

        assert!(!transaction_is_fully_signed(&transaction));
    }

    #[tokio::test]
    async fn recording_submitter_resubmit_raw_tx_matches_submit_withdrawal_raw_tx() {
        let submitter = RecordingSubmitter::default();
        let proposal = sample_proposal(0);
        let raw_tx = raw_tx_from_transaction(&proposal.transaction).expect("proposal raw tx");
        let submitted_raw_tx_id =
            transaction_id_base58(&proposal.transaction).expect("proposal tx id");

        submitter
            .submit_withdrawal(&proposal)
            .await
            .expect("submit withdrawal");
        submitter
            .resubmit_raw_tx(&raw_tx)
            .await
            .expect("resubmit raw tx");

        let resubmitted = submitter
            .resubmitted_raw_txs
            .lock()
            .expect("resubmitted raw txs lock")
            .last()
            .cloned()
            .expect("recorded resubmitted raw tx");
        let resubmitted_raw_tx_id = submitter
            .resubmitted_raw_tx_ids
            .lock()
            .expect("resubmitted raw tx ids lock")
            .last()
            .cloned()
            .expect("recorded resubmitted raw tx id");
        assert_eq!(raw_tx, resubmitted);
        assert_eq!(submitted_raw_tx_id, resubmitted_raw_tx_id);
        assert_eq!(
            submitted_raw_tx_id,
            withdrawal_raw_tx::raw_tx_id_base58(&resubmitted)
        );
    }

    #[tokio::test]
    async fn sequencer_confirmation_tick_marks_mempool_accepted_withdrawal_confirmed() {
        let (_validator, withdrawal_state_store, _dir) = open_context().await;
        let proposal = sample_proposal(0);
        seed_sequenced_submit_state(
            withdrawal_state_store.as_ref(),
            &proposal,
            WithdrawalState::MempoolAccepted,
            111,
            None,
        )
        .await;

        let submitter = RecordingSubmitter::default();
        submitter.set_included_block(
            transaction_id_base58(&proposal.transaction).expect("tx id"),
            sample_included_block(7_654),
        );
        submitter.set_nockchain_tip_height(7_664);

        let observed = withdrawal_sequencer_confirmation_tick_once(
            withdrawal_state_store.as_ref(),
            &submitter,
            10,
        )
        .await
        .expect("sequencer confirmation tick");
        assert_eq!(observed, 1);

        let confirmed = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch confirmed withdrawal")
            .expect("confirmed withdrawal row");
        assert_eq!(confirmed.state, WithdrawalState::Confirmed);
        assert!(
            withdrawal_state_store
                .list_reserved_input_names()
                .await
                .expect("list reserved inputs after confirmation tick")
                .is_empty(),
            "confirmation tick should release reserved inputs"
        );
    }

    #[tokio::test]
    async fn sequencer_confirmation_tick_leaves_non_included_mempool_accepted_withdrawal() {
        let (_validator, withdrawal_state_store, _dir) = open_context().await;
        let proposal = sample_proposal(0);
        seed_sequenced_submit_state(
            withdrawal_state_store.as_ref(),
            &proposal,
            WithdrawalState::MempoolAccepted,
            111,
            None,
        )
        .await;

        let submitter = RecordingSubmitter::default();
        submitter.set_nockchain_tip_height(10_000);

        let observed = withdrawal_sequencer_confirmation_tick_once(
            withdrawal_state_store.as_ref(),
            &submitter,
            10,
        )
        .await
        .expect("sequencer confirmation tick");
        assert_eq!(observed, 0);

        let row = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch withdrawal")
            .expect("withdrawal row");
        assert_eq!(row.state, WithdrawalState::MempoolAccepted);
    }

    #[tokio::test]
    async fn sequencer_confirmation_tick_waits_for_nockchain_depth() {
        let (_validator, withdrawal_state_store, _dir) = open_context().await;
        let proposal = sample_proposal(0);
        seed_sequenced_submit_state(
            withdrawal_state_store.as_ref(),
            &proposal,
            WithdrawalState::MempoolAccepted,
            111,
            None,
        )
        .await;

        let submitter = RecordingSubmitter::default();
        submitter.set_included_block(
            transaction_id_base58(&proposal.transaction).expect("tx id"),
            sample_included_block(100),
        );
        submitter.set_nockchain_tip_height(102);

        let observed = withdrawal_sequencer_confirmation_tick_once(
            withdrawal_state_store.as_ref(),
            &submitter,
            3,
        )
        .await
        .expect("sequencer confirmation tick");
        assert_eq!(observed, 0);

        let row = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch withdrawal")
            .expect("withdrawal row");
        assert_eq!(row.state, WithdrawalState::MempoolAccepted);
    }

    #[tokio::test]
    async fn sequencer_confirmation_tick_depth_zero_confirms_nonzero_inclusion() {
        let (_validator, withdrawal_state_store, _dir) = open_context().await;
        let proposal = sample_proposal(0);
        seed_sequenced_submit_state(
            withdrawal_state_store.as_ref(),
            &proposal,
            WithdrawalState::MempoolAccepted,
            111,
            None,
        )
        .await;

        let submitter = RecordingSubmitter::default();
        submitter.set_included_block(
            transaction_id_base58(&proposal.transaction).expect("tx id"),
            sample_included_block(100),
        );
        submitter.set_nockchain_tip_height(100);

        let observed = withdrawal_sequencer_confirmation_tick_once(
            withdrawal_state_store.as_ref(),
            &submitter,
            0,
        )
        .await
        .expect("sequencer confirmation tick");
        assert_eq!(observed, 1);
    }

    #[tokio::test]
    async fn sequencer_confirmation_tick_tip_zero_confirms_nothing() {
        let (_validator, withdrawal_state_store, _dir) = open_context().await;
        let proposal = sample_proposal(0);
        seed_sequenced_submit_state(
            withdrawal_state_store.as_ref(),
            &proposal,
            WithdrawalState::MempoolAccepted,
            111,
            None,
        )
        .await;

        let submitter = RecordingSubmitter::default();
        submitter.set_included_block(
            transaction_id_base58(&proposal.transaction).expect("tx id"),
            sample_included_block(1),
        );
        submitter.set_nockchain_tip_height(0);

        let observed = withdrawal_sequencer_confirmation_tick_once(
            withdrawal_state_store.as_ref(),
            &submitter,
            0,
        )
        .await
        .expect("sequencer confirmation tick");
        assert_eq!(observed, 0);
    }

    #[tokio::test]
    async fn sequencer_confirmation_tick_ignores_authorized_withdrawal() {
        let (_validator, withdrawal_state_store, _dir) = open_context().await;
        let authorized = sample_proposal(0);
        register_proposal_ordering(&withdrawal_state_store, &authorized, 1).await;
        withdrawal_state_store
            .record_proposal_canonicalized_with_certificate(
                &authorized,
                Some(&sample_commit_certificate(&authorized)),
                100,
            )
            .await
            .expect("record authorized canonicalized");
        withdrawal_state_store
            .record_proposal_authorized(&authorized)
            .await
            .expect("record authorized proposal");

        let submitter = RecordingSubmitter::default();
        submitter.set_included_block(
            transaction_id_base58(&authorized.transaction).expect("tx id"),
            sample_included_block(100),
        );
        submitter.set_nockchain_tip_height(200);

        let observed = withdrawal_sequencer_confirmation_tick_once(
            withdrawal_state_store.as_ref(),
            &submitter,
            10,
        )
        .await
        .expect("sequencer confirmation tick");
        assert_eq!(observed, 0);
        assert_eq!(
            withdrawal_state_store
                .fetch_sequenced_withdrawal(&authorized.id)
                .await
                .expect("fetch authorized")
                .expect("authorized row")
                .state,
            WithdrawalState::Authorized
        );
    }

    #[tokio::test]
    async fn sequencer_orphan_retry_tick_resubmits_stale_mempool_accepted_withdrawal() {
        let (_validator, withdrawal_state_store, _dir) = open_context().await;
        let proposal = sample_proposal(0);
        register_proposal_ordering(&withdrawal_state_store, &proposal, 1).await;
        withdrawal_state_store
            .record_proposal_canonicalized_with_certificate(
                &proposal,
                Some(&sample_commit_certificate(&proposal)),
                100,
            )
            .await
            .expect("record canonicalized");
        withdrawal_state_store
            .record_proposal_authorized(&proposal)
            .await
            .expect("record authorized");
        withdrawal_state_store
            .record_submit_outcome(&proposal, WithdrawalState::MempoolAccepted, 1, 100, None)
            .await
            .expect("record mempool accepted");

        let submitter = RecordingSubmitter::default();
        submitter.script_resubmit_results([Ok(WithdrawalNetworkSubmitStatus::MempoolAccepted)]);
        let tracker = SequencerBaseHeightTracker::default();
        tracker.record_confirmed_base_height(115);

        let retried = withdrawal_sequencer_orphan_retry_tick_once(
            withdrawal_state_store.as_ref(),
            &submitter,
            &tracker,
            10,
        )
        .await
        .expect("orphan retry tick");
        assert_eq!(retried, 1);
        assert_eq!(
            submitter
                .resubmitted_raw_txs
                .lock()
                .expect("resubmitted raw txs lock")
                .len(),
            1
        );
        let resubmitted = submitter
            .resubmitted_raw_txs
            .lock()
            .expect("resubmitted raw txs lock")
            .last()
            .cloned()
            .expect("recorded resubmitted raw tx");
        assert_eq!(
            raw_tx_from_transaction(&proposal.transaction).expect("proposal raw tx"),
            resubmitted
        );
        assert_eq!(
            transaction_id_base58(&proposal.transaction).expect("proposal tx id"),
            submitter
                .resubmitted_raw_tx_ids
                .lock()
                .expect("resubmitted raw tx ids lock")
                .last()
                .cloned()
                .expect("recorded resubmitted raw tx id")
        );

        let row = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch retried withdrawal")
            .expect("retried withdrawal row");
        let proposal_hash = proposal.proposal_hash().expect("proposal hash");
        let submitted_raw_tx_id = transaction_id_base58(&proposal.transaction).expect("tx id");
        assert_eq!(row.state, WithdrawalState::MempoolAccepted);
        assert_eq!(row.submit_attempt_count, 2);
        assert_eq!(row.last_submit_attempt_base_height, Some(115));
        assert_eq!(row.last_submit_error, None);
        assert_eq!(row.proposal_hash.as_deref(), Some(proposal_hash.as_str()));
        assert_eq!(
            row.authorized_transaction_name.as_deref(),
            Some(submitted_raw_tx_id.as_str())
        );
        assert_eq!(
            withdrawal_state_store
                .list_reserved_input_names()
                .await
                .expect("reserved inputs after orphan retry"),
            proposal.selected_inputs
        );
    }

    #[tokio::test]
    async fn sequencer_orphan_retry_tick_skips_currently_included_withdrawal() {
        let (_validator, withdrawal_state_store, _dir) = open_context().await;
        let proposal = sample_proposal(0);
        register_proposal_ordering(&withdrawal_state_store, &proposal, 1).await;
        withdrawal_state_store
            .record_proposal_canonicalized_with_certificate(
                &proposal,
                Some(&sample_commit_certificate(&proposal)),
                100,
            )
            .await
            .expect("record canonicalized");
        withdrawal_state_store
            .record_proposal_authorized(&proposal)
            .await
            .expect("record authorized");
        withdrawal_state_store
            .record_submit_outcome(&proposal, WithdrawalState::MempoolAccepted, 1, 100, None)
            .await
            .expect("record mempool accepted");

        let submitter = RecordingSubmitter::default();
        submitter.set_included_block(
            transaction_id_base58(&proposal.transaction).expect("tx id"),
            sample_included_block(110),
        );
        submitter.script_resubmit_results([Ok(WithdrawalNetworkSubmitStatus::MempoolAccepted)]);
        let tracker = SequencerBaseHeightTracker::default();
        tracker.record_confirmed_base_height(115);

        let retried = withdrawal_sequencer_orphan_retry_tick_once(
            withdrawal_state_store.as_ref(),
            &submitter,
            &tracker,
            10,
        )
        .await
        .expect("orphan retry tick");
        assert_eq!(retried, 0);
        assert!(submitter
            .resubmitted_raw_txs
            .lock()
            .expect("resubmitted raw txs lock")
            .is_empty());

        let row = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch included withdrawal")
            .expect("included withdrawal row");
        assert_eq!(row.state, WithdrawalState::MempoolAccepted);
        assert_eq!(row.submit_attempt_count, 1);
        assert_eq!(row.last_submit_attempt_base_height, Some(100));
    }

    #[tokio::test]
    async fn sequencer_orphan_retry_tick_skips_fresh_mempool_accepted_withdrawal() {
        let (_validator, withdrawal_state_store, _dir) = open_context().await;
        let proposal = sample_proposal(0);
        register_proposal_ordering(&withdrawal_state_store, &proposal, 1).await;
        withdrawal_state_store
            .record_proposal_canonicalized_with_certificate(
                &proposal,
                Some(&sample_commit_certificate(&proposal)),
                100,
            )
            .await
            .expect("record canonicalized");
        withdrawal_state_store
            .record_proposal_authorized(&proposal)
            .await
            .expect("record authorized");
        withdrawal_state_store
            .record_submit_outcome(&proposal, WithdrawalState::MempoolAccepted, 1, 100, None)
            .await
            .expect("record mempool accepted");

        let submitter = RecordingSubmitter::default();
        let tracker = SequencerBaseHeightTracker::default();
        tracker.record_confirmed_base_height(105);

        let retried = withdrawal_sequencer_orphan_retry_tick_once(
            withdrawal_state_store.as_ref(),
            &submitter,
            &tracker,
            10,
        )
        .await
        .expect("orphan retry tick");
        assert_eq!(retried, 0);
        assert!(submitter
            .resubmitted_raw_txs
            .lock()
            .expect("resubmitted raw txs lock")
            .is_empty());

        let row = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch fresh mempool accepted withdrawal")
            .expect("fresh mempool accepted withdrawal row");
        assert_eq!(row.submit_attempt_count, 1);
        assert_eq!(row.last_submit_attempt_base_height, Some(100));
    }

    #[tokio::test]
    async fn sequencer_orphan_retry_tick_ignores_confirmed_withdrawal() {
        let (_validator, withdrawal_state_store, _dir) = open_context().await;
        let proposal = sample_proposal(0);
        seed_confirmed_sequenced_withdrawal(withdrawal_state_store.as_ref(), &proposal, 100).await;

        let submitter = RecordingSubmitter::default();
        let tracker = SequencerBaseHeightTracker::default();
        tracker.record_confirmed_base_height(200);

        let retried = withdrawal_sequencer_orphan_retry_tick_once(
            withdrawal_state_store.as_ref(),
            &submitter,
            &tracker,
            10,
        )
        .await
        .expect("orphan retry tick");
        assert_eq!(retried, 0);
        assert!(submitter
            .resubmitted_raw_txs
            .lock()
            .expect("resubmitted raw txs lock")
            .is_empty());

        let row = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch confirmed withdrawal")
            .expect("confirmed withdrawal row");
        assert_eq!(row.state, WithdrawalState::Confirmed);
        assert_eq!(row.submit_attempt_count, 1);
        assert_eq!(row.last_submit_attempt_base_height, Some(100));
    }

    #[tokio::test]
    async fn sequencer_orphan_retry_tick_skips_mempool_accepted_withdrawal_missing_last_submit_attempt_base_height(
    ) {
        let (_validator, withdrawal_state_store, dir) = open_context().await;
        let proposal = sample_proposal(0);
        seed_sequenced_submit_state(
            withdrawal_state_store.as_ref(),
            &proposal,
            WithdrawalState::MempoolAccepted,
            100,
            None,
        )
        .await;
        clear_last_submit_attempt_base_height(&dir, &proposal.id);

        let submitter = RecordingSubmitter::default();
        let tracker = SequencerBaseHeightTracker::default();
        tracker.record_confirmed_base_height(200);

        let retried = withdrawal_sequencer_orphan_retry_tick_once(
            withdrawal_state_store.as_ref(),
            &submitter,
            &tracker,
            10,
        )
        .await
        .expect("orphan retry tick");
        assert_eq!(retried, 0);
        assert!(submitter
            .resubmitted_raw_txs
            .lock()
            .expect("resubmitted raw txs lock")
            .is_empty());

        let row = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch mempool-accepted withdrawal")
            .expect("mempool-accepted withdrawal row");
        assert_eq!(row.state, WithdrawalState::MempoolAccepted);
        assert_eq!(row.submit_attempt_count, 1);
        assert_eq!(row.last_submit_attempt_base_height, None);
        assert_eq!(row.last_submit_error, None);
    }

    #[tokio::test]
    async fn sequencer_orphan_retry_tick_short_circuits_when_tx_is_already_in_mempool() {
        let (_validator, withdrawal_state_store, _dir) = open_context().await;
        let proposal = sample_proposal(0);
        register_proposal_ordering(&withdrawal_state_store, &proposal, 1).await;
        withdrawal_state_store
            .record_proposal_canonicalized_with_certificate(
                &proposal,
                Some(&sample_commit_certificate(&proposal)),
                100,
            )
            .await
            .expect("record canonicalized");
        withdrawal_state_store
            .record_proposal_authorized(&proposal)
            .await
            .expect("record authorized");
        withdrawal_state_store
            .record_submit_outcome(&proposal, WithdrawalState::MempoolAccepted, 1, 100, None)
            .await
            .expect("record mempool accepted");

        let submitter = RecordingSubmitter::default();
        submitter
            .script_resubmit_results([Ok(WithdrawalNetworkSubmitStatus::AlreadyMempoolAccepted)]);
        let tracker = SequencerBaseHeightTracker::default();
        tracker.record_confirmed_base_height(118);

        let retried = withdrawal_sequencer_orphan_retry_tick_once(
            withdrawal_state_store.as_ref(),
            &submitter,
            &tracker,
            10,
        )
        .await
        .expect("orphan retry tick");
        assert_eq!(retried, 1);
        assert_eq!(
            submitter
                .resubmitted_raw_txs
                .lock()
                .expect("resubmitted raw txs lock")
                .len(),
            1
        );

        let row = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch short-circuited retry withdrawal")
            .expect("short-circuited retry withdrawal row");
        assert_eq!(row.state, WithdrawalState::MempoolAccepted);
        assert_eq!(row.submit_attempt_count, 2);
        assert_eq!(row.last_submit_attempt_base_height, Some(118));
        assert_eq!(row.last_submit_error, None);
    }

    #[tokio::test]
    async fn sequencer_orphan_retry_tick_records_retry_exhausted_and_keeps_mempool_accepted() {
        let (_validator, withdrawal_state_store, _dir) = open_context().await;
        let proposal = sample_proposal(0);
        register_proposal_ordering(&withdrawal_state_store, &proposal, 1).await;
        withdrawal_state_store
            .record_proposal_canonicalized_with_certificate(
                &proposal,
                Some(&sample_commit_certificate(&proposal)),
                100,
            )
            .await
            .expect("record canonicalized");
        withdrawal_state_store
            .record_proposal_authorized(&proposal)
            .await
            .expect("record authorized");
        withdrawal_state_store
            .record_submit_outcome(&proposal, WithdrawalState::MempoolAccepted, 1, 100, None)
            .await
            .expect("record mempool accepted");

        let submitter = RecordingSubmitter::default();
        submitter.script_resubmit_results([Ok(WithdrawalNetworkSubmitStatus::RetryExhausted)]);
        let tracker = SequencerBaseHeightTracker::default();
        tracker.record_confirmed_base_height(120);

        let retried = withdrawal_sequencer_orphan_retry_tick_once(
            withdrawal_state_store.as_ref(),
            &submitter,
            &tracker,
            10,
        )
        .await
        .expect("orphan retry tick");
        assert_eq!(retried, 1);

        let row = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch stalled retry withdrawal")
            .expect("stalled retry withdrawal row");
        assert_eq!(row.state, WithdrawalState::MempoolAccepted);
        assert_eq!(row.submit_attempt_count, 2);
        assert_eq!(row.last_submit_attempt_base_height, Some(120));
        assert_eq!(
            row.last_submit_error.as_deref(),
            Some(
                "transaction was not reported mempool-accepted before the retry budget was exhausted"
            )
        );
    }

    #[tokio::test]
    async fn sequencer_orphan_retry_tick_records_submit_error_and_keeps_mempool_accepted() {
        let (_validator, withdrawal_state_store, _dir) = open_context().await;
        let proposal = sample_proposal(0);
        register_proposal_ordering(&withdrawal_state_store, &proposal, 1).await;
        withdrawal_state_store
            .record_proposal_canonicalized_with_certificate(
                &proposal,
                Some(&sample_commit_certificate(&proposal)),
                100,
            )
            .await
            .expect("record canonicalized");
        withdrawal_state_store
            .record_proposal_authorized(&proposal)
            .await
            .expect("record authorized");
        withdrawal_state_store
            .record_submit_outcome(&proposal, WithdrawalState::MempoolAccepted, 1, 100, None)
            .await
            .expect("record mempool accepted");

        let submitter = RecordingSubmitter::default();
        submitter.script_resubmit_results([Err("nockchain retry failed".to_string())]);
        let tracker = SequencerBaseHeightTracker::default();
        tracker.record_confirmed_base_height(120);

        let retried = withdrawal_sequencer_orphan_retry_tick_once(
            withdrawal_state_store.as_ref(),
            &submitter,
            &tracker,
            10,
        )
        .await
        .expect("orphan retry tick");
        assert_eq!(retried, 1);

        let row = withdrawal_state_store
            .fetch_sequenced_withdrawal(&proposal.id)
            .await
            .expect("fetch failed retry withdrawal")
            .expect("failed retry withdrawal row");
        assert_eq!(row.state, WithdrawalState::MempoolAccepted);
        assert_eq!(row.submit_attempt_count, 2);
        assert_eq!(row.last_submit_attempt_base_height, Some(120));
        assert_eq!(
            row.last_submit_error.as_deref(),
            Some("Bridge runtime error: nockchain retry failed")
        );
        assert_eq!(
            withdrawal_state_store
                .list_reserved_input_names()
                .await
                .expect("reserved inputs after failed retry"),
            proposal.selected_inputs
        );
    }
}
