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
    (response_failed_not_dropped, "nockchain-libp2p-io.response_failed_not_dropped", Count),
    (response_dropped, "nockchain-libp2p-io.response_dropped", Count),
    // Per-cause poke timings
    (timer_poke_time, "nockchain-libp2p-io.timer_poke_time", TimingCount),
    (heard_tx_poke_time, "nockchain-libp2p-io.heard_tx_poke_time", TimingCount),
    (heard_block_poke_time, "nockchain-libp2p-io.heard_block_poke_time", TimingCount),
    (heard_elders_poke_time, "nockchain-libp2p-io.heard_elders_poke_time", TimingCount)
];
