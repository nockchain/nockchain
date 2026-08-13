#include "ai_pow_gemm.h"

#include <cuda_runtime.h>

#include <cstddef>
#include <new>

namespace {

constexpr int kThreads = 256;

__global__ void pearl_tile_state_batch_kernel(
    const int8_t* __restrict__ a_rows,
    const int8_t* __restrict__ b_cols,
    uint32_t h,
    uint32_t w,
    uint32_t k,
    uint32_t rank,
    uint32_t steps,
    int32_t* __restrict__ states) {
  extern __shared__ int32_t shared[];
  int32_t* accum = shared;
  int32_t* reduction = shared + h * w;
  int32_t* state = states + static_cast<size_t>(blockIdx.x) * 16;
  const uint32_t cells = h * w;
  const uint32_t tid = threadIdx.x;
  const size_t a_stride = static_cast<size_t>(h) * k;
  const size_t b_stride = static_cast<size_t>(w) * k;
  const int8_t* attempt_a = a_rows + static_cast<size_t>(blockIdx.x) * a_stride;
  const int8_t* attempt_b = b_cols + static_cast<size_t>(blockIdx.x) * b_stride;

  for (uint32_t cell = tid; cell < cells; cell += blockDim.x) {
    accum[cell] = 0;
  }
  if (tid < 16) {
    state[tid] = 0;
  }
  __syncthreads();

  for (uint32_t step = 0; step < steps; ++step) {
    const uint32_t lo = step * rank;
    for (uint32_t cell = tid; cell < cells; cell += blockDim.x) {
      const uint32_t row = cell / w;
      const uint32_t col = cell - row * w;
      int32_t delta = 0;
      const int8_t* a = attempt_a + static_cast<size_t>(row) * k + lo;
      const int8_t* b = attempt_b + static_cast<size_t>(col) * k + lo;
      for (uint32_t index = 0; index < rank; ++index) {
        delta += static_cast<int32_t>(a[index]) * static_cast<int32_t>(b[index]);
      }
      accum[cell] += delta;
    }
    __syncthreads();

    int32_t value = 0;
    for (uint32_t cell = tid; cell < cells; cell += blockDim.x) {
      value ^= accum[cell];
    }
    reduction[tid] = value;
    __syncthreads();
    for (uint32_t stride = blockDim.x / 2; stride != 0; stride >>= 1) {
      if (tid < stride) {
        reduction[tid] ^= reduction[tid + stride];
      }
      __syncthreads();
    }
    if (tid == 0) {
      const uint32_t slot = step & 15;
      const uint32_t prior = static_cast<uint32_t>(state[slot]);
      state[slot] = static_cast<int32_t>((prior << 13 | prior >> 19) ^
                                         static_cast<uint32_t>(reduction[0]));
    }
    __syncthreads();
  }
}

bool valid_shape(uint32_t h, uint32_t w, uint32_t k, uint32_t rank,
                 uint32_t dot_product_len) {
  return h != 0 && w != 0 && k != 0 && rank != 0 && dot_product_len != 0 &&
         dot_product_len <= k && dot_product_len % rank == 0 &&
         static_cast<size_t>(h) * w <= 4096;
}

}  // namespace

struct AiPowCudaSession {
  uint32_t max_attempts;
  uint32_t h;
  uint32_t w;
  uint32_t k;
  uint32_t rank;
  uint32_t steps;
  size_t a_attempt_bytes;
  size_t b_attempt_bytes;
  cudaStream_t stream;
  int8_t* d_a;
  int8_t* d_b;
  int32_t* d_states;
};

extern "C" int ai_pow_cuda_session_create(
    uint32_t max_attempts, uint32_t h, uint32_t w, uint32_t k, uint32_t rank,
    uint32_t dot_product_len, AiPowCudaSession** session_out) {
  if (session_out == nullptr || max_attempts == 0 ||
      !valid_shape(h, w, k, rank, dot_product_len)) {
    return static_cast<int>(cudaErrorInvalidValue);
  }
  *session_out = nullptr;
  AiPowCudaSession* session = new (std::nothrow) AiPowCudaSession{};
  if (session == nullptr) return static_cast<int>(cudaErrorMemoryAllocation);
  session->max_attempts = max_attempts;
  session->h = h;
  session->w = w;
  session->k = k;
  session->rank = rank;
  session->steps = dot_product_len / rank;
  session->a_attempt_bytes = static_cast<size_t>(h) * k;
  session->b_attempt_bytes = static_cast<size_t>(w) * k;

  cudaError_t error = cudaStreamCreateWithFlags(&session->stream, cudaStreamNonBlocking);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_a, session->a_attempt_bytes * max_attempts);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_b, session->b_attempt_bytes * max_attempts);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_states,
                     static_cast<size_t>(max_attempts) * 16 * sizeof(int32_t));
  if (error != cudaSuccess) goto fail;
  *session_out = session;
  return static_cast<int>(cudaSuccess);

fail:
  if (session->d_states != nullptr) cudaFree(session->d_states);
  if (session->d_b != nullptr) cudaFree(session->d_b);
  if (session->d_a != nullptr) cudaFree(session->d_a);
  if (session->stream != nullptr) cudaStreamDestroy(session->stream);
  delete session;
  return static_cast<int>(error);
}

extern "C" int ai_pow_cuda_session_run(
    AiPowCudaSession* session, const int8_t* a_rows, const int8_t* b_cols,
    uint32_t attempts, int32_t* states_out) {
  if (session == nullptr || a_rows == nullptr || b_cols == nullptr ||
      states_out == nullptr || attempts == 0 || attempts > session->max_attempts) {
    return static_cast<int>(cudaErrorInvalidValue);
  }
  const size_t a_bytes = session->a_attempt_bytes * attempts;
  const size_t b_bytes = session->b_attempt_bytes * attempts;
  const size_t state_bytes = static_cast<size_t>(attempts) * 16 * sizeof(int32_t);
  cudaError_t error = cudaMemcpyAsync(session->d_a, a_rows, a_bytes,
                                      cudaMemcpyHostToDevice, session->stream);
  if (error != cudaSuccess) return static_cast<int>(error);
  error = cudaMemcpyAsync(session->d_b, b_cols, b_bytes,
                          cudaMemcpyHostToDevice, session->stream);
  if (error != cudaSuccess) return static_cast<int>(error);

  const size_t shared_bytes =
      (static_cast<size_t>(session->h) * session->w + kThreads) * sizeof(int32_t);
  pearl_tile_state_batch_kernel<<<attempts, kThreads, shared_bytes, session->stream>>>(
      session->d_a, session->d_b, session->h, session->w, session->k,
      session->rank, session->steps, session->d_states);
  error = cudaGetLastError();
  if (error != cudaSuccess) return static_cast<int>(error);
  error = cudaMemcpyAsync(states_out, session->d_states, state_bytes,
                          cudaMemcpyDeviceToHost, session->stream);
  if (error != cudaSuccess) return static_cast<int>(error);
  return static_cast<int>(cudaStreamSynchronize(session->stream));
}

extern "C" int ai_pow_cuda_session_destroy(AiPowCudaSession* session) {
  if (session == nullptr) return static_cast<int>(cudaSuccess);
  cudaError_t first = cudaSuccess;
  cudaError_t error = cudaFree(session->d_states);
  if (first == cudaSuccess) first = error;
  error = cudaFree(session->d_b);
  if (first == cudaSuccess) first = error;
  error = cudaFree(session->d_a);
  if (first == cudaSuccess) first = error;
  error = cudaStreamDestroy(session->stream);
  if (first == cudaSuccess) first = error;
  delete session;
  return static_cast<int>(first);
}

extern "C" int ai_pow_cuda_tile_state(
    const int8_t* a_rows, const int8_t* b_cols, uint32_t h, uint32_t w,
    uint32_t k, uint32_t rank, uint32_t dot_product_len,
    int32_t state_out[16], void*) {
  AiPowCudaSession* session = nullptr;
  int status = ai_pow_cuda_session_create(
      1, h, w, k, rank, dot_product_len, &session);
  if (status != 0) return status;
  status = ai_pow_cuda_session_run(session, a_rows, b_cols, 1, state_out);
  const int destroy_status = ai_pow_cuda_session_destroy(session);
  return status == 0 ? destroy_status : status;
}
