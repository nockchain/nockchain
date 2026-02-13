use std::sync::{Arc, OnceLock};

use gnort::instrument::UnitOfTime;
use gnort::*;

use crate::tui::types::NetworkState;

metrics_struct![
    BridgeHealthMetrics,
    (running_status, "bridge.health.running_status", Gauge),
    (base_hold_height, "bridge.health.base_hold_height", Gauge),
    (nock_hold_height, "bridge.health.nock_hold_height", Gauge),
    (base_block_height, "bridge.health.base_block_height", Gauge),
    (nock_block_height, "bridge.health.nock_block_height", Gauge),
    (last_deposit_nonce, "bridge.health.last_deposit_nonce", Gauge),
    (
        ingress_broadcast_signature_requests,
        "bridge.ingress.broadcast_signature.requests",
        Count
    ),
    (
        ingress_broadcast_signature_invalid_deposit_id_len,
        "bridge.ingress.broadcast_signature.invalid.deposit_id_length",
        Count
    ),
    (
        ingress_broadcast_signature_invalid_proposal_hash_len,
        "bridge.ingress.broadcast_signature.invalid.proposal_hash_length",
        Count
    ),
    (
        ingress_broadcast_signature_invalid_signature_len,
        "bridge.ingress.broadcast_signature.invalid.signature_length",
        Count
    ),
    (
        ingress_broadcast_signature_invalid_signer_address_len,
        "bridge.ingress.broadcast_signature.invalid.signer_address_length",
        Count
    ),
    (
        ingress_broadcast_signature_invalid_deposit_id_decode,
        "bridge.ingress.broadcast_signature.invalid.deposit_id_decode",
        Count
    ),
    (
        ingress_broadcast_signature_ignored_self,
        "bridge.ingress.broadcast_signature.ignored.self",
        Count
    ),
    (
        ingress_broadcast_signature_known_proposal,
        "bridge.ingress.broadcast_signature.proposal.known",
        Count
    ),
    (
        ingress_broadcast_signature_unknown_proposal,
        "bridge.ingress.broadcast_signature.proposal.unknown",
        Count
    ),
    (
        ingress_broadcast_signature_known_signer,
        "bridge.ingress.broadcast_signature.signer.known",
        Count
    ),
    (
        ingress_broadcast_signature_unknown_signer,
        "bridge.ingress.broadcast_signature.signer.unknown",
        Count
    ),
    (
        ingress_broadcast_signature_unknown_signer_known_proposal,
        "bridge.ingress.broadcast_signature.signer.unknown_for_known_proposal",
        Count
    ),
    (
        ingress_broadcast_signature_hash_mismatch,
        "bridge.ingress.broadcast_signature.proposal_hash_mismatch",
        Count
    ),
    (
        ingress_broadcast_signature_result_added,
        "bridge.ingress.broadcast_signature.result.added",
        Count
    ),
    (
        ingress_broadcast_signature_result_threshold_reached,
        "bridge.ingress.broadcast_signature.result.threshold_reached",
        Count
    ),
    (
        ingress_broadcast_signature_result_duplicate,
        "bridge.ingress.broadcast_signature.result.duplicate",
        Count
    ),
    (
        ingress_broadcast_signature_result_stale,
        "bridge.ingress.broadcast_signature.result.stale",
        Count
    ),
    (
        ingress_broadcast_signature_result_invalid,
        "bridge.ingress.broadcast_signature.result.invalid",
        Count
    ),
    (
        ingress_broadcast_signature_result_error,
        "bridge.ingress.broadcast_signature.result.error",
        Count
    ),
    (
        proposal_cache_total,
        "bridge.proposal_cache.entries.total",
        Gauge
    ),
    (
        proposal_cache_collecting,
        "bridge.proposal_cache.entries.collecting",
        Gauge
    ),
    (
        proposal_cache_ready,
        "bridge.proposal_cache.entries.ready",
        Gauge
    ),
    (
        proposal_cache_posting,
        "bridge.proposal_cache.entries.posting",
        Gauge
    ),
    (
        proposal_cache_confirmed,
        "bridge.proposal_cache.entries.confirmed",
        Gauge
    ),
    (
        proposal_cache_failed,
        "bridge.proposal_cache.entries.failed",
        Gauge
    ),
    (
        proposal_cache_total_peer_signatures,
        "bridge.proposal_cache.signatures.peer.total",
        Gauge
    ),
    (
        proposal_cache_max_peer_signatures_per_proposal,
        "bridge.proposal_cache.signatures.peer.max_per_proposal",
        Gauge
    ),
    (
        proposal_cache_proposals_with_my_signature,
        "bridge.proposal_cache.signatures.my.proposals",
        Gauge
    ),
    (
        proposal_cache_pending_signature_deposit_count,
        "bridge.proposal_cache.pending.deposits",
        Gauge
    ),
    (
        proposal_cache_pending_signature_total,
        "bridge.proposal_cache.pending.signatures",
        Gauge
    ),
    (
        proposal_cache_oldest_age_secs,
        "bridge.proposal_cache.age.oldest_seconds",
        Gauge
    ),
    (
        proposal_cache_oldest_confirmed_age_secs,
        "bridge.proposal_cache.age.oldest_confirmed_seconds",
        Gauge
    ),
    (
        proposal_cache_oldest_failed_age_secs,
        "bridge.proposal_cache.age.oldest_failed_seconds",
        Gauge
    ),
    (
        proposal_cache_pending_oldest_age_secs,
        "bridge.proposal_cache.age.oldest_pending_signature_seconds",
        Gauge
    ),
    (
        proposal_cache_approx_state_bytes,
        "bridge.proposal_cache.approx_bytes.states",
        Gauge
    ),
    (
        proposal_cache_approx_peer_signature_bytes,
        "bridge.proposal_cache.approx_bytes.peer_signatures",
        Gauge
    ),
    (
        proposal_cache_approx_my_signature_bytes,
        "bridge.proposal_cache.approx_bytes.my_signatures",
        Gauge
    ),
    (
        proposal_cache_approx_pending_signature_bytes,
        "bridge.proposal_cache.approx_bytes.pending_signatures",
        Gauge
    ),
    (
        proposal_cache_approx_total_bytes,
        "bridge.proposal_cache.approx_bytes.total",
        Gauge
    ),
    (
        proposal_cache_metrics_update_error,
        "bridge.proposal_cache.metrics_update_error",
        Count
    ),
    (
        proposal_cache_pending_signature_queued_unknown_deposit,
        "bridge.proposal_cache.pending.queued_unknown_deposit",
        Count
    ),
    (
        proposal_cache_pending_signature_applied,
        "bridge.proposal_cache.pending.applied",
        Count
    ),
    (
        proposal_cache_pending_signature_mismatched,
        "bridge.proposal_cache.pending.mismatched_hash",
        Count
    ),
    (
        proposal_cache_pending_signature_verify_failed,
        "bridge.proposal_cache.pending.verify_failed",
        Count
    ),
    (
        proposal_cache_pending_signature_address_mismatch,
        "bridge.proposal_cache.pending.address_mismatch",
        Count
    ),
    (
        proposal_cache_signature_duplicate,
        "bridge.proposal_cache.signature.duplicate",
        Count
    ),
    (
        proposal_cache_signature_verify_failed,
        "bridge.proposal_cache.signature.verify_failed",
        Count
    ),
    (
        proposal_cache_signature_address_mismatch,
        "bridge.proposal_cache.signature.address_mismatch",
        Count
    ),
    (
        proposal_cache_gc_runs,
        "bridge.proposal_cache.gc.runs",
        Count
    ),
    (
        proposal_cache_gc_last_removed,
        "bridge.proposal_cache.gc.last_removed",
        Gauge
    ),
    (
        tui_snapshot_requests,
        "bridge.tui.snapshot.requests",
        Count
    ),
    (
        tui_snapshot_uncached_requests,
        "bridge.tui.snapshot.uncached_requests",
        Count
    ),
    (
        tui_snapshot_alert_limit_requested,
        "bridge.tui.snapshot.requested.alert_limit",
        Gauge
    ),
    (
        tui_snapshot_limit_requested,
        "bridge.tui.snapshot.requested.limit",
        Gauge
    ),
    (
        tui_snapshot_offset_requested,
        "bridge.tui.snapshot.requested.offset",
        Gauge
    ),
    (
        tui_snapshot_limit_over_cache,
        "bridge.tui.snapshot.requested.limit_over_cache",
        Count
    ),
    (
        tui_snapshot_limit_over_10000,
        "bridge.tui.snapshot.requested.limit_over_10000",
        Count
    ),
    (
        tui_snapshot_response_time,
        "bridge.tui.snapshot.response_time",
        TimingCount
    ),
    (
        tui_snapshot_to_response_time,
        "bridge.tui.snapshot.to_response_time",
        TimingCount
    ),
    (
        tui_snapshot_uncached_load_time,
        "bridge.tui.snapshot.uncached_load_time",
        TimingCount
    ),
    (
        tui_snapshot_build_cache_time,
        "bridge.tui.snapshot.build_cache_time",
        TimingCount
    ),
    (
        tui_snapshot_build_proposals_time,
        "bridge.tui.snapshot.build_proposals_time",
        TimingCount
    ),
    (
        tui_proposals_pending_inbound_count,
        "bridge.tui.proposals.pending_inbound_count",
        Gauge
    ),
    (
        tui_proposals_history_count,
        "bridge.tui.proposals.history_count",
        Gauge
    ),
    (
        tui_proposals_last_submitted_present,
        "bridge.tui.proposals.last_submitted_present",
        Gauge
    ),
    (
        tui_proposals_pending_inbound_signature_count,
        "bridge.tui.proposals.pending_inbound_signature_count",
        Gauge
    ),
    (
        tui_proposals_history_signature_count,
        "bridge.tui.proposals.history_signature_count",
        Gauge
    ),
    (
        tui_proposals_pending_inbound_approx_bytes,
        "bridge.tui.proposals.pending_inbound_approx_bytes",
        Gauge
    ),
    (
        tui_proposals_history_approx_bytes,
        "bridge.tui.proposals.history_approx_bytes",
        Gauge
    ),
    (
        tui_proposals_last_submitted_approx_bytes,
        "bridge.tui.proposals.last_submitted_approx_bytes",
        Gauge
    ),
    (
        tui_proposals_approx_total_bytes,
        "bridge.tui.proposals.approx_total_bytes",
        Gauge
    ),
    (
        deposit_log_snapshot_time,
        "bridge.tui.deposit_log.snapshot_time",
        TimingCount
    ),
    (
        deposit_log_count_time,
        "bridge.tui.deposit_log.count_time",
        TimingCount
    ),
    (
        deposit_log_page_time,
        "bridge.tui.deposit_log.page_time",
        TimingCount
    ),
    (
        bridge_state_snapshot_time,
        "bridge.runtime.bridge_state.snapshot_time",
        TimingCount
    ),
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
    (
        tui_deposit_log_limit_requested,
        "bridge.tui.deposit_log.requested.limit",
        Gauge
    ),
    (
        tui_deposit_log_offset_requested,
        "bridge.tui.deposit_log.requested.offset",
        Gauge
    ),
    (
        tui_deposit_log_rows_returned,
        "bridge.tui.deposit_log.returned.rows",
        Gauge
    ),
    (
        tui_deposit_log_limit_over_cache,
        "bridge.tui.deposit_log.requested.limit_over_cache",
        Count
    ),
    (
        tui_deposit_log_limit_over_10000,
        "bridge.tui.deposit_log.requested.limit_over_10000",
        Count
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
            metrics.tui_snapshot_response_time =
                metrics.tui_snapshot_response_time.with_unit(UnitOfTime::Micros);
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
            Arc::new(metrics)
        })
        .clone()
}

pub fn update_bridge_metrics(network: &NetworkState, last_deposit_nonce: Option<u64>) {
    let metrics = init_metrics();

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
