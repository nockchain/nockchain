#pragma once

#include <cstdint>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct AiPowCudaPeakSession AiPowCudaPeakSession;

typedef struct AiPowCudaPeakSearchResult {
  uint64_t winner_ordinal;
  uint8_t jackpot[32];
  float kernel_ms;
} AiPowCudaPeakSearchResult;

typedef struct AiPowCudaPeakPrepareResult {
  uint8_t kappa[32];
  uint8_t h_a[32];
  uint8_t h_b[32];
  uint8_t s_a[32];
  uint8_t s_b[32];
  float commitment_ms;
  float noise_ms;
} AiPowCudaPeakPrepareResult;

typedef struct AiPowCudaPeakKernelInfo {
  uint32_t sm_count;
  uint32_t threads_per_cta;
  uint32_t active_ctas_per_sm;
  uint32_t registers_per_thread;
  uint64_t static_shared_bytes;
  uint64_t dynamic_shared_bytes;
} AiPowCudaPeakKernelInfo;

int ai_pow_cuda_peak_kernel_info(
    uint32_t device_ordinal,
    AiPowCudaPeakKernelInfo* info_out);

// Creates one immutable dense Pearl V3 template session. A is row-major and B
// is column-major. The first kernel family accepts tile=16, k=8192, rank=512,
// m divisible by 256, and n divisible by 128.
int ai_pow_cuda_peak_session_create(
    uint32_t device_ordinal,
    uint32_t m,
    uint32_t n,
    uint32_t k,
    uint32_t rank,
    uint32_t tile,
    const int8_t* a_prime,
    const int8_t* b_prime,
    const uint8_t pow_key[32],
    AiPowCudaPeakSession** session_out);

// Creates a persistent source-matrix session. The source matrices remain
// resident while prepare replaces every attempt-bound device buffer.
int ai_pow_cuda_peak_source_session_create(
    uint32_t device_ordinal,
    uint32_t m,
    uint32_t n,
    uint32_t k,
    uint32_t rank,
    uint32_t tile,
    const int8_t* a,
    const int8_t* b,
    AiPowCudaPeakSession** session_out);

// Derives the complete dense Pearl V3 transcript for one 76-byte header and
// 52-byte mining configuration. The returned values are required for scalar
// winner validation and proof construction.
int ai_pow_cuda_peak_session_prepare(
    AiPowCudaPeakSession* session,
    const uint8_t sigma[76],
    const uint8_t mu[52],
    AiPowCudaPeakPrepareResult* result_out);

// Searches [ordinal_start, ordinal_start + ordinal_count). The lowest matching
// ordinal is returned. UINT64_MAX means no winner.
int ai_pow_cuda_peak_session_search(
    AiPowCudaPeakSession* session,
    uint64_t ordinal_start,
    uint64_t ordinal_count,
    const uint8_t target[32],
    AiPowCudaPeakSearchResult* result_out);

// Recomputes one ticket on the device. This path is for differential tests and
// winner readback, not for the no-hit loop.
int ai_pow_cuda_peak_session_debug(
    AiPowCudaPeakSession* session,
    uint64_t ordinal,
    int32_t state_out[16],
    uint8_t jackpot_out[32]);

int ai_pow_cuda_peak_session_destroy(AiPowCudaPeakSession* session);

#ifdef __cplusplus
}
#endif
