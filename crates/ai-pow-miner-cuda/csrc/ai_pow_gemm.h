#pragma once

#include <cstdint>

#ifdef __cplusplus
extern "C" {
#endif

// Opaque persistent allocations and stream for generic and canonical jobs.
typedef struct AiPowCudaSession AiPowCudaSession;
typedef struct AiPowCudaV3Session AiPowCudaV3Session;

// Returns the CUDA runtime's visible device count.
int ai_pow_cuda_device_count(uint32_t* count_out);

// Generic opened-strip tile-state session. Retained for dense Pearl jobs and
// differential tests. Canonical production mining uses the V3 session below.
int ai_pow_cuda_session_create(
    uint32_t device_ordinal,
    uint32_t max_attempts,
    uint32_t h,
    uint32_t w,
    uint32_t k,
    uint32_t rank,
    uint32_t dot_product_len,
    AiPowCudaSession** session_out);

int ai_pow_cuda_session_run(
    AiPowCudaSession* session,
    const int8_t* a_rows,
    const int8_t* b_cols,
    uint32_t attempts,
    int32_t* states_out);

int ai_pow_cuda_session_destroy(AiPowCudaSession* session);

// Creates the persistent canonical Pearl V3 session. Fixed inputs are copied
// once. `sigma` is the 76-byte base header and `mu` is the 52-byte mining
// configuration. The device adds each attempt ordinal to sigma's timestamp.
int ai_pow_cuda_v3_session_create(
    uint32_t device_ordinal,
    uint32_t max_attempts,
    const int8_t* a_matrix,
    const int8_t* b_matrix,
    const uint8_t sigma[76],
    const uint8_t mu[52],
    const uint8_t* routing_data,
    uint32_t routing_data_len,
    const uint8_t* routing_offsets,
    uint32_t routing_offsets_len,
    const uint32_t row_indices[8],
    const uint32_t col_indices[8],
    AiPowCudaV3Session** session_out);

// Searches consecutive extranonces and returns the lowest successful local
// index in `winner_local`; UINT32_MAX means no winner. `jackpot_out` receives
// the device jackpot for a hit and is ignored for a miss.
int ai_pow_cuda_v3_session_search(
    AiPowCudaV3Session* session,
    uint32_t extranonce_start,
    uint32_t attempts,
    const uint8_t target[32],
    uint32_t capture_debug,
    uint32_t* winner_local,
    uint8_t jackpot_out[32]);

// Copies selected per-attempt V3 intermediates for differential tests.
int ai_pow_cuda_v3_session_debug(
    AiPowCudaV3Session* session,
    uint32_t extranonce,
    uint8_t kappa[32],
    uint8_t h_a[32],
    uint8_t h_b[32],
    uint8_t s_a[32],
    uint8_t s_b[32],
    int8_t a_rows[8192],
    int8_t b_cols[8192],
    int32_t state[16],
    uint8_t jackpot[32]);
int ai_pow_cuda_v3_session_destroy(AiPowCudaV3Session* session);

// Compatibility entry point for one generic opened-strip attempt.
int ai_pow_cuda_tile_state(
    const int8_t* a_rows,
    const int8_t* b_cols,
    uint32_t h,
    uint32_t w,
    uint32_t k,
    uint32_t rank,
    uint32_t dot_product_len,
    int32_t state_out[16],
    void* stream);

#ifdef __cplusplus
}
#endif
