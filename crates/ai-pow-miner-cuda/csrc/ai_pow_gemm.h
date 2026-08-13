#pragma once

#include <cstdint>

#ifdef __cplusplus
extern "C" {
#endif

// Computes the Pearl rolling tile state for h-by-w opened strips. A is
// h-by-k row-major. B is w-by-k column-major. Returns a CUDA error code.
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
