use gnort::*;

metrics_struct![
    NockAppMetrics,
    (handle_shutdown, "nockapp.handle_shutdown", Count),
    (handle_save_permit_res, "nockapp.handle_save_permit_res", Count),
    (handle_action, "nockapp.handle_action", Count),
    (handle_exit, "nockapp.handle_exit", Count),
    (poke_during_exit, "nockapp.poke_during_exit", Count),
    (peek_during_exit, "nockapp.peek_during_exit", Count),
    (least_free_space_seen_in_slam, "nockapp.least_free_space_seen_in_slam", Gauge),
    (save_jam_time, "nockapp.save_jam_time", TimingCount),
    (load_cue_time, "nockapp.load_cue_time", TimingCount),
    (serf_loop_blocking_recv, "nockapp.serf_loop.blocking_recv", TimingCount),
    (serf_loop_all, "nockapp.serf_loop.all", TimingCount),
    (serf_loop_load_state, "nockapp.serf_loop.load_state", TimingCount),
    (serf_loop_get_state_bytes, "nockapp.serf_loop.get_state_bytes", TimingCount),
    (serf_loop_get_kernel_state_slab, "nockapp.serf_loop.get_kernel_state_slab", TimingCount),
    (serf_loop_get_cold_state_slab, "nockapp.serf_loop.get_cold_state_slab", TimingCount),
    (serf_loop_checkpoint, "nockapp.serf_loop.checkpoint", TimingCount),
    (serf_loop_noun_encode_cold_state, "nockapp.serf_loop.noun_encode_cold_state", TimingCount),
    (serf_loop_copy_state_noun, "nockapp.serf_loop.copy_state_noun", TimingCount),
    (serf_loop_peek, "nockapp.serf_loop.peek", TimingCount),
    (serf_loop_poke, "nockapp.serf_loop.poke", TimingCount),
    (serf_loop_provide_metrics, "nockapp.serf_loop.provide_metrics", TimingCount),
    (next_effect_lagged_error, "nockapp.next_effect.lag", Count)
];
