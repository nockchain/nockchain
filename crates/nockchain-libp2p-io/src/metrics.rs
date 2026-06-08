use gnort::*;

metrics_struct![
    NockchainP2PMetrics,
    (gossip_acked_heard_block, "nockchain-libp2p-io.gossip_acked_heard_block", Count),
    (gossip_acked_heard_tx, "nockchain-libp2p-io.gossip_acked_heard_tx", Count),
    (gossip_acked_heard_elders, "nockchain-libp2p-io.gossip_acked_heard_elders", Count),
    (gossip_nacked_heard_block, "nockchain-libp2p-io.gossip_nacked_heard_block", Count),
    (gossip_nacked_heard_tx, "nockchain-libp2p-io.gossip_nacked_heard_tx", Count),
    (gossip_nacked_heard_elders, "nockchain-libp2p-io.gossip_nacked_heard_elders", Count),
    (gossip_erred_heard_block, "nockchain-libp2p-io.gossip_erred_heard_block", Count),
    (gossip_erred_heard_tx, "nockchain-libp2p-io.gossip_erred_heard_tx", Count),
    (gossip_erred_heard_elders, "nockchain-libp2p-io.gossip_erred_heard_elders", Count),
    (gossip_dropped, "nockchain-libp2p-io.gossip_dropped", Count),
    (legacy_gossip_received, "nockchain-libp2p-io.legacy_gossip_received", Count),
    (
        legacy_gossip_compatibility_rejected,
        "nockchain-libp2p-io.legacy_gossip_compatibility_rejected", Count
    ),
    (authenticated_gossip_verified, "nockchain-libp2p-io.authenticated_gossip_verified", Count),
    (authenticated_gossip_sent, "nockchain-libp2p-io.authenticated_gossip_sent", Count),
    (requests_peeked_some, "nockchain-libp2p-io.requests_peeked_some", Count),
    (requests_peeked_none, "nockchain-libp2p-io.requests_peeked_none", Count),
    (requests_erred_block_by_height, "nockchain-libp2p-io.requests_erred_block_by_height", Count),
    (requests_erred_elders_by_id, "nockchain-libp2p-io.requests_erred_elders_by_id", Count),
    (requests_erred_raw_tx_by_id, "nockchain-libp2p-io.requests_erred_raw_tx_by_id", Count),
    (requests_dropped, "nockchain-libp2p-io.requests_dropped", Count),
    (requests_crown_error_external, "nockchain-libp2p-io.requests_crown_error_external", Count),
    (requests_crown_error_mutex, "nockchain-libp2p-io.requests_crown_error_mutex", Count),
    (
        requests_crown_error_invalid_kernel_input,
        "nockchain-libp2p-io.requests_crown_error_invalid_kernel_input", Count
    ),
    (
        requests_crown_error_unknown_effect,
        "nockchain-libp2p-io.requests_crown_error_unknown_effect", Count
    ),
    (requests_crown_error_io_error, "nockchain-libp2p-io.requests_crown_error_io_error", Count),
    (
        requests_crown_error_noun_error, "nockchain-libp2p-io.requests_crown_error_noun_error",
        Count
    ),
    (
        requests_crown_error_interpreter_error,
        "nockchain-libp2p-io.requests_crown_error_interpreter_error", Count
    ),
    (
        requests_crown_error_kernel_error, "nockchain-libp2p-io.requests_crown_error_kernel_error",
        Count
    ),
    (
        requests_crown_error_utf8_from_error,
        "nockchain-libp2p-io.requests_crown_error_utf8_from_error", Count
    ),
    (
        requests_crown_error_utf8_error, "nockchain-libp2p-io.requests_crown_error_utf8_error",
        Count
    ),
    (
        requests_crown_error_newt_error, "nockchain-libp2p-io.requests_crown_error_newt_error",
        Count
    ),
    (
        requests_crown_error_boot_error, "nockchain-libp2p-io.requests_crown_error_boot_error",
        Count
    ),
    (
        requests_crown_error_serf_load_error,
        "nockchain-libp2p-io.requests_crown_error_serf_load_error", Count
    ),
    (
        requests_crown_error_serf_init_allocation_error,
        "nockchain-libp2p-io.requests_crown_error_serf_init_allocation_error", Count
    ),
    (
        requests_crown_error_serf_init_panic,
        "nockchain-libp2p-io.requests_crown_error_serf_init_panic", Count
    ),
    (requests_crown_error_work_bail, "nockchain-libp2p-io.requests_crown_error_work_bail", Count),
    (requests_crown_error_peek_bail, "nockchain-libp2p-io.requests_crown_error_peek_bail", Count),
    (requests_crown_error_work_swap, "nockchain-libp2p-io.requests_crown_error_work_swap", Count),
    (
        requests_crown_error_tank_error, "nockchain-libp2p-io.requests_crown_error_tank_error",
        Count
    ),
    (requests_crown_error_play_bail, "nockchain-libp2p-io.requests_crown_error_play_bail", Count),
    (
        requests_crown_error_queue_recv, "nockchain-libp2p-io.requests_crown_error_queue_recv",
        Count
    ),
    (
        requests_crown_error_save_error, "nockchain-libp2p-io.requests_crown_error_save_error",
        Count
    ),
    (requests_crown_error_int_error, "nockchain-libp2p-io.requests_crown_error_int_error", Count),
    (
        requests_crown_error_join_error, "nockchain-libp2p-io.requests_crown_error_join_error",
        Count
    ),
    (
        requests_crown_error_decode_error, "nockchain-libp2p-io.requests_crown_error_decode_error",
        Count
    ),
    (
        requests_crown_error_encode_error, "nockchain-libp2p-io.requests_crown_error_encode_error",
        Count
    ),
    (
        requests_crown_error_state_jam_format_error,
        "nockchain-libp2p-io.requests_crown_error_state_jam_format_error", Count
    ),
    (requests_crown_error_unknown, "nockchain-libp2p-io.requests_crown_error_unknown", Count),
    (
        requests_crown_error_conversion_error,
        "nockchain-libp2p-io.requests_crown_error_conversion_error", Count
    ),
    (
        requests_crown_error_unknown_error,
        "nockchain-libp2p-io.requests_crown_error_unknown_error", Count
    ),
    (
        requests_crown_error_queue_error, "nockchain-libp2p-io.requests_crown_error_queue_error",
        Count
    ),
    (
        requests_crown_error_serf_mpsc_error,
        "nockchain-libp2p-io.requests_crown_error_serf_mpsc_error", Count
    ),
    (
        requests_crown_error_oneshot_channel_error,
        "nockchain-libp2p-io.requests_crown_error_oneshot_channel_error", Count
    ),
    (responses_acked_heard_block, "nockchain-libp2p-io.responses_acked_heard_block", Count),
    (responses_acked_heard_tx, "nockchain-libp2p-io.responses_acked_heard_tx", Count),
    (responses_acked_heard_elders, "nockchain-libp2p-io.responses_acked_heard_elders", Count),
    (responses_nacked_heard_block, "nockchain-libp2p-io.responses_nacked_heard_block", Count),
    (responses_nacked_heard_tx, "nockchain-libp2p-io.responses_nacked_heard_tx", Count),
    (responses_nacked_heard_elders, "nockchain-libp2p-io.responses_nacked_heard_elders", Count),
    (responses_erred_heard_block, "nockchain-libp2p-io.responses_erred_heard_block", Count),
    (responses_erred_heard_tx, "nockchain-libp2p-io.responses_erred_heard_tx", Count),
    (responses_erred_heard_elders, "nockchain-libp2p-io.responses_erred_heard_elders", Count),
    (responses_dropped, "nockchain-libp2p-io.responses_dropped", Count),
    (block_request_cache_hits, "nockchain-libp2p-io.block_request_cache_hits", Count),
    (tx_request_cache_hits, "nockchain-libp2p-io.tx_request_cache_hits", Count),
    (block_seen_cache_hits, "nockchain-libp2p-io.block_seen_cache_hits", Count),
    (tx_seen_cache_hits, "nockchain-libp2p-io.tx_seen_cache_hits", Count),
    (block_request_cache_misses, "nockchain-libp2p-io.block_request_cache_misses", Count),
    (block_request_cache_negative, "nockchain-libp2p-io.block_request_cache_negative", Count),
    (tx_request_cache_misses, "nockchain-libp2p-io.tx_request_cache_misses", Count),
    (block_seen_cache_misses, "nockchain-libp2p-io.block_seen_cache_misses", Count),
    (tx_seen_cache_misses, "nockchain-libp2p-io.tx_seen_cache_misses", Count),
    (highest_block_height_seen, "nockchain-libp2p-io.highest_block_height_seen", Gauge),
    (peer_count, "nockchain-libp2p-io.peer_count", Gauge),
    // Peer connection health
    (peer_connections_established, "nockchain-libp2p-io.peer_connections_established", Count),
    (peer_connections_closed, "nockchain-libp2p-io.peer_connections_closed", Count),
    (peer_connection_failures, "nockchain-libp2p-io.peer_connection_failures", Count),
    (
        incoming_connections_blocked_by_limits,
        "nockchain-libp2p-io.incoming_connections_blocked_by_limits", Count
    ),
    (incoming_connections_pruned, "nockchain-libp2p-io.incoming_connections_pruned", Count),
    (kademlia_bootstrap_attempts, "nockchain-libp2p-io.kademlia_bootstrap_attempts", Count),
    (kademlia_bootstrap_failures, "nockchain-libp2p-io.kademlia_bootstrap_failures", Count),
    (active_peer_connections, "nockchain-libp2p-io.active_peer_connections", Gauge),
    // Block sync progress
    (blocks_requested_by_height, "nockchain-libp2p-io.blocks_requested_by_height", Count),
    (blocks_received_by_height, "nockchain-libp2p-io.blocks_received_by_height", Count),
    (block_request_timeouts, "nockchain-libp2p-io.block_request_timeouts", Count),
    (last_block_height_received, "nockchain-libp2p-io.last_block_height_received", Gauge),
    // Request/response patterns
    (
        request_response_active_streams, "nockchain-libp2p-io.request_response_active_streams",
        Gauge
    ),
    (peer_request_rate_limited, "nockchain-libp2p-io.peer_request_rate_limited", Count),
    (request_failed, "nockchain-libp2p-io.request_failed", Count),
    (gen1_outbound_failures, "nockchain-libp2p-io.gen1_outbound_failures", Count),
    (gen2_outbound_failures, "nockchain-libp2p-io.gen2_outbound_failures", Count),
    (gen1_outbound_timeouts, "nockchain-libp2p-io.gen1_outbound_timeouts", Count),
    (gen2_outbound_timeouts, "nockchain-libp2p-io.gen2_outbound_timeouts", Count),
    (gen2_batch_requests_sent, "nockchain-libp2p-io.gen2_batch_requests_sent", Count),
    (gen2_batch_requests_received, "nockchain-libp2p-io.gen2_batch_requests_received", Count),
    (gen2_batch_items_sent, "nockchain-libp2p-io.gen2_batch_items_sent", Count),
    (gen2_batch_items_received, "nockchain-libp2p-io.gen2_batch_items_received", Count),
    (gen2_batch_rejected_malformed, "nockchain-libp2p-io.gen2_batch_rejected_malformed", Count),
    (
        gen2_batch_rejected_too_many_items,
        "nockchain-libp2p-io.gen2_batch_rejected_too_many_items", Count
    ),
    (
        gen2_batch_rejected_too_many_bytes,
        "nockchain-libp2p-io.gen2_batch_rejected_too_many_bytes", Count
    ),
    (
        gen2_batch_rejected_backpressure, "nockchain-libp2p-io.gen2_batch_rejected_backpressure",
        Count
    ),
    (gen2_batch_item_error_decode, "nockchain-libp2p-io.gen2_batch_item_error_decode", Count),
    (
        gen2_batch_item_error_backpressure,
        "nockchain-libp2p-io.gen2_batch_item_error_backpressure", Count
    ),
    (
        gen2_batch_item_error_too_large, "nockchain-libp2p-io.gen2_batch_item_error_too_large",
        Count
    ),
    (
        gen2_batch_item_error_invalid_pow, "nockchain-libp2p-io.gen2_batch_item_error_invalid_pow",
        Count
    ),
    (gen2_batch_item_error_internal, "nockchain-libp2p-io.gen2_batch_item_error_internal", Count),
    (
        gen2_batch_result_unexpected_item_id,
        "nockchain-libp2p-io.gen2_batch_result_unexpected_item_id", Count
    ),
    (req_res_fallback_total, "nockchain-libp2p-io.req_res_fallback_total", Count),
    (
        req_res_block_by_height_gen1_routed,
        "nockchain-libp2p-io.req_res_block_by_height_gen1_routed", Count
    ),
    (req_res_retry_scheduled_total, "nockchain-libp2p-io.req_res_retry_scheduled_total", Count),
    (
        req_res_effect_dedup_suppressed, "nockchain-libp2p-io.req_res_effect_dedup_suppressed",
        Count
    ),
    (req_res_inflight_total, "nockchain-libp2p-io.req_res_inflight_total", Gauge),
    (req_res_inflight_max_per_peer, "nockchain-libp2p-io.req_res_inflight_max_per_peer", Gauge),
    (gen2_batch_pending_items, "nockchain-libp2p-io.gen2_batch_pending_items", Gauge),
    (gen2_batch_pending_peers, "nockchain-libp2p-io.gen2_batch_pending_peers", Gauge),
    (response_failed_not_dropped, "nockchain-libp2p-io.response_failed_not_dropped", Count),
    (response_dropped, "nockchain-libp2p-io.response_dropped", Count),
    (ip_exclusions_active, "nockchain-libp2p-io.ip_exclusions_active", Gauge),
    (address_cooldowns_active, "nockchain-libp2p-io.address_cooldowns_active", Gauge),
    (ip_exclusions_created, "nockchain-libp2p-io.ip_exclusions_created", Count),
    (ip_exclusions_expired, "nockchain-libp2p-io.ip_exclusions_expired", Count),
    (ip_exclusion_dial_denied, "nockchain-libp2p-io.ip_exclusion_dial_denied", Count),
    // Legacy name; despite "dial_denied" it actually counts cooldown
    // *creations*, not denied dials. Kept for dashboard backward compatibility;
    // new dashboards should prefer `address_cooldowns_created` below. See #2013.
    (address_cooldown_dial_denied, "nockchain-libp2p-io.address_cooldown_dial_denied", Count),
    (address_cooldowns_created, "nockchain-libp2p-io.address_cooldowns_created", Count),
    (
        kad_addresses_pruned_for_exclusion,
        "nockchain-libp2p-io.kad_addresses_pruned_for_exclusion", Count
    ),
    (kad_peers_pruned_for_exclusion, "nockchain-libp2p-io.kad_peers_pruned_for_exclusion", Count),
    (
        identify_addresses_skipped_for_exclusion,
        "nockchain-libp2p-io.identify_addresses_skipped_for_exclusion", Count
    ),
    (
        fast_sync_peers_skipped_for_health,
        "nockchain-libp2p-io.fast_sync_peers_skipped_for_health", Count
    ),
    (request_peer_cooldowns_created, "nockchain-libp2p-io.request_peer_cooldowns_created", Count),
    (wrong_peer_id_observed, "nockchain-libp2p-io.wrong_peer_id_observed", Count),
    (local_peer_abuse_recorded, "nockchain-libp2p-io.local_peer_abuse_recorded", Count),
    (request_replay_rejected, "nockchain-libp2p-io.request_replay_rejected", Count),
    (ip_bucket_connection_rejected, "nockchain-libp2p-io.ip_bucket_connection_rejected", Count),
    (ip_bucket_request_rejected, "nockchain-libp2p-io.ip_bucket_request_rejected", Count),
    (gossip_ip_bucket_rejected, "nockchain-libp2p-io.gossip_ip_bucket_rejected", Count),
    (same_ip_kad_cardinality, "nockchain-libp2p-io.same_ip_kad_cardinality", Gauge),
    // Per-cause poke timings
    (timer_poke_time, "nockchain-libp2p-io.timer_poke_time", TimingCount),
    (heard_tx_poke_time, "nockchain-libp2p-io.heard_tx_poke_time", TimingCount),
    (heard_block_poke_time, "nockchain-libp2p-io.heard_block_poke_time", TimingCount),
    (heard_elders_poke_time, "nockchain-libp2p-io.heard_elders_poke_time", TimingCount),
    // ---- deadlock / livelock watchdog signals ----
    // These are the Datadog-facing side of the 2026-04-17/18 incidents.
    // Alert recipes live in the Datadog monitors; these metrics feed them.
    //
    // heartbeat_tick: monotonic counter bumped every 5s by the libp2p
    // driver heartbeat task. `rate() == 0` for > 30s means the tokio
    // runtime itself is parked (4/17 shape).
    (heartbeat_tick, "nockchain-libp2p-io.heartbeat_tick", Count),
    // poke_completion_lag_seconds: seconds since the traffic cop last
    // observed a successful kernel-poke round trip. `max > 180` means
    // the kernel side wedged even though the driver runtime is fine
    // (4/18 shape). Exported as a gauge so Datadog can alert on max
    // across any instance.
    (poke_completion_lag_seconds, "nockchain-libp2p-io.poke_completion_lag_seconds", Gauge),
    // oneshot_recv_error_total: every time a `handle_request_response`
    // task returns Err(OneShotRecvError). Pre-fix (2026-04-17) this ran
    // at ~16 k/hr on LAX1; post-fix it is 0. An alert at `rate > 10/s`
    // would catch any regression of the gated-poke timing-drop bug.
    (oneshot_recv_error_total, "nockchain-libp2p-io.oneshot_recv_error_total", Count),
    // watchdog_stack_dump_total: every time the watchdog (or a SIGQUIT)
    // writes a stack dump. Any non-zero value is an operational signal
    // worth paging on, by construction, a dump only fires on a stall
    // or an operator request, both of which deserve attention.
    (watchdog_stack_dump_total, "nockchain-libp2p-io.watchdog_stack_dump_total", Count),
    // ---- catch-up signal (Phase 1 of catch-up prefetch epic) ----
    // Observation only; nothing yet acts on these. See `catch_up.rs`.
    //
    // sync_mode: 0 = Cold, 1 = CatchingUp, 2 = Tip. Encoded as a numeric
    // gauge so Datadog can plot transitions; the corresponding label
    // string is in trace logs.
    (sync_mode, "nockchain-libp2p-io.sync_mode", Gauge),
    // sync_mode_transitions_total: bumps once per mode change. Sustained
    // flapping would show up as a high rate without a corresponding
    // sync-progress signal.
    (sync_mode_transitions_total, "nockchain-libp2p-io.sync_mode_transitions_total", Count),
    // behind_tip_estimate: lower bound on tip - frontier, taken from the
    // strongest of (max-deferred-height, peer-observed-max-height). 0
    // when no signal indicates we are behind.
    (behind_tip_estimate, "nockchain-libp2p-io.behind_tip_estimate", Gauge),
    // deferred_blocks_above_frontier: max-deferred-height - frontier.
    // Distinct from `behind_tip_estimate` because it reflects only
    // gossip-derived backlog, not peer responses.
    (deferred_blocks_above_frontier, "nockchain-libp2p-io.deferred_blocks_above_frontier", Gauge),
    // peer_observed_max_height: highest height seen in any successful
    // outbound block response. Demonstrates that some peer has at least
    // this much chain.
    (peer_observed_max_height, "nockchain-libp2p-io.peer_observed_max_height", Gauge),
    // ---- Phase 2: deferred-buffer cache short-circuit ----
    // prefetch_cache_hits_total: kernel-emitted block-by-height requests
    // that were satisfied from the deferred buffer without outbound
    // dispatch. Each hit saves one network round-trip.
    (prefetch_cache_hits_total, "nockchain-libp2p-io.prefetch_cache_hits_total", Count),
    // prefetch_cache_misses_total: kernel-emitted block-by-height
    // requests that were not in the deferred buffer and went out to
    // the network as usual. Hit / (hit + miss) gives the cache-hit rate.
    (prefetch_cache_misses_total, "nockchain-libp2p-io.prefetch_cache_misses_total", Count),
    // prefetch_buffer_size: total deferred heard-blocks across all
    // heights, exposed as a gauge so growth and drain are both visible.
    (prefetch_buffer_size, "nockchain-libp2p-io.prefetch_buffer_size", Gauge),
    // ---- Phase 4: catch-up triggered prefetch issuance ----
    // prefetch_issued_total: catch-up range prefetches dispatched in
    // place of a kernel-singleton block-by-height request.
    (prefetch_issued_total, "nockchain-libp2p-io.prefetch_issued_total", Count),
    // prefetch_singleton_suppressed_total: kernel-singleton requests
    // suppressed because an inflight prefetch already covers the
    // requested height.
    (
        prefetch_singleton_suppressed_total,
        "nockchain-libp2p-io.prefetch_singleton_suppressed_total", Count
    ),
    // prefetch_no_eligible_peer_total: catch-up prefetch eligibility
    // checks where no connected peer satisfied range-capability and
    // per-peer inflight cap. Drops back to the existing singleton path.
    (
        prefetch_no_eligible_peer_total, "nockchain-libp2p-io.prefetch_no_eligible_peer_total",
        Count
    ),
    // ---- Phase 5: hardening ----
    // prefetch_invalidated_total: deferred-buffer entries dropped because
    // the kernel reported them as a liar-block (forked / poisoned). One
    // increment per dropped block id; growth here without a corresponding
    // sync-progress signal is the LAX1-shape stall signature.
    (prefetch_invalidated_total, "nockchain-libp2p-io.prefetch_invalidated_total", Count),
    // prefetch_height_stuck_total: per-height retry budget exhaustions.
    // Once a height crosses this threshold we back off rather than
    // re-amplify alternates. Should be near-zero in steady state.
    (prefetch_height_stuck_total, "nockchain-libp2p-io.prefetch_height_stuck_total", Count),
    // prefetch_throttled_total: prefetch issuance suppressed because the
    // candidate peer's 60s prefetch byte total exceeded the cap. Bounds
    // bad-peer amplification.
    (prefetch_throttled_total, "nockchain-libp2p-io.prefetch_throttled_total", Count),
    // ---- capability-aware catch-up peer selection ----
    // prefetch_peer_selected_total: range-prefetch peer choices accepted
    // by the capability-aware picker.
    (prefetch_peer_selected_total, "nockchain-libp2p-io.prefetch_peer_selected_total", Count),
    // prefetch_peer_probe_total: bounded probes sent to Gen2 peers with
    // unknown range-serving capability.
    (prefetch_peer_probe_total, "nockchain-libp2p-io.prefetch_peer_probe_total", Count),
    // prefetch_peer_capability_supported_total: peers promoted to
    // range-supported after a successful range response.
    (
        prefetch_peer_capability_supported_total,
        "nockchain-libp2p-io.prefetch_peer_capability_supported_total", Count
    ),
    // prefetch_peer_capability_unsupported_total: peers marked unable to
    // serve the range request shape.
    (
        prefetch_peer_capability_unsupported_total,
        "nockchain-libp2p-io.prefetch_peer_capability_unsupported_total", Count
    ),
    // prefetch_peer_cooldown_total: transient range failures that put a
    // peer on range-prefetch cooldown.
    (prefetch_peer_cooldown_total, "nockchain-libp2p-io.prefetch_peer_cooldown_total", Count),
    // prefetch_peer_no_gen2_range_peer_total: range prefetch skipped
    // because no connected Gen2 peer could be selected.
    (
        prefetch_peer_no_gen2_range_peer_total,
        "nockchain-libp2p-io.prefetch_peer_no_gen2_range_peer_total", Count
    ),
    // ---- outgoing-gossip suppression while behind tip ----
    // gossip_suppressed_behind_tip_total: kernel-emitted %gossip effects
    // (heard-block / heard-tx / future variants) that were NOT broadcast
    // because the catch-up signal reports SyncMode::CatchingUp. While
    // catching up, a node is intentionally quiet: no historic block
    // rebroadcasts, local tx submission gossip, or mining output. Should
    // fall to ~0 once a node reaches Tip.
    (
        gossip_suppressed_behind_tip_total,
        "nockchain-libp2p-io.gossip_suppressed_behind_tip_total", Count
    )
];
