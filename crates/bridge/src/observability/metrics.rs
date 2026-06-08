use std::sync::{Arc, OnceLock};

use gnort::instrument::UnitOfTime;
use gnort::*;

use crate::observability::tui::types::NetworkState;

metrics_struct![
    BridgeHealthMetrics,
    (running_status, "bridge.health.running_status", Gauge),
    (stop_local_requests, "bridge.stop.local.requests", Count),
    (stop_local_triggered, "bridge.stop.local.triggered", Count),
    (stop_local_duplicate, "bridge.stop.local.duplicate", Count),
    (base_hold_height, "bridge.health.base_hold_height", Gauge),
    (nock_hold_height, "bridge.health.nock_hold_height", Gauge),
    (base_tip_height, "bridge.health.base_tip_height", Gauge),
    (base_block_height, "bridge.health.base_block_height", Gauge),
    (nock_block_height, "bridge.health.nock_block_height", Gauge),
    (last_deposit_nonce, "bridge.health.last_deposit_nonce", Gauge),
    (deposit_log_max_nonce, "bridge.health.deposit_log_max_nonce", Gauge),
    (ingress_broadcast_signature_requests, "bridge.ingress.broadcast_signature.requests", Count),
    (
        ingress_broadcast_signature_invalid_deposit_id_len,
        "bridge.ingress.broadcast_signature.invalid.deposit_id_length", Count
    ),
    (
        ingress_broadcast_signature_invalid_proposal_hash_len,
        "bridge.ingress.broadcast_signature.invalid.proposal_hash_length", Count
    ),
    (
        ingress_broadcast_signature_invalid_signature_len,
        "bridge.ingress.broadcast_signature.invalid.signature_length", Count
    ),
    (
        ingress_broadcast_signature_invalid_signer_address_len,
        "bridge.ingress.broadcast_signature.invalid.signer_address_length", Count
    ),
    (
        ingress_broadcast_signature_invalid_deposit_id_decode,
        "bridge.ingress.broadcast_signature.invalid.deposit_id_decode", Count
    ),
    (
        ingress_broadcast_signature_ignored_self,
        "bridge.ingress.broadcast_signature.ignored.self", Count
    ),
    (
        ingress_broadcast_signature_known_proposal,
        "bridge.ingress.broadcast_signature.proposal.known", Count
    ),
    (
        ingress_broadcast_signature_unknown_proposal,
        "bridge.ingress.broadcast_signature.proposal.unknown", Count
    ),
    (
        ingress_broadcast_signature_known_signer,
        "bridge.ingress.broadcast_signature.signer.known", Count
    ),
    (
        ingress_broadcast_signature_unknown_signer,
        "bridge.ingress.broadcast_signature.signer.unknown", Count
    ),
    (
        ingress_broadcast_signature_unknown_signer_known_proposal,
        "bridge.ingress.broadcast_signature.signer.unknown_for_known_proposal", Count
    ),
    (
        ingress_broadcast_signature_hash_mismatch,
        "bridge.ingress.broadcast_signature.proposal_hash_mismatch", Count
    ),
    (
        ingress_broadcast_signature_result_added,
        "bridge.ingress.broadcast_signature.result.added", Count
    ),
    (
        ingress_broadcast_signature_result_threshold_reached,
        "bridge.ingress.broadcast_signature.result.threshold_reached", Count
    ),
    (
        ingress_broadcast_signature_result_duplicate,
        "bridge.ingress.broadcast_signature.result.duplicate", Count
    ),
    (
        ingress_broadcast_signature_result_stale,
        "bridge.ingress.broadcast_signature.result.stale", Count
    ),
    (
        ingress_broadcast_signature_result_invalid,
        "bridge.ingress.broadcast_signature.result.invalid", Count
    ),
    (
        ingress_broadcast_signature_result_error,
        "bridge.ingress.broadcast_signature.result.error", Count
    ),
    (proposal_cache_total, "bridge.proposal_cache.entries.total", Gauge),
    (proposal_cache_collecting, "bridge.proposal_cache.entries.collecting", Gauge),
    (proposal_cache_ready, "bridge.proposal_cache.entries.ready", Gauge),
    (proposal_cache_posting, "bridge.proposal_cache.entries.posting", Gauge),
    (proposal_cache_confirmed, "bridge.proposal_cache.entries.confirmed", Gauge),
    (proposal_cache_failed, "bridge.proposal_cache.entries.failed", Gauge),
    (proposal_cache_total_peer_signatures, "bridge.proposal_cache.signatures.peer.total", Gauge),
    (
        proposal_cache_max_peer_signatures_per_proposal,
        "bridge.proposal_cache.signatures.peer.max_per_proposal", Gauge
    ),
    (
        proposal_cache_proposals_with_my_signature,
        "bridge.proposal_cache.signatures.my.proposals", Gauge
    ),
    (
        proposal_cache_pending_signature_deposit_count, "bridge.proposal_cache.pending.deposits",
        Gauge
    ),
    (proposal_cache_pending_signature_total, "bridge.proposal_cache.pending.signatures", Gauge),
    (proposal_cache_oldest_age_secs, "bridge.proposal_cache.age.oldest_seconds", Gauge),
    (
        proposal_cache_oldest_confirmed_age_secs,
        "bridge.proposal_cache.age.oldest_confirmed_seconds", Gauge
    ),
    (
        proposal_cache_oldest_failed_age_secs, "bridge.proposal_cache.age.oldest_failed_seconds",
        Gauge
    ),
    (
        proposal_cache_pending_oldest_age_secs,
        "bridge.proposal_cache.age.oldest_pending_signature_seconds", Gauge
    ),
    (proposal_cache_approx_state_bytes, "bridge.proposal_cache.approx_bytes.states", Gauge),
    (
        proposal_cache_approx_peer_signature_bytes,
        "bridge.proposal_cache.approx_bytes.peer_signatures", Gauge
    ),
    (
        proposal_cache_approx_my_signature_bytes,
        "bridge.proposal_cache.approx_bytes.my_signatures", Gauge
    ),
    (
        proposal_cache_approx_pending_signature_bytes,
        "bridge.proposal_cache.approx_bytes.pending_signatures", Gauge
    ),
    (proposal_cache_approx_total_bytes, "bridge.proposal_cache.approx_bytes.total", Gauge),
    (proposal_cache_metrics_update_error, "bridge.proposal_cache.metrics_update_error", Count),
    (
        proposal_cache_pending_signature_queued_unknown_deposit,
        "bridge.proposal_cache.pending.queued_unknown_deposit", Count
    ),
    (proposal_cache_pending_signature_applied, "bridge.proposal_cache.pending.applied", Count),
    (
        proposal_cache_pending_signature_mismatched,
        "bridge.proposal_cache.pending.mismatched_hash", Count
    ),
    (
        proposal_cache_pending_signature_verify_failed,
        "bridge.proposal_cache.pending.verify_failed", Count
    ),
    (
        proposal_cache_pending_signature_address_mismatch,
        "bridge.proposal_cache.pending.address_mismatch", Count
    ),
    (proposal_cache_signature_duplicate, "bridge.proposal_cache.signature.duplicate", Count),
    (
        proposal_cache_signature_verify_failed, "bridge.proposal_cache.signature.verify_failed",
        Count
    ),
    (
        proposal_cache_signature_address_mismatch,
        "bridge.proposal_cache.signature.address_mismatch", Count
    ),
    (proposal_cache_gc_runs, "bridge.proposal_cache.gc.runs", Count),
    (proposal_cache_gc_last_removed, "bridge.proposal_cache.gc.last_removed", Gauge),
    (tui_snapshot_requests, "bridge.tui.snapshot.requests", Count),
    (tui_snapshot_uncached_requests, "bridge.tui.snapshot.uncached_requests", Count),
    (tui_snapshot_alert_limit_requested, "bridge.tui.snapshot.requested.alert_limit", Gauge),
    (tui_snapshot_limit_requested, "bridge.tui.snapshot.requested.limit", Gauge),
    (tui_snapshot_offset_requested, "bridge.tui.snapshot.requested.offset", Gauge),
    (tui_snapshot_limit_over_cache, "bridge.tui.snapshot.requested.limit_over_cache", Count),
    (tui_snapshot_limit_over_10000, "bridge.tui.snapshot.requested.limit_over_10000", Count),
    (tui_snapshot_response_time, "bridge.tui.snapshot.response_time", TimingCount),
    (tui_snapshot_to_response_time, "bridge.tui.snapshot.to_response_time", TimingCount),
    (tui_snapshot_uncached_load_time, "bridge.tui.snapshot.uncached_load_time", TimingCount),
    (tui_snapshot_build_cache_time, "bridge.tui.snapshot.build_cache_time", TimingCount),
    (tui_snapshot_build_proposals_time, "bridge.tui.snapshot.build_proposals_time", TimingCount),
    (tui_proposals_pending_inbound_count, "bridge.tui.proposals.pending_inbound_count", Gauge),
    (tui_proposals_history_count, "bridge.tui.proposals.history_count", Gauge),
    (tui_proposals_last_submitted_present, "bridge.tui.proposals.last_submitted_present", Gauge),
    (
        tui_proposals_pending_inbound_signature_count,
        "bridge.tui.proposals.pending_inbound_signature_count", Gauge
    ),
    (
        tui_proposals_history_signature_count, "bridge.tui.proposals.history_signature_count",
        Gauge
    ),
    (
        tui_proposals_pending_inbound_approx_bytes,
        "bridge.tui.proposals.pending_inbound_approx_bytes", Gauge
    ),
    (tui_proposals_history_approx_bytes, "bridge.tui.proposals.history_approx_bytes", Gauge),
    (
        tui_proposals_last_submitted_approx_bytes,
        "bridge.tui.proposals.last_submitted_approx_bytes", Gauge
    ),
    (tui_proposals_approx_total_bytes, "bridge.tui.proposals.approx_total_bytes", Gauge),
    (deposit_log_snapshot_time, "bridge.tui.deposit_log.snapshot_time", TimingCount),
    (deposit_log_count_time, "bridge.tui.deposit_log.count_time", TimingCount),
    (deposit_log_page_time, "bridge.tui.deposit_log.page_time", TimingCount),
    (bridge_state_snapshot_time, "bridge.runtime.bridge_state.snapshot_time", TimingCount),
    (
        bridge_state_peek_unsettled_deposits_time,
        "bridge.runtime.bridge_state.peek.unsettled_deposits_time", TimingCount
    ),
    (
        bridge_state_peek_unsettled_withdrawals_time,
        "bridge.runtime.bridge_state.peek.unsettled_withdrawals_time", TimingCount
    ),
    (
        bridge_state_peek_base_next_height_time,
        "bridge.runtime.bridge_state.peek.base_next_height_time", TimingCount
    ),
    (
        bridge_state_peek_nock_next_height_time,
        "bridge.runtime.bridge_state.peek.nock_next_height_time", TimingCount
    ),
    (
        bridge_state_peek_base_hold_info_time,
        "bridge.runtime.bridge_state.peek.base_hold_info_time", TimingCount
    ),
    (
        bridge_state_peek_nock_hold_info_time,
        "bridge.runtime.bridge_state.peek.nock_hold_info_time", TimingCount
    ),
    (
        bridge_state_peek_stop_state_time, "bridge.runtime.bridge_state.peek.stop_state_time",
        TimingCount
    ),
    (
        bridge_state_peek_is_fakenet_time, "bridge.runtime.bridge_state.peek.is_fakenet_time",
        TimingCount
    ),
    (tui_deposit_log_limit_requested, "bridge.tui.deposit_log.requested.limit", Gauge),
    (tui_deposit_log_offset_requested, "bridge.tui.deposit_log.requested.offset", Gauge),
    (tui_deposit_log_rows_returned, "bridge.tui.deposit_log.returned.rows", Gauge),
    (
        tui_deposit_log_limit_over_cache, "bridge.tui.deposit_log.requested.limit_over_cache",
        Count
    ),
    (
        tui_deposit_log_limit_over_10000, "bridge.tui.deposit_log.requested.limit_over_10000",
        Count
    ),
    (withdrawal_frontier_present, "bridge.withdrawal.frontier.present", Gauge),
    (withdrawal_frontier_nonce, "bridge.withdrawal.frontier.nonce", Gauge),
    (
        withdrawal_frontier_local_row_present, "bridge.withdrawal.frontier.local_row_present",
        Gauge
    ),
    (withdrawal_frontier_local_state, "bridge.withdrawal.frontier.local_state", Gauge),
    (withdrawal_frontier_local_nonce_gap, "bridge.withdrawal.frontier.local_nonce_gap", Gauge),
    (
        withdrawal_frontier_status_fetch_error, "bridge.withdrawal.frontier.status_fetch_error",
        Count
    ),
    (
        withdrawal_frontier_status_fetch_time, "bridge.withdrawal.frontier.status_fetch_time",
        TimingCount
    ),
    (withdrawal_lifecycle_total, "bridge.withdrawal.lifecycle.total", Gauge),
    (withdrawal_lifecycle_live, "bridge.withdrawal.lifecycle.live", Gauge),
    (
        withdrawal_lifecycle_ordering_blocking, "bridge.withdrawal.lifecycle.ordering_blocking",
        Gauge
    ),
    (withdrawal_lifecycle_pending, "bridge.withdrawal.lifecycle.pending", Gauge),
    (withdrawal_lifecycle_assembling, "bridge.withdrawal.lifecycle.assembling", Gauge),
    (withdrawal_lifecycle_prepared, "bridge.withdrawal.lifecycle.prepared", Gauge),
    (withdrawal_lifecycle_peer_canonical, "bridge.withdrawal.lifecycle.peer_canonical", Gauge),
    (withdrawal_lifecycle_authorized, "bridge.withdrawal.lifecycle.authorized", Gauge),
    (
        withdrawal_lifecycle_mempool_accepted, "bridge.withdrawal.lifecycle.mempool_accepted",
        Gauge
    ),
    (withdrawal_lifecycle_confirmed, "bridge.withdrawal.lifecycle.confirmed", Gauge),
    (withdrawal_lifecycle_below_frontier, "bridge.withdrawal.lifecycle.below_frontier", Gauge),
    (withdrawal_lifecycle_above_frontier, "bridge.withdrawal.lifecycle.above_frontier", Gauge),
    (withdrawal_registration_attempts, "bridge.withdrawal.registration.attempts", Count),
    (withdrawal_registration_accepted, "bridge.withdrawal.registration.accepted", Count),
    (withdrawal_registration_rejected, "bridge.withdrawal.registration.rejected", Count),
    (withdrawal_registration_error, "bridge.withdrawal.registration.error", Count),
    (withdrawal_registration_stalled, "bridge.withdrawal.registration.stalled", Count),
    (withdrawal_activation_ready, "bridge.withdrawal.activation.ready", Gauge),
    (withdrawal_activation_waiting, "bridge.withdrawal.activation.waiting", Gauge),
    (
        withdrawal_activation_nock_next_height, "bridge.withdrawal.activation.nock_next_height",
        Gauge
    ),
    (
        withdrawal_projection_cursor_base_next_height,
        "bridge.withdrawal.projection.cursor_base_next_height", Gauge
    ),
    (
        withdrawal_projection_cursor_nock_next_height,
        "bridge.withdrawal.projection.cursor_nock_next_height", Gauge
    ),
    (withdrawal_projection_replay_rows, "bridge.withdrawal.projection.replay_rows", Gauge),
    (withdrawal_projection_replay_error, "bridge.withdrawal.projection.replay_error", Count),
    (
        withdrawal_projection_immutable_mismatch,
        "bridge.withdrawal.projection.immutable_mismatch", Count
    ),
    (withdrawal_assembly_ticks, "bridge.withdrawal.assembly.ticks", Count),
    (withdrawal_assembly_idle, "bridge.withdrawal.assembly.idle", Count),
    (withdrawal_assembly_built, "bridge.withdrawal.assembly.built", Count),
    (withdrawal_assembly_not_frontier, "bridge.withdrawal.assembly.not_frontier", Count),
    (
        withdrawal_assembly_snapshot_unavailable,
        "bridge.withdrawal.assembly.snapshot_unavailable", Count
    ),
    (withdrawal_assembly_plan_failed, "bridge.withdrawal.assembly.plan_failed", Count),
    (withdrawal_assembly_tick_time, "bridge.withdrawal.assembly.tick_time", TimingCount),
    (withdrawal_signing_ticks, "bridge.withdrawal.signing.ticks", Count),
    (withdrawal_signing_idle, "bridge.withdrawal.signing.idle", Count),
    (withdrawal_signing_signed, "bridge.withdrawal.signing.signed", Count),
    (withdrawal_signing_not_frontier, "bridge.withdrawal.signing.not_frontier", Count),
    (withdrawal_signing_cache_miss, "bridge.withdrawal.signing.cache_miss", Count),
    (withdrawal_signing_hydrated, "bridge.withdrawal.signing.hydrated", Count),
    (withdrawal_signing_hydration_error, "bridge.withdrawal.signing.hydration_error", Count),
    (withdrawal_signing_verify_failed, "bridge.withdrawal.signing.verify_failed", Count),
    (withdrawal_signing_tick_time, "bridge.withdrawal.signing.tick_time", TimingCount),
    (withdrawal_submission_ticks, "bridge.withdrawal.submission.ticks", Count),
    (withdrawal_submission_idle, "bridge.withdrawal.submission.idle", Count),
    (withdrawal_submission_authorized, "bridge.withdrawal.submission.authorized", Count),
    (
        withdrawal_submission_mempool_accepted, "bridge.withdrawal.submission.mempool_accepted",
        Count
    ),
    (withdrawal_submission_deferred, "bridge.withdrawal.submission.deferred", Count),
    (withdrawal_submission_error, "bridge.withdrawal.submission.error", Count),
    (withdrawal_submission_not_frontier, "bridge.withdrawal.submission.not_frontier", Count),
    (
        withdrawal_submission_missing_authorized_artifacts,
        "bridge.withdrawal.submission.missing_authorized_artifacts", Count
    ),
    (withdrawal_submission_tick_time, "bridge.withdrawal.submission.tick_time", TimingCount),
    (withdrawal_proposal_cache_proposals, "bridge.withdrawal.proposal_cache.proposals", Gauge),
    (withdrawal_proposal_cache_signatures, "bridge.withdrawal.proposal_cache.signatures", Gauge),
    (withdrawal_proposal_cache_cache_miss, "bridge.withdrawal.proposal_cache.cache_miss", Count),
    (withdrawal_proposal_cache_hydrated, "bridge.withdrawal.proposal_cache.hydrated", Count),
    (withdrawal_proposal_cache_evicted, "bridge.withdrawal.proposal_cache.evicted", Count),
    (withdrawal_proposal_cache_poisoned, "bridge.withdrawal.proposal_cache.poisoned", Count),
    (sequencer_withdrawal_frontier_present, "sequencer.withdrawal.frontier.present", Gauge),
    (sequencer_withdrawal_frontier_nonce, "sequencer.withdrawal.frontier.nonce", Gauge),
    (sequencer_withdrawal_frontier_state, "sequencer.withdrawal.frontier.state", Gauge),
    (
        sequencer_withdrawal_base_confirmed_height, "sequencer.withdrawal.base_confirmed_height",
        Gauge
    ),
    (
        sequencer_withdrawal_registration_requests, "sequencer.withdrawal.registration.requests",
        Count
    ),
    (
        sequencer_withdrawal_registration_accepted, "sequencer.withdrawal.registration.accepted",
        Count
    ),
    (
        sequencer_withdrawal_registration_rejected, "sequencer.withdrawal.registration.rejected",
        Count
    ),
    (sequencer_withdrawal_registration_error, "sequencer.withdrawal.registration.error", Count),
    (
        sequencer_withdrawal_registration_idempotent,
        "sequencer.withdrawal.registration.idempotent", Count
    ),
    (
        sequencer_withdrawal_registration_rejected_lower_than_head,
        "sequencer.withdrawal.registration.rejected_lower_than_head", Count
    ),
    (
        sequencer_withdrawal_base_verifier_accepted, "sequencer.withdrawal.base_verifier.accepted",
        Count
    ),
    (
        sequencer_withdrawal_base_verifier_rejected, "sequencer.withdrawal.base_verifier.rejected",
        Count
    ),
    (
        sequencer_withdrawal_base_verifier_rpc_error,
        "sequencer.withdrawal.base_verifier.rpc_error", Count
    ),
    (
        sequencer_withdrawal_base_verifier_verify_time,
        "sequencer.withdrawal.base_verifier.verify_time", TimingCount
    ),
    (
        sequencer_withdrawal_canonicalize_requests, "sequencer.withdrawal.canonicalize.requests",
        Count
    ),
    (
        sequencer_withdrawal_canonicalize_accepted, "sequencer.withdrawal.canonicalize.accepted",
        Count
    ),
    (
        sequencer_withdrawal_canonicalize_rejected, "sequencer.withdrawal.canonicalize.rejected",
        Count
    ),
    (
        sequencer_withdrawal_canonicalize_not_frontier,
        "sequencer.withdrawal.canonicalize.not_frontier", Count
    ),
    (
        sequencer_withdrawal_canonicalize_certificate_verify_failed,
        "sequencer.withdrawal.canonicalize.certificate_verify_failed", Count
    ),
    (sequencer_withdrawal_authorize_requests, "sequencer.withdrawal.authorize.requests", Count),
    (sequencer_withdrawal_authorize_accepted, "sequencer.withdrawal.authorize.accepted", Count),
    (sequencer_withdrawal_authorize_rejected, "sequencer.withdrawal.authorize.rejected", Count),
    (
        sequencer_withdrawal_authorize_not_frontier, "sequencer.withdrawal.authorize.not_frontier",
        Count
    ),
    (sequencer_withdrawal_submit_attempts, "sequencer.withdrawal.submit.attempts", Count),
    (
        sequencer_withdrawal_submit_mempool_accepted,
        "sequencer.withdrawal.submit.mempool_accepted", Count
    ),
    (sequencer_withdrawal_submit_deferred, "sequencer.withdrawal.submit.deferred", Count),
    (sequencer_withdrawal_submit_error, "sequencer.withdrawal.submit.error", Count),
    (sequencer_withdrawal_confirmation_polls, "sequencer.withdrawal.confirmation.polls", Count),
    (
        sequencer_withdrawal_confirmation_confirmed, "sequencer.withdrawal.confirmation.confirmed",
        Count
    ),
    (
        sequencer_withdrawal_confirmation_not_found, "sequencer.withdrawal.confirmation.not_found",
        Count
    ),
    (sequencer_withdrawal_confirmation_error, "sequencer.withdrawal.confirmation.error", Count),
    (
        sequencer_withdrawal_orphan_retry_attempts, "sequencer.withdrawal.orphan_retry.attempts",
        Count
    ),
    (sequencer_withdrawal_orphan_retry_error, "sequencer.withdrawal.orphan_retry.error", Count),
    (sequencer_withdrawal_journal_enabled, "sequencer.withdrawal.journal.enabled", Gauge),
    (
        sequencer_withdrawal_journal_recovery_events_replayed,
        "sequencer.withdrawal.journal.recovery_events_replayed", Gauge
    ),
    (
        sequencer_withdrawal_journal_recovery_error, "sequencer.withdrawal.journal.recovery_error",
        Count
    ),
    (
        sequencer_withdrawal_journal_append_success, "sequencer.withdrawal.journal.append_success",
        Count
    ),
    (
        sequencer_withdrawal_journal_append_disabled,
        "sequencer.withdrawal.journal.append_disabled", Count
    ),
    (
        sequencer_withdrawal_journal_append_error, "sequencer.withdrawal.journal.append_error",
        Count
    ),
    (
        sequencer_withdrawal_journal_append_time, "sequencer.withdrawal.journal.append_time",
        TimingCount
    ),
    (
        sequencer_withdrawal_journal_projection_mismatch,
        "sequencer.withdrawal.journal.projection_mismatch", Count
    )
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum RunningStatusMetric {
    Stop = 0,
    Running = 1,
}

impl From<RunningStatusMetric> for f64 {
    fn from(value: RunningStatusMetric) -> Self {
        value as i32 as f64
    }
}

static METRICS: OnceLock<Arc<BridgeHealthMetrics>> = OnceLock::new();

pub fn init_metrics() -> Arc<BridgeHealthMetrics> {
    METRICS
        .get_or_init(|| {
            let mut metrics = BridgeHealthMetrics::register(gnort::global_metrics_registry())
                .expect("Failed to register metrics!");
            metrics.deposit_log_snapshot_time = metrics
                .deposit_log_snapshot_time
                .with_unit(UnitOfTime::Micros);
            metrics.deposit_log_count_time =
                metrics.deposit_log_count_time.with_unit(UnitOfTime::Micros);
            metrics.deposit_log_page_time =
                metrics.deposit_log_page_time.with_unit(UnitOfTime::Micros);
            metrics.bridge_state_snapshot_time = metrics
                .bridge_state_snapshot_time
                .with_unit(UnitOfTime::Micros);
            metrics.bridge_state_peek_unsettled_deposits_time = metrics
                .bridge_state_peek_unsettled_deposits_time
                .with_unit(UnitOfTime::Micros);
            metrics.bridge_state_peek_unsettled_withdrawals_time = metrics
                .bridge_state_peek_unsettled_withdrawals_time
                .with_unit(UnitOfTime::Micros);
            metrics.bridge_state_peek_base_next_height_time = metrics
                .bridge_state_peek_base_next_height_time
                .with_unit(UnitOfTime::Micros);
            metrics.bridge_state_peek_nock_next_height_time = metrics
                .bridge_state_peek_nock_next_height_time
                .with_unit(UnitOfTime::Micros);
            metrics.bridge_state_peek_base_hold_info_time = metrics
                .bridge_state_peek_base_hold_info_time
                .with_unit(UnitOfTime::Micros);
            metrics.bridge_state_peek_nock_hold_info_time = metrics
                .bridge_state_peek_nock_hold_info_time
                .with_unit(UnitOfTime::Micros);
            metrics.bridge_state_peek_stop_state_time = metrics
                .bridge_state_peek_stop_state_time
                .with_unit(UnitOfTime::Micros);
            metrics.bridge_state_peek_is_fakenet_time = metrics
                .bridge_state_peek_is_fakenet_time
                .with_unit(UnitOfTime::Micros);
            metrics.tui_snapshot_response_time = metrics
                .tui_snapshot_response_time
                .with_unit(UnitOfTime::Micros);
            metrics.tui_snapshot_to_response_time = metrics
                .tui_snapshot_to_response_time
                .with_unit(UnitOfTime::Micros);
            metrics.tui_snapshot_uncached_load_time = metrics
                .tui_snapshot_uncached_load_time
                .with_unit(UnitOfTime::Micros);
            metrics.tui_snapshot_build_cache_time = metrics
                .tui_snapshot_build_cache_time
                .with_unit(UnitOfTime::Micros);
            metrics.tui_snapshot_build_proposals_time = metrics
                .tui_snapshot_build_proposals_time
                .with_unit(UnitOfTime::Micros);
            metrics.withdrawal_frontier_status_fetch_time = metrics
                .withdrawal_frontier_status_fetch_time
                .with_unit(UnitOfTime::Micros);
            metrics.withdrawal_assembly_tick_time = metrics
                .withdrawal_assembly_tick_time
                .with_unit(UnitOfTime::Micros);
            metrics.withdrawal_signing_tick_time = metrics
                .withdrawal_signing_tick_time
                .with_unit(UnitOfTime::Micros);
            metrics.withdrawal_submission_tick_time = metrics
                .withdrawal_submission_tick_time
                .with_unit(UnitOfTime::Micros);
            metrics.sequencer_withdrawal_base_verifier_verify_time = metrics
                .sequencer_withdrawal_base_verifier_verify_time
                .with_unit(UnitOfTime::Micros);
            metrics.sequencer_withdrawal_journal_append_time = metrics
                .sequencer_withdrawal_journal_append_time
                .with_unit(UnitOfTime::Micros);
            Arc::new(metrics)
        })
        .clone()
}

pub fn update_bridge_metrics(network: &NetworkState, last_deposit_nonce: Option<u64>) {
    let metrics = init_metrics();
    update_bridge_metrics_with(&metrics, network, last_deposit_nonce);
}

pub fn update_base_tip_height(tip_height: Option<u64>) {
    let metrics = init_metrics();
    update_base_tip_height_with(&metrics, tip_height);
}

pub fn update_deposit_log_max_nonce(max_nonce: Option<u64>) {
    let metrics = init_metrics();
    update_deposit_log_max_nonce_with(&metrics, max_nonce);
}

pub fn advance_deposit_log_max_nonce(inserted: u64, first_epoch_nonce: u64) {
    let metrics = init_metrics();
    advance_deposit_log_max_nonce_with(&metrics, inserted, first_epoch_nonce);
}

fn update_bridge_metrics_with(
    metrics: &BridgeHealthMetrics,
    network: &NetworkState,
    last_deposit_nonce: Option<u64>,
) {
    let running_metric = if network.kernel_stopped {
        RunningStatusMetric::Stop
    } else {
        RunningStatusMetric::Running
    };
    let hold_base_height = if network.base_hold {
        network.base_hold_height.unwrap_or_default()
    } else {
        0
    };
    let hold_nock_height = if network.nock_hold {
        network.nock_hold_height.unwrap_or_default()
    } else {
        0
    };
    let base_height = if network.base.last_updated.is_some() {
        network.base.height
    } else {
        0
    };
    let nock_height = if network.nockchain.last_updated.is_some() {
        network.nockchain.height
    } else {
        0
    };
    let last_deposit_nonce = last_deposit_nonce.unwrap_or_default();

    metrics.running_status.swap(f64::from(running_metric));
    metrics.base_hold_height.swap(hold_base_height as f64);
    metrics.nock_hold_height.swap(hold_nock_height as f64);
    metrics.base_block_height.swap(base_height as f64);
    metrics.nock_block_height.swap(nock_height as f64);
    metrics.last_deposit_nonce.swap(last_deposit_nonce as f64);
}

fn update_base_tip_height_with(metrics: &BridgeHealthMetrics, tip_height: Option<u64>) {
    metrics
        .base_tip_height
        .swap(tip_height.unwrap_or_default() as f64);
}

fn update_deposit_log_max_nonce_with(metrics: &BridgeHealthMetrics, max_nonce: Option<u64>) {
    metrics
        .deposit_log_max_nonce
        .swap(max_nonce.unwrap_or_default() as f64);
}

fn advance_deposit_log_max_nonce_with(
    metrics: &BridgeHealthMetrics,
    inserted: u64,
    first_epoch_nonce: u64,
) {
    if inserted == 0 {
        return;
    }

    let current = metrics.deposit_log_max_nonce.load() as u64;
    let next = if current == 0 {
        first_epoch_nonce.saturating_add(inserted - 1)
    } else {
        current.saturating_add(inserted)
    };
    metrics.deposit_log_max_nonce.swap(next as f64);
}

#[cfg(test)]
mod tests {
    use gnort::{MetricsRegistry, RegistryConfig};

    use super::{
        advance_deposit_log_max_nonce_with, update_base_tip_height_with,
        update_deposit_log_max_nonce_with, BridgeHealthMetrics,
    };

    #[test]
    fn update_base_tip_height_reports_latest_tip() {
        let metrics =
            BridgeHealthMetrics::register(&MetricsRegistry::new(RegistryConfig::default()))
                .expect("metrics should register");

        update_base_tip_height_with(&metrics, Some(113));
        assert_eq!(metrics.base_tip_height.load(), 113.0);

        update_base_tip_height_with(&metrics, None);
        assert_eq!(metrics.base_tip_height.load(), 0.0);
    }

    #[test]
    fn update_deposit_log_max_nonce_reports_local_log_head() {
        let metrics =
            BridgeHealthMetrics::register(&MetricsRegistry::new(RegistryConfig::default()))
                .expect("metrics should register");

        update_deposit_log_max_nonce_with(&metrics, Some(113));
        assert_eq!(metrics.deposit_log_max_nonce.load(), 113.0);

        update_deposit_log_max_nonce_with(&metrics, None);
        assert_eq!(metrics.deposit_log_max_nonce.load(), 0.0);
    }

    #[test]
    fn advance_deposit_log_max_nonce_avoids_requerying_the_log() {
        let metrics =
            BridgeHealthMetrics::register(&MetricsRegistry::new(RegistryConfig::default()))
                .expect("metrics should register");

        advance_deposit_log_max_nonce_with(&metrics, 2, 101);
        assert_eq!(metrics.deposit_log_max_nonce.load(), 102.0);

        advance_deposit_log_max_nonce_with(&metrics, 3, 101);
        assert_eq!(metrics.deposit_log_max_nonce.load(), 105.0);

        advance_deposit_log_max_nonce_with(&metrics, 0, 101);
        assert_eq!(metrics.deposit_log_max_nonce.load(), 105.0);
    }
}
