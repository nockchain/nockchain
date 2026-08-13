#pragma once

#include <cstdint>

#ifdef __cplusplus
extern "C" {
#endif

// Opaque reusable CUDA allocation and stream for one opened-strip shape.
typedef struct AiPowCudaSession AiPowCudaSession;

int ai_pow_cuda_session_create(
    uint32_t max_attempts,
    uint32_t h,
    uint32_t w,
    uint32_t k,
    uint32_t rank,
    uint32_t dot_product_len,
    AiPowCudaSession** session_out);

// Uploads `attempts` contiguous A/B strip pairs and computes one 16-word state
// per pair. A is attempts-by-h-by-k row-major. B is attempts-by-w-by-k.
int ai_pow_cuda_session_run(
    AiPowCudaSession* session,
    const int8_t* a_rows,
    const int8_t* b_cols,
    uint32_t attempts,
    int32_t* states_out);

int ai_pow_cuda_session_destroy(AiPowCudaSession* session);

// Compatibility entry point for one attempt.
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
