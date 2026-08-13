#include "ai_pow_gemm.h"

#include <cuda_runtime.h>

#include <cstddef>

namespace {

constexpr int kThreads = 256;

__global__ void pearl_tile_state_kernel(
    const int8_t* __restrict__ a_rows,
    const int8_t* __restrict__ b_cols,
    uint32_t h,
    uint32_t w,
    uint32_t k,
    uint32_t rank,
    uint32_t steps,
    int32_t* __restrict__ state) {
  extern __shared__ int32_t accum[];
  const uint32_t cells = h * w;
  const uint32_t tid = threadIdx.x;

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
      const int8_t* a = a_rows + static_cast<size_t>(row) * k + lo;
      const int8_t* b = b_cols + static_cast<size_t>(col) * k + lo;
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
    __shared__ int32_t reduction[kThreads];
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

}  // namespace

extern "C" int ai_pow_cuda_tile_state(
    const int8_t* a_rows,
    const int8_t* b_cols,
    uint32_t h,
    uint32_t w,
    uint32_t k,
    uint32_t rank,
    uint32_t dot_product_len,
    int32_t state_out[16],
    void* stream_ptr) {
  if (a_rows == nullptr || b_cols == nullptr || state_out == nullptr || h == 0 ||
      w == 0 || k == 0 || rank == 0 || dot_product_len == 0 ||
      dot_product_len > k || dot_product_len % rank != 0) {
    return static_cast<int>(cudaErrorInvalidValue);
  }
  const size_t a_bytes = static_cast<size_t>(h) * k;
  const size_t b_bytes = static_cast<size_t>(w) * k;
  const size_t cells = static_cast<size_t>(h) * w;
  if (cells > 4096) {
    return static_cast<int>(cudaErrorInvalidValue);
  }

  cudaStream_t stream = static_cast<cudaStream_t>(stream_ptr);
  int8_t* d_a = nullptr;
  int8_t* d_b = nullptr;
  int32_t* d_state = nullptr;
  cudaError_t error = cudaMallocAsync(&d_a, a_bytes, stream);
  if (error != cudaSuccess) return static_cast<int>(error);
  error = cudaMallocAsync(&d_b, b_bytes, stream);
  if (error != cudaSuccess) goto cleanup_a;
  error = cudaMallocAsync(&d_state, 16 * sizeof(int32_t), stream);
  if (error != cudaSuccess) goto cleanup_b;
  error = cudaMemcpyAsync(d_a, a_rows, a_bytes, cudaMemcpyHostToDevice, stream);
  if (error != cudaSuccess) goto cleanup_state;
  error = cudaMemcpyAsync(d_b, b_cols, b_bytes, cudaMemcpyHostToDevice, stream);
  if (error != cudaSuccess) goto cleanup_state;

  pearl_tile_state_kernel<<<1, kThreads, cells * sizeof(int32_t), stream>>>(
      d_a, d_b, h, w, k, rank, dot_product_len / rank, d_state);
  error = cudaGetLastError();
  if (error != cudaSuccess) goto cleanup_state;
  error = cudaMemcpyAsync(
      state_out, d_state, 16 * sizeof(int32_t), cudaMemcpyDeviceToHost, stream);
  if (error != cudaSuccess) goto cleanup_state;
  error = cudaStreamSynchronize(stream);

cleanup_state:
  cudaFreeAsync(d_state, stream);
cleanup_b:
  cudaFreeAsync(d_b, stream);
cleanup_a:
  cudaFreeAsync(d_a, stream);
  if (error == cudaSuccess) {
    error = cudaStreamSynchronize(stream);
  }
  return static_cast<int>(error);
}
