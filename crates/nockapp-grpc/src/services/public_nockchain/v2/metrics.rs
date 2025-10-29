use std::sync::Arc;

use gnort::*;
use once_cell::sync::OnceCell;

metrics_struct![
    NockchainGrpcApiMetrics,
    (balance_update_peek_time, "nockchain_public_grpc.balance_update_peek_time", TimingCount),
    (balance_update_success_hit, "nockchain_public_grpc.balance_update_success.hit", TimingCount),
    (
        balance_update_success_miss, "nockchain_public_grpc.balance_update_success.miss",
        TimingCount
    ),
    (balance_update_error, "nockchain_public_grpc.balance_update_error", TimingCount),
    (
        balance_request_error_invalid_request_address_missing,
        "nockchain_public_grpc.balance_request_error.invalid_request.address_missing", Count
    ),
    (
        balance_request_error_invalid_request_address_format,
        "nockchain_public_grpc.balance_request_error.invalid_request.address_format", Count
    ),
    (
        balance_request_error_invalid_request_page_missing,
        "nockchain_public_grpc.balance_request_error.invalid_request.page_missing", Count
    ),
    (
        balance_request_error_invalid_request_token_invalid,
        "nockchain_public_grpc.balance_request_error.invalid_request.token_invalid", Count
    ),
    (
        balance_request_error_invalid_request_token_mismatch,
        "nockchain_public_grpc.balance_request_error.invalid_request.token_mismatch", Count
    ),
    (
        balance_request_error_invalid_request_snapshot_mismatch,
        "nockchain_public_grpc.balance_request_error.invalid_request.snapshot_mismatch", Count
    ),
    (
        balance_request_error_invalid_request_limit_exceeded,
        "nockchain_public_grpc.balance_request_error.invalid_request.limit_exceeded", Count
    ),
    (
        balance_request_error_invalid_request_other,
        "nockchain_public_grpc.balance_request_error.invalid_request.other", Count
    ),
    (
        balance_request_error_peek_failed,
        "nockchain_public_grpc.balance_request_error.peek_failed", Count
    ),
    (balance_request_error_decode, "nockchain_public_grpc.balance_request_error.decode", Count),
    (balance_request_error_nockapp, "nockchain_public_grpc.balance_request_error.nockapp", Count),
    (
        balance_request_error_invalid_request_missing_selector,
        "nockchain_public_grpc.balance_request_error.invalid_request.missing_selector", Count
    ),
    (
        balance_request_error_invalid_request_invalid_first_name,
        "nockchain_public_grpc.balance_request_error.invalid_request.invalid_first_name", Count
    ),
    (send_tx_success, "nockchain_public_grpc.send_tx_success", TimingCount),
    (send_tx_error, "nockchain_public_grpc.send_tx_error", TimingCount),
    (
        send_tx_error_invalid_request_tx_id_missing,
        "nockchain_public_grpc.send_tx_error.invalid_request.tx_id_missing", Count
    ),
    (
        send_tx_error_invalid_request_raw_tx_missing,
        "nockchain_public_grpc.send_tx_error.invalid_request.raw_tx_missing", Count
    ),
    (
        send_tx_error_invalid_request_tx_id_invalid,
        "nockchain_public_grpc.send_tx_error.invalid_request.tx_id_invalid", Count
    ),
    (
        send_tx_error_invalid_request_raw_tx_invalid,
        "nockchain_public_grpc.send_tx_error.invalid_request.raw_tx_invalid", Count
    ),
    (
        send_tx_error_invalid_request_tx_id_mismatch,
        "nockchain_public_grpc.send_tx_error.invalid_request.tx_id_mismatch", Count
    ),
    (send_tx_error_poke_failed, "nockchain_public_grpc.send_tx_error.poke_failed", Count),
    (send_tx_error_nockapp, "nockchain_public_grpc.send_tx_error.nockapp", Count),
    (send_tx_error_internal, "nockchain_public_grpc.send_tx_error.internal", Count),
    (send_tx_poke_time, "nockchain_public_grpc.send_tx_poke_time", TimingCount),
    (tx_accepted_success, "nockchain_public_grpc.tx_accepted_success", TimingCount),
    (tx_accepted_error, "nockchain_public_grpc.tx_accepted_error", TimingCount),
    (
        tx_accepted_error_invalid_request_missing_tx_id,
        "nockchain_public_grpc.tx_accepted_error.invalid_request.tx_id_missing", Count
    ),
    (
        tx_accepted_error_invalid_request_empty_tx_id,
        "nockchain_public_grpc.tx_accepted_error.invalid_request.tx_id_empty", Count
    ),
    (tx_accepted_error_peek_failed, "nockchain_public_grpc.tx_accepted_error.peek_failed", Count),
    (tx_accepted_error_decode, "nockchain_public_grpc.tx_accepted_error.decode", Count),
    (tx_accepted_error_nockapp, "nockchain_public_grpc.tx_accepted_error.nockapp", Count),
    (tx_accepted_peek_time, "nockchain_public_grpc.tx_accepted_peek_time", TimingCount),
    (heaviest_chain_peek, "nockchain_public_grpc.heaviest_chain_peek", TimingCount),
    (heaviest_chain_age_seconds, "nockchain_public_grpc.heaviest_chain_age_seconds", Gauge),
    (heaviest_chain_cache_miss, "nockchain_public_grpc.heaviest_chain_cache_miss", Count),
    (
        heaviest_chain_refresh_failure, "nockchain_public_grpc.heaviest_chain_refresh_failure",
        Count
    ),
    (balance_cache_address_hit, "nockchain_public_grpc.balance_cache_address_hit", Count),
    (balance_cache_address_miss, "nockchain_public_grpc.balance_cache_address_miss", Count),
    (balance_cache_first_name_hit, "nockchain_public_grpc.balance_cache_first_name_hit", Count),
    (balance_cache_first_name_miss, "nockchain_public_grpc.balance_cache_first_name_miss", Count)
];

static METRICS: OnceCell<Arc<NockchainGrpcApiMetrics>> = OnceCell::new();

pub fn init_metrics() -> Arc<NockchainGrpcApiMetrics> {
    METRICS
        .get_or_init(|| {
            Arc::new(
                NockchainGrpcApiMetrics::register(gnort::global_metrics_registry())
                    .expect("Failed to register metrics!"),
            )
        })
        .clone()
}
