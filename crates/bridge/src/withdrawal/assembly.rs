use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use alloy::primitives::Address;
use async_trait::async_trait;
use nockapp::driver::IODriverFn;
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::one_punch::OnePunchWire;
use nockapp::wire::Wire;
use nockapp::NounAllocator;
use noun_serde::{NounDecode, NounEncode};
use prost::Message;
use tokio::time::{interval, MissedTickBehavior};
use tracing::{debug, error, info, warn};
use wallet_tx_builder::lock_resolver::LockRootLockMatcher;
use wallet_tx_builder::planner::{plan_withdrawal_tx, PlanError};
use wallet_tx_builder::types::{
    ChainContext, RawNoteDataEntry, RefundOutputTemplate, WithdrawalPlanRequest,
};

#[cfg(test)]
use crate::core::withdrawal::assembly::scheduled_assembler_node_id;
use crate::core::withdrawal::assembly::scheduled_assembler_turn_node_id;
use crate::core::withdrawal::signing::{
    plan_signing_sequencer_status, WithdrawalSigningSequencerDecision,
};
use crate::observability::health::PeerEndpoint;
use crate::observability::metrics;
use crate::observability::status::BridgeStatus;
use crate::shared::config::WithdrawalActivationCutoff;
use crate::shared::errors::BridgeError;
use crate::shared::ingress::proto::{
    SequencedWithdrawalStatusResponse, WithdrawalCommitCertificate,
};
use crate::shared::kernel_projection::{
    plan_kernel_projection_boot, KernelProjectionBootPlan, KernelProjectionCursor,
    KernelProjectionPosition,
};
use crate::shared::runtime::{BridgePoke, BridgeRuntimeHandle};
use crate::shared::stop::{trigger_local_stop, StopController, StopHandle};
use crate::shared::types::{
    BaseBlockCommitAck, BridgeCause, BridgeCauseVariant, BridgeEffect, BridgeEffectVariant,
    PendingBaseBlockCommit, Tip5Hash,
};
use crate::withdrawal::proposals::{
    reconstruct_withdrawal_proposal, TrackedWithdrawalRequest, WithdrawalProposalRegistry,
    WithdrawalProposalValidationOutcome,
};
use crate::withdrawal::snapshot::BridgeNoteSnapshotService;
use crate::withdrawal::state::{
    AcquireWithdrawalAssemblyOutcome, LiveWithdrawalView, WithdrawalFallbackPolicy, WithdrawalState,
};
#[cfg(test)]
use crate::withdrawal::submission::WithdrawalSequencerSubmitOutcome;
use crate::withdrawal::submission::{
    register_withdrawal_or_alert, sequenced_withdrawal_released, WithdrawalSequencerPort,
};
use crate::withdrawal::transport::{
    required_withdrawal_commit_signature_threshold, signed_proposal_matches_base,
    verify_withdrawal_commit_certificate, WithdrawalProposalTransport,
};
use crate::withdrawal::types::{
    CreateWithdrawalTxData, NockWithdrawalRequestKernelData, SelectedWithdrawalNoteData,
    WithdrawalId, WithdrawalProposalData, WithdrawalSnapshot,
};

const WITHDRAWAL_PROJECTION_REPLAY_OVERLAP: u64 = 1;

#[async_trait]
pub trait WithdrawalKernelPort: Send + Sync {
    /// Requests that the kernel assemble a withdrawal transaction for the given
    /// staged withdrawal attempt.
    async fn poke_create_withdrawal_tx(
        &self,
        request: CreateWithdrawalTxData,
    ) -> Result<(), BridgeError>;

    /// Requests that the kernel add this node's signature contribution to a
    /// previously built withdrawal proposal.
    async fn poke_sign_tx(&self, proposal: WithdrawalProposalData) -> Result<(), BridgeError>;

    /// Reads the kernel's current Base hashchain next height so Rust can
    /// determine the latest processed Base batch end during boot restore.
    async fn peek_base_next_height(&self) -> Result<Option<u64>, BridgeError>;

    /// Reads the kernel's current Nock hashchain next height so projection
    /// cursors can verify the whole bridge kernel position.
    async fn peek_nock_next_height(&self) -> Result<Option<u64>, BridgeError>;

    /// Reads withdrawal burn requests from Base history with
    /// `base_batch_end >= start_height` so Rust can replay any restore gap.
    async fn peek_base_hashchain_withdrawals_since_height(
        &self,
        start_height: u64,
    ) -> Result<Vec<NockWithdrawalRequestKernelData>, BridgeError>;

    /// Reads a staged Base batch whose withdrawal requests must be durably
    /// persisted before the kernel advances the Base hashchain.
    async fn peek_pending_base_block_commit(
        &self,
    ) -> Result<Option<PendingBaseBlockCommit>, BridgeError>;

    /// Acknowledges that Rust has durably persisted the pending Base batch's
    /// withdrawal requests and the kernel may commit the Base hashchain entry.
    async fn poke_base_block_withdrawals_committed(
        &self,
        ack: BaseBlockCommitAck,
    ) -> Result<(), BridgeError>;

    /// Explicitly repairs a stale staged Base batch so the Base watcher can
    /// reprocess it through the normal incoming Base path.
    async fn poke_repair_pending_base_block_commit(
        &self,
        ack: BaseBlockCommitAck,
    ) -> Result<(), BridgeError>;
}

#[async_trait]
impl WithdrawalKernelPort for BridgeRuntimeHandle {
    async fn poke_create_withdrawal_tx(
        &self,
        request: CreateWithdrawalTxData,
    ) -> Result<(), BridgeError> {
        let cause = BridgeCause(0, BridgeCauseVariant::CreateWithdrawalTx(request));
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = cause.to_noun(&mut slab);
        slab.set_root(noun);
        let wire = OnePunchWire::Poke.to_wire();
        self.send_poke(BridgePoke::new(wire, slab)).await
    }

    async fn poke_sign_tx(&self, proposal: WithdrawalProposalData) -> Result<(), BridgeError> {
        let cause = BridgeCause(0, BridgeCauseVariant::SignTx(proposal));
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = cause.to_noun(&mut slab);
        slab.set_root(noun);
        let wire = OnePunchWire::Poke.to_wire();
        self.send_poke(BridgePoke::new(wire, slab)).await
    }

    async fn peek_base_next_height(&self) -> Result<Option<u64>, BridgeError> {
        BridgeRuntimeHandle::peek_base_next_height(self).await
    }

    async fn peek_nock_next_height(&self) -> Result<Option<u64>, BridgeError> {
        BridgeRuntimeHandle::peek_nock_next_height(self).await
    }

    async fn peek_base_hashchain_withdrawals_since_height(
        &self,
        start_height: u64,
    ) -> Result<Vec<NockWithdrawalRequestKernelData>, BridgeError> {
        BridgeRuntimeHandle::peek_base_hashchain_withdrawals_since_height(self, start_height).await
    }

    async fn peek_pending_base_block_commit(
        &self,
    ) -> Result<Option<PendingBaseBlockCommit>, BridgeError> {
        BridgeRuntimeHandle::peek_pending_base_block_commit(self).await
    }

    async fn poke_base_block_withdrawals_committed(
        &self,
        ack: BaseBlockCommitAck,
    ) -> Result<(), BridgeError> {
        BridgeRuntimeHandle::send_base_block_withdrawals_committed(self, ack).await
    }

    async fn poke_repair_pending_base_block_commit(
        &self,
        ack: BaseBlockCommitAck,
    ) -> Result<(), BridgeError> {
        BridgeRuntimeHandle::send_repair_pending_base_block_commit(self, ack).await
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithdrawalAssemblyPlannerConfig {
    pub spend_authority_lock_root: Tip5Hash,
    pub spend_authority_spend_condition: nockchain_types::v1::SpendCondition,
    pub refund_lock_root: Tip5Hash,
    pub refund_note_data: Vec<RawNoteDataEntry>,
    pub nicks_fee_per_nock: u64,
    pub blockchain_constants: nockchain_types::BlockchainConstants,
    pub bythos_phase: u64,
    pub base_fee: u64,
    pub input_fee_divisor: u64,
    pub min_fee: u64,
}

impl WithdrawalAssemblyPlannerConfig {
    /// Builds the tx-planner chain context for the supplied snapshot height
    /// using the configured blockchain constants and fee parameters.
    fn chain_context(
        &self,
        height: nockchain_types::tx_engine::common::BlockHeight,
    ) -> ChainContext {
        ChainContext {
            height,
            bythos_phase: nockchain_types::tx_engine::common::BlockHeight(
                nockchain_math::belt::Belt(self.bythos_phase),
            ),
            base_fee: self.base_fee,
            input_fee_divisor: self.input_fee_divisor,
            min_fee: self.min_fee,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WithdrawalExecutionEffect {
    BaseBlockWithdrawalsPending,
    WithdrawalProposalBuilt,
    WithdrawalTxSigned,
}

/// Narrows the full kernel effect surface down to the withdrawal effects owned
/// by the Rust execution driver.
fn classify_withdrawal_execution_effect(
    variant: &BridgeEffectVariant,
) -> Option<WithdrawalExecutionEffect> {
    match variant {
        BridgeEffectVariant::BaseBlockWithdrawalsPending(_) => {
            Some(WithdrawalExecutionEffect::BaseBlockWithdrawalsPending)
        }
        BridgeEffectVariant::WithdrawalProposalBuilt(_) => {
            Some(WithdrawalExecutionEffect::WithdrawalProposalBuilt)
        }
        BridgeEffectVariant::WithdrawalTxSigned(_) => {
            Some(WithdrawalExecutionEffect::WithdrawalTxSigned)
        }
        _ => None,
    }
}

/// Orders withdrawal requests deterministically by the canonical Base request
/// ordering used for local nonce assignment.
fn sort_withdrawal_requests(requests: &mut [NockWithdrawalRequestKernelData]) {
    requests.sort_by(|left, right| {
        left.base_batch_end
            .cmp(&right.base_batch_end)
            .then_with(|| left.base_event_id.0.cmp(&right.base_event_id.0))
    });
}

/// Tracks newly emitted kernel withdrawal requests in the proposal registry's
/// durable request registry.
///
/// This persists request metadata in the withdrawal registry. Prepared proposals
/// built later are cached locally until canonical sequencer facts take over.
pub async fn persist_withdrawal_requests(
    mut requests: Vec<NockWithdrawalRequestKernelData>,
    proposal_registry: &WithdrawalProposalRegistry,
) -> Result<u64, BridgeError> {
    sort_withdrawal_requests(&mut requests);

    let mut tracked_ids = std::collections::HashSet::new();
    tracked_ids.extend(
        requests
            .iter()
            .map(NockWithdrawalRequestKernelData::withdrawal_id),
    );
    proposal_registry
        .track_withdrawal_requests(&requests)
        .await?;

    Ok(tracked_ids.len() as u64)
}

pub async fn persist_pending_base_block_withdrawals<K: WithdrawalKernelPort>(
    pending: PendingBaseBlockCommit,
    kernel: &K,
    proposal_registry: &WithdrawalProposalRegistry,
) -> Result<u64, BridgeError> {
    let ack = pending.ack();
    let tracked = persist_withdrawal_requests(pending.withdrawals, proposal_registry).await?;
    kernel.poke_base_block_withdrawals_committed(ack).await?;
    Ok(tracked)
}

pub async fn persist_pending_base_block_withdrawals_after_activation<K: WithdrawalKernelPort>(
    pending: PendingBaseBlockCommit,
    kernel: &K,
    proposal_registry: &WithdrawalProposalRegistry,
    activation_cutoff: WithdrawalActivationCutoff,
) -> Result<u64, BridgeError> {
    let ack = pending.ack();
    if proposal_registry
        .load_kernel_projection_cursor()
        .await?
        .is_none()
    {
        if proposal_registry.has_kernel_projection_rows().await? {
            return Err(BridgeError::Runtime(
                "kernel projection cursor is missing but kernel-derived projection rows exist"
                    .into(),
            ));
        }
        let current_position = peek_kernel_projection_position(kernel).await?;
        if !withdrawal_activation_reached(&current_position, activation_cutoff) {
            kernel.poke_base_block_withdrawals_committed(ack).await?;
            return Ok(0);
        }
        restore_tracked_withdrawal_requests(kernel, proposal_registry, activation_cutoff).await?;
    }

    let tracked = persist_withdrawal_requests(pending.withdrawals, proposal_registry).await?;
    kernel.poke_base_block_withdrawals_committed(ack).await?;
    Ok(tracked)
}

/// Completes any Base batch that the kernel staged before a crash/restart.
pub async fn recover_pending_base_block_commit<K: WithdrawalKernelPort>(
    kernel: &K,
    proposal_registry: &WithdrawalProposalRegistry,
) -> Result<u64, BridgeError> {
    let Some(pending) = kernel.peek_pending_base_block_commit().await? else {
        return Ok(0);
    };
    persist_pending_base_block_withdrawals(pending, kernel, proposal_registry).await
}

pub async fn recover_pending_base_block_commit_after_activation<K: WithdrawalKernelPort>(
    kernel: &K,
    proposal_registry: &WithdrawalProposalRegistry,
    activation_cutoff: WithdrawalActivationCutoff,
) -> Result<u64, BridgeError> {
    let Some(pending) = kernel.peek_pending_base_block_commit().await? else {
        return Ok(0);
    };
    persist_pending_base_block_withdrawals_after_activation(
        pending, kernel, proposal_registry, activation_cutoff,
    )
    .await
}

pub async fn repair_pending_base_block_commit<K: WithdrawalKernelPort>(
    kernel: &K,
) -> Result<bool, BridgeError> {
    let Some(pending) = kernel.peek_pending_base_block_commit().await? else {
        return Ok(false);
    };
    kernel
        .poke_repair_pending_base_block_commit(pending.ack())
        .await?;
    Ok(true)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WithdrawalActivationSnapshot {
    pub(crate) current_position: KernelProjectionPosition,
    existing_cursor: Option<KernelProjectionCursor>,
    has_kernel_projection_rows: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WithdrawalActivationReadiness {
    Ready(WithdrawalActivationSnapshot),
    Waiting(WithdrawalActivationSnapshot),
}

pub(crate) async fn withdrawal_activation_readiness<K: WithdrawalKernelPort>(
    kernel: &K,
    proposal_registry: &WithdrawalProposalRegistry,
    activation_cutoff: WithdrawalActivationCutoff,
) -> Result<WithdrawalActivationReadiness, BridgeError> {
    let current_position = peek_kernel_projection_position(kernel).await?;
    let existing_cursor = proposal_registry.load_kernel_projection_cursor().await?;
    let has_kernel_projection_rows = proposal_registry.has_kernel_projection_rows().await?;
    let snapshot = WithdrawalActivationSnapshot {
        current_position,
        existing_cursor,
        has_kernel_projection_rows,
    };
    if snapshot.existing_cursor.is_none()
        && !snapshot.has_kernel_projection_rows
        && !withdrawal_activation_reached(&snapshot.current_position, activation_cutoff)
    {
        Ok(WithdrawalActivationReadiness::Waiting(snapshot))
    } else {
        Ok(WithdrawalActivationReadiness::Ready(snapshot))
    }
}

/// Rebuilds the tracked withdrawal-request set by replaying any missing Base
/// withdrawal burns from the kernel projection cursor, then reloading the
/// durable pre-confirmation view.
pub async fn restore_tracked_withdrawal_requests<K: WithdrawalKernelPort>(
    kernel: &K,
    proposal_registry: &WithdrawalProposalRegistry,
    activation_cutoff: WithdrawalActivationCutoff,
) -> Result<u64, BridgeError> {
    let snapshot = match withdrawal_activation_readiness(
        kernel, proposal_registry, activation_cutoff,
    )
    .await?
    {
        WithdrawalActivationReadiness::Ready(snapshot) => snapshot,
        WithdrawalActivationReadiness::Waiting(snapshot) => {
            let kernel_base_next_height = snapshot.current_position.base_next_height;
            let kernel_nock_next_height = snapshot.current_position.nock_next_height;
            debug!(
                target: "bridge.withdrawal",
                kernel_base_next_height,
                kernel_nock_next_height,
                activation_nock_next_height = activation_cutoff.nock_next_height,
                "waiting for withdrawal activation cutoff before initializing projection cursor"
            );
            return Ok(0);
        }
    };
    let current_position = snapshot.current_position;
    let existing_cursor = snapshot.existing_cursor;
    let has_rows = snapshot.has_kernel_projection_rows;

    let cursor = match plan_kernel_projection_boot(
        existing_cursor,
        has_rows,
        &current_position,
        current_position.clone(),
    )? {
        KernelProjectionBootPlan::UseExisting(cursor) => (cursor, false),
        KernelProjectionBootPlan::Initialize(cursor) => (cursor, true),
    };
    let (cursor, initialized) = cursor;

    if cursor.base_next_height == current_position.base_next_height
        && cursor.nock_next_height == current_position.nock_next_height
    {
        if initialized {
            proposal_registry
                .set_kernel_projection_cursor(cursor)
                .await?;
        }
        return proposal_registry
            .restore_tracked_withdrawal_requests()
            .await;
    }

    let replay_start = cursor
        .base_next_height
        .saturating_sub(WITHDRAWAL_PROJECTION_REPLAY_OVERLAP);
    let mut requests = kernel
        .peek_base_hashchain_withdrawals_since_height(replay_start)
        .await?;
    if let Some(request) = requests
        .iter()
        .find(|request| request.base_batch_end >= current_position.base_next_height)
    {
        return Err(BridgeError::Runtime(format!(
            "kernel returned withdrawal request beyond observed Base hashchain tip: request_base_batch_end={} kernel_base_next_height={}",
            request.base_batch_end, current_position.base_next_height
        )));
    }
    sort_withdrawal_requests(&mut requests);
    proposal_registry
        .replay_withdrawal_request_projection(
            requests,
            KernelProjectionCursor::from_position(current_position),
        )
        .await
}

fn withdrawal_activation_reached(
    current_position: &KernelProjectionPosition,
    activation_cutoff: WithdrawalActivationCutoff,
) -> bool {
    current_position.nock_next_height >= activation_cutoff.nock_next_height
}

async fn peek_kernel_projection_position<K: WithdrawalKernelPort>(
    kernel: &K,
) -> Result<KernelProjectionPosition, BridgeError> {
    let base_next_height = kernel.peek_base_next_height().await?.ok_or_else(|| {
        BridgeError::Runtime("kernel Base hashchain next height is unavailable".into())
    })?;
    let nock_next_height = kernel.peek_nock_next_height().await?.ok_or_else(|| {
        BridgeError::Runtime("kernel Nock hashchain next height is unavailable".into())
    })?;
    Ok(KernelProjectionPosition {
        base_next_height,
        base_tip_hash: None,
        nock_next_height,
        nock_tip_hash: None,
    })
}

/// Validates and durably stages a kernel-built withdrawal proposal as the
/// prepared attempt for its `(withdrawal_id, epoch)`.
///
/// This is the kernel `withdrawal-proposal-built` effect path. It expects the
/// withdrawal request to already be tracked; proposal bodies are cached
/// process-locally, while the local projection stores only lifecycle state.
pub async fn persist_built_withdrawal_proposal(
    proposal: &WithdrawalProposalData,
    proposal_registry: &WithdrawalProposalRegistry,
) -> Result<WithdrawalProposalValidationOutcome, BridgeError> {
    proposal_registry
        .validate_and_cache_prepared(proposal)
        .await
        .map_err(|err| {
            BridgeError::Runtime(format!("withdrawal proposal validation failed: {err}"))
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignedProposalPersistenceOutcome {
    Inserted,
    Replay,
    Noop,
}

/// Persists a signed proposal contribution after verifying that it only adds
/// witness data to the already stored base proposal.
async fn persist_signed_withdrawal_proposal(
    proposal: &WithdrawalProposalData,
    local_node_id: u64,
    proposal_registry: &WithdrawalProposalRegistry,
) -> Result<SignedProposalPersistenceOutcome, BridgeError> {
    let Some(base_proposal) = proposal_registry
        .fetch_cached_proposal(proposal.id.clone(), proposal.epoch)
        .await?
    else {
        return Err(BridgeError::Runtime(format!(
            "missing persisted base withdrawal proposal for {:?} epoch {}",
            proposal.id, proposal.epoch
        )));
    };
    if !signed_proposal_matches_base(&base_proposal, proposal) {
        return Err(BridgeError::Runtime(
            "signed withdrawal proposal diverged from persisted base proposal".into(),
        ));
    }
    if base_proposal.transaction == proposal.transaction {
        return Ok(SignedProposalPersistenceOutcome::Noop);
    }

    let proposal_hash = proposal.proposal_hash()?;
    if proposal_registry
        .has_signed_proposal_from_signer(
            &proposal.id, proposal.epoch, &proposal_hash, local_node_id,
        )
        .await?
    {
        return Ok(SignedProposalPersistenceOutcome::Replay);
    }

    proposal_registry
        .record_proposal_signed(proposal, local_node_id)
        .await?;
    Ok(SignedProposalPersistenceOutcome::Inserted)
}

#[derive(Clone)]
pub struct WithdrawalExecutionDriverContext {
    pub runtime: Arc<BridgeRuntimeHandle>,
    pub stop_controller: StopController,
    pub bridge_status: BridgeStatus,
    pub stop: StopHandle,
    pub proposal_registry: Arc<WithdrawalProposalRegistry>,
    pub withdrawal_transport: Arc<WithdrawalProposalTransport>,
    pub peers: Vec<PeerEndpoint>,
    pub activation_cutoff: WithdrawalActivationCutoff,
}

/// Creates the runtime driver that consumes withdrawal-related kernel effects
/// and translates them into durable Rust-side proposal state changes.
pub fn create_withdrawal_execution_driver(
    context: WithdrawalExecutionDriverContext,
    local_node_id: u64,
) -> IODriverFn {
    use nockapp::driver::{make_driver, NockAppHandle};

    make_driver(move |handle: NockAppHandle| {
        let runtime = context.runtime.clone();
        let stop_controller = context.stop_controller.clone();
        let bridge_status = context.bridge_status.clone();
        let stop = context.stop.clone();
        let proposal_registry = context.proposal_registry.clone();
        let withdrawal_transport = context.withdrawal_transport.clone();
        let peers = context.peers.clone();
        let activation_cutoff = context.activation_cutoff;

        async move {
            loop {
                if stop.is_stopped() {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }

                let effect = match handle.next_effect().await {
                    Ok(effect) => effect,
                    Err(_) => continue,
                };

                let bridge_effect = {
                    let root = unsafe { effect.root() };
                    let space = effect.noun_space();
                    match BridgeEffect::from_noun(root, &space) {
                        Ok(effect) => effect,
                        Err(err) => {
                            warn!("failed to decode withdrawal effect: {}", err);
                            continue;
                        }
                    }
                };

                let outcome = match classify_withdrawal_execution_effect(&bridge_effect.variant) {
                    Some(WithdrawalExecutionEffect::BaseBlockWithdrawalsPending) => {
                        let BridgeEffectVariant::BaseBlockWithdrawalsPending(pending) =
                            bridge_effect.variant
                        else {
                            continue;
                        };
                        let first_height = pending.first_height;
                        let last_height = pending.last_height;
                        persist_pending_base_block_withdrawals_after_activation(
                            pending,
                            runtime.as_ref(),
                            proposal_registry.as_ref(),
                            activation_cutoff,
                        )
                        .await
                        .map(|tracked| {
                            info!(
                                target: "bridge.withdrawal",
                                tracked,
                                first_height,
                                last_height,
                                "tracked pending base block withdrawals and acked kernel commit",
                            );
                        })
                    }
                    Some(WithdrawalExecutionEffect::WithdrawalProposalBuilt) => {
                        let BridgeEffectVariant::WithdrawalProposalBuilt(proposal) =
                            bridge_effect.variant
                        else {
                            continue;
                        };
                        match withdrawal_transport
                            .current_expected_assembler_node_id(&proposal)
                            .await
                        {
                            Ok(expected_node_id) if expected_node_id != local_node_id => {
                                if let Err(err) =
                                    proposal_registry.release_assembly_lock(&proposal.id).await
                                {
                                    warn!(
                                        target: "bridge.withdrawal",
                                        withdrawal_id = ?proposal.id,
                                        epoch = proposal.epoch,
                                        error = %err,
                                        "failed to release stale withdrawal assembly lock",
                                    );
                                    Err(err)
                                } else {
                                    info!(
                                        target: "bridge.withdrawal",
                                        withdrawal_id = ?proposal.id,
                                        epoch = proposal.epoch,
                                        expected_node_id,
                                        local_node_id,
                                        "dropping built withdrawal proposal because assembler handoff advanced",
                                    );
                                    Ok(())
                                }
                            }
                            Err(err) => Err(err),
                            Ok(_) => match persist_built_withdrawal_proposal(
                                &proposal,
                                proposal_registry.as_ref(),
                            )
                            .await
                            {
                                Ok(validation) => {
                                    if validation == WithdrawalProposalValidationOutcome::Inserted {
                                        match withdrawal_transport
                                            .broadcast_proposal_to_peers(&proposal, &peers)
                                            .await
                                        {
                                            Ok(broadcast) => {
                                                info!(
                                                    target: "bridge.withdrawal",
                                                    withdrawal_id = ?proposal.id,
                                                    epoch = proposal.epoch,
                                                    validation = ?validation,
                                                    accepted_peers = broadcast.accepted_node_ids.len(),
                                                    canonicalized = broadcast.canonicalized,
                                                    "persisted and broadcast built withdrawal proposal",
                                                );
                                                Ok(())
                                            }
                                            Err(err) => Err(err),
                                        }
                                    } else {
                                        info!(
                                            target: "bridge.withdrawal",
                                            withdrawal_id = ?proposal.id,
                                            epoch = proposal.epoch,
                                            validation = ?validation,
                                            "persisted built withdrawal proposal",
                                        );
                                        Ok(())
                                    }
                                }
                                Err(err) => Err(err),
                            },
                        }
                    }
                    Some(WithdrawalExecutionEffect::WithdrawalTxSigned) => {
                        let BridgeEffectVariant::WithdrawalTxSigned(proposal) =
                            bridge_effect.variant
                        else {
                            continue;
                        };
                        match withdrawal_transport
                            .sequencer_frontier_allows_withdrawal(&proposal.id)
                            .await
                        {
                            Ok(true) => {
                                match persist_signed_withdrawal_proposal(
                                    &proposal,
                                    local_node_id,
                                    proposal_registry.as_ref(),
                                )
                                .await
                                {
                                    Ok(SignedProposalPersistenceOutcome::Inserted) => {
                                        if let Err(err) = withdrawal_transport
                                            .record_signed_progress_at_sequencer(
                                                &proposal, local_node_id,
                                            )
                                            .await
                                        {
                                            Err(err)
                                        } else {
                                            withdrawal_transport
                                            .broadcast_signed_proposal_to_peers(&proposal, &peers)
                                            .await
                                            .map(|broadcast| {
                                                info!(
                                                    target: "bridge.withdrawal",
                                                    withdrawal_id = ?proposal.id,
                                                    epoch = proposal.epoch,
                                                    accepted_peers = broadcast.accepted_node_ids.len(),
                                                    "persisted and broadcast signed withdrawal proposal",
                                                );
                                            })
                                        }
                                    }
                                    Ok(SignedProposalPersistenceOutcome::Replay) => {
                                        info!(
                                            target: "bridge.withdrawal",
                                            withdrawal_id = ?proposal.id,
                                            epoch = proposal.epoch,
                                            "signed withdrawal proposal replay ignored",
                                        );
                                        Ok(())
                                    }
                                    Ok(SignedProposalPersistenceOutcome::Noop) => {
                                        info!(
                                            target: "bridge.withdrawal",
                                            withdrawal_id = ?proposal.id,
                                            epoch = proposal.epoch,
                                            "signed withdrawal proposal produced no new witness contribution",
                                        );
                                        Ok(())
                                    }
                                    Err(err) => Err(err),
                                }
                            }
                            Ok(false) => {
                                info!(
                                    target: "bridge.withdrawal",
                                    withdrawal_id = ?proposal.id,
                                    epoch = proposal.epoch,
                                    "dropping signed withdrawal proposal because it is not the sequencer frontier",
                                );
                                continue;
                            }
                            Err(err) => {
                                warn!(
                                    target: "bridge.withdrawal",
                                    withdrawal_id = ?proposal.id,
                                    epoch = proposal.epoch,
                                    error = %err,
                                    "failed to verify signed withdrawal proposal against sequencer frontier",
                                );
                                Err(err)
                            }
                        }
                    }
                    None => continue,
                };

                if let Err(err) = outcome {
                    error!(
                        target: "bridge.withdrawal",
                        error=%err,
                        "withdrawal execution driver hit a durable conflict, triggering local stop"
                    );
                    let reason = format!("withdrawal execution persistence conflict: {err}");
                    trigger_local_stop(
                        runtime.clone(),
                        stop_controller.clone(),
                        bridge_status.clone(),
                        reason,
                    )
                    .await;
                }
            }
        }
    })
}

pub struct WithdrawalAssemblyContext<K> {
    pub kernel: Arc<K>,
    pub snapshot_service: Arc<BridgeNoteSnapshotService>,
    pub sequencer: Arc<dyn WithdrawalSequencerPort>,
    pub proposal_registry: Arc<WithdrawalProposalRegistry>,
    pub bridge_status: BridgeStatus,
    pub planner: WithdrawalAssemblyPlannerConfig,
    pub fallback_policy: WithdrawalFallbackPolicy,
    pub local_node_id: u64,
    pub node_pkhs: Vec<nockchain_types::tx_engine::common::Hash>,
}

pub struct WithdrawalSigningContext<K> {
    pub kernel: Arc<K>,
    pub sequencer: Arc<dyn WithdrawalSequencerPort>,
    pub proposal_registry: Arc<WithdrawalProposalRegistry>,
    pub local_node_id: u64,
    pub local_signer_pkh: Tip5Hash,
    pub node_eth_addresses: HashMap<u64, Address>,
    pub fatal_stop: Option<WithdrawalFatalStopContext>,
}

#[derive(Clone)]
pub struct WithdrawalFatalStopContext {
    pub runtime: Arc<BridgeRuntimeHandle>,
    pub stop_controller: StopController,
    pub bridge_status: BridgeStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WithdrawalAssemblyTickOutcome {
    Idle,
    Busy(WithdrawalId),
    AlreadyPrepared(WithdrawalId),
    RequestedBuild {
        id: WithdrawalId,
        epoch: u64,
        selected_inputs: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WithdrawalSigningTickOutcome {
    Idle,
    RequestedSign { id: WithdrawalId, epoch: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WithdrawalAssemblyLoopPolicy {
    pub poll_interval: Duration,
}

impl Default for WithdrawalAssemblyLoopPolicy {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WithdrawalSigningLoopPolicy {
    pub poll_interval: Duration,
}

impl Default for WithdrawalSigningLoopPolicy {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(1),
        }
    }
}

/// Runs one withdrawal assembly tick.
///
/// The tick expires stale local assembly locks, picks the next stageable
/// tracked withdrawal, enforces deterministic proposer ownership, and asks the
/// kernel to build the next epoch candidate transaction.
pub async fn withdrawal_assembly_tick_once<K: WithdrawalKernelPort>(
    context: &WithdrawalAssemblyContext<K>,
) -> Result<WithdrawalAssemblyTickOutcome, BridgeError> {
    expire_stale_live_attempts(context).await?;
    let Some(stageable) = next_stageable_withdrawal_request(context).await? else {
        return Ok(WithdrawalAssemblyTickOutcome::Idle);
    };
    let request = stageable.tracked;
    let request_id = request.id.clone();

    let local_epoch = context
        .proposal_registry
        .next_expected_epoch(&request_id)
        .await?;
    let epoch = local_epoch.max(stageable.sequencer_epoch);
    let handoff_index = if epoch == stageable.sequencer_epoch {
        stageable.sequencer_handoff_index
    } else {
        0
    };
    if scheduled_assembler_turn_node_id(&request_id, epoch, handoff_index, &context.node_pkhs)?
        != context.local_node_id
    {
        return Ok(WithdrawalAssemblyTickOutcome::Idle);
    }
    let Some(current_base_height) = current_confirmed_base_height(context) else {
        return Ok(WithdrawalAssemblyTickOutcome::Idle);
    };
    match context
        .proposal_registry
        .acquire_withdrawal_assembly(&request_id, epoch, current_base_height)
        .await?
    {
        AcquireWithdrawalAssemblyOutcome::Busy { active } => {
            Ok(WithdrawalAssemblyTickOutcome::Busy(active))
        }
        AcquireWithdrawalAssemblyOutcome::AlreadyTracked { id, .. } => {
            Ok(WithdrawalAssemblyTickOutcome::AlreadyPrepared(id))
        }
        AcquireWithdrawalAssemblyOutcome::Acquired => {
            let now = SystemTime::now();
            context.snapshot_service.refresh_if_stale(now).await?;
            let reserved_inputs = context.sequencer.get_reserved_withdrawal_inputs().await?;
            let snapshot = context
                .snapshot_service
                .spendable_snapshot(&reserved_inputs)
                .ok_or_else(|| {
                    BridgeError::Runtime("no confirmed bridge note snapshot available".into())
                });
            let snapshot = match snapshot {
                Ok(snapshot) => snapshot,
                Err(err) => {
                    context
                        .proposal_registry
                        .release_assembly_lock(&request_id)
                        .await?;
                    return Err(err);
                }
            };

            let build = match plan_withdrawal_build(&request, &snapshot, &context.planner) {
                Ok(build) => build,
                Err(WithdrawalBuildPlanningError::InsufficientFunds {
                    selected_total,
                    required,
                }) => {
                    context
                        .proposal_registry
                        .release_assembly_lock(&request_id)
                        .await?;
                    debug!(
                        target: "bridge.withdrawal",
                        withdrawal_id = ?request_id,
                        selected_total,
                        required,
                        snapshot_height = (snapshot.metadata.height.0).0,
                        "waiting for enough safe bridge-owned notes to assemble withdrawal",
                    );
                    return Ok(WithdrawalAssemblyTickOutcome::Idle);
                }
                Err(WithdrawalBuildPlanningError::Bridge(err)) => {
                    context
                        .proposal_registry
                        .release_assembly_lock(&request_id)
                        .await?;
                    return Err(err);
                }
            };

            let create = CreateWithdrawalTxData {
                id: request_id.clone(),
                recipient: request.recipient.clone(),
                amount: build.net_amount,
                burned_amount: build.burned_amount,
                base_batch_end: request.base_batch_end,
                epoch,
                snapshot: WithdrawalSnapshot {
                    height: (snapshot.metadata.height.0).0,
                    block_id: snapshot.metadata.block_id.clone(),
                },
                fee: build.fee,
                selected_notes: build.selected_notes.clone(),
            };

            info!(
                target: "bridge.withdrawal",
                withdrawal_id = ?request_id,
                epoch,
                selected_inputs = build.selected_inputs.len(),
                snapshot_height = create.snapshot.height,
                burned_amount = build.burned_amount,
                net_amount = build.net_amount,
                fee = build.fee,
                "requesting withdrawal proposal build from kernel",
            );
            if let Err(err) = context.kernel.poke_create_withdrawal_tx(create).await {
                context
                    .proposal_registry
                    .release_assembly_lock(&request_id)
                    .await?;
                return Err(err);
            }

            Ok(WithdrawalAssemblyTickOutcome::RequestedBuild {
                id: request_id,
                epoch,
                selected_inputs: build.selected_inputs.len(),
            })
        }
    }
}

/// Releases stale assembly locks so other epochs can continue making progress
/// after a local build attempt stalls before proposal creation.
async fn expire_stale_live_attempts<K: WithdrawalKernelPort>(
    context: &WithdrawalAssemblyContext<K>,
) -> Result<(), BridgeError> {
    let Some(current_base_height) = current_confirmed_base_height(context) else {
        return Ok(());
    };
    let live = context.proposal_registry.list_live_withdrawals().await?;

    for row in live {
        match row.state {
            WithdrawalState::Assembling
                if assembly_lock_timed_out(
                    row.turn_started_base_height, current_base_height,
                    context.fallback_policy.assembly_timeout_blocks,
                ) =>
            {
                let tracked = TrackedWithdrawalRequest::from_live_withdrawal(&row)?;
                register_withdrawal_or_alert(
                    context.sequencer.as_ref(),
                    &context.bridge_status,
                    &tracked,
                )
                .await?;
                let status = context
                    .sequencer
                    .get_sequenced_withdrawal_status(&row.id)
                    .await?;
                if status.found && status.current_epoch == row.current_epoch {
                    let next_handoff_index = status
                        .handoff_index
                        .checked_add(1)
                        .ok_or_else(|| {
                            BridgeError::Runtime(format!(
                                "pre-canonical handoff index overflow for stale assembling withdrawal {:?} epoch {}",
                                row.id, row.current_epoch
                            ))
                        })?;
                    context
                        .sequencer
                        .advance_precanonical_handoff(
                            &row.id, row.current_epoch, next_handoff_index, current_base_height,
                        )
                        .await?;
                }
                let _ = context
                    .proposal_registry
                    .release_stale_assembly_lock(&row.id, row.current_epoch)
                    .await?;
            }
            WithdrawalState::Prepared
                if assembly_lock_timed_out(
                    row.turn_started_base_height, current_base_height,
                    context.fallback_policy.assembly_timeout_blocks,
                ) =>
            {
                let proposal = context
                    .proposal_registry
                    .fetch_cached_proposal(row.id.clone(), row.current_epoch)
                    .await?
                    .ok_or_else(|| {
                        BridgeError::Runtime(format!(
                            "missing cached proposal for stale prepared withdrawal {:?} epoch {}",
                            row.id, row.current_epoch
                        ))
                    })?;
                context
                    .proposal_registry
                    .mark_proposal_expired(&proposal)
                    .await?;
            }
            _ => {}
        }
    }

    Ok(())
}

fn current_confirmed_base_height<K>(context: &WithdrawalAssemblyContext<K>) -> Option<u64> {
    let height = context.bridge_status.network().base.height;
    (height > 0).then_some(height)
}

fn assembly_lock_timed_out(
    turn_started_base_height: Option<u64>,
    current_base_height: u64,
    timeout_blocks: u64,
) -> bool {
    let Some(turn_started_base_height) = turn_started_base_height else {
        return false;
    };
    if current_base_height < turn_started_base_height {
        return false;
    }
    if timeout_blocks == 0 {
        return true;
    }
    current_base_height - turn_started_base_height >= timeout_blocks
}

/// Runs one signing tick over peer-canonical proposals that still need this
/// node's signature contribution.
pub async fn withdrawal_signing_tick_once<K: WithdrawalKernelPort>(
    context: &WithdrawalSigningContext<K>,
) -> Result<WithdrawalSigningTickOutcome, BridgeError> {
    let frontier_started = Instant::now();
    let frontier = context.sequencer.current_live_withdrawal_nonce().await;
    metrics::init_metrics()
        .withdrawal_frontier_status_fetch_time
        .add_timing(&frontier_started.elapsed());
    if frontier.is_err() {
        metrics::init_metrics()
            .withdrawal_frontier_status_fetch_error
            .increment();
    }
    let Some(frontier_nonce) = frontier? else {
        metrics::init_metrics()
            .withdrawal_frontier_present
            .swap(0.0);
        return Ok(WithdrawalSigningTickOutcome::Idle);
    };
    let metrics = metrics::init_metrics();
    metrics.withdrawal_frontier_present.swap(1.0);
    metrics
        .withdrawal_frontier_nonce
        .swap(frontier_nonce as f64);
    let Some(row) =
        fetch_post_canonical_withdrawal_for_signing_by_nonce(context, frontier_nonce).await?
    else {
        metrics.withdrawal_frontier_local_row_present.swap(0.0);
        return Ok(WithdrawalSigningTickOutcome::Idle);
    };
    metrics.withdrawal_frontier_local_row_present.swap(1.0);
    if !sequencer_nonce_allows_signing(context, &row, frontier_nonce).await? {
        metrics.withdrawal_signing_not_frontier.increment();
        return Ok(WithdrawalSigningTickOutcome::Idle);
    }
    let Some(proposal_hash) = row.proposal_hash.as_deref() else {
        return Ok(WithdrawalSigningTickOutcome::Idle);
    };
    if context
        .proposal_registry
        .has_signed_proposal_from_signer(
            &row.id, row.current_epoch, proposal_hash, context.local_node_id,
        )
        .await?
    {
        return Ok(WithdrawalSigningTickOutcome::Idle);
    }

    let Some(proposal) = load_or_hydrate_signing_proposal(context, &row).await? else {
        return Ok(WithdrawalSigningTickOutcome::Idle);
    };
    if !transaction_needs_signer(&proposal.transaction, &context.local_signer_pkh) {
        return Ok(WithdrawalSigningTickOutcome::Idle);
    }

    context.kernel.poke_sign_tx(proposal.clone()).await?;
    Ok(WithdrawalSigningTickOutcome::RequestedSign {
        id: proposal.id,
        epoch: proposal.epoch,
    })
}

async fn fetch_post_canonical_withdrawal_for_signing_by_nonce<K: WithdrawalKernelPort>(
    context: &WithdrawalSigningContext<K>,
    frontier_nonce: u64,
) -> Result<Option<LiveWithdrawalView>, BridgeError> {
    if let Some(row) = context
        .proposal_registry
        .fetch_live_withdrawal_by_nonce(frontier_nonce)
        .await?
    {
        if row.state == WithdrawalState::PeerCanonical {
            return Ok(Some(row));
        }
        let tracked = TrackedWithdrawalRequest::from_live_withdrawal(&row)?;
        return hydrate_peer_canonical_withdrawal_for_signing(
            context,
            &tracked,
            Some(row.current_epoch),
            frontier_nonce,
        )
        .await;
    }

    let tracked = context
        .proposal_registry
        .load_sorted_tracked_withdrawal_requests()
        .await?
        .into_iter()
        .find(|tracked| tracked.withdrawal_nonce == frontier_nonce);
    match tracked {
        Some(tracked) => {
            hydrate_peer_canonical_withdrawal_for_signing(context, &tracked, None, frontier_nonce)
                .await
        }
        None => Ok(None),
    }
}

async fn hydrate_peer_canonical_withdrawal_for_signing<K: WithdrawalKernelPort>(
    context: &WithdrawalSigningContext<K>,
    tracked: &TrackedWithdrawalRequest,
    expected_epoch: Option<u64>,
    frontier_nonce: u64,
) -> Result<Option<LiveWithdrawalView>, BridgeError> {
    if tracked.withdrawal_nonce != frontier_nonce {
        return Ok(None);
    }

    let status = context
        .sequencer
        .get_sequenced_withdrawal_status(&tracked.id)
        .await?;
    if !status.found {
        return Ok(None);
    }
    if !matches!(
        plan_signing_sequencer_status(
            &tracked.id, tracked.withdrawal_nonce, status.withdrawal_nonce, status.found,
            &status.state,
        )?,
        WithdrawalSigningSequencerDecision::Continue
    ) {
        return Ok(None);
    }
    if status.proposal_hash.is_empty() {
        return Ok(None);
    }
    if let Some(expected_epoch) = expected_epoch {
        if status.current_epoch != expected_epoch {
            return Err(BridgeError::Runtime(format!(
                "sequencer canonical epoch {} does not match local epoch {} for {:?}",
                status.current_epoch, expected_epoch, tracked.id
            )));
        }
    }

    let artifacts = context
        .sequencer
        .load_canonical_proposal_artifacts(&tracked.id)
        .await?
        .ok_or_else(|| {
            BridgeError::Runtime(format!(
                "missing sequencer canonical artifacts for withdrawal {:?} epoch {}",
                tracked.id, status.current_epoch
            ))
        })?;
    if artifacts.epoch != status.current_epoch {
        return Err(BridgeError::Runtime(format!(
            "sequencer canonical artifact epoch {} does not match status epoch {} for {:?}",
            artifacts.epoch, status.current_epoch, tracked.id
        )));
    }
    let Some(commit_certificate_bytes) = artifacts.commit_certificate.clone() else {
        return Err(BridgeError::Runtime(format!(
            "missing sequencer commit certificate for canonical withdrawal {:?} epoch {}",
            tracked.id, status.current_epoch
        )));
    };
    let commit_certificate = WithdrawalCommitCertificate::decode(
        commit_certificate_bytes.as_slice(),
    )
    .map_err(|err| {
        BridgeError::Runtime(format!(
            "failed to decode sequencer commit certificate for {:?} epoch {}: {err}",
            tracked.id, status.current_epoch
        ))
    })?;
    let proposal = reconstruct_withdrawal_proposal(tracked, artifacts)?;
    let proposal_hash = proposal.proposal_hash()?;
    if proposal_hash != status.proposal_hash {
        return Err(BridgeError::Runtime(format!(
            "hydrated canonical proposal hash {} does not match sequencer status hash {} for {:?}",
            proposal_hash, status.proposal_hash, tracked.id
        )));
    }
    let required_commit_signers = required_withdrawal_commit_signature_threshold(&proposal)?;
    verify_withdrawal_commit_certificate(
        &proposal, &proposal_hash, &commit_certificate, required_commit_signers,
        &context.node_eth_addresses,
    )?;

    context
        .proposal_registry
        .mark_proposal_canonical_with_certificate(&proposal, &commit_certificate)
        .await?;
    context
        .proposal_registry
        .cache_reconstructed_proposal(proposal)
        .await?;
    Ok(context
        .proposal_registry
        .fetch_live_withdrawal_by_nonce(frontier_nonce)
        .await?
        .filter(|row| row.state == WithdrawalState::PeerCanonical))
}

async fn load_or_hydrate_signing_proposal<K: WithdrawalKernelPort>(
    context: &WithdrawalSigningContext<K>,
    row: &LiveWithdrawalView,
) -> Result<Option<WithdrawalProposalData>, BridgeError> {
    if let Some(proposal) = context
        .proposal_registry
        .fetch_cached_proposal(row.id.clone(), row.current_epoch)
        .await?
    {
        return Ok(Some(proposal));
    }
    metrics::init_metrics()
        .withdrawal_signing_cache_miss
        .increment();
    let Some(artifacts) = context
        .sequencer
        .load_canonical_proposal_artifacts(&row.id)
        .await?
    else {
        return Ok(None);
    };
    let Some(commit_certificate_bytes) = artifacts.commit_certificate.clone() else {
        return Err(BridgeError::Runtime(format!(
            "missing sequencer commit certificate for canonical withdrawal {:?} epoch {}",
            row.id, row.current_epoch
        )));
    };
    let commit_certificate = WithdrawalCommitCertificate::decode(
        commit_certificate_bytes.as_slice(),
    )
    .map_err(|err| {
        BridgeError::Runtime(format!(
            "failed to decode sequencer commit certificate for {:?} epoch {}: {err}",
            row.id, row.current_epoch
        ))
    })?;
    let tracked = TrackedWithdrawalRequest::from_live_withdrawal(row)?;
    let proposal = reconstruct_withdrawal_proposal(&tracked, artifacts)?;
    if proposal.epoch != row.current_epoch {
        return Err(BridgeError::Runtime(format!(
            "hydrated canonical proposal epoch {} does not match live row epoch {} for {:?}",
            proposal.epoch, row.current_epoch, row.id
        )));
    }
    let proposal_hash = proposal.proposal_hash()?;
    let required_commit_signers = required_withdrawal_commit_signature_threshold(&proposal)?;
    verify_withdrawal_commit_certificate(
        &proposal, &proposal_hash, &commit_certificate, required_commit_signers,
        &context.node_eth_addresses,
    )?;
    context
        .proposal_registry
        .cache_reconstructed_proposal(proposal.clone())
        .await?;
    metrics::init_metrics()
        .withdrawal_signing_hydrated
        .increment();
    Ok(Some(proposal))
}

/// Returns whether the sequencer's ordering view agrees with the local tracked
/// withdrawal nonce and still expects bridge-side signature collection for this
/// withdrawal.
async fn sequencer_nonce_allows_signing<K: WithdrawalKernelPort>(
    context: &WithdrawalSigningContext<K>,
    row: &LiveWithdrawalView,
    frontier_nonce: u64,
) -> Result<bool, BridgeError> {
    let Some(local_nonce) = row.withdrawal_nonce else {
        return Err(BridgeError::Runtime(format!(
            "missing tracked withdrawal nonce for signing withdrawal {:?}",
            row.id
        )));
    };
    if local_nonce != frontier_nonce {
        return Ok(false);
    }
    let status = context
        .sequencer
        .get_sequenced_withdrawal_status(&row.id)
        .await?;
    ensure_sequencer_proposal_hash_matches_local(row, &status)?;
    Ok(matches!(
        plan_signing_sequencer_status(
            &row.id, local_nonce, status.withdrawal_nonce, status.found, &status.state,
        )?,
        WithdrawalSigningSequencerDecision::Continue
    ))
}

fn ensure_sequencer_proposal_hash_matches_local(
    row: &LiveWithdrawalView,
    status: &SequencedWithdrawalStatusResponse,
) -> Result<(), BridgeError> {
    let Some(local_hash) = row.proposal_hash.as_deref() else {
        return Ok(());
    };
    if status.proposal_hash.is_empty() || status.proposal_hash == local_hash {
        return Ok(());
    }
    Err(BridgeError::Runtime(format!(
        "withdrawal signing proposal hash mismatch for {:?}: local {}, sequencer {}",
        row.id, local_hash, status.proposal_hash
    )))
}

/// Runs the long-lived withdrawal assembly loop until the bridge is stopped.
pub async fn run_withdrawal_assembly_loop<K: WithdrawalKernelPort>(
    context: WithdrawalAssemblyContext<K>,
    stop: StopHandle,
    policy: WithdrawalAssemblyLoopPolicy,
) {
    let mut ticker = interval(policy.poll_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;
        if stop.is_stopped() {
            continue;
        }

        let metrics = metrics::init_metrics();
        metrics.withdrawal_assembly_ticks.increment();
        let started = Instant::now();
        match withdrawal_assembly_tick_once(&context).await {
            Ok(outcome) => {
                metrics
                    .withdrawal_assembly_tick_time
                    .add_timing(&started.elapsed());
                match outcome {
                    WithdrawalAssemblyTickOutcome::Idle => {
                        metrics.withdrawal_assembly_idle.increment();
                    }
                    WithdrawalAssemblyTickOutcome::RequestedBuild { .. } => {
                        metrics.withdrawal_assembly_built.increment();
                    }
                    WithdrawalAssemblyTickOutcome::Busy(_)
                    | WithdrawalAssemblyTickOutcome::AlreadyPrepared(_) => {}
                }
            }
            Err(err) => {
                metrics
                    .withdrawal_assembly_tick_time
                    .add_timing(&started.elapsed());
                warn!(
                    target: "bridge.withdrawal",
                    error=%err,
                    "withdrawal assembly tick failed"
                );
            }
        }
    }
}

/// Runs the long-lived withdrawal signing loop until the bridge is stopped.
pub async fn run_withdrawal_signing_loop<K: WithdrawalKernelPort>(
    context: WithdrawalSigningContext<K>,
    stop: StopHandle,
    policy: WithdrawalSigningLoopPolicy,
) {
    let mut ticker = interval(policy.poll_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;
        if stop.is_stopped() {
            continue;
        }

        let metrics = metrics::init_metrics();
        metrics.withdrawal_signing_ticks.increment();
        let started = Instant::now();
        match withdrawal_signing_tick_once(&context).await {
            Ok(outcome) => {
                metrics
                    .withdrawal_signing_tick_time
                    .add_timing(&started.elapsed());
                match outcome {
                    WithdrawalSigningTickOutcome::Idle => {
                        metrics.withdrawal_signing_idle.increment();
                    }
                    WithdrawalSigningTickOutcome::RequestedSign { .. } => {
                        metrics.withdrawal_signing_signed.increment();
                    }
                }
            }
            Err(err) => {
                metrics
                    .withdrawal_signing_tick_time
                    .add_timing(&started.elapsed());
                warn!(
                    target: "bridge.withdrawal",
                    error=%err,
                    "withdrawal signing tick failed"
                );
                if is_fatal_withdrawal_signing_error(&err) {
                    if let Some(stop_context) = context.fatal_stop.as_ref() {
                        trigger_local_stop(
                            stop_context.runtime.clone(),
                            stop_context.stop_controller.clone(),
                            stop_context.bridge_status.clone(),
                            format!("fatal withdrawal signing invariant violation: {err}"),
                        )
                        .await;
                    }
                }
            }
        }
    }
}

fn is_fatal_withdrawal_signing_error(err: &BridgeError) -> bool {
    match err {
        BridgeError::Runtime(message) => {
            message.starts_with("withdrawal signing nonce mismatch for ")
        }
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedWithdrawalBuild {
    selected_inputs: Vec<nockchain_types::v1::Name>,
    selected_notes: Vec<SelectedWithdrawalNoteData>,
    net_amount: u64,
    burned_amount: u64,
    fee: u64,
}

#[derive(Debug)]
enum WithdrawalBuildPlanningError {
    InsufficientFunds { selected_total: u64, required: u64 },
    Bridge(BridgeError),
}

struct StageableWithdrawalRequest {
    tracked: TrackedWithdrawalRequest,
    sequencer_epoch: u64,
    sequencer_handoff_index: u64,
}

/// Returns the next tracked withdrawal request that is safe to assemble on
/// this node, skipping requests already represented by live durable state or
/// already advanced at the sequencer.
async fn next_stageable_withdrawal_request<K: WithdrawalKernelPort>(
    context: &WithdrawalAssemblyContext<K>,
) -> Result<Option<StageableWithdrawalRequest>, BridgeError> {
    let tracked_requests = context
        .proposal_registry
        .load_sorted_tracked_withdrawal_requests()
        .await?;
    let frontier_started = Instant::now();
    let frontier = context.sequencer.current_live_withdrawal_nonce().await;
    metrics::init_metrics()
        .withdrawal_frontier_status_fetch_time
        .add_timing(&frontier_started.elapsed());
    if frontier.is_err() {
        metrics::init_metrics()
            .withdrawal_frontier_status_fetch_error
            .increment();
    }
    let frontier_nonce = match frontier? {
        Some(frontier_nonce) => frontier_nonce,
        None => {
            metrics::init_metrics()
                .withdrawal_frontier_present
                .swap(0.0);
            let mut frontier_nonce = None;
            for tracked in &tracked_requests {
                let status = context
                    .sequencer
                    .get_sequenced_withdrawal_status(&tracked.id)
                    .await?;
                if sequenced_withdrawal_released(&status) {
                    continue;
                }
                register_withdrawal_or_alert(
                    context.sequencer.as_ref(),
                    &context.bridge_status,
                    tracked,
                )
                .await?;
                if let Some(nonce) = context.sequencer.current_live_withdrawal_nonce().await? {
                    frontier_nonce = Some(nonce);
                    break;
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
            let Some(frontier_nonce) = frontier_nonce else {
                return Ok(None);
            };
            frontier_nonce
        }
    };
    let metrics = metrics::init_metrics();
    metrics.withdrawal_frontier_present.swap(1.0);
    metrics
        .withdrawal_frontier_nonce
        .swap(frontier_nonce as f64);
    let Some(tracked) = tracked_requests
        .into_iter()
        .find(|tracked| tracked.withdrawal_nonce == frontier_nonce)
    else {
        metrics.withdrawal_frontier_local_row_present.swap(0.0);
        return Ok(None);
    };
    metrics.withdrawal_frontier_local_row_present.swap(1.0);
    if context
        .proposal_registry
        .fetch_live_withdrawal(&tracked.id)
        .await?
        .is_some()
    {
        // TODO: Consider a more explicit helper here, e.g. checking whether
        // the tracked withdrawal is still pending, since `fetch_live_withdrawal`
        // currently encodes that distinction indirectly.
        return Ok(None);
    }
    let status = context
        .sequencer
        .get_sequenced_withdrawal_status(&tracked.id)
        .await?;
    if status.found {
        if status.withdrawal_nonce != frontier_nonce {
            return Err(BridgeError::Runtime(format!(
                "sequencer withdrawal nonce mismatch for {:?}: local {} sequencer {}",
                tracked.id, frontier_nonce, status.withdrawal_nonce
            )));
        }
        if status.state != WithdrawalState::Pending.as_str() {
            return Ok(None);
        }
    } else {
        return Ok(None);
    }
    Ok(Some(StageableWithdrawalRequest {
        tracked,
        sequencer_epoch: status.current_epoch,
        sequencer_handoff_index: status.handoff_index,
    }))
}

/// Computes the note-selection and fee plan for one withdrawal build using the
/// latest confirmed bridge-owned note snapshot.
fn plan_withdrawal_build(
    request: &TrackedWithdrawalRequest,
    snapshot: &wallet_tx_builder::adapter::NormalizedSnapshot,
    planner: &WithdrawalAssemblyPlannerConfig,
) -> Result<PlannedWithdrawalBuild, WithdrawalBuildPlanningError> {
    let matcher = LockRootLockMatcher::from_lock_root(&planner.spend_authority_lock_root)
        .map_err(|err| {
            WithdrawalBuildPlanningError::Bridge(BridgeError::Runtime(format!(
                "invalid bridge lock root: {err}"
            )))
        })?
        .with_spend_condition(planner.spend_authority_spend_condition.clone());

    let plan = plan_withdrawal_tx(
        &WithdrawalPlanRequest {
            chain_context: planner.chain_context(snapshot.metadata.height.clone()),
            candidates: snapshot.candidates.clone(),
            burned_amount: request.amount,
            nicks_fee_per_nock: planner.nicks_fee_per_nock,
            recipient_lock_root: request.recipient.clone(),
            beid: request.id.base_event_id.to_belt_digits(),
            base_hash: request.id.as_of.clone(),
            base_batch_end: request.base_batch_end,
            refund_output: RefundOutputTemplate {
                lock_root: planner.refund_lock_root.clone(),
                note_data: planner.refund_note_data.clone(),
            },
        },
        &matcher,
    )
    .map_err(|err| match err {
        PlanError::InsufficientFunds {
            selected_total,
            required,
        } => WithdrawalBuildPlanningError::InsufficientFunds {
            selected_total,
            required,
        },
        err => WithdrawalBuildPlanningError::Bridge(BridgeError::Runtime(format!(
            "withdrawal note selection failed: {err}"
        ))),
    })?;

    let mut selected_inputs = Vec::with_capacity(plan.plan.selected.len());
    let mut selected_notes = Vec::with_capacity(plan.plan.selected.len());
    for selected in plan.plan.selected {
        let Some(candidate) = snapshot
            .candidates
            .iter()
            .find(|candidate| candidate.identity() == &selected)
        else {
            return Err(WithdrawalBuildPlanningError::Bridge(BridgeError::Runtime(
                format!(
                    "selected withdrawal note missing from pinned snapshot: {}/{}",
                    selected.name.first.to_base58(),
                    selected.name.last.to_base58()
                ),
            )));
        };
        selected_inputs.push(selected.name.clone());
        selected_notes.push(SelectedWithdrawalNoteData {
            name: selected.name,
            note: nockchain_types::v1::Note::try_from(candidate).map_err(|err| {
                WithdrawalBuildPlanningError::Bridge(BridgeError::Runtime(err.to_string()))
            })?,
        });
    }

    Ok(PlannedWithdrawalBuild {
        selected_inputs,
        selected_notes,
        net_amount: plan.net_recipient_amount,
        burned_amount: plan.burned_amount,
        fee: plan.plan.final_fee,
    })
}

/// Returns whether this transaction is still missing the local signer's PKH
/// signature on any threshold-controlled input.
fn transaction_needs_signer(
    transaction: &nockchain_types::v1::Transaction,
    signer_pkh: &Tip5Hash,
) -> bool {
    let nockchain_types::v1::Transaction::V1(transaction) = transaction;
    let nockchain_types::v1::InputMetadata::SpendConditions(input_metadata) =
        &transaction.metadata.inputs
    else {
        return false;
    };
    let nockchain_types::v1::WitnessData::Witnesses(witness_map) = &transaction.witness_data else {
        return false;
    };

    input_metadata.0.iter().any(|(name, spend_condition)| {
        let Some(required) = spend_condition.required_pkh_policy() else {
            return false;
        };
        if !required.contains(signer_pkh) {
            return false;
        }
        let Some((_, witness)) = witness_map
            .0
            .iter()
            .find(|(witness_name, _)| witness_name == name)
        else {
            return false;
        };
        let num_signed = witness.pkh_signature.0.len();
        let signer_already_present = witness
            .pkh_signature
            .0
            .iter()
            .any(|entry| entry.pkh == *signer_pkh);
        !signer_already_present && num_signed < required.threshold
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use alloy::primitives::Address;
    use nockchain_math::belt::Belt;
    use nockchain_types::tx_engine::common::{BlockHeight, Name, Nicks};
    use nockchain_types::tx_engine::v1::note::{
        Balance, BalanceUpdate, Note, NoteData, NoteDataEntry, NoteV1,
    };
    use nockchain_types::tx_engine::v1::tx::{Lock, SpendCondition};
    use tempfile::tempdir;
    use wallet_tx_builder::fee::{compute_bridge_fee, NICKS_PER_NOCK};

    use super::*;
    use crate::observability::status::BridgeStatus;
    use crate::observability::tui::types::{AlertSeverity, NetworkState};
    use crate::shared::ingress::proto::{
        SequencedWithdrawalStatusResponse, WithdrawalCommitCertificate, WithdrawalCommitSignature,
    };
    use crate::shared::signing::BridgeSigner;
    use crate::shared::types::{zero_tip5_hash, AtomBytes};
    use crate::withdrawal::proposals::WithdrawalProjectionStore;
    use crate::withdrawal::snapshot::{BridgeNoteSnapshotSource, BridgeOwnedNoteSelectors};
    use crate::withdrawal::transport::{compute_withdrawal_commit_digest, withdrawal_id_to_proto};
    use crate::withdrawal::types::{WithdrawalSequencerProposalArtifacts, WithdrawalSnapshot};

    #[derive(Default)]
    struct RecordingKernelPort {
        requests: Mutex<Vec<CreateWithdrawalTxData>>,
        signed: Mutex<Vec<WithdrawalProposalData>>,
        base_next_height: Mutex<Option<u64>>,
        nock_next_height: Mutex<Option<u64>>,
        base_history: Mutex<Vec<NockWithdrawalRequestKernelData>>,
        pending_base_commit: Mutex<Option<PendingBaseBlockCommit>>,
        base_commit_acks: Mutex<Vec<BaseBlockCommitAck>>,
        base_commit_ack_error: Mutex<Option<String>>,
        create_error: Mutex<Option<String>>,
    }

    #[async_trait]
    impl WithdrawalKernelPort for RecordingKernelPort {
        async fn poke_create_withdrawal_tx(
            &self,
            request: CreateWithdrawalTxData,
        ) -> Result<(), BridgeError> {
            self.requests.lock().expect("requests lock").push(request);
            if let Some(err) = self.create_error.lock().expect("create error lock").clone() {
                return Err(BridgeError::Runtime(err));
            }
            Ok(())
        }

        async fn poke_sign_tx(&self, proposal: WithdrawalProposalData) -> Result<(), BridgeError> {
            self.signed
                .lock()
                .expect("signed proposals lock")
                .push(proposal);
            Ok(())
        }

        async fn peek_base_next_height(&self) -> Result<Option<u64>, BridgeError> {
            Ok(*self.base_next_height.lock().expect("base next height lock"))
        }

        async fn peek_nock_next_height(&self) -> Result<Option<u64>, BridgeError> {
            Ok(*self.nock_next_height.lock().expect("nock next height lock"))
        }

        async fn peek_base_hashchain_withdrawals_since_height(
            &self,
            start_height: u64,
        ) -> Result<Vec<NockWithdrawalRequestKernelData>, BridgeError> {
            Ok(self
                .base_history
                .lock()
                .expect("base history lock")
                .clone()
                .into_iter()
                .filter(|request| request.base_batch_end >= start_height)
                .collect())
        }

        async fn peek_pending_base_block_commit(
            &self,
        ) -> Result<Option<PendingBaseBlockCommit>, BridgeError> {
            Ok(self
                .pending_base_commit
                .lock()
                .expect("pending base commit lock")
                .clone())
        }

        async fn poke_base_block_withdrawals_committed(
            &self,
            ack: BaseBlockCommitAck,
        ) -> Result<(), BridgeError> {
            if let Some(err) = self
                .base_commit_ack_error
                .lock()
                .expect("base commit ack error lock")
                .clone()
            {
                return Err(BridgeError::Runtime(err));
            }
            self.base_commit_acks
                .lock()
                .expect("base commit acks lock")
                .push(ack);
            self.pending_base_commit
                .lock()
                .expect("pending base commit lock")
                .take();
            Ok(())
        }

        async fn poke_repair_pending_base_block_commit(
            &self,
            ack: BaseBlockCommitAck,
        ) -> Result<(), BridgeError> {
            let pending = self
                .pending_base_commit
                .lock()
                .expect("pending base commit lock")
                .take();
            assert_eq!(pending.as_ref().map(PendingBaseBlockCommit::ack), Some(ack));
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingSequencerPort {
        registered: Mutex<Vec<(WithdrawalId, u64)>>,
        statuses: Mutex<HashMap<WithdrawalId, SequencedWithdrawalStatusResponse>>,
        canonical_artifacts: Mutex<HashMap<WithdrawalId, WithdrawalSequencerProposalArtifacts>>,
        reserved_inputs: Mutex<Vec<nockchain_types::v1::Name>>,
        register_error: Mutex<Option<String>>,
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
                .and_modify(|status| {
                    if status.withdrawal_nonce == 0 {
                        status.withdrawal_nonce = tracked.withdrawal_nonce;
                        status.found = true;
                        if status.state.is_empty() {
                            status.state = WithdrawalState::Pending.as_str().to_string();
                        }
                    }
                })
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

        async fn advance_precanonical_handoff(
            &self,
            id: &WithdrawalId,
            epoch: u64,
            next_handoff_index: u64,
            turn_started_base_height: u64,
        ) -> Result<(), BridgeError> {
            self.statuses
                .lock()
                .expect("sequencer statuses lock")
                .entry(id.clone())
                .and_modify(|status| {
                    if status.current_epoch == epoch
                        && status.state == WithdrawalState::Pending.as_str()
                        && status.handoff_index < next_handoff_index
                    {
                        status.handoff_index = next_handoff_index;
                        status.turn_started_base_height = Some(turn_started_base_height);
                    }
                })
                .or_insert(SequencedWithdrawalStatusResponse {
                    found: true,
                    current_epoch: epoch,
                    state: WithdrawalState::Pending.as_str().to_string(),
                    proposal_hash: String::new(),
                    authorized_transaction_name: String::new(),
                    withdrawal_nonce: 0,
                    handoff_index: next_handoff_index,
                    turn_started_base_height: Some(turn_started_base_height),

                    current_confirmed_base_height: None,

                    handoff_window_blocks: 0,

                    blocks_until_handoff: None,
                });
            Ok(())
        }

        async fn authorize_proposal(
            &self,
            _proposal: &WithdrawalProposalData,
            _withdrawal_nonce: u64,
            _commit_certificate: &WithdrawalCommitCertificate,
            _caller_node_id: u64,
        ) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn submit_proposal(
            &self,
            _proposal: &WithdrawalProposalData,
            _withdrawal_nonce: u64,
            _caller_node_id: u64,
        ) -> Result<Option<WithdrawalSequencerSubmitOutcome>, BridgeError> {
            Ok(Some(WithdrawalSequencerSubmitOutcome::MempoolAccepted))
        }

        async fn get_next_pending_withdrawal_ordering(
            &self,
        ) -> Result<Option<crate::withdrawal::submission::NextPendingWithdrawalOrdering>, BridgeError>
        {
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
                    return Ok(Some(
                        crate::withdrawal::submission::NextPendingWithdrawalOrdering {
                            id,
                            withdrawal_nonce,
                        },
                    ));
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

        async fn get_reserved_withdrawal_inputs(
            &self,
        ) -> Result<Vec<nockchain_types::v1::Name>, BridgeError> {
            Ok(self
                .reserved_inputs
                .lock()
                .expect("reserved inputs lock")
                .clone())
        }

        async fn load_canonical_proposal_artifacts(
            &self,
            id: &WithdrawalId,
        ) -> Result<Option<WithdrawalSequencerProposalArtifacts>, BridgeError> {
            Ok(self
                .canonical_artifacts
                .lock()
                .expect("canonical artifacts lock")
                .get(id)
                .cloned())
        }
    }

    fn sample_bridge_status(base_height: u64) -> BridgeStatus {
        let bridge_status = BridgeStatus::new(std::sync::Arc::new(std::sync::RwLock::new(vec![])));
        let mut network = NetworkState::default();
        network.base.height = base_height;
        bridge_status.update_network(network);
        bridge_status
    }

    #[test]
    fn assembly_lock_timeout_uses_base_height_windows() {
        assert!(!assembly_lock_timed_out(Some(100), 129, 30));
        assert!(assembly_lock_timed_out(Some(100), 130, 30));
        assert!(assembly_lock_timed_out(Some(100), 100, 0));
        assert!(!assembly_lock_timed_out(None, 130, 30));
    }

    #[derive(Clone)]
    struct StaticSnapshotSource {
        pages: Vec<BalanceUpdate>,
    }

    #[async_trait]
    impl BridgeNoteSnapshotSource for StaticSnapshotSource {
        async fn fetch_pages(
            &self,
            _selectors: &BridgeOwnedNoteSelectors,
        ) -> Result<Vec<BalanceUpdate>, BridgeError> {
            Ok(self.pages.clone())
        }
    }

    fn sample_transaction() -> nockchain_types::v1::Transaction {
        const TRANSACTION_JAM: &[u8] = include_bytes!(
            "../../test-fixtures/transactions/9MpGym52AumtwyBxYPyVsWHvcamUYwZkc1Nq7w3cFGF28u8ceVDwt3e.tx"
        );

        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = slab
            .cue_into(TRANSACTION_JAM.to_vec().into())
            .expect("cue transaction fixture");
        let space = nockapp::NounAllocator::noun_space(&slab);
        nockchain_types::v1::Transaction::from_noun(&noun, &space)
            .expect("decode transaction fixture")
    }

    fn partially_signed_transaction() -> (nockchain_types::v1::Transaction, Tip5Hash) {
        let mut transaction = sample_transaction();
        let nockchain_types::v1::Transaction::V1(transaction_v1) = &mut transaction;
        let nockchain_types::v1::InputMetadata::SpendConditions(input_metadata) =
            &transaction_v1.metadata.inputs
        else {
            panic!("fixture transaction must use spend-condition metadata");
        };
        let nockchain_types::v1::WitnessData::Witnesses(witness_map) =
            &mut transaction_v1.witness_data
        else {
            panic!("fixture transaction must use witness data");
        };

        for (name, spend_condition) in &input_metadata.0 {
            let Some(required) = spend_condition.required_pkh_policy() else {
                continue;
            };
            let Some((_, witness)) = witness_map
                .0
                .iter_mut()
                .find(|(witness_name, _)| witness_name == name)
            else {
                continue;
            };
            let Some(removed) = witness.pkh_signature.0.pop() else {
                continue;
            };
            if required.contains(&removed.pkh)
                && witness
                    .pkh_signature
                    .0
                    .iter()
                    .all(|entry| entry.pkh != removed.pkh)
                && witness.pkh_signature.0.len() < required.threshold
            {
                return (transaction, removed.pkh);
            }
            witness.pkh_signature.0.push(removed);
        }

        panic!("fixture transaction does not contain a removable signature");
    }

    fn sample_proposal_for_request(
        request: &NockWithdrawalRequestKernelData,
        transaction: nockchain_types::v1::Transaction,
    ) -> WithdrawalProposalData {
        WithdrawalProposalData {
            id: request.withdrawal_id(),
            recipient: request.recipient.clone(),
            amount: request.amount.saturating_sub(1),
            burned_amount: request.amount,
            base_batch_end: request.base_batch_end,
            epoch: 0,
            snapshot: WithdrawalSnapshot {
                height: 900,
                block_id: Tip5Hash([Belt(91), Belt(92), Belt(93), Belt(94), Belt(95)]),
            },
            selected_inputs: vec![sample_name(10)],
            transaction,
        }
    }

    const TEST_WITHDRAWAL_OPERATOR_KEY: &str =
        "0x0000000000000000000000000000000000000000000000000000000000000001";

    fn test_withdrawal_signer() -> BridgeSigner {
        BridgeSigner::new(TEST_WITHDRAWAL_OPERATOR_KEY.to_string())
            .expect("valid withdrawal test signer")
    }

    fn sample_node_eth_addresses() -> HashMap<u64, Address> {
        HashMap::from([(3, test_withdrawal_signer().address())])
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
            proposal_hash: proposal_hash.clone(),
            signatures: vec![WithdrawalCommitSignature {
                signer_node_id: 3,
                withdrawal_id: Some(withdrawal_id_to_proto(&proposal.id)),
                epoch: proposal.epoch,
                proposal_hash,
                signature: signature.as_bytes().to_vec(),
            }],
        }
    }

    fn sample_canonical_artifacts(
        proposal: &WithdrawalProposalData,
        commit_certificate: Option<&WithdrawalCommitCertificate>,
    ) -> WithdrawalSequencerProposalArtifacts {
        WithdrawalSequencerProposalArtifacts {
            id: proposal.id.clone(),
            epoch: proposal.epoch,
            proposal_hash: proposal.proposal_hash().expect("proposal hash"),
            amount: proposal.amount,
            base_batch_end: proposal.base_batch_end,
            snapshot: proposal.snapshot.clone(),
            selected_inputs: proposal.selected_inputs.clone(),
            transaction: proposal.transaction.clone(),
            commit_certificate: commit_certificate.map(|certificate| certificate.encode_to_vec()),
            authorized_transaction_name: None,
            authorized_transaction_jam: None,
            authorized_raw_tx: None,
        }
    }

    fn sample_base_event_id(start: u8) -> AtomBytes {
        AtomBytes((0..32).map(|offset| start.wrapping_add(offset)).collect())
    }

    #[test]
    fn atom_bytes_to_belt_digits_matches_known_vector() {
        assert_eq!(
            sample_base_event_id(1).to_belt_digits(),
            vec![
                Belt(578_437_696_156_539_417),
                Belt(10_923_933_468_832_943_055),
                Belt(14_755_409_445_788_166_057),
                Belt(2_314_601_845_482_878_064),
            ]
        );
    }

    fn sample_withdrawal_request() -> NockWithdrawalRequestKernelData {
        NockWithdrawalRequestKernelData {
            base_event_id: sample_base_event_id(1),
            recipient: Tip5Hash([Belt(71), Belt(72), Belt(73), Belt(74), Belt(75)]),
            amount: 11,
            base_batch_end: 57_600,
            as_of: Tip5Hash([Belt(81), Belt(82), Belt(83), Belt(84), Belt(85)]),
        }
    }

    fn sample_pending_base_commit(
        withdrawals: Vec<NockWithdrawalRequestKernelData>,
    ) -> PendingBaseBlockCommit {
        PendingBaseBlockCommit {
            blocks_hash: Tip5Hash([Belt(91), Belt(92), Belt(93), Belt(94), Belt(95)]),
            first_height: 57_501,
            last_height: 57_600,
            withdrawals,
        }
    }

    fn sample_withdrawal_request_with_seed(seed: u8) -> NockWithdrawalRequestKernelData {
        NockWithdrawalRequestKernelData {
            base_event_id: sample_base_event_id(seed),
            recipient: Tip5Hash([
                Belt(70 + u64::from(seed)),
                Belt(71 + u64::from(seed)),
                Belt(72 + u64::from(seed)),
                Belt(73 + u64::from(seed)),
                Belt(74 + u64::from(seed)),
            ]),
            amount: 11,
            base_batch_end: 57_600 + u64::from(seed),
            as_of: Tip5Hash([
                Belt(80 + u64::from(seed)),
                Belt(81 + u64::from(seed)),
                Belt(82 + u64::from(seed)),
                Belt(83 + u64::from(seed)),
                Belt(84 + u64::from(seed)),
            ]),
        }
    }

    #[test]
    fn legacy_create_withdrawal_txs_effect_is_not_live_execution_work() {
        assert_eq!(
            classify_withdrawal_execution_effect(&BridgeEffectVariant::CreateWithdrawalTxs(
                Vec::new()
            )),
            None
        );
        assert_eq!(
            classify_withdrawal_execution_effect(
                &BridgeEffectVariant::BaseBlockWithdrawalsPending(sample_pending_base_commit(
                    Vec::new()
                ))
            ),
            Some(WithdrawalExecutionEffect::BaseBlockWithdrawalsPending)
        );
    }

    fn sample_name(seed: u64) -> Name {
        Name::new(
            Tip5Hash([
                Belt(seed + 1),
                Belt(seed + 2),
                Belt(seed + 3),
                Belt(seed + 4),
                Belt(seed + 5),
            ]),
            Tip5Hash([
                Belt(seed + 101),
                Belt(seed + 102),
                Belt(seed + 103),
                Belt(seed + 104),
                Belt(seed + 105),
            ]),
        )
    }

    fn note_for_lock(lock: &Lock, name: Name, assets: u64) -> Note {
        note_for_lock_at_origin(lock, name, assets, 800)
    }

    fn note_for_lock_at_origin(lock: &Lock, name: Name, assets: u64, origin_page: u64) -> Note {
        let note_data = NoteData::new(vec![NoteDataEntry::lock(lock.clone())]);
        Note::V1(NoteV1::new(
            BlockHeight(Belt(origin_page)),
            name,
            note_data,
            Nicks(assets as usize),
        ))
    }

    fn single_page_snapshot(notes: Vec<(Name, Note)>) -> Vec<BalanceUpdate> {
        single_page_snapshot_at_height(900, notes)
    }

    fn single_page_snapshot_at_height(height: u64, notes: Vec<(Name, Note)>) -> Vec<BalanceUpdate> {
        vec![BalanceUpdate {
            height: BlockHeight(Belt(height)),
            block_id: Tip5Hash([Belt(91), Belt(92), Belt(93), Belt(94), Belt(95)]),
            notes: Balance(notes),
        }]
    }

    async fn open_services() -> (tempfile::TempDir, Arc<WithdrawalProposalRegistry>) {
        let dir = tempdir().expect("tempdir");
        let projection_path: PathBuf = dir.path().join("withdrawal-local-state.sqlite");
        let projection_store = Arc::new(
            WithdrawalProjectionStore::open(projection_path)
                .await
                .expect("open withdrawal projection store"),
        );
        let registry = Arc::new(
            WithdrawalProposalRegistry::new_without_transaction_body_validator_for_tests(
                projection_store,
            ),
        );
        (dir, registry)
    }

    async fn set_kernel_projection_cursor(
        registry: &WithdrawalProposalRegistry,
        base_next_height: u64,
        nock_next_height: u64,
    ) {
        registry
            .set_kernel_projection_cursor(KernelProjectionCursor::from_position(
                KernelProjectionPosition {
                    base_next_height,
                    base_tip_hash: None,
                    nock_next_height,
                    nock_tip_hash: None,
                },
            ))
            .await
            .expect("set kernel projection cursor");
    }

    async fn load_kernel_projection_cursor(
        registry: &WithdrawalProposalRegistry,
    ) -> KernelProjectionCursor {
        registry
            .load_kernel_projection_cursor()
            .await
            .expect("load kernel projection cursor")
            .expect("kernel projection cursor exists")
    }

    fn activation(nock_next_height: u64) -> WithdrawalActivationCutoff {
        WithdrawalActivationCutoff { nock_next_height }
    }

    async fn fetch_tracked_from_registry(
        registry: &WithdrawalProposalRegistry,
        id: &WithdrawalId,
    ) -> Option<TrackedWithdrawalRequest> {
        registry
            .load_sorted_tracked_withdrawal_requests()
            .await
            .expect("load tracked withdrawals")
            .into_iter()
            .find(|tracked| tracked.id == *id)
    }

    fn planner_config(lock_root: Tip5Hash) -> WithdrawalAssemblyPlannerConfig {
        WithdrawalAssemblyPlannerConfig {
            spend_authority_lock_root: lock_root.clone(),
            spend_authority_spend_condition: SpendCondition::simple_pkh(Tip5Hash([
                Belt(0),
                Belt(1),
                Belt(2),
                Belt(3),
                Belt(4),
            ])),
            refund_lock_root: lock_root,
            refund_note_data: Vec::new(),
            nicks_fee_per_nock: 0,
            blockchain_constants: nockchain_types::BlockchainConstants::default(),
            bythos_phase: 0,
            base_fee: 0,
            input_fee_divisor: 4,
            min_fee: 0,
        }
    }

    #[tokio::test]
    async fn assembly_tick_alerts_when_withdrawal_registration_fails() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        let bridge_status = sample_bridge_status(1);
        let snapshot_service = Arc::new(BridgeNoteSnapshotService::new(
            Arc::new(StaticSnapshotSource { pages: Vec::new() }),
            BridgeOwnedNoteSelectors {
                first_names: vec!["bridge-first".to_string()],
            },
            Duration::from_secs(60),
        ));
        let sequencer = Arc::new(RecordingSequencerPort {
            register_error: Mutex::new(Some("sequencer unavailable".to_string())),
            ..Default::default()
        });
        let context = WithdrawalAssemblyContext {
            kernel: Arc::new(RecordingKernelPort::default()),
            snapshot_service,
            sequencer,
            proposal_registry: registry,
            bridge_status: bridge_status.clone(),
            planner: planner_config(Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)])),
            fallback_policy: WithdrawalFallbackPolicy::default(),
            local_node_id: 0,
            node_pkhs: vec![Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)])],
        };

        let err = withdrawal_assembly_tick_once(&context)
            .await
            .expect_err("registration failure should fail assembly tick");
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
    async fn assembly_tick_does_not_register_released_sequencer_rows() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        let snapshot_service = Arc::new(BridgeNoteSnapshotService::new(
            Arc::new(StaticSnapshotSource { pages: Vec::new() }),
            BridgeOwnedNoteSelectors {
                first_names: vec!["bridge-first".to_string()],
            },
            Duration::from_secs(60),
        ));
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
        let context = WithdrawalAssemblyContext {
            kernel: Arc::new(RecordingKernelPort::default()),
            snapshot_service,
            sequencer: sequencer.clone(),
            proposal_registry: registry,
            bridge_status: sample_bridge_status(1),
            planner: planner_config(Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)])),
            fallback_policy: WithdrawalFallbackPolicy::default(),
            local_node_id: 0,
            node_pkhs: vec![Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)])],
        };

        let outcome = withdrawal_assembly_tick_once(&context)
            .await
            .expect("assembly tick");
        assert_eq!(outcome, WithdrawalAssemblyTickOutcome::Idle);
        assert!(
            sequencer
                .registered
                .lock()
                .expect("registered withdrawals lock")
                .is_empty(),
            "released sequencer rows should not be re-registered"
        );
    }

    #[tokio::test]
    async fn persist_withdrawal_requests_tracks_requests_idempotently() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();

        let tracked =
            persist_withdrawal_requests(vec![request.clone(), request.clone()], registry.as_ref())
                .await
                .expect("track requests");
        assert_eq!(tracked, 1);

        assert_eq!(
            fetch_tracked_from_registry(registry.as_ref(), &request.withdrawal_id())
                .await
                .unwrap()
                .amount,
            request.amount
        );
    }

    #[tokio::test]
    async fn recover_pending_base_block_commit_tracks_requests_and_acks_kernel() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();
        let pending = sample_pending_base_commit(vec![request.clone()]);
        let kernel = RecordingKernelPort {
            pending_base_commit: Mutex::new(Some(pending.clone())),
            ..Default::default()
        };

        let tracked = recover_pending_base_block_commit(&kernel, registry.as_ref())
            .await
            .expect("recover pending base commit");
        assert_eq!(tracked, 1);

        let stored = fetch_tracked_from_registry(registry.as_ref(), &request.withdrawal_id())
            .await
            .expect("request tracked");
        assert_eq!(stored.amount, request.amount);

        let acks = kernel
            .base_commit_acks
            .lock()
            .expect("base commit acks lock");
        assert_eq!(acks.as_slice(), &[pending.ack()]);
        assert!(kernel
            .pending_base_commit
            .lock()
            .expect("pending base commit lock")
            .is_none());
    }

    #[tokio::test]
    async fn recover_pending_base_block_commit_is_idempotent_after_db_commit_before_ack() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("pre-track request");

        let pending = sample_pending_base_commit(vec![request.clone(), request.clone()]);
        let kernel = RecordingKernelPort {
            pending_base_commit: Mutex::new(Some(pending.clone())),
            ..Default::default()
        };

        let tracked = recover_pending_base_block_commit(&kernel, registry.as_ref())
            .await
            .expect("recover pending base commit");
        assert_eq!(tracked, 1);

        {
            let acks = kernel
                .base_commit_acks
                .lock()
                .expect("base commit acks lock");
            assert_eq!(acks.as_slice(), &[pending.ack()]);
        }
        assert_eq!(
            registry
                .load_sorted_tracked_withdrawal_requests()
                .await
                .expect("load tracked requests")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn recover_pending_base_block_commit_acks_empty_batch_without_rows() {
        let (_dir, registry) = open_services().await;
        let pending = sample_pending_base_commit(Vec::new());
        let kernel = RecordingKernelPort {
            pending_base_commit: Mutex::new(Some(pending.clone())),
            ..Default::default()
        };

        let tracked = recover_pending_base_block_commit(&kernel, registry.as_ref())
            .await
            .expect("recover pending base commit");
        assert_eq!(tracked, 0);
        assert_eq!(
            kernel
                .base_commit_acks
                .lock()
                .expect("base commit acks lock")
                .as_slice(),
            &[pending.ack()]
        );
        assert!(registry
            .load_sorted_tracked_withdrawal_requests()
            .await
            .expect("load tracked requests")
            .is_empty());
    }

    #[tokio::test]
    async fn repair_pending_base_block_commit_repairs_without_tracking_or_ack() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();
        let pending = sample_pending_base_commit(vec![request]);
        let kernel = RecordingKernelPort {
            pending_base_commit: Mutex::new(Some(pending.clone())),
            ..Default::default()
        };

        let repaired = repair_pending_base_block_commit(&kernel)
            .await
            .expect("repair pending base commit");
        assert!(repaired);
        assert!(kernel
            .base_commit_acks
            .lock()
            .expect("base commit acks lock")
            .is_empty());
        assert!(kernel
            .pending_base_commit
            .lock()
            .expect("pending base commit lock")
            .is_none());
        assert!(registry
            .load_sorted_tracked_withdrawal_requests()
            .await
            .expect("load tracked requests")
            .is_empty());
    }

    #[tokio::test]
    async fn pending_base_block_before_activation_is_acked_without_tracking() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();
        let pending = sample_pending_base_commit(vec![request.clone()]);
        let kernel = RecordingKernelPort {
            base_next_height: Mutex::new(Some(request.base_batch_end)),
            nock_next_height: Mutex::new(Some(0)),
            pending_base_commit: Mutex::new(Some(pending.clone())),
            ..Default::default()
        };

        let tracked = persist_pending_base_block_withdrawals_after_activation(
            pending.clone(),
            &kernel,
            registry.as_ref(),
            activation(1),
        )
        .await
        .expect("pre-activation pending block should be acked");

        assert_eq!(tracked, 0);
        assert_eq!(
            kernel
                .base_commit_acks
                .lock()
                .expect("base commit acks lock")
                .as_slice(),
            &[pending.ack()]
        );
        assert!(registry
            .load_kernel_projection_cursor()
            .await
            .expect("load cursor")
            .is_none());
        assert!(registry
            .load_sorted_tracked_withdrawal_requests()
            .await
            .expect("load tracked requests")
            .is_empty());
    }

    #[tokio::test]
    async fn persist_pending_base_block_withdrawals_returns_ack_error_after_tracking_requests() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();
        let pending = sample_pending_base_commit(vec![request.clone()]);
        let kernel = RecordingKernelPort {
            pending_base_commit: Mutex::new(Some(pending.clone())),
            base_commit_ack_error: Mutex::new(Some("ack enqueue failed".into())),
            ..Default::default()
        };

        let err =
            persist_pending_base_block_withdrawals(pending.clone(), &kernel, registry.as_ref())
                .await
                .expect_err("ack failure should return error");
        assert!(
            err.to_string().contains("ack enqueue failed"),
            "unexpected error: {err}"
        );
        assert!(
            fetch_tracked_from_registry(registry.as_ref(), &request.withdrawal_id())
                .await
                .is_some()
        );
        assert!(kernel
            .base_commit_acks
            .lock()
            .expect("base commit acks lock")
            .is_empty());
        assert_eq!(
            kernel
                .pending_base_commit
                .lock()
                .expect("pending base commit lock")
                .as_ref()
                .map(PendingBaseBlockCommit::ack),
            Some(pending.ack())
        );
    }

    #[tokio::test]
    async fn restore_tracked_withdrawal_requests_waits_for_activation_cutoff() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();
        let kernel = RecordingKernelPort {
            base_next_height: Mutex::new(Some(request.base_batch_end)),
            nock_next_height: Mutex::new(Some(0)),
            base_history: Mutex::new(vec![request.clone()]),
            ..Default::default()
        };

        let restored =
            restore_tracked_withdrawal_requests(&kernel, registry.as_ref(), activation(1))
                .await
                .expect("restore before activation");

        assert_eq!(restored, 0);
        assert!(registry
            .load_kernel_projection_cursor()
            .await
            .expect("load cursor")
            .is_none());
        assert!(registry
            .load_sorted_tracked_withdrawal_requests()
            .await
            .expect("load tracked requests")
            .is_empty());
    }

    #[tokio::test]
    async fn restore_tracked_withdrawal_requests_initializes_at_current_position_after_activation()
    {
        let (_dir, registry) = open_services().await;
        let historical_request = sample_withdrawal_request_with_seed(1);
        let activation_cutoff = activation(5);
        let kernel_base_next_height = historical_request.base_batch_end.saturating_add(2);
        let kernel = RecordingKernelPort {
            base_next_height: Mutex::new(Some(kernel_base_next_height)),
            nock_next_height: Mutex::new(Some(5)),
            base_history: Mutex::new(vec![historical_request.clone()]),
            ..Default::default()
        };

        let restored =
            restore_tracked_withdrawal_requests(&kernel, registry.as_ref(), activation_cutoff)
                .await
                .expect("restore after activation");

        assert_eq!(restored, 0);
        assert!(fetch_tracked_from_registry(
            registry.as_ref(),
            &historical_request.withdrawal_id()
        )
        .await
        .is_none());
        let cursor = load_kernel_projection_cursor(registry.as_ref()).await;
        assert_eq!(cursor.base_next_height, kernel_base_next_height);
        assert_eq!(cursor.nock_next_height, 5);
    }

    #[tokio::test]
    async fn projection_replay_tail_gap_inserts_missing_request_and_advances_cursor() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();
        let kernel_base_next_height = request.base_batch_end.saturating_add(1);

        // The cursor is one Base batch behind the kernel, so replay should
        // insert the missing request and then advance to the kernel position.
        let kernel = RecordingKernelPort {
            base_next_height: Mutex::new(Some(kernel_base_next_height)),
            nock_next_height: Mutex::new(Some(3)),
            base_history: Mutex::new(vec![request.clone()]),
            ..Default::default()
        };
        set_kernel_projection_cursor(registry.as_ref(), request.base_batch_end, 3).await;

        let restored =
            restore_tracked_withdrawal_requests(&kernel, registry.as_ref(), activation(0))
                .await
                .expect("restore tracked requests");

        // The missing kernel fact becomes durable, and the cursor advances only
        // after the projection write succeeds.
        assert_eq!(restored, 1);
        assert!(
            fetch_tracked_from_registry(registry.as_ref(), &request.withdrawal_id())
                .await
                .is_some()
        );
        let cursor = load_kernel_projection_cursor(registry.as_ref()).await;
        assert_eq!(cursor.base_next_height, kernel_base_next_height);
        assert_eq!(cursor.nock_next_height, 3);
    }

    #[tokio::test]
    async fn projection_replay_overlap_is_idempotent() {
        let (_dir, registry) = open_services().await;
        let existing = sample_withdrawal_request();
        let new_request = sample_withdrawal_request_with_seed(2);

        // Seed an already-projected row and place the cursor just after it.
        // Replay overlap will fetch that row again along with a new tail row.
        registry
            .track_withdrawal_request(&existing)
            .await
            .expect("track existing request");
        set_kernel_projection_cursor(
            registry.as_ref(),
            existing.base_batch_end.saturating_add(1),
            0,
        )
        .await;
        let kernel = RecordingKernelPort {
            base_next_height: Mutex::new(Some(new_request.base_batch_end.saturating_add(1))),
            nock_next_height: Mutex::new(Some(0)),
            base_history: Mutex::new(vec![new_request.clone(), existing.clone()]),
            ..Default::default()
        };

        let restored =
            restore_tracked_withdrawal_requests(&kernel, registry.as_ref(), activation(0))
                .await
                .expect("restore tracked requests");

        // The replayed overlap row is validated in place; only the new row gets
        // the next durable withdrawal nonce.
        assert_eq!(restored, 2);
        let tracked = registry
            .load_sorted_tracked_withdrawal_requests()
            .await
            .expect("load tracked requests");
        assert_eq!(tracked.len(), 2);
        assert_eq!(tracked[0].id, existing.withdrawal_id());
        assert_eq!(tracked[0].withdrawal_nonce, 1);
        assert_eq!(tracked[1].id, new_request.withdrawal_id());
        assert_eq!(tracked[1].withdrawal_nonce, 2);
    }

    #[tokio::test]
    async fn projection_replay_same_batch_ordering_remains_deterministic() {
        let (_dir, registry) = open_services().await;
        let later_event = sample_withdrawal_request();
        let mut earlier_event = sample_withdrawal_request_with_seed(9);

        // Both requests land in the same Base batch. Their order must be driven
        // by base_event_id, not by the order returned from the kernel peek.
        earlier_event.base_batch_end = later_event.base_batch_end;
        earlier_event.base_event_id = sample_base_event_id(0);
        let kernel = RecordingKernelPort {
            base_next_height: Mutex::new(Some(later_event.base_batch_end.saturating_add(1))),
            nock_next_height: Mutex::new(Some(0)),
            base_history: Mutex::new(vec![later_event.clone(), earlier_event.clone()]),
            ..Default::default()
        };
        set_kernel_projection_cursor(registry.as_ref(), later_event.base_batch_end, 0).await;

        restore_tracked_withdrawal_requests(&kernel, registry.as_ref(), activation(0))
            .await
            .expect("restore tracked requests");

        let tracked = registry
            .load_sorted_tracked_withdrawal_requests()
            .await
            .expect("load tracked requests");

        // The lower base_event_id receives the first local withdrawal nonce
        // even though it appeared second in base_history.
        assert_eq!(tracked.len(), 2);
        assert_eq!(tracked[0].id, earlier_event.withdrawal_id());
        assert_eq!(tracked[0].withdrawal_nonce, 1);
        assert_eq!(tracked[1].id, later_event.withdrawal_id());
        assert_eq!(tracked[1].withdrawal_nonce, 2);
    }

    #[tokio::test]
    async fn projection_replay_immutable_mismatch_fails_without_advancing_cursor() {
        let (_dir, registry) = open_services().await;
        let existing = sample_withdrawal_request();
        let mut conflicting = existing.clone();
        conflicting.amount = conflicting.amount.saturating_add(1);
        let new_request = sample_withdrawal_request_with_seed(2);

        // The overlapping replay row has the same withdrawal id as the stored
        // row but a different immutable amount.
        registry
            .track_withdrawal_request(&existing)
            .await
            .expect("track existing request");
        set_kernel_projection_cursor(
            registry.as_ref(),
            existing.base_batch_end.saturating_add(1),
            0,
        )
        .await;
        let original_cursor = load_kernel_projection_cursor(registry.as_ref()).await;
        let kernel = RecordingKernelPort {
            base_next_height: Mutex::new(Some(new_request.base_batch_end.saturating_add(1))),
            nock_next_height: Mutex::new(Some(0)),
            base_history: Mutex::new(vec![conflicting, new_request.clone()]),
            ..Default::default()
        };

        let err = restore_tracked_withdrawal_requests(&kernel, registry.as_ref(), activation(0))
            .await
            .expect_err("conflicting replay should fail");

        // Projection failure rolls back the whole replay transaction: the
        // cursor stays put and the later valid row is not inserted.
        assert!(
            err.to_string()
                .contains("stored withdrawal request does not match kernel request"),
            "unexpected error: {err}"
        );
        let cursor = load_kernel_projection_cursor(registry.as_ref()).await;
        assert_eq!(cursor.base_next_height, original_cursor.base_next_height);
        assert_eq!(cursor.nock_next_height, original_cursor.nock_next_height);
        assert!(registry
            .fetch_live_withdrawal(&new_request.withdrawal_id())
            .await
            .expect("fetch new live withdrawal")
            .is_none());
    }

    #[tokio::test]
    async fn projection_replay_rejects_future_hashchain_request_without_advancing_cursor() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();
        let kernel_base_next_height = request.base_batch_end.saturating_add(1);
        let mut future_request = sample_withdrawal_request_with_seed(2);

        // The replay source must not return facts at or beyond the observed
        // Base next height. Set the bad row exactly at next height so the
        // failing boundary is explicit in the test.
        future_request.base_batch_end = kernel_base_next_height;
        let kernel = RecordingKernelPort {
            base_next_height: Mutex::new(Some(kernel_base_next_height)),
            nock_next_height: Mutex::new(Some(0)),
            base_history: Mutex::new(vec![request.clone(), future_request.clone()]),
            ..Default::default()
        };
        set_kernel_projection_cursor(registry.as_ref(), request.base_batch_end, 0).await;
        let original_cursor = load_kernel_projection_cursor(registry.as_ref()).await;

        let err = restore_tracked_withdrawal_requests(&kernel, registry.as_ref(), activation(0))
            .await
            .expect_err("future hashchain request should fail closed");

        // Future facts are a kernel/projection contract violation, so no rows
        // or cursor movement are allowed.
        assert!(
            err.to_string()
                .contains("beyond observed Base hashchain tip"),
            "unexpected error: {err}"
        );
        let cursor = load_kernel_projection_cursor(registry.as_ref()).await;
        assert_eq!(cursor.base_next_height, original_cursor.base_next_height);
        assert!(registry
            .load_sorted_tracked_withdrawal_requests()
            .await
            .expect("load tracked requests")
            .is_empty());
    }

    #[tokio::test]
    async fn restore_tracked_withdrawal_requests_reloads_active_rows_from_durable_state() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();
        let kernel = RecordingKernelPort {
            base_next_height: Mutex::new(Some(request.base_batch_end.saturating_add(1))),
            nock_next_height: Mutex::new(Some(0)),
            base_history: Mutex::new(vec![request.clone()]),
            ..Default::default()
        };
        set_kernel_projection_cursor(registry.as_ref(), request.base_batch_end, 0).await;

        let restored =
            restore_tracked_withdrawal_requests(&kernel, registry.as_ref(), activation(0))
                .await
                .expect("restore tracked requests");
        assert_eq!(restored, 1);

        let tracked = fetch_tracked_from_registry(registry.as_ref(), &request.withdrawal_id())
            .await
            .expect("tracked request restored");
        assert_eq!(tracked.recipient, request.recipient);
        assert_eq!(tracked.amount, request.amount);
        assert_eq!(tracked.base_batch_end, request.base_batch_end);
    }

    #[tokio::test]
    async fn restore_tracked_withdrawal_requests_reloads_durable_rows_when_up_to_date() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();

        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        let kernel = RecordingKernelPort {
            base_next_height: Mutex::new(Some(request.base_batch_end.saturating_add(1))),
            nock_next_height: Mutex::new(Some(0)),
            ..Default::default()
        };
        set_kernel_projection_cursor(
            registry.as_ref(),
            request.base_batch_end.saturating_add(1),
            0,
        )
        .await;

        let restored =
            restore_tracked_withdrawal_requests(&kernel, registry.as_ref(), activation(0))
                .await
                .expect("restore tracked requests");
        assert_eq!(restored, 1);
        assert_eq!(
            registry
                .load_sorted_tracked_withdrawal_requests()
                .await
                .expect("load tracked requests")
                .into_iter()
                .find(|tracked| tracked.id == request.withdrawal_id())
                .expect("tracked request restored")
                .withdrawal_nonce,
            1
        );
    }

    #[tokio::test]
    async fn restore_tracked_withdrawal_requests_replays_same_batch_gap_inclusively() {
        let (_dir, registry) = open_services().await;
        let existing = sample_withdrawal_request();
        let same_batch_later = NockWithdrawalRequestKernelData {
            as_of: Tip5Hash([Belt(31), Belt(32), Belt(33), Belt(34), Belt(35)]),
            base_event_id: AtomBytes((0..32).map(|offset| offset + 9).collect()),
            recipient: Tip5Hash([Belt(111), Belt(112), Belt(113), Belt(114), Belt(115)]),
            amount: existing.amount.saturating_add(1),
            base_batch_end: existing.base_batch_end,
        };

        registry
            .track_withdrawal_request(&existing)
            .await
            .expect("track existing request");

        let kernel = RecordingKernelPort {
            base_next_height: Mutex::new(Some(existing.base_batch_end.saturating_add(2))),
            nock_next_height: Mutex::new(Some(0)),
            base_history: Mutex::new(vec![existing.clone(), same_batch_later.clone()]),
            ..Default::default()
        };
        set_kernel_projection_cursor(registry.as_ref(), existing.base_batch_end, 0).await;

        let restored =
            restore_tracked_withdrawal_requests(&kernel, registry.as_ref(), activation(0))
                .await
                .expect("restore tracked requests");
        assert_eq!(restored, 2);

        let tracked = registry
            .load_sorted_tracked_withdrawal_requests()
            .await
            .expect("load tracked requests");
        assert_eq!(tracked.len(), 2);
        assert_eq!(tracked[0].id, existing.withdrawal_id());
        assert_eq!(tracked[0].withdrawal_nonce, 1);
        assert_eq!(tracked[1].id, same_batch_later.withdrawal_id());
        assert_eq!(tracked[1].withdrawal_nonce, 2);
    }

    #[tokio::test]
    async fn boot_restore_matches_live_ingestion_nonce_assignment_for_same_request_set() {
        let (_live_dir, live_validator) = open_services().await;
        let (_restore_dir, restored_validator) = open_services().await;
        let existing = sample_withdrawal_request();
        let same_batch_later = NockWithdrawalRequestKernelData {
            as_of: Tip5Hash([Belt(31), Belt(32), Belt(33), Belt(34), Belt(35)]),
            base_event_id: AtomBytes((0..32).map(|offset| offset + 9).collect()),
            recipient: Tip5Hash([Belt(111), Belt(112), Belt(113), Belt(114), Belt(115)]),
            amount: existing.amount.saturating_add(1),
            base_batch_end: existing.base_batch_end,
        };
        let later_batch = sample_withdrawal_request_with_seed(3);

        persist_withdrawal_requests(
            vec![later_batch.clone(), same_batch_later.clone(), existing.clone()],
            live_validator.as_ref(),
        )
        .await
        .expect("persist live request batch");

        let kernel = RecordingKernelPort {
            base_next_height: Mutex::new(Some(later_batch.base_batch_end.saturating_add(1))),
            nock_next_height: Mutex::new(Some(0)),
            base_history: Mutex::new(vec![
                later_batch.clone(),
                same_batch_later.clone(),
                existing.clone(),
            ]),
            ..Default::default()
        };
        set_kernel_projection_cursor(restored_validator.as_ref(), existing.base_batch_end, 0).await;

        restore_tracked_withdrawal_requests(&kernel, restored_validator.as_ref(), activation(0))
            .await
            .expect("restore tracked requests");

        let live_nonces = live_validator
            .load_sorted_tracked_withdrawal_requests()
            .await
            .expect("load live tracked requests")
            .into_iter()
            .map(|tracked| (tracked.id, tracked.withdrawal_nonce))
            .collect::<Vec<_>>();
        let restored_nonces = restored_validator
            .load_sorted_tracked_withdrawal_requests()
            .await
            .expect("load restored tracked requests")
            .into_iter()
            .map(|tracked| (tracked.id, tracked.withdrawal_nonce))
            .collect::<Vec<_>>();
        assert_eq!(restored_nonces, live_nonces);
    }

    #[tokio::test]
    async fn restart_restore_preserves_registration_and_next_pending_ordering() {
        let (_dir, registry) = open_services().await;
        let existing = sample_withdrawal_request();
        let same_batch_later = NockWithdrawalRequestKernelData {
            as_of: Tip5Hash([Belt(31), Belt(32), Belt(33), Belt(34), Belt(35)]),
            base_event_id: AtomBytes((0..32).map(|offset| offset + 9).collect()),
            recipient: Tip5Hash([Belt(111), Belt(112), Belt(113), Belt(114), Belt(115)]),
            amount: existing.amount.saturating_add(1),
            base_batch_end: existing.base_batch_end,
        };
        let later_batch = sample_withdrawal_request_with_seed(3);
        persist_withdrawal_requests(
            vec![later_batch.clone(), same_batch_later.clone(), existing.clone()],
            registry.as_ref(),
        )
        .await
        .expect("persist live request batch");

        let empty_snapshot_service = || {
            Arc::new(BridgeNoteSnapshotService::new(
                Arc::new(StaticSnapshotSource { pages: vec![] }),
                BridgeOwnedNoteSelectors {
                    first_names: vec!["bridge-first".to_string()],
                },
                Duration::from_secs(60),
            ))
        };
        let live_kernel = Arc::new(RecordingKernelPort::default());
        let live_sequencer = Arc::new(RecordingSequencerPort::default());
        let live_context = WithdrawalAssemblyContext {
            kernel: live_kernel,
            snapshot_service: empty_snapshot_service(),
            sequencer: live_sequencer.clone(),
            proposal_registry: registry.clone(),
            bridge_status: sample_bridge_status(1),
            planner: planner_config(zero_tip5_hash()),
            fallback_policy: WithdrawalFallbackPolicy::default(),
            local_node_id: 0,
            node_pkhs: vec![Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)])],
        };

        let live_stageable = next_stageable_withdrawal_request(&live_context)
            .await
            .expect("live next stageable")
            .expect("live next stageable request");
        let live_registered = live_sequencer
            .registered
            .lock()
            .expect("live registered withdrawals lock")
            .clone();

        let restore_kernel = RecordingKernelPort {
            base_next_height: Mutex::new(Some(later_batch.base_batch_end.saturating_add(1))),
            nock_next_height: Mutex::new(Some(0)),
            ..Default::default()
        };
        set_kernel_projection_cursor(
            registry.as_ref(),
            later_batch.base_batch_end.saturating_add(1),
            0,
        )
        .await;
        restore_tracked_withdrawal_requests(&restore_kernel, registry.as_ref(), activation(0))
            .await
            .expect("restore tracked requests");

        let restored_kernel = Arc::new(RecordingKernelPort::default());
        let restored_sequencer = Arc::new(RecordingSequencerPort::default());
        let restored_context = WithdrawalAssemblyContext {
            kernel: restored_kernel,
            snapshot_service: empty_snapshot_service(),
            sequencer: restored_sequencer.clone(),
            proposal_registry: registry,
            bridge_status: sample_bridge_status(1),
            planner: planner_config(zero_tip5_hash()),
            fallback_policy: WithdrawalFallbackPolicy::default(),
            local_node_id: 0,
            node_pkhs: vec![Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)])],
        };

        let restored_stageable = next_stageable_withdrawal_request(&restored_context)
            .await
            .expect("restored next stageable")
            .expect("restored next stageable request");
        let restored_registered = restored_sequencer
            .registered
            .lock()
            .expect("restored registered withdrawals lock")
            .clone();

        assert_eq!(restored_registered, live_registered);
        assert_eq!(restored_stageable.tracked.id, live_stageable.tracked.id);
        assert_eq!(
            restored_stageable.tracked.withdrawal_nonce,
            live_stageable.tracked.withdrawal_nonce
        );
    }

    #[tokio::test]
    async fn restore_tracked_withdrawal_requests_skips_durably_confirmed_rows() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();
        let proposal = WithdrawalProposalData {
            id: request.withdrawal_id(),
            recipient: request.recipient.clone(),
            amount: request.amount.saturating_sub(1),
            burned_amount: request.amount,
            base_batch_end: request.base_batch_end,
            epoch: 0,
            snapshot: WithdrawalSnapshot {
                height: 900,
                block_id: Tip5Hash([Belt(91), Belt(92), Belt(93), Belt(94), Belt(95)]),
            },
            selected_inputs: vec![sample_name(10)],
            transaction: sample_transaction(),
        };

        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");
        registry
            .validate_and_cache_prepared(&proposal)
            .await
            .expect("persist proposal");
        registry
            .mark_proposal_confirmed(
                &proposal,
                123,
                Tip5Hash([Belt(801), Belt(802), Belt(803), Belt(804), Belt(805)]),
            )
            .await
            .expect("mark confirmed");

        let kernel = RecordingKernelPort {
            base_next_height: Mutex::new(Some(request.base_batch_end.saturating_add(1))),
            nock_next_height: Mutex::new(Some(0)),
            base_history: Mutex::new(vec![request.clone()]),
            ..Default::default()
        };
        set_kernel_projection_cursor(registry.as_ref(), request.base_batch_end, 0).await;

        let restored =
            restore_tracked_withdrawal_requests(&kernel, registry.as_ref(), activation(0))
                .await
                .expect("restore tracked requests");
        assert_eq!(restored, 0);
        assert!(
            registry
                .fetch_live_withdrawal(&request.withdrawal_id())
                .await
                .expect("fetch live withdrawal")
                .is_none(),
            "confirmed durable rows should not be restored from kernel unsettled boot seeding"
        );
    }

    #[tokio::test]
    async fn withdrawal_assembly_tick_selects_unreserved_inputs_and_pokes_kernel() {
        let (_dir, registry) = open_services().await;
        let mut request = sample_withdrawal_request();
        request.amount = NICKS_PER_NOCK * 2;
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");

        let spend_condition = SpendCondition::simple_pkh(Tip5Hash([
            Belt(500),
            Belt(501),
            Belt(502),
            Belt(503),
            Belt(504),
        ]));
        let bridge_lock = Lock::SpendCondition(spend_condition.clone());
        let bridge_lock_root = bridge_lock.hash().expect("bridge lock root");
        let reserved_name = Name::new(
            spend_condition
                .first_name()
                .expect("first name")
                .into_hash(),
            Tip5Hash([Belt(601), Belt(602), Belt(603), Belt(604), Belt(605)]),
        );
        let spendable_name = Name::new(
            spend_condition
                .first_name()
                .expect("first name")
                .into_hash(),
            Tip5Hash([Belt(701), Belt(702), Belt(703), Belt(704), Belt(705)]),
        );
        let pages = single_page_snapshot(vec![
            (
                reserved_name.clone(),
                note_for_lock(&bridge_lock, reserved_name.clone(), 5),
            ),
            (
                spendable_name.clone(),
                note_for_lock(&bridge_lock, spendable_name.clone(), 140_000),
            ),
        ]);
        let snapshot_service = Arc::new(BridgeNoteSnapshotService::new(
            Arc::new(StaticSnapshotSource { pages }),
            BridgeOwnedNoteSelectors {
                first_names: vec!["bridge-first".to_string()],
            },
            Duration::from_secs(60),
        ));
        snapshot_service.refresh().await.expect("refresh snapshot");

        let kernel = Arc::new(RecordingKernelPort::default());
        let sequencer = Arc::new(RecordingSequencerPort::default());
        sequencer
            .reserved_inputs
            .lock()
            .expect("reserved inputs lock")
            .push(reserved_name.clone());
        let mut planner = planner_config(bridge_lock_root);
        planner.nicks_fee_per_nock = 195;
        planner.base_fee = 0;
        planner.min_fee = 1;
        let withdrawal_fee = compute_bridge_fee(request.amount, planner.nicks_fee_per_nock);
        let context = WithdrawalAssemblyContext {
            kernel: kernel.clone(),
            snapshot_service,
            sequencer,
            proposal_registry: registry,
            bridge_status: sample_bridge_status(1),
            planner,
            fallback_policy: WithdrawalFallbackPolicy::default(),
            local_node_id: 0,
            node_pkhs: vec![Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)])],
        };

        let outcome = withdrawal_assembly_tick_once(&context)
            .await
            .expect("assembly tick");
        assert!(matches!(
            outcome,
            WithdrawalAssemblyTickOutcome::RequestedBuild {
                selected_inputs: 1,
                ..
            }
        ));

        let requests = kernel.requests.lock().expect("requests lock");
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0]
                .selected_notes
                .iter()
                .map(|note| note.name.clone())
                .collect::<Vec<_>>(),
            vec![spendable_name]
        );
        assert!(requests[0].fee > 0);
        assert_eq!(
            requests[0]
                .amount
                .saturating_add(requests[0].fee)
                .saturating_add(withdrawal_fee),
            requests[0].burned_amount
        );
        assert_eq!(requests[0].burned_amount, request.amount);
        assert_eq!(requests[0].epoch, 0);
        assert_eq!(requests[0].snapshot.height, 900);
    }

    #[tokio::test]
    async fn withdrawal_assembly_tick_waits_for_safe_origin_inputs() {
        const SNAPSHOT_HEIGHT: u64 = 900;
        const CONFIRMATION_DEPTH: u64 = 5;
        const UNSAFE_NOTE_ORIGIN: u64 = SNAPSHOT_HEIGHT - CONFIRMATION_DEPTH + 1;

        let (_dir, registry) = open_services().await;
        let mut request = sample_withdrawal_request();
        request.amount = NICKS_PER_NOCK * 2;
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");

        let spend_condition = SpendCondition::simple_pkh(Tip5Hash([
            Belt(500),
            Belt(501),
            Belt(502),
            Belt(503),
            Belt(504),
        ]));
        let bridge_lock = Lock::SpendCondition(spend_condition.clone());
        let bridge_lock_root = bridge_lock.hash().expect("bridge lock root");
        let unsafe_name = Name::new(
            spend_condition
                .first_name()
                .expect("first name")
                .into_hash(),
            Tip5Hash([Belt(901), Belt(902), Belt(903), Belt(904), Belt(905)]),
        );
        let pages = single_page_snapshot_at_height(
            SNAPSHOT_HEIGHT,
            vec![(
                unsafe_name.clone(),
                // The note has enough value for the withdrawal, but its origin is
                // one block newer than the safe tip below, so it must not be
                // selected yet.
                note_for_lock_at_origin(
                    &bridge_lock,
                    unsafe_name.clone(),
                    140_000,
                    UNSAFE_NOTE_ORIGIN,
                ),
            )],
        );
        let snapshot_service = Arc::new(
            BridgeNoteSnapshotService::new(
                Arc::new(StaticSnapshotSource { pages }),
                BridgeOwnedNoteSelectors {
                    first_names: vec!["bridge-first".to_string()],
                },
                Duration::from_secs(60),
            )
            // With snapshot height 900 and depth 5, the spendable safe tip is
            // 895, so the origin-896 note is excluded.
            .with_nockchain_confirmation_depth(CONFIRMATION_DEPTH),
        );
        snapshot_service.refresh().await.expect("refresh snapshot");

        let kernel = Arc::new(RecordingKernelPort::default());
        let sequencer = Arc::new(RecordingSequencerPort::default());
        let mut planner = planner_config(bridge_lock_root);
        planner.nicks_fee_per_nock = 195;
        planner.base_fee = 0;
        planner.min_fee = 1;
        let context = WithdrawalAssemblyContext {
            kernel: kernel.clone(),
            snapshot_service,
            sequencer,
            proposal_registry: registry.clone(),
            bridge_status: sample_bridge_status(1),
            planner,
            fallback_policy: WithdrawalFallbackPolicy::default(),
            local_node_id: 0,
            node_pkhs: vec![Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)])],
        };

        let outcome = withdrawal_assembly_tick_once(&context)
            .await
            .expect("assembly tick should wait for confirmed spendable notes");

        assert_eq!(outcome, WithdrawalAssemblyTickOutcome::Idle);
        assert!(
            registry
                .fetch_live_withdrawal(&request.withdrawal_id())
                .await
                .expect("fetch live withdrawal after idle tick")
                .is_none(),
            "waiting for safe notes should release the assembly lock"
        );
        assert!(
            kernel.requests.lock().expect("requests lock").is_empty(),
            "unsafe-origin note should not be sent to the kernel"
        );
    }

    #[tokio::test]
    async fn withdrawal_assembly_tick_succeeds_after_reserved_input_picture_changes() {
        let (_dir, registry) = open_services().await;
        let mut request = sample_withdrawal_request();
        request.amount = NICKS_PER_NOCK * 2;
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");

        let spend_condition = SpendCondition::simple_pkh(Tip5Hash([
            Belt(500),
            Belt(501),
            Belt(502),
            Belt(503),
            Belt(504),
        ]));
        let bridge_lock = Lock::SpendCondition(spend_condition.clone());
        let bridge_lock_root = bridge_lock.hash().expect("bridge lock root");
        let only_name = Name::new(
            spend_condition
                .first_name()
                .expect("first name")
                .into_hash(),
            Tip5Hash([Belt(801), Belt(802), Belt(803), Belt(804), Belt(805)]),
        );
        // The note is otherwise usable: it has enough value and the default
        // note origin is older than the snapshot. The first tick idles only
        // because the sequencer currently reports this input as reserved.
        let pages = single_page_snapshot(vec![(
            only_name.clone(),
            note_for_lock(&bridge_lock, only_name.clone(), 140_000),
        )]);
        let snapshot_service = Arc::new(BridgeNoteSnapshotService::new(
            Arc::new(StaticSnapshotSource { pages }),
            BridgeOwnedNoteSelectors {
                first_names: vec!["bridge-first".to_string()],
            },
            Duration::from_secs(60),
        ));
        snapshot_service.refresh().await.expect("refresh snapshot");

        let kernel = Arc::new(RecordingKernelPort::default());
        let sequencer = Arc::new(RecordingSequencerPort::default());
        sequencer
            .reserved_inputs
            .lock()
            .expect("reserved inputs lock")
            .push(only_name.clone());
        let mut planner = planner_config(bridge_lock_root);
        planner.nicks_fee_per_nock = 195;
        planner.base_fee = 0;
        planner.min_fee = 1;
        let context = WithdrawalAssemblyContext {
            kernel: kernel.clone(),
            snapshot_service,
            sequencer: sequencer.clone(),
            proposal_registry: registry.clone(),
            bridge_status: sample_bridge_status(1),
            planner,
            fallback_policy: WithdrawalFallbackPolicy::default(),
            local_node_id: 0,
            node_pkhs: vec![Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)])],
        };

        let outcome = withdrawal_assembly_tick_once(&context)
            .await
            .expect("assembly should idle while only input is reserved");
        assert_eq!(outcome, WithdrawalAssemblyTickOutcome::Idle);
        assert!(
            registry
                .fetch_live_withdrawal(&request.withdrawal_id())
                .await
                .expect("fetch live withdrawal after idle tick")
                .is_none(),
            "idle tick should release the assembly lock"
        );
        assert!(
            kernel.requests.lock().expect("requests lock").is_empty(),
            "idle tick should not poke the kernel"
        );

        sequencer
            .reserved_inputs
            .lock()
            .expect("reserved inputs lock")
            .clear();

        // Once the sequencer reservation picture changes, the same snapshot
        // can be used to build the withdrawal.
        let outcome = withdrawal_assembly_tick_once(&context)
            .await
            .expect("assembly tick after reserved-input refresh");
        assert!(matches!(
            outcome,
            WithdrawalAssemblyTickOutcome::RequestedBuild {
                id,
                epoch: 0,
                selected_inputs: 1,
            } if id == request.withdrawal_id()
        ));
        let requests = kernel.requests.lock().expect("requests lock");
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0]
                .selected_notes
                .iter()
                .map(|note| note.name.clone())
                .collect::<Vec<_>>(),
            vec![only_name]
        );
    }

    #[tokio::test]
    async fn withdrawal_assembly_tick_uses_tracked_state_without_runtime_unsettled_peek() {
        let (_dir, registry) = open_services().await;
        let mut request = sample_withdrawal_request();
        request.amount = NICKS_PER_NOCK * 2;
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");

        let spend_condition = SpendCondition::simple_pkh(Tip5Hash([
            Belt(500),
            Belt(501),
            Belt(502),
            Belt(503),
            Belt(504),
        ]));
        let bridge_lock = Lock::SpendCondition(spend_condition.clone());
        let bridge_lock_root = bridge_lock.hash().expect("bridge lock root");
        let spendable_name = Name::new(
            spend_condition
                .first_name()
                .expect("first name")
                .into_hash(),
            Tip5Hash([Belt(701), Belt(702), Belt(703), Belt(704), Belt(705)]),
        );
        let pages = single_page_snapshot(vec![(
            spendable_name.clone(),
            note_for_lock(&bridge_lock, spendable_name.clone(), 140_000),
        )]);
        let snapshot_service = Arc::new(BridgeNoteSnapshotService::new(
            Arc::new(StaticSnapshotSource { pages }),
            BridgeOwnedNoteSelectors {
                first_names: vec!["bridge-first".to_string()],
            },
            Duration::from_secs(60),
        ));
        snapshot_service.refresh().await.expect("refresh snapshot");

        let kernel = Arc::new(RecordingKernelPort::default());
        let mut planner = planner_config(bridge_lock_root);
        planner.nicks_fee_per_nock = 195;
        planner.base_fee = 0;
        planner.min_fee = 1;
        let context = WithdrawalAssemblyContext {
            kernel: kernel.clone(),
            snapshot_service,
            sequencer: Arc::new(RecordingSequencerPort::default()),
            proposal_registry: registry,
            bridge_status: sample_bridge_status(1),
            planner,
            fallback_policy: WithdrawalFallbackPolicy::default(),
            local_node_id: 0,
            node_pkhs: vec![Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)])],
        };

        let outcome = withdrawal_assembly_tick_once(&context)
            .await
            .expect("assembly tick");
        assert!(matches!(
            outcome,
            WithdrawalAssemblyTickOutcome::RequestedBuild {
                selected_inputs: 1,
                ..
            }
        ));
        assert_eq!(kernel.requests.lock().expect("requests lock").len(), 1);
    }

    #[tokio::test]
    async fn withdrawal_assembly_tick_respects_deterministic_turn_taking() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");

        let snapshot_service = Arc::new(BridgeNoteSnapshotService::new(
            Arc::new(StaticSnapshotSource { pages: vec![] }),
            BridgeOwnedNoteSelectors {
                first_names: vec!["bridge-first".to_string()],
            },
            Duration::from_secs(60),
        ));
        let kernel = Arc::new(RecordingKernelPort::default());
        let context = WithdrawalAssemblyContext {
            kernel: kernel.clone(),
            snapshot_service,
            sequencer: Arc::new(RecordingSequencerPort::default()),
            proposal_registry: registry,
            bridge_status: sample_bridge_status(1),
            planner: planner_config(zero_tip5_hash()),
            fallback_policy: WithdrawalFallbackPolicy::default(),
            local_node_id: 1,
            node_pkhs: vec![
                Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
                Tip5Hash([Belt(6), Belt(7), Belt(8), Belt(9), Belt(10)]),
            ],
        };

        let outcome = withdrawal_assembly_tick_once(&context)
            .await
            .expect("assembly tick");
        assert_eq!(outcome, WithdrawalAssemblyTickOutcome::Idle);
        assert!(kernel.requests.lock().expect("requests lock").is_empty());
    }

    #[tokio::test]
    async fn withdrawal_assembly_tick_skips_requests_already_submitted_at_sequencer() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");

        let snapshot_service = Arc::new(BridgeNoteSnapshotService::new(
            Arc::new(StaticSnapshotSource { pages: vec![] }),
            BridgeOwnedNoteSelectors {
                first_names: vec!["bridge-first".to_string()],
            },
            Duration::from_secs(60),
        ));
        let kernel = Arc::new(RecordingKernelPort::default());
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
                    state: "mempool_accepted".to_string(),
                    proposal_hash: String::new(),
                    authorized_transaction_name: "submitted-tx".to_string(),
                    withdrawal_nonce: 1,
                    handoff_index: 0,
                    turn_started_base_height: None,

                    current_confirmed_base_height: None,

                    handoff_window_blocks: 0,

                    blocks_until_handoff: None,
                },
            );
        let context = WithdrawalAssemblyContext {
            kernel: kernel.clone(),
            snapshot_service,
            sequencer,
            proposal_registry: registry,
            bridge_status: sample_bridge_status(1),
            planner: planner_config(zero_tip5_hash()),
            fallback_policy: WithdrawalFallbackPolicy::default(),
            local_node_id: 0,
            node_pkhs: vec![Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)])],
        };

        let outcome = withdrawal_assembly_tick_once(&context)
            .await
            .expect("assembly tick");
        assert_eq!(outcome, WithdrawalAssemblyTickOutcome::Idle);
        assert!(kernel.requests.lock().expect("requests lock").is_empty());
    }

    #[tokio::test]
    async fn withdrawal_assembly_tick_blocks_later_request_while_prior_nonce_authorized() {
        let (_dir, registry) = open_services().await;
        let earlier = sample_withdrawal_request_with_seed(1);
        let later = sample_withdrawal_request_with_seed(2);
        registry
            .track_withdrawal_request(&earlier)
            .await
            .expect("track earlier request");
        registry
            .track_withdrawal_request(&later)
            .await
            .expect("track later request");

        let snapshot_service = Arc::new(BridgeNoteSnapshotService::new(
            Arc::new(StaticSnapshotSource { pages: vec![] }),
            BridgeOwnedNoteSelectors {
                first_names: vec!["bridge-first".to_string()],
            },
            Duration::from_secs(60),
        ));
        let kernel = Arc::new(RecordingKernelPort::default());
        let sequencer = Arc::new(RecordingSequencerPort::default());
        sequencer
            .statuses
            .lock()
            .expect("sequencer statuses lock")
            .insert(
                earlier.withdrawal_id(),
                SequencedWithdrawalStatusResponse {
                    found: true,
                    current_epoch: 0,
                    state: "authorized".to_string(),
                    proposal_hash: String::new(),
                    authorized_transaction_name: "stalled-tx".to_string(),
                    withdrawal_nonce: 1,
                    handoff_index: 0,
                    turn_started_base_height: None,

                    current_confirmed_base_height: None,

                    handoff_window_blocks: 0,

                    blocks_until_handoff: None,
                },
            );
        let context = WithdrawalAssemblyContext {
            kernel: kernel.clone(),
            snapshot_service,
            sequencer,
            proposal_registry: registry,
            bridge_status: sample_bridge_status(1),
            planner: planner_config(zero_tip5_hash()),
            fallback_policy: WithdrawalFallbackPolicy::default(),
            local_node_id: 0,
            node_pkhs: vec![Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)])],
        };

        let outcome = withdrawal_assembly_tick_once(&context)
            .await
            .expect("assembly tick");
        assert_eq!(outcome, WithdrawalAssemblyTickOutcome::Idle);
        assert!(kernel.requests.lock().expect("requests lock").is_empty());
    }

    #[tokio::test]
    async fn withdrawal_assembly_tick_stages_later_request_after_prior_nonce_mempool_acceptance() {
        let (_dir, registry) = open_services().await;
        let mut earlier = sample_withdrawal_request_with_seed(1);
        let mut later = sample_withdrawal_request_with_seed(2);
        earlier.amount = NICKS_PER_NOCK * 2;
        later.amount = NICKS_PER_NOCK * 2;
        registry
            .track_withdrawal_request(&earlier)
            .await
            .expect("track earlier request");
        registry
            .track_withdrawal_request(&later)
            .await
            .expect("track later request");

        let spend_condition = SpendCondition::simple_pkh(Tip5Hash([
            Belt(500),
            Belt(501),
            Belt(502),
            Belt(503),
            Belt(504),
        ]));
        let bridge_lock = Lock::SpendCondition(spend_condition.clone());
        let bridge_lock_root = bridge_lock.hash().expect("bridge lock root");
        let spendable_name = Name::new(
            spend_condition
                .first_name()
                .expect("first name")
                .into_hash(),
            Tip5Hash([Belt(701), Belt(702), Belt(703), Belt(704), Belt(705)]),
        );
        let pages = single_page_snapshot(vec![(
            spendable_name.clone(),
            note_for_lock(&bridge_lock, spendable_name.clone(), 140_000),
        )]);
        let snapshot_service = Arc::new(BridgeNoteSnapshotService::new(
            Arc::new(StaticSnapshotSource { pages }),
            BridgeOwnedNoteSelectors {
                first_names: vec!["bridge-first".to_string()],
            },
            Duration::from_secs(60),
        ));
        snapshot_service.refresh().await.expect("refresh snapshot");

        let node_pkhs = vec![
            Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            Tip5Hash([Belt(6), Belt(7), Belt(8), Belt(9), Belt(10)]),
        ];
        let local_node_id =
            scheduled_assembler_node_id(&later.withdrawal_id(), 0, &node_pkhs).expect("assembler");
        let kernel = Arc::new(RecordingKernelPort::default());
        let sequencer = Arc::new(RecordingSequencerPort::default());
        sequencer
            .statuses
            .lock()
            .expect("sequencer statuses lock")
            .insert(
                earlier.withdrawal_id(),
                SequencedWithdrawalStatusResponse {
                    found: true,
                    current_epoch: 0,
                    state: "mempool_accepted".to_string(),
                    proposal_hash: String::new(),
                    authorized_transaction_name: "accepted-tx".to_string(),
                    withdrawal_nonce: 1,
                    handoff_index: 0,
                    turn_started_base_height: None,

                    current_confirmed_base_height: None,

                    handoff_window_blocks: 0,

                    blocks_until_handoff: None,
                },
            );
        let mut planner = planner_config(bridge_lock_root);
        planner.nicks_fee_per_nock = 195;
        planner.base_fee = 0;
        planner.min_fee = 1;
        let context = WithdrawalAssemblyContext {
            kernel: kernel.clone(),
            snapshot_service,
            sequencer,
            proposal_registry: registry,
            bridge_status: sample_bridge_status(1),
            planner,
            fallback_policy: WithdrawalFallbackPolicy::default(),
            local_node_id,
            node_pkhs,
        };

        let outcome = withdrawal_assembly_tick_once(&context)
            .await
            .expect("assembly tick");
        assert!(matches!(
            outcome,
            WithdrawalAssemblyTickOutcome::RequestedBuild {
                id,
                epoch: 0,
                selected_inputs: 1,
            } if id == later.withdrawal_id()
        ));
        assert_eq!(kernel.requests.lock().expect("requests lock").len(), 1);
    }

    #[tokio::test]
    async fn stale_assembling_attempt_hands_off_to_next_turn_without_bumping_epoch() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");

        let spend_condition = SpendCondition::simple_pkh(Tip5Hash([
            Belt(500),
            Belt(501),
            Belt(502),
            Belt(503),
            Belt(504),
        ]));
        let bridge_lock = Lock::SpendCondition(spend_condition.clone());
        let bridge_lock_root = bridge_lock.hash().expect("bridge lock root");
        let spendable_name = Name::new(
            spend_condition
                .first_name()
                .expect("first name")
                .into_hash(),
            Tip5Hash([Belt(701), Belt(702), Belt(703), Belt(704), Belt(705)]),
        );
        let pages = single_page_snapshot(vec![(
            spendable_name.clone(),
            note_for_lock(&bridge_lock, spendable_name.clone(), 20),
        )]);
        let snapshot_service = Arc::new(BridgeNoteSnapshotService::new(
            Arc::new(StaticSnapshotSource { pages }),
            BridgeOwnedNoteSelectors {
                first_names: vec!["bridge-first".to_string()],
            },
            Duration::from_secs(60),
        ));
        snapshot_service.refresh().await.expect("refresh snapshot");

        let node_pkhs = vec![
            Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            Tip5Hash([Belt(6), Belt(7), Belt(8), Belt(9), Belt(10)]),
        ];
        let first_owner = scheduled_assembler_node_id(&request.withdrawal_id(), 0, &node_pkhs)
            .expect("initial epoch owner");
        let next_owner =
            scheduled_assembler_turn_node_id(&request.withdrawal_id(), 0, 1, &node_pkhs)
                .expect("handoff owner");
        assert_ne!(first_owner, next_owner);

        let acquired = registry
            .acquire_withdrawal_assembly(&request.withdrawal_id(), 0, 1)
            .await
            .expect("acquire stale assembly");
        assert_eq!(acquired, AcquireWithdrawalAssemblyOutcome::Acquired);

        let kernel = Arc::new(RecordingKernelPort::default());
        let sequencer = Arc::new(RecordingSequencerPort::default());
        let mut planner = planner_config(bridge_lock_root);
        planner.base_fee = 0;
        planner.min_fee = 1;

        let first_context = WithdrawalAssemblyContext {
            kernel: kernel.clone(),
            snapshot_service: snapshot_service.clone(),
            sequencer: sequencer.clone(),
            proposal_registry: registry.clone(),
            bridge_status: sample_bridge_status(1),
            planner: planner.clone(),
            fallback_policy: WithdrawalFallbackPolicy {
                assembly_timeout_blocks: 0,
                submission_timeout_blocks: 30,
            },
            local_node_id: first_owner,
            node_pkhs: node_pkhs.clone(),
        };

        let outcome = withdrawal_assembly_tick_once(&first_context)
            .await
            .expect("stale owner tick");
        assert_eq!(outcome, WithdrawalAssemblyTickOutcome::Idle);
        assert!(registry
            .fetch_live_withdrawal(&request.withdrawal_id())
            .await
            .expect("fetch live withdrawal after stale handoff")
            .is_none());
        let status = sequencer
            .get_sequenced_withdrawal_status(&request.withdrawal_id())
            .await
            .expect("sequencer status");
        assert!(status.found);
        assert_eq!(status.current_epoch, 0);
        assert_eq!(status.state, WithdrawalState::Pending.as_str());
        assert_eq!(status.handoff_index, 1);

        let next_context = WithdrawalAssemblyContext {
            kernel: kernel.clone(),
            snapshot_service,
            sequencer,
            proposal_registry: registry,
            bridge_status: sample_bridge_status(1),
            planner,
            fallback_policy: WithdrawalFallbackPolicy {
                assembly_timeout_blocks: 0,
                submission_timeout_blocks: 30,
            },
            local_node_id: next_owner,
            node_pkhs,
        };
        let next_outcome = withdrawal_assembly_tick_once(&next_context)
            .await
            .expect("handoff owner tick");
        assert!(matches!(
            next_outcome,
            WithdrawalAssemblyTickOutcome::RequestedBuild {
                id,
                epoch: 0,
                selected_inputs: 1,
            } if id == request.withdrawal_id()
        ));
    }

    #[tokio::test]
    async fn withdrawal_assembly_tick_expires_stale_prepared_attempt_and_reassembles() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");

        let spend_condition = SpendCondition::simple_pkh(Tip5Hash([
            Belt(500),
            Belt(501),
            Belt(502),
            Belt(503),
            Belt(504),
        ]));
        let bridge_lock = Lock::SpendCondition(spend_condition.clone());
        let bridge_lock_root = bridge_lock.hash().expect("bridge lock root");
        let spendable_name = Name::new(
            spend_condition
                .first_name()
                .expect("first name")
                .into_hash(),
            Tip5Hash([Belt(701), Belt(702), Belt(703), Belt(704), Belt(705)]),
        );
        let pages = single_page_snapshot(vec![(
            spendable_name.clone(),
            note_for_lock(&bridge_lock, spendable_name.clone(), 20),
        )]);
        let snapshot_service = Arc::new(BridgeNoteSnapshotService::new(
            Arc::new(StaticSnapshotSource { pages }),
            BridgeOwnedNoteSelectors {
                first_names: vec!["bridge-first".to_string()],
            },
            Duration::from_secs(60),
        ));
        snapshot_service.refresh().await.expect("refresh snapshot");

        let proposal_epoch_0 = WithdrawalProposalData {
            id: request.withdrawal_id(),
            recipient: request.recipient.clone(),
            amount: request.amount.saturating_sub(1),
            burned_amount: request.amount,
            base_batch_end: request.base_batch_end,
            epoch: 0,
            snapshot: WithdrawalSnapshot {
                height: 900,
                block_id: Tip5Hash([Belt(91), Belt(92), Belt(93), Belt(94), Belt(95)]),
            },
            selected_inputs: vec![spendable_name.clone()],
            transaction: sample_transaction(),
        };
        let acquired = registry
            .acquire_withdrawal_assembly(&request.withdrawal_id(), 0, 1)
            .await
            .expect("acquire epoch 0 assembly");
        assert_eq!(acquired, AcquireWithdrawalAssemblyOutcome::Acquired);
        registry
            .validate_and_cache_prepared(&proposal_epoch_0)
            .await
            .expect("persist epoch 0 proposal");
        registry
            .mark_proposal_prepared(&proposal_epoch_0)
            .await
            .expect("mark stale prepared");
        let stale_live = registry
            .fetch_live_withdrawal(&request.withdrawal_id())
            .await
            .expect("fetch stale prepared withdrawal")
            .expect("stale prepared withdrawal remains live");
        assert_eq!(stale_live.state, WithdrawalState::Prepared);
        assert_eq!(stale_live.turn_started_base_height, Some(1));

        let node_pkhs = vec![
            Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            Tip5Hash([Belt(6), Belt(7), Belt(8), Belt(9), Belt(10)]),
        ];
        let local_node_id = scheduled_assembler_node_id(&request.withdrawal_id(), 1, &node_pkhs)
            .expect("scheduled epoch 1 assembler");
        let kernel = Arc::new(RecordingKernelPort::default());
        let mut planner = planner_config(bridge_lock_root);
        planner.base_fee = 0;
        planner.min_fee = 1;
        let sequencer = Arc::new(RecordingSequencerPort::default());
        let context = WithdrawalAssemblyContext {
            kernel: kernel.clone(),
            snapshot_service,
            sequencer: sequencer.clone(),
            proposal_registry: registry,
            bridge_status: sample_bridge_status(1),
            planner,
            fallback_policy: WithdrawalFallbackPolicy {
                assembly_timeout_blocks: 0,
                submission_timeout_blocks: 30,
            },
            local_node_id,
            node_pkhs,
        };

        let outcome = withdrawal_assembly_tick_once(&context)
            .await
            .expect("assembly tick");
        assert!(matches!(
            outcome,
            WithdrawalAssemblyTickOutcome::RequestedBuild {
                id,
                epoch: 1,
                selected_inputs: 1,
            } if id == request.withdrawal_id()
        ));

        let live = context
            .proposal_registry
            .fetch_live_withdrawal(&request.withdrawal_id())
            .await
            .expect("fetch live withdrawal")
            .expect("reassembled withdrawal remains live");
        assert_eq!(live.state, WithdrawalState::Assembling);
        assert_eq!(live.current_epoch, 1);
        let requests = kernel.requests.lock().expect("requests lock");
        assert_eq!(requests.len(), 1);
    }

    #[tokio::test]
    async fn withdrawal_assembly_tick_reassembles_after_expired_prepared_attempt_returns_to_pending(
    ) {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");

        let spend_condition = SpendCondition::simple_pkh(Tip5Hash([
            Belt(500),
            Belt(501),
            Belt(502),
            Belt(503),
            Belt(504),
        ]));
        let bridge_lock = Lock::SpendCondition(spend_condition.clone());
        let bridge_lock_root = bridge_lock.hash().expect("bridge lock root");
        let spendable_name = Name::new(
            spend_condition
                .first_name()
                .expect("first name")
                .into_hash(),
            Tip5Hash([Belt(701), Belt(702), Belt(703), Belt(704), Belt(705)]),
        );
        let pages = single_page_snapshot(vec![(
            spendable_name.clone(),
            note_for_lock(&bridge_lock, spendable_name.clone(), 20),
        )]);
        let snapshot_service = Arc::new(BridgeNoteSnapshotService::new(
            Arc::new(StaticSnapshotSource { pages }),
            BridgeOwnedNoteSelectors {
                first_names: vec!["bridge-first".to_string()],
            },
            Duration::from_secs(60),
        ));
        snapshot_service.refresh().await.expect("refresh snapshot");

        let proposal_epoch_0 = WithdrawalProposalData {
            id: request.withdrawal_id(),
            recipient: request.recipient.clone(),
            amount: request.amount.saturating_sub(1),
            burned_amount: request.amount,
            base_batch_end: request.base_batch_end,
            epoch: 0,
            snapshot: WithdrawalSnapshot {
                height: 900,
                block_id: Tip5Hash([Belt(91), Belt(92), Belt(93), Belt(94), Belt(95)]),
            },
            selected_inputs: vec![spendable_name.clone()],
            transaction: sample_transaction(),
        };
        let acquired = registry
            .acquire_withdrawal_assembly(&request.withdrawal_id(), 0, 1)
            .await
            .expect("acquire epoch 0 assembly");
        assert_eq!(acquired, AcquireWithdrawalAssemblyOutcome::Acquired);
        registry
            .validate_and_cache_prepared(&proposal_epoch_0)
            .await
            .expect("persist epoch 0 proposal");
        registry
            .mark_proposal_prepared(&proposal_epoch_0)
            .await
            .expect("mark prepared");
        registry
            .mark_proposal_expired(&proposal_epoch_0)
            .await
            .expect("expire prepared proposal");
        assert!(registry
            .fetch_live_withdrawal(&request.withdrawal_id())
            .await
            .expect("fetch live withdrawal after expiry")
            .is_none());

        let node_pkhs = vec![
            Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            Tip5Hash([Belt(6), Belt(7), Belt(8), Belt(9), Belt(10)]),
        ];
        let local_node_id = scheduled_assembler_node_id(&request.withdrawal_id(), 1, &node_pkhs)
            .expect("scheduled epoch 1 assembler");
        let kernel = Arc::new(RecordingKernelPort::default());
        let mut planner = planner_config(bridge_lock_root);
        planner.base_fee = 0;
        planner.min_fee = 1;
        let sequencer = Arc::new(RecordingSequencerPort::default());
        let context = WithdrawalAssemblyContext {
            kernel: kernel.clone(),
            snapshot_service,
            sequencer,
            proposal_registry: registry,
            bridge_status: sample_bridge_status(1),
            planner,
            fallback_policy: WithdrawalFallbackPolicy {
                assembly_timeout_blocks: 30,
                submission_timeout_blocks: 30,
            },
            local_node_id,
            node_pkhs,
        };

        let outcome = withdrawal_assembly_tick_once(&context)
            .await
            .expect("assembly tick after expiry");
        assert!(matches!(
            outcome,
            WithdrawalAssemblyTickOutcome::RequestedBuild {
                id,
                epoch: 1,
                selected_inputs: 1,
            } if id == request.withdrawal_id()
        ));

        let live = context
            .proposal_registry
            .fetch_live_withdrawal(&request.withdrawal_id())
            .await
            .expect("fetch live withdrawal")
            .expect("reassembled withdrawal remains live");
        assert_eq!(live.state, WithdrawalState::Assembling);
        assert_eq!(live.current_epoch, 1);
        let requests = kernel.requests.lock().expect("requests lock");
        assert_eq!(requests.len(), 1);
    }

    #[tokio::test]
    async fn withdrawal_assembly_tick_waits_for_confirmed_base_height_before_claiming_assembly() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");

        let spend_condition = SpendCondition::simple_pkh(Tip5Hash([
            Belt(500),
            Belt(501),
            Belt(502),
            Belt(503),
            Belt(504),
        ]));
        let bridge_lock = Lock::SpendCondition(spend_condition.clone());
        let bridge_lock_root = bridge_lock.hash().expect("bridge lock root");
        let spendable_name = Name::new(
            spend_condition
                .first_name()
                .expect("first name")
                .into_hash(),
            Tip5Hash([Belt(701), Belt(702), Belt(703), Belt(704), Belt(705)]),
        );
        let pages = single_page_snapshot(vec![(
            spendable_name.clone(),
            note_for_lock(&bridge_lock, spendable_name.clone(), 20),
        )]);
        let snapshot_service = Arc::new(BridgeNoteSnapshotService::new(
            Arc::new(StaticSnapshotSource { pages }),
            BridgeOwnedNoteSelectors {
                first_names: vec!["bridge-first".to_string()],
            },
            Duration::from_secs(60),
        ));
        snapshot_service.refresh().await.expect("refresh snapshot");

        let node_pkhs = vec![
            Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            Tip5Hash([Belt(6), Belt(7), Belt(8), Belt(9), Belt(10)]),
        ];
        let local_node_id = scheduled_assembler_node_id(&request.withdrawal_id(), 0, &node_pkhs)
            .expect("scheduled epoch 0 assembler");
        let kernel = Arc::new(RecordingKernelPort::default());
        let mut planner = planner_config(bridge_lock_root);
        planner.base_fee = 0;
        planner.min_fee = 1;
        let sequencer = Arc::new(RecordingSequencerPort::default());
        let context = WithdrawalAssemblyContext {
            kernel: kernel.clone(),
            snapshot_service,
            sequencer,
            proposal_registry: registry,
            bridge_status: sample_bridge_status(0),
            planner,
            fallback_policy: WithdrawalFallbackPolicy {
                assembly_timeout_blocks: 30,
                submission_timeout_blocks: 30,
            },
            local_node_id,
            node_pkhs,
        };

        let outcome = withdrawal_assembly_tick_once(&context)
            .await
            .expect("assembly tick without confirmed base height");
        assert_eq!(outcome, WithdrawalAssemblyTickOutcome::Idle);
        assert!(context
            .proposal_registry
            .fetch_live_withdrawal(&request.withdrawal_id())
            .await
            .expect("fetch live withdrawal after idle tick")
            .is_none());
        let requests = kernel.requests.lock().expect("requests lock");
        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn withdrawal_assembly_tick_releases_assembly_lock_on_kernel_build_failure() {
        let (_dir, registry) = open_services().await;
        let mut request = sample_withdrawal_request();
        request.amount = NICKS_PER_NOCK * 2;
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");

        let spend_condition = SpendCondition::simple_pkh(Tip5Hash([
            Belt(500),
            Belt(501),
            Belt(502),
            Belt(503),
            Belt(504),
        ]));
        let bridge_lock = Lock::SpendCondition(spend_condition.clone());
        let bridge_lock_root = bridge_lock.hash().expect("bridge lock root");
        let spendable_name = Name::new(
            spend_condition
                .first_name()
                .expect("first name")
                .into_hash(),
            Tip5Hash([Belt(701), Belt(702), Belt(703), Belt(704), Belt(705)]),
        );
        let pages = single_page_snapshot(vec![(
            spendable_name.clone(),
            note_for_lock(&bridge_lock, spendable_name.clone(), 140_000),
        )]);
        let snapshot_service = Arc::new(BridgeNoteSnapshotService::new(
            Arc::new(StaticSnapshotSource { pages }),
            BridgeOwnedNoteSelectors {
                first_names: vec!["bridge-first".to_string()],
            },
            Duration::from_secs(60),
        ));
        snapshot_service.refresh().await.expect("refresh snapshot");

        let kernel = Arc::new(RecordingKernelPort {
            create_error: Mutex::new(Some("kernel build failed".to_string())),
            ..Default::default()
        });
        let sequencer = Arc::new(RecordingSequencerPort::default());
        let mut planner = planner_config(bridge_lock_root);
        planner.nicks_fee_per_nock = 195;
        planner.base_fee = 0;
        planner.min_fee = 1;
        let context = WithdrawalAssemblyContext {
            kernel,
            snapshot_service,
            sequencer: sequencer.clone(),
            proposal_registry: registry.clone(),
            bridge_status: sample_bridge_status(1),
            planner,
            fallback_policy: WithdrawalFallbackPolicy::default(),
            local_node_id: 0,
            node_pkhs: vec![Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)])],
        };

        let err = withdrawal_assembly_tick_once(&context)
            .await
            .expect_err("kernel build failure should surface");
        assert!(
            err.to_string().contains("kernel build failed"),
            "unexpected error: {err}"
        );
        assert!(
            registry
                .fetch_live_withdrawal(&request.withdrawal_id())
                .await
                .expect("fetch live withdrawal after failure")
                .is_none(),
            "assembly lock should be released after kernel build failure"
        );
    }

    #[tokio::test]
    async fn withdrawal_assembly_tick_blocks_while_live_peer_canonical_attempt_exists() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");

        let spend_condition = SpendCondition::simple_pkh(Tip5Hash([
            Belt(500),
            Belt(501),
            Belt(502),
            Belt(503),
            Belt(504),
        ]));
        let bridge_lock = Lock::SpendCondition(spend_condition.clone());
        let bridge_lock_root = bridge_lock.hash().expect("bridge lock root");
        let spendable_name = Name::new(
            spend_condition
                .first_name()
                .expect("first name")
                .into_hash(),
            Tip5Hash([Belt(701), Belt(702), Belt(703), Belt(704), Belt(705)]),
        );
        let pages = single_page_snapshot(vec![(
            spendable_name.clone(),
            note_for_lock(&bridge_lock, spendable_name.clone(), 20),
        )]);
        let snapshot_service = Arc::new(BridgeNoteSnapshotService::new(
            Arc::new(StaticSnapshotSource { pages }),
            BridgeOwnedNoteSelectors {
                first_names: vec!["bridge-first".to_string()],
            },
            Duration::from_secs(60),
        ));
        snapshot_service.refresh().await.expect("refresh snapshot");

        let proposal_epoch_0 = WithdrawalProposalData {
            id: request.withdrawal_id(),
            recipient: request.recipient.clone(),
            amount: request.amount.saturating_sub(1),
            burned_amount: request.amount,
            base_batch_end: request.base_batch_end,
            epoch: 0,
            snapshot: WithdrawalSnapshot {
                height: 900,
                block_id: Tip5Hash([Belt(91), Belt(92), Belt(93), Belt(94), Belt(95)]),
            },
            selected_inputs: vec![spendable_name],
            transaction: sample_transaction(),
        };
        registry
            .validate_and_cache_prepared(&proposal_epoch_0)
            .await
            .expect("persist epoch 0 proposal");
        registry
            .mark_proposal_prepared(&proposal_epoch_0)
            .await
            .expect("mark prepared");
        registry
            .mark_proposal_canonical(&proposal_epoch_0)
            .await
            .expect("mark canonical");

        let node_pkhs = vec![
            Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            Tip5Hash([Belt(6), Belt(7), Belt(8), Belt(9), Belt(10)]),
        ];
        let kernel = Arc::new(RecordingKernelPort::default());
        let mut planner = planner_config(bridge_lock_root);
        planner.base_fee = 0;
        planner.min_fee = 1;
        let context = WithdrawalAssemblyContext {
            kernel: kernel.clone(),
            snapshot_service,
            sequencer: Arc::new(RecordingSequencerPort::default()),
            proposal_registry: registry.clone(),
            bridge_status: sample_bridge_status(1),
            planner,
            fallback_policy: WithdrawalFallbackPolicy {
                assembly_timeout_blocks: 0,
                submission_timeout_blocks: 30,
            },
            local_node_id: scheduled_assembler_node_id(&request.withdrawal_id(), 1, &node_pkhs)
                .expect("later epoch owner"),
            node_pkhs,
        };

        let outcome = withdrawal_assembly_tick_once(&context)
            .await
            .expect("assembly tick");
        assert_eq!(outcome, WithdrawalAssemblyTickOutcome::Idle);
        assert_eq!(kernel.requests.lock().expect("requests lock").len(), 0);
        let live = registry
            .fetch_live_withdrawal(&request.withdrawal_id())
            .await
            .expect("fetch live withdrawal")
            .expect("peer-canonical withdrawal remains live");
        assert_eq!(live.state, WithdrawalState::PeerCanonical);
        assert_eq!(live.current_epoch, 0);
    }

    #[tokio::test]
    async fn withdrawal_signing_tick_pokes_kernel_for_peer_canonical_proposal() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();
        let (transaction, local_signer_pkh) = partially_signed_transaction();
        let proposal = WithdrawalProposalData {
            id: request.withdrawal_id(),
            recipient: request.recipient.clone(),
            amount: request.amount.saturating_sub(1),
            burned_amount: request.amount,
            base_batch_end: request.base_batch_end,
            epoch: 0,
            snapshot: WithdrawalSnapshot {
                height: 900,
                block_id: Tip5Hash([Belt(91), Belt(92), Belt(93), Belt(94), Belt(95)]),
            },
            selected_inputs: vec![sample_name(10)],
            transaction,
        };

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
            .mark_proposal_canonical(&proposal)
            .await
            .expect("mark canonical");

        let kernel = Arc::new(RecordingKernelPort::default());
        let sequencer = Arc::new(RecordingSequencerPort::default());
        let withdrawal_nonce = registry
            .withdrawal_nonce(&proposal.id)
            .await
            .expect("fetch withdrawal nonce")
            .expect("tracked withdrawal nonce");
        sequencer
            .statuses
            .lock()
            .expect("sequencer statuses lock")
            .insert(
                proposal.id.clone(),
                SequencedWithdrawalStatusResponse {
                    found: false,
                    current_epoch: 0,
                    state: String::new(),
                    proposal_hash: String::new(),
                    authorized_transaction_name: String::new(),
                    withdrawal_nonce,
                    handoff_index: 0,
                    turn_started_base_height: None,

                    current_confirmed_base_height: None,

                    handoff_window_blocks: 0,

                    blocks_until_handoff: None,
                },
            );
        sequencer
            .register_withdrawal(
                &fetch_tracked_from_registry(registry.as_ref(), &proposal.id)
                    .await
                    .expect("tracked withdrawal"),
            )
            .await
            .expect("register local frontier");
        let context = WithdrawalSigningContext {
            kernel: kernel.clone(),
            sequencer,
            proposal_registry: registry,
            local_node_id: 3,
            local_signer_pkh,
            node_eth_addresses: HashMap::new(),
            fatal_stop: None,
        };

        let outcome = withdrawal_signing_tick_once(&context)
            .await
            .expect("signing tick");
        assert_eq!(
            outcome,
            WithdrawalSigningTickOutcome::RequestedSign {
                id: proposal.id.clone(),
                epoch: proposal.epoch,
            }
        );
        assert_eq!(kernel.signed.lock().expect("signed proposals").len(), 1);
    }

    #[tokio::test]
    async fn withdrawal_signing_tick_rejects_sequencer_proposal_hash_mismatch() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();
        let (transaction, local_signer_pkh) = partially_signed_transaction();
        let proposal = WithdrawalProposalData {
            id: request.withdrawal_id(),
            recipient: request.recipient.clone(),
            amount: request.amount.saturating_sub(1),
            burned_amount: request.amount,
            base_batch_end: request.base_batch_end,
            epoch: 0,
            snapshot: WithdrawalSnapshot {
                height: 900,
                block_id: Tip5Hash([Belt(91), Belt(92), Belt(93), Belt(94), Belt(95)]),
            },
            selected_inputs: vec![sample_name(10)],
            transaction,
        };

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
            .mark_proposal_canonical(&proposal)
            .await
            .expect("mark canonical");

        let kernel = Arc::new(RecordingKernelPort::default());
        let sequencer = Arc::new(RecordingSequencerPort::default());
        let withdrawal_nonce = registry
            .withdrawal_nonce(&proposal.id)
            .await
            .expect("fetch withdrawal nonce")
            .expect("tracked withdrawal nonce");
        sequencer
            .statuses
            .lock()
            .expect("sequencer statuses lock")
            .insert(
                proposal.id.clone(),
                SequencedWithdrawalStatusResponse {
                    found: true,
                    current_epoch: proposal.epoch,
                    state: WithdrawalState::Pending.as_str().to_string(),
                    // Deliberately different from the locally persisted proposal.proposal_hash().
                    proposal_hash: "sequencer-canonical-hash".to_string(),
                    authorized_transaction_name: String::new(),
                    withdrawal_nonce,
                    handoff_index: 0,
                    turn_started_base_height: None,

                    current_confirmed_base_height: None,

                    handoff_window_blocks: 0,

                    blocks_until_handoff: None,
                },
            );
        sequencer
            .register_withdrawal(
                &fetch_tracked_from_registry(registry.as_ref(), &proposal.id)
                    .await
                    .expect("tracked withdrawal"),
            )
            .await
            .expect("register local frontier");
        let context = WithdrawalSigningContext {
            kernel: kernel.clone(),
            sequencer,
            proposal_registry: registry,
            local_node_id: 3,
            local_signer_pkh,
            node_eth_addresses: HashMap::new(),
            fatal_stop: None,
        };

        let err = withdrawal_signing_tick_once(&context)
            .await
            .expect_err("signing tick should fail on proposal hash mismatch");
        assert!(
            err.to_string()
                .contains("withdrawal signing proposal hash mismatch"),
            "unexpected error: {err}"
        );
        assert!(kernel.signed.lock().expect("signed proposals").is_empty());
    }

    #[tokio::test]
    async fn withdrawal_signing_tick_hydrates_peer_canonical_from_sequencer_artifacts() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();
        let (transaction, local_signer_pkh) = partially_signed_transaction();
        let proposal = sample_proposal_for_request(&request, transaction);
        let commit_certificate = sample_commit_certificate(&proposal).await;
        let proposal_hash = proposal.proposal_hash().expect("proposal hash");

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
            .restore_tracked_withdrawal_requests()
            .await
            .expect("clear reboot-local proposal cache");

        let withdrawal_nonce = fetch_tracked_from_registry(registry.as_ref(), &proposal.id)
            .await
            .expect("tracked withdrawal")
            .withdrawal_nonce;
        let sequencer = Arc::new(RecordingSequencerPort::default());
        sequencer
            .statuses
            .lock()
            .expect("sequencer statuses lock")
            .insert(
                proposal.id.clone(),
                SequencedWithdrawalStatusResponse {
                    found: true,
                    current_epoch: proposal.epoch,
                    state: WithdrawalState::Pending.as_str().to_string(),
                    proposal_hash: proposal_hash.clone(),
                    authorized_transaction_name: String::new(),
                    withdrawal_nonce,
                    handoff_index: 0,
                    turn_started_base_height: None,

                    current_confirmed_base_height: None,

                    handoff_window_blocks: 0,

                    blocks_until_handoff: None,
                },
            );
        sequencer
            .canonical_artifacts
            .lock()
            .expect("canonical artifacts lock")
            .insert(
                proposal.id.clone(),
                sample_canonical_artifacts(&proposal, Some(&commit_certificate)),
            );

        let kernel = Arc::new(RecordingKernelPort::default());
        let context = WithdrawalSigningContext {
            kernel: kernel.clone(),
            sequencer,
            proposal_registry: registry.clone(),
            local_node_id: 3,
            local_signer_pkh,
            node_eth_addresses: sample_node_eth_addresses(),
            fatal_stop: None,
        };

        let outcome = withdrawal_signing_tick_once(&context)
            .await
            .expect("signing tick");

        assert_eq!(
            outcome,
            WithdrawalSigningTickOutcome::RequestedSign {
                id: proposal.id.clone(),
                epoch: proposal.epoch,
            }
        );
        assert_eq!(kernel.signed.lock().expect("signed proposals").len(), 1);
        let live = registry
            .fetch_live_withdrawal(&proposal.id)
            .await
            .expect("fetch live withdrawal")
            .expect("live withdrawal");
        assert_eq!(live.state, WithdrawalState::PeerCanonical);
        assert_eq!(live.proposal_hash.as_deref(), Some(proposal_hash.as_str()));
        assert!(live.peer_commit_certificate.is_some());
    }

    #[tokio::test]
    async fn withdrawal_signing_tick_fails_when_sequencer_nonce_disagrees() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();
        let (transaction, local_signer_pkh) = partially_signed_transaction();
        let proposal = WithdrawalProposalData {
            id: request.withdrawal_id(),
            recipient: request.recipient.clone(),
            amount: request.amount.saturating_sub(1),
            burned_amount: request.amount,
            base_batch_end: request.base_batch_end,
            epoch: 0,
            snapshot: WithdrawalSnapshot {
                height: 900,
                block_id: Tip5Hash([Belt(91), Belt(92), Belt(93), Belt(94), Belt(95)]),
            },
            selected_inputs: vec![sample_name(10)],
            transaction,
        };

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
            .mark_proposal_canonical(&proposal)
            .await
            .expect("mark canonical");

        let kernel = Arc::new(RecordingKernelPort::default());
        let sequencer = Arc::new(RecordingSequencerPort::default());
        let withdrawal_nonce = registry
            .withdrawal_nonce(&proposal.id)
            .await
            .expect("fetch withdrawal nonce")
            .expect("tracked withdrawal nonce")
            .saturating_add(1);
        sequencer
            .statuses
            .lock()
            .expect("sequencer statuses lock")
            .insert(
                proposal.id.clone(),
                SequencedWithdrawalStatusResponse {
                    found: false,
                    current_epoch: 0,
                    state: String::new(),
                    proposal_hash: String::new(),
                    authorized_transaction_name: String::new(),
                    withdrawal_nonce,
                    handoff_index: 0,
                    turn_started_base_height: None,

                    current_confirmed_base_height: None,

                    handoff_window_blocks: 0,

                    blocks_until_handoff: None,
                },
            );
        sequencer
            .register_withdrawal(
                &fetch_tracked_from_registry(registry.as_ref(), &proposal.id)
                    .await
                    .expect("tracked withdrawal"),
            )
            .await
            .expect("register local frontier");
        let context = WithdrawalSigningContext {
            kernel: kernel.clone(),
            sequencer,
            proposal_registry: registry,
            local_node_id: 3,
            local_signer_pkh,
            node_eth_addresses: HashMap::new(),
            fatal_stop: None,
        };

        let err = withdrawal_signing_tick_once(&context)
            .await
            .expect_err("signing tick should fail on nonce mismatch");
        assert!(
            err.to_string()
                .contains("withdrawal signing nonce mismatch"),
            "unexpected error: {err}"
        );
        assert!(kernel.signed.lock().expect("signed proposals").is_empty());
    }

    #[tokio::test]
    async fn withdrawal_signing_tick_excludes_rows_below_and_above_frontier() {
        // Only the sequencer frontier nonce may collect signatures. This keeps
        // stale local peer-canonical state and future local peer-canonical
        // state passive even if both are still present in the local DB. The
        // sequencer itself must not advance the future row past Pending while
        // a lower nonce remains live.
        let (_dir, registry) = open_services().await;
        let stale_request = sample_withdrawal_request_with_seed(1);
        let frontier_request = sample_withdrawal_request_with_seed(2);
        let future_request = sample_withdrawal_request_with_seed(3);
        let (transaction, local_signer_pkh) = partially_signed_transaction();
        let stale_proposal = sample_proposal_for_request(&stale_request, transaction.clone());
        let future_proposal = sample_proposal_for_request(&future_request, transaction);

        for request in [&stale_request, &frontier_request, &future_request] {
            registry
                .track_withdrawal_request(request)
                .await
                .expect("track request");
        }
        for proposal in [&stale_proposal, &future_proposal] {
            registry
                .validate_and_cache_prepared(proposal)
                .await
                .expect("persist proposal");
            registry
                .mark_proposal_prepared(proposal)
                .await
                .expect("mark prepared");
            registry
                .mark_proposal_canonical(proposal)
                .await
                .expect("mark canonical");
        }

        let kernel = Arc::new(RecordingKernelPort::default());
        let sequencer = Arc::new(RecordingSequencerPort::default());
        {
            let mut statuses = sequencer.statuses.lock().expect("sequencer statuses lock");
            statuses.insert(
                stale_request.withdrawal_id(),
                SequencedWithdrawalStatusResponse {
                    found: true,
                    current_epoch: 0,
                    state: WithdrawalState::MempoolAccepted.as_str().to_string(),
                    proposal_hash: String::new(),
                    authorized_transaction_name: "stale-tx".to_string(),
                    withdrawal_nonce: 1,
                    handoff_index: 0,
                    turn_started_base_height: None,

                    current_confirmed_base_height: None,

                    handoff_window_blocks: 0,

                    blocks_until_handoff: None,
                },
            );
            statuses.insert(
                frontier_request.withdrawal_id(),
                SequencedWithdrawalStatusResponse {
                    found: true,
                    current_epoch: 0,
                    state: WithdrawalState::Pending.as_str().to_string(),
                    proposal_hash: String::new(),
                    authorized_transaction_name: String::new(),
                    withdrawal_nonce: 2,
                    handoff_index: 0,
                    turn_started_base_height: None,

                    current_confirmed_base_height: None,

                    handoff_window_blocks: 0,

                    blocks_until_handoff: None,
                },
            );
            statuses.insert(
                future_request.withdrawal_id(),
                SequencedWithdrawalStatusResponse {
                    found: true,
                    current_epoch: 0,
                    state: WithdrawalState::Pending.as_str().to_string(),
                    proposal_hash: String::new(),
                    authorized_transaction_name: String::new(),
                    withdrawal_nonce: 3,
                    handoff_index: 0,
                    turn_started_base_height: None,

                    current_confirmed_base_height: None,

                    handoff_window_blocks: 0,

                    blocks_until_handoff: None,
                },
            );
        }
        let context = WithdrawalSigningContext {
            kernel: kernel.clone(),
            sequencer,
            proposal_registry: registry,
            local_node_id: 3,
            local_signer_pkh,
            node_eth_addresses: HashMap::new(),
            fatal_stop: None,
        };

        let outcome = withdrawal_signing_tick_once(&context)
            .await
            .expect("signing tick");

        assert_eq!(outcome, WithdrawalSigningTickOutcome::Idle);
        assert!(kernel.signed.lock().expect("signed proposals").is_empty());
    }

    #[tokio::test]
    async fn persist_built_withdrawal_proposal_records_prepared_live_state() {
        let (_dir, registry) = open_services().await;
        let request = sample_withdrawal_request();
        registry
            .track_withdrawal_request(&request)
            .await
            .expect("track request");

        let proposal = WithdrawalProposalData {
            id: request.withdrawal_id(),
            recipient: request.recipient.clone(),
            amount: request.amount.saturating_sub(1),
            burned_amount: request.amount,
            base_batch_end: request.base_batch_end,
            epoch: 0,
            snapshot: WithdrawalSnapshot {
                height: 900,
                block_id: Tip5Hash([Belt(91), Belt(92), Belt(93), Belt(94), Belt(95)]),
            },
            selected_inputs: vec![sample_name(10), sample_name(20)],
            transaction: sample_transaction(),
        };

        let outcome = persist_built_withdrawal_proposal(&proposal, registry.as_ref())
            .await
            .expect("persist built proposal");
        assert_eq!(outcome, WithdrawalProposalValidationOutcome::Inserted);

        let live = registry
            .fetch_live_withdrawal(&proposal.id)
            .await
            .expect("fetch live withdrawal")
            .expect("prepared withdrawal exists");
        assert_eq!(live.state, WithdrawalState::Prepared);
        assert_eq!(live.current_epoch, 0);
    }
}
