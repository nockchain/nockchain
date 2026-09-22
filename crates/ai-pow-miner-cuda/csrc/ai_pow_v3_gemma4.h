#pragma once

#include <cstdint>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct AiPowCudaGemma4Session AiPowCudaGemma4Session;

typedef struct AiPowCudaGemma4SearchResult {
  uint64_t winner_ordinal;
  uint8_t jackpot[32];
  float kernel_ms;
} AiPowCudaGemma4SearchResult;

typedef struct AiPowCudaGemma4PrepareResult {
  uint8_t kappa[32];
  uint8_t h_a[32];
  uint8_t h_b[32];
  uint8_t s_a[32];
  uint8_t s_b[32];
  float commitment_ms;
  float noise_ms;
} AiPowCudaGemma4PrepareResult;

typedef struct AiPowCudaGemma4InferenceResult {
  uint64_t winner_ordinal;
  uint8_t jackpot[32];
  float kernel_ms;
  float output_ms;
} AiPowCudaGemma4InferenceResult;

typedef struct AiPowCudaGemma4KernelInfo {
  uint32_t sm_count;
  uint32_t threads_per_cta;
  uint32_t active_ctas_per_sm;
  uint32_t registers_per_thread;
  uint64_t static_shared_bytes;
  uint64_t dynamic_shared_bytes;
} AiPowCudaGemma4KernelInfo;

int ai_pow_cuda_gemma4_kernel_info(
    uint32_t device_ordinal,
    AiPowCudaGemma4KernelInfo* info_out);

// Creates an immutable fused Gemma gate/up session from prepared noised
// operands. A is row-major and B is column-major. The admitted geometry is
// m=4096, n=43008, k=5376, rank=128, and tile=16.
int ai_pow_cuda_gemma4_session_create(
    uint32_t device_ordinal,
    uint32_t m,
    uint32_t n,
    uint32_t k,
    uint32_t rank,
    uint32_t tile,
    const int8_t* a_prime,
    const int8_t* b_prime,
    const uint8_t pow_key[32],
    AiPowCudaGemma4Session** session_out);

// Creates a persistent fused Gemma source-matrix session. The source operands
// remain resident while prepare replaces every candidate-bound device buffer.
int ai_pow_cuda_gemma4_source_session_create(
    uint32_t device_ordinal,
    uint32_t m,
    uint32_t n,
    uint32_t k,
    uint32_t rank,
    uint32_t tile,
    const int8_t* a,
    const int8_t* b,
    AiPowCudaGemma4Session** session_out);

// Creates a source session over borrowed device pointers on the supplied CUDA
// stream. The caller keeps both tensors and the stream alive until destruction.
int ai_pow_cuda_gemma4_source_session_create_device(
    uint32_t device_ordinal,
    uint32_t m,
    uint32_t n,
    uint32_t k,
    uint32_t rank,
    uint32_t tile,
    const int8_t* a_device,
    const int8_t* b_device,
    void* cuda_stream,
    AiPowCudaGemma4Session** session_out);

// Rebinds a borrowed device session after the prior synchronous call completes.
int ai_pow_cuda_gemma4_session_bind_device(
    AiPowCudaGemma4Session* session,
    const int8_t* a_device,
    const int8_t* b_device,
    void* cuda_stream);

// Derives the complete dense Pearl V3 transcript for one 76-byte header and
// 52-byte mining configuration.
int ai_pow_cuda_gemma4_session_prepare(
    AiPowCudaGemma4Session* session,
    const uint8_t sigma[76],
    const uint8_t mu[52],
    AiPowCudaGemma4PrepareResult* result_out);

// Executes the noised mining GEMM and materializes the exact clean BF16 output.
// Scales and output use host memory. output_bf16 has logical_m * n elements.
int ai_pow_cuda_gemma4_session_infer(
    AiPowCudaGemma4Session* session,
    uint32_t logical_m,
    const float* a_scales,
    const float* b_scales,
    const uint8_t target[32],
    uint16_t* output_bf16,
    AiPowCudaGemma4InferenceResult* result_out);

// Device-resident variant for in-process model serving. Scale and output
// pointers belong to the caller and are ordered on the session stream.
int ai_pow_cuda_gemma4_session_infer_device(
    AiPowCudaGemma4Session* session,
    uint32_t logical_m,
    const float* a_scales_device,
    const float* b_scales_device,
    const uint8_t target[32],
    uint16_t* output_bf16_device,
    AiPowCudaGemma4InferenceResult* result_out);

// Searches [ordinal_start, ordinal_start + ordinal_count). The lowest matching
// ordinal is returned. UINT64_MAX means no winner.
int ai_pow_cuda_gemma4_session_search(
    AiPowCudaGemma4Session* session,
    uint64_t ordinal_start,
    uint64_t ordinal_count,
    const uint8_t target[32],
    AiPowCudaGemma4SearchResult* result_out);

// Recomputes one ticket on the device for differential validation.
int ai_pow_cuda_gemma4_session_debug(
    AiPowCudaGemma4Session* session,
    uint64_t ordinal,
    int32_t state_out[16],
    uint8_t jackpot_out[32]);

int ai_pow_cuda_gemma4_session_destroy(AiPowCudaGemma4Session* session);

#ifdef __cplusplus
}
#endif
