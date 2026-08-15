#include "ai_pow_v3_peak.h"

#include <cuda_runtime.h>

#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <limits>

namespace {

constexpr int kBm = 256;
constexpr int kBn = 128;
constexpr int kBk = 64;
constexpr int kWm = 64;
constexpr int kWn = 64;
constexpr int kMmaM = 16;
constexpr int kMmaN = 8;
constexpr int kMmaK = 32;
constexpr int kStages = 2;
constexpr int kWarpsM = kBm / kWm;
constexpr int kWarpsN = kBn / kWn;
constexpr int kWarpsPerCta = kWarpsM * kWarpsN;
constexpr int kThreads = kWarpsPerCta * 32;
constexpr int kMmaPerWarpM = kWm / kMmaM;
constexpr int kMmaPerWarpN = kWn / kMmaN;
constexpr int kMmaPerWarpK = kBk / kMmaK;
constexpr int kHashTile = 16;
constexpr int kHashTilesMPerWarp = kMmaPerWarpM;
constexpr int kHashTilesNPerWarp = kMmaPerWarpN / 2;
constexpr int kHashTilesPerWarp = kHashTilesMPerWarp * kHashTilesNPerWarp;
constexpr int kHashTilesPerCta = kWarpsPerCta * kHashTilesPerWarp;
constexpr int kTranscriptSlots = 16;
constexpr int kRank = 512;
constexpr int kK = 8192;
constexpr int kCadenceTiles = kRank / kBk;
constexpr int kGroups = kK / kRank;
constexpr int kSmemABytes = kBm * kBk;
constexpr int kSmemBBytes = kBn * kBk;
constexpr int kDynamicSmemBytes = kStages * (kSmemABytes + kSmemBBytes);
constexpr uint64_t kNoWinner = UINT64_MAX;
constexpr int kRoutingBits = 4;
constexpr int kRoutingMask = (1 << kRoutingBits) - 1;
constexpr uint32_t kB3ChunkStart = 1u << 0;
constexpr uint32_t kB3ChunkEnd = 1u << 1;
constexpr uint32_t kB3Parent = 1u << 2;
constexpr uint32_t kB3Root = 1u << 3;
constexpr uint32_t kB3Keyed = 1u << 4;
constexpr uint32_t kChunkBytes = 1024;
constexpr uint32_t kSigmaBytes = 76;
constexpr uint32_t kMuBytes = 52;
constexpr uint32_t kTranscriptBytes = kSigmaBytes + kMuBytes;
constexpr int kPrepareThreads = 256;

__device__ __constant__ uint32_t kB3Iv[8] = {
    0x6a09e667u, 0xbb67ae85u, 0x3c6ef372u, 0xa54ff53au,
    0x510e527fu, 0x9b05688cu, 0x1f83d9abu, 0x5be0cd19u,
};
__device__ __constant__ uint32_t kSeedSaltA[8] = {
    0x6c404982u, 0x1615eda0u, 0x92f61696u, 0xf876f0fcu,
    0x2adbdb92u, 0x52b82370u, 0x1977d4f0u, 0x7b0190c3u,
};
__device__ __constant__ uint32_t kSeedSaltB[8] = {
    0x32063011u, 0xca0163ecu, 0x71afe22bu, 0x4f4d3f8bu,
    0x39c6e91au, 0x04cce888u, 0x1d304448u, 0xa99ab871u,
};

static_assert(kThreads == 256);
static_assert(kHashTilesPerCta == 128);
static_assert(kGroups == kTranscriptSlots);
static_assert(kCadenceTiles == 8);

bool checked_product(size_t left, size_t right, size_t* out) {
  if (left != 0 && right > std::numeric_limits<size_t>::max() / left) return false;
  *out = left * right;
  return true;
}

__device__ __forceinline__ uint32_t rotr32(uint32_t value, int shift) {
  return (value >> shift) | (value << (32 - shift));
}

#define B3_G(a, b, c, d, mx, my) do { \
  (a) = (a) + (b) + (mx);               \
  (d) = rotr32((d) ^ (a), 16);          \
  (c) = (c) + (d);                      \
  (b) = rotr32((b) ^ (c), 12);          \
  (a) = (a) + (b) + (my);               \
  (d) = rotr32((d) ^ (a), 8);           \
  (c) = (c) + (d);                      \
  (b) = rotr32((b) ^ (c), 7);           \
} while (0)

template <int Round>
struct B3Schedule;

template <>
struct B3Schedule<0> {
  static __device__ __forceinline__ constexpr int index(int i) {
    constexpr uint8_t schedule[16] = {0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15};
    return schedule[i];
  }
};
template <>
struct B3Schedule<1> {
  static __device__ __forceinline__ constexpr int index(int i) {
    constexpr uint8_t schedule[16] = {2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8};
    return schedule[i];
  }
};
template <>
struct B3Schedule<2> {
  static __device__ __forceinline__ constexpr int index(int i) {
    constexpr uint8_t schedule[16] = {3, 4, 10, 12, 13, 2, 7, 14, 6, 5, 9, 0, 11, 15, 8, 1};
    return schedule[i];
  }
};
template <>
struct B3Schedule<3> {
  static __device__ __forceinline__ constexpr int index(int i) {
    constexpr uint8_t schedule[16] = {10, 7, 12, 9, 14, 3, 13, 15, 4, 0, 11, 2, 5, 8, 1, 6};
    return schedule[i];
  }
};
template <>
struct B3Schedule<4> {
  static __device__ __forceinline__ constexpr int index(int i) {
    constexpr uint8_t schedule[16] = {12, 13, 9, 11, 15, 10, 14, 8, 7, 2, 5, 3, 0, 1, 6, 4};
    return schedule[i];
  }
};
template <>
struct B3Schedule<5> {
  static __device__ __forceinline__ constexpr int index(int i) {
    constexpr uint8_t schedule[16] = {9, 14, 11, 5, 8, 12, 15, 1, 13, 3, 0, 10, 2, 6, 4, 7};
    return schedule[i];
  }
};
template <>
struct B3Schedule<6> {
  static __device__ __forceinline__ constexpr int index(int i) {
    constexpr uint8_t schedule[16] = {11, 15, 5, 0, 1, 9, 8, 6, 14, 10, 2, 12, 3, 4, 7, 13};
    return schedule[i];
  }
};

template <int Round>
__device__ __forceinline__ void b3_round(uint32_t state[16], const uint32_t message[16]) {
#define MSG(i) message[B3Schedule<Round>::index(i)]
  B3_G(state[0], state[4], state[8], state[12], MSG(0), MSG(1));
  B3_G(state[1], state[5], state[9], state[13], MSG(2), MSG(3));
  B3_G(state[2], state[6], state[10], state[14], MSG(4), MSG(5));
  B3_G(state[3], state[7], state[11], state[15], MSG(6), MSG(7));
  B3_G(state[0], state[5], state[10], state[15], MSG(8), MSG(9));
  B3_G(state[1], state[6], state[11], state[12], MSG(10), MSG(11));
  B3_G(state[2], state[7], state[8], state[13], MSG(12), MSG(13));
  B3_G(state[3], state[4], state[9], state[14], MSG(14), MSG(15));
#undef MSG
}

__device__ __forceinline__ void b3_compress(
    const uint32_t message[16],
    const uint32_t cv[8],
    uint64_t counter,
    uint32_t block_len,
    uint32_t flags,
    uint32_t output[8]) {
  uint32_t state[16];
#pragma unroll
  for (int i = 0; i < 8; ++i) state[i] = cv[i];
  state[8] = kB3Iv[0];
  state[9] = kB3Iv[1];
  state[10] = kB3Iv[2];
  state[11] = kB3Iv[3];
  state[12] = static_cast<uint32_t>(counter);
  state[13] = static_cast<uint32_t>(counter >> 32);
  state[14] = block_len;
  state[15] = flags;
  b3_round<0>(state, message);
  b3_round<1>(state, message);
  b3_round<2>(state, message);
  b3_round<3>(state, message);
  b3_round<4>(state, message);
  b3_round<5>(state, message);
  b3_round<6>(state, message);
#pragma unroll
  for (int i = 0; i < 8; ++i) output[i] = state[i] ^ state[i + 8];
}

__device__ __forceinline__ void b3_hash_bytes(
    const uint8_t* input,
    uint32_t length,
    const uint32_t key[8],
    uint32_t base_flags,
    uint32_t output[8]) {
  uint32_t cv[8];
#pragma unroll
  for (int i = 0; i < 8; ++i) cv[i] = key[i];
  const uint32_t blocks = (length + 63u) / 64u;
  for (uint32_t block_index = 0; block_index < blocks; ++block_index) {
    uint32_t message[16];
#pragma unroll
    for (int word = 0; word < 16; ++word) {
      uint32_t value = 0;
#pragma unroll
      for (int byte = 0; byte < 4; ++byte) {
        const uint32_t offset = block_index * 64 + word * 4 + byte;
        if (offset < length) value |= uint32_t(input[offset]) << (byte * 8);
      }
      message[word] = value;
    }
    const bool first = block_index == 0;
    const bool last = block_index + 1 == blocks;
    const uint32_t block_len = last ? length - block_index * 64u : 64u;
    uint32_t next[8];
    b3_compress(message, cv, 0, block_len,
                base_flags | (first ? kB3ChunkStart : 0) |
                    (last ? (kB3ChunkEnd | kB3Root) : 0),
                next);
#pragma unroll
    for (int i = 0; i < 8; ++i) cv[i] = next[i];
  }
#pragma unroll
  for (int i = 0; i < 8; ++i) output[i] = cv[i];
}

__device__ __forceinline__ void b3_single_block(
    const uint32_t message[16],
    const uint32_t key[8],
    uint32_t flags,
    uint32_t output[8]) {
  b3_compress(message, key, 0, 64,
              flags | kB3ChunkStart | kB3ChunkEnd | kB3Root, output);
}

__device__ __forceinline__ void b3_hash_pair(
    const uint32_t left[8],
    const uint32_t right[8],
    uint32_t output[8]) {
  uint32_t message[16];
#pragma unroll
  for (int i = 0; i < 8; ++i) {
    message[i] = left[i];
    message[i + 8] = right[i];
  }
  b3_single_block(message, kB3Iv, 0, output);
}

__device__ __forceinline__ void b3_chunk_cv(
    const uint8_t* bytes,
    uint64_t counter,
    const uint32_t key[8],
    uint32_t output[8]) {
  uint32_t cv[8];
#pragma unroll
  for (int i = 0; i < 8; ++i) cv[i] = key[i];
  for (uint32_t block_index = 0; block_index < 16; ++block_index) {
    uint32_t message[16];
#pragma unroll
    for (int word = 0; word < 16; ++word) {
      const uint8_t* source = bytes + block_index * 64 + word * 4;
      message[word] = uint32_t(source[0]) | (uint32_t(source[1]) << 8) |
                      (uint32_t(source[2]) << 16) |
                      (uint32_t(source[3]) << 24);
    }
    uint32_t next[8];
    b3_compress(message, cv, counter, 64,
                kB3Keyed | (block_index == 0 ? kB3ChunkStart : 0) |
                    (block_index == 15 ? kB3ChunkEnd : 0),
                next);
#pragma unroll
    for (int i = 0; i < 8; ++i) cv[i] = next[i];
  }
#pragma unroll
  for (int i = 0; i < 8; ++i) output[i] = cv[i];
}

__device__ __forceinline__ void b3_parent_cv(
    const uint32_t left[8],
    const uint32_t right[8],
    const uint32_t key[8],
    bool root,
    uint32_t output[8]) {
  uint32_t message[16];
#pragma unroll
  for (int i = 0; i < 8; ++i) {
    message[i] = left[i];
    message[i + 8] = right[i];
  }
  b3_compress(message, key, 0, 64,
              kB3Keyed | kB3Parent | (root ? kB3Root : 0), output);
}

__device__ __forceinline__ void b3_random_hash(
    uint32_t index,
    bool b_side,
    const uint32_t key[8],
    uint32_t prepend,
    uint32_t output[8]) {
  uint32_t message[16]{};
  message[prepend] = index + 1;
  message[8] = b_side ? 0x65745f42u : 0x65745f41u;
  message[9] = 0x726f736eu;
  b3_single_block(message, key, kB3Keyed, output);
}

__device__ __forceinline__ void b3_keyed_block(
    const uint32_t message[16], const uint32_t key[8], uint32_t output[8]) {
  b3_single_block(message, key, kB3Keyed, output);
}

__device__ __forceinline__ bool hash_le_target(
    const uint32_t hash[8], const uint32_t target[8]) {
#pragma unroll
  for (int i = 7; i >= 0; --i) {
    if (hash[i] < target[i]) return true;
    if (hash[i] > target[i]) return false;
  }
  return true;
}

__device__ __forceinline__ int swizzled_offset(int row, int col) {
  const int vector_col = col >> 4;
  const int vector_row = row >> 1;
  const int tile_col = (vector_col & 3) + (row & 1) * 4;
  const int tile_row = vector_row & 3;
  const int partition = tile_col >> 2;
  const int permuted = (tile_col & 3) ^ tile_row;
  return ((partition << 2) + permuted) * 16 + (col & 15) + vector_row * 128;
}

__device__ __forceinline__ void cp_async_16(void* destination, const void* source) {
  const uint32_t shared = static_cast<uint32_t>(__cvta_generic_to_shared(destination));
  asm volatile("cp.async.cg.shared.global.L2::128B [%0], [%1], 16;\n" ::
                   "r"(shared), "l"(source));
}

__device__ __forceinline__ void cp_async_commit() {
  asm volatile("cp.async.commit_group;\n");
}

template <int Groups>
__device__ __forceinline__ void cp_async_wait() {
  asm volatile("cp.async.wait_group %0;\n" :: "n"(Groups));
}

__device__ __forceinline__ void cp_async_wait_all() {
  asm volatile("cp.async.wait_all;\n");
}

__device__ __forceinline__ void ldmatrix_x4(uint32_t (&destination)[4], const void* source) {
  const uint32_t shared = static_cast<uint32_t>(__cvta_generic_to_shared(source));
  asm volatile(
      "ldmatrix.sync.aligned.x4.m8n8.shared.b16 {%0,%1,%2,%3}, [%4];\n" :
      "=r"(destination[0]), "=r"(destination[1]), "=r"(destination[2]),
      "=r"(destination[3]) : "r"(shared));
}

__device__ __forceinline__ void mma_s8(
    int32_t (&accumulator)[4], const uint32_t (&a)[4], const uint32_t (&b)[2]) {
  asm volatile(
      "mma.sync.aligned.m16n8k32.row.col.s32.s8.s8.s32.satfinite "
      "{%0,%1,%2,%3}, {%4,%5,%6,%7}, {%8,%9}, {%0,%1,%2,%3};\n" :
      "+r"(accumulator[0]), "+r"(accumulator[1]), "+r"(accumulator[2]),
      "+r"(accumulator[3]) : "r"(a[0]), "r"(a[1]), "r"(a[2]), "r"(a[3]),
      "r"(b[0]), "r"(b[1]));
}

#define ACCUMULATOR_XOR(row, low_col, high_col) (                              \
    static_cast<uint32_t>(accumulator[row][low_col][0]) ^                       \
    static_cast<uint32_t>(accumulator[row][low_col][1]) ^                       \
    static_cast<uint32_t>(accumulator[row][low_col][2]) ^                       \
    static_cast<uint32_t>(accumulator[row][low_col][3]) ^                       \
    static_cast<uint32_t>(accumulator[row][high_col][0]) ^                      \
    static_cast<uint32_t>(accumulator[row][high_col][1]) ^                      \
    static_cast<uint32_t>(accumulator[row][high_col][2]) ^                      \
    static_cast<uint32_t>(accumulator[row][high_col][3]))

__global__ void peak_kappa_kernel(
    const uint8_t* transcript,
    uint32_t* kappa) {
  if (blockIdx.x != 0 || threadIdx.x != 0) return;
  uint32_t output[8];
  b3_hash_bytes(transcript, kTranscriptBytes, kB3Iv, 0, output);
#pragma unroll
  for (int i = 0; i < 8; ++i) kappa[i] = output[i];
}

__global__ void peak_matrix_chunk_kernel(
    const uint8_t* matrix,
    uint32_t chunk_count,
    const uint32_t* key,
    uint32_t* output) {
  for (uint32_t chunk = blockIdx.x * blockDim.x + threadIdx.x;
       chunk < chunk_count;
       chunk += blockDim.x * gridDim.x) {
    uint32_t cv[8];
    b3_chunk_cv(matrix + size_t(chunk) * kChunkBytes, chunk, key, cv);
#pragma unroll
    for (int i = 0; i < 8; ++i) output[size_t(chunk) * 8 + i] = cv[i];
  }
}

__global__ void peak_matrix_parent_kernel(
    const uint32_t* input,
    uint32_t child_count,
    const uint32_t* key,
    uint32_t* output) {
  const uint32_t parent_count = (child_count + 1) / 2;
  for (uint32_t parent = blockIdx.x * blockDim.x + threadIdx.x;
       parent < parent_count;
       parent += blockDim.x * gridDim.x) {
    const uint32_t left = parent * 2;
    if (left + 1 == child_count) {
#pragma unroll
      for (int i = 0; i < 8; ++i) {
        output[size_t(parent) * 8 + i] = input[size_t(left) * 8 + i];
      }
      continue;
    }
    uint32_t cv[8];
    b3_parent_cv(input + size_t(left) * 8,
                 input + size_t(left + 1) * 8,
                 key, child_count == 2, cv);
#pragma unroll
    for (int i = 0; i < 8; ++i) output[size_t(parent) * 8 + i] = cv[i];
  }
}

__global__ void peak_seed_kernel(
    const uint32_t* kappa,
    const uint32_t* h_a,
    const uint32_t* h_b,
    uint32_t m,
    uint32_t n,
    uint32_t* s_a,
    uint32_t* s_b) {
  if (blockIdx.x != 0 || threadIdx.x != 0) return;
  uint32_t message_a[16]{};
  uint32_t message_b[16]{};
#pragma unroll
  for (int i = 0; i < 8; ++i) {
    message_a[i] = h_a[i];
    message_b[i] = h_b[i];
  }
  message_a[8] = m;
  message_b[8] = n;
  uint32_t bound_a[8];
  uint32_t bound_b[8];
  uint32_t local_s_b[8];
  uint32_t local_s_a[8];
  b3_single_block(message_a, kSeedSaltA, kB3Keyed, bound_a);
  b3_single_block(message_b, kSeedSaltB, kB3Keyed, bound_b);
  b3_hash_pair(kappa, bound_b, local_s_b);
  b3_hash_pair(local_s_b, bound_a, local_s_a);
#pragma unroll
  for (int i = 0; i < 8; ++i) {
    s_a[i] = local_s_a[i];
    s_b[i] = local_s_b[i];
  }
}

__global__ void peak_uniform_noise_kernel(
    uint32_t hash_count,
    bool b_side,
    const uint32_t* key,
    int8_t* factors) {
  for (uint32_t hash_index = blockIdx.x * blockDim.x + threadIdx.x;
       hash_index < hash_count;
       hash_index += blockDim.x * gridDim.x) {
    uint32_t hash[8];
    b3_random_hash(hash_index, b_side, key, 0, hash);
#pragma unroll
    for (int word = 0; word < 8; ++word) {
#pragma unroll
      for (int byte = 0; byte < 4; ++byte) {
        const uint8_t value = uint8_t(hash[word] >> (byte * 8));
        factors[size_t(hash_index) * 32 + word * 4 + byte] =
            static_cast<int8_t>(int(value & 63) - 32);
      }
    }
  }
}

__global__ void peak_position_kernel(
    uint32_t hash_count,
    bool b_side,
    const uint32_t* key,
    uint32_t* positions) {
  for (uint32_t hash_index = blockIdx.x * blockDim.x + threadIdx.x;
       hash_index < hash_count;
       hash_index += blockDim.x * gridDim.x) {
    uint32_t hash[8];
    b3_random_hash(hash_index, b_side, key, 1, hash);
#pragma unroll
    for (int slot = 0; slot < 8; ++slot) {
      const uint32_t random = hash[slot];
      const uint32_t plus = random & (kRank - 1);
      const uint32_t minus = plus ^ (1 + __umulhi(kRank - 1, random));
      const size_t index = size_t(hash_index) * 8 + slot;
      positions[index * 2] = plus;
      positions[index * 2 + 1] = minus;
    }
  }
}

__global__ void peak_apply_noise_kernel(
    const int8_t* source,
    uint64_t element_count,
    const int8_t* factors,
    const uint32_t* positions,
    int8_t* output) {
  for (uint64_t index = uint64_t(blockIdx.x) * blockDim.x + threadIdx.x;
       index < element_count;
       index += uint64_t(blockDim.x) * gridDim.x) {
    const uint32_t outer = static_cast<uint32_t>(index / kK);
    const uint32_t inner = static_cast<uint32_t>(index % kK);
    const uint32_t plus = positions[size_t(inner) * 2];
    const uint32_t minus = positions[size_t(inner) * 2 + 1];
    const int value = int(source[index]) +
                      int(factors[size_t(outer) * kRank + plus]) -
                      int(factors[size_t(outer) * kRank + minus]);
    output[index] = static_cast<int8_t>(value);
  }
}

extern "C" __global__ __launch_bounds__(kThreads, 1)
void ai_pow_v3_peak_kernel(
    const int8_t* __restrict__ a,
    const int8_t* __restrict__ b,
    int m,
    int n,
    const uint32_t* __restrict__ target,
    const uint32_t* __restrict__ key,
    uint64_t ordinal_start,
    uint64_t ordinal_end,
    uint64_t* __restrict__ winner) {
  const int tid = threadIdx.x;
  const int warp = tid >> 5;
  const int lane = tid & 31;
  const int warp_m = (warp / kWarpsN) * kWm;
  const int warp_n = (warp % kWarpsN) * kWn;
  const int m_blocks = m / kBm;
  const int n_blocks = n / kBn;
  const int routing_x_extent = (1 << kRoutingBits) * m_blocks;
  const int routing_y_extent =
      (n_blocks + (1 << kRoutingBits) - 1) >> kRoutingBits;
  const int total_cta_tiles = routing_x_extent * routing_y_extent;
  const int grid_stride = gridDim.x;

  extern __shared__ int8_t smem[];
  __shared__ uint32_t transcript[kTranscriptSlots][kHashTilesPerCta];
#define SA(stage) (smem + (stage) * kSmemABytes)
#define SB(stage) (smem + kStages * kSmemABytes + (stage) * kSmemBBytes)

  const int subgroup = lane >> 3;
  const int row_in_subgroup = lane & 7;
  const int a_m_offset = (subgroup & 1) * 8;
  const int a_k_offset = (subgroup >> 1) * 16;
  const int b_k_offset = subgroup * 16;
  constexpr int kVectorsA = kSmemABytes / 16 / kThreads;
  constexpr int kVectorsB = kSmemBBytes / 16 / kThreads;
  int load_row_a[kVectorsA];
  int load_col_a[kVectorsA];
  int load_row_b[kVectorsB];
  int load_col_b[kVectorsB];
#pragma unroll
  for (int vector = 0; vector < kVectorsA; ++vector) {
    const int index = tid + vector * kThreads;
    load_row_a[vector] = index / (kBk / 16);
    load_col_a[vector] = (index % (kBk / 16)) * 16;
  }
#pragma unroll
  for (int vector = 0; vector < kVectorsB; ++vector) {
    const int index = tid + vector * kThreads;
    load_row_b[vector] = index / (kBk / 16);
    load_col_b[vector] = (index % (kBk / 16)) * 16;
  }

  for (int logical_tile = blockIdx.x; logical_tile < total_cta_tiles;
       logical_tile += grid_stride) {
    const int routed_x = logical_tile % routing_x_extent;
    const int routed_y = logical_tile / routing_x_extent;
    const int m_block = routed_x >> kRoutingBits;
    const int n_block = (routed_x & kRoutingMask) + (routed_y << kRoutingBits);
    if (n_block >= n_blocks) continue;
    const int cta_m = m_block * kBm;
    const int cta_n = n_block * kBn;

    for (int index = tid; index < kTranscriptSlots * kHashTilesPerCta;
         index += kThreads) {
      transcript[index / kHashTilesPerCta][index % kHashTilesPerCta] = 0;
    }

    int32_t accumulator[kMmaPerWarpM][kMmaPerWarpN][4];
#pragma unroll
    for (int mm = 0; mm < kMmaPerWarpM; ++mm) {
#pragma unroll
      for (int nn = 0; nn < kMmaPerWarpN; ++nn) {
#pragma unroll
        for (int word = 0; word < 4; ++word) accumulator[mm][nn][word] = 0;
      }
    }

#define ISSUE_STAGE(stage, k_tile) do {                                         \
  if ((k_tile) < kK / kBk) {                                                    \
    const int8_t* global_a = a + static_cast<size_t>(cta_m) * kK + (k_tile) * kBk; \
    const int8_t* global_b = b + static_cast<size_t>(cta_n) * kK + (k_tile) * kBk; \
    _Pragma("unroll")                                                           \
    for (int vector = 0; vector < kVectorsA; ++vector) {                         \
      const int row = load_row_a[vector];                                       \
      const int col = load_col_a[vector];                                       \
      cp_async_16(SA(stage) + swizzled_offset(row, col),                         \
                  global_a + static_cast<size_t>(row) * kK + col);               \
    }                                                                            \
    _Pragma("unroll")                                                           \
    for (int vector = 0; vector < kVectorsB; ++vector) {                         \
      const int row = load_row_b[vector];                                       \
      const int col = load_col_b[vector];                                       \
      cp_async_16(SB(stage) + swizzled_offset(row, col),                         \
                  global_b + static_cast<size_t>(row) * kK + col);               \
    }                                                                            \
  }                                                                              \
  cp_async_commit();                                                             \
} while (0)

    uint32_t a_fragment[2][kMmaPerWarpM][4];
    uint32_t b_fragment[kMmaPerWarpK][kMmaPerWarpN][2];

#define LOAD_A(buffer, stage, kk) do {                                           \
  _Pragma("unroll")                                                             \
  for (int mm = 0; mm < kMmaPerWarpM; ++mm) {                                   \
    ldmatrix_x4(a_fragment[buffer][mm],                                          \
      SA(stage) + swizzled_offset(                                               \
        warp_m + mm * kMmaM + a_m_offset + row_in_subgroup,                     \
        (kk) * kMmaK + a_k_offset));                                             \
  }                                                                              \
} while (0)

#define LOAD_B(stage) do {                                                       \
  _Pragma("unroll")                                                             \
  for (int nn = 0; nn < kMmaPerWarpN; ++nn) {                                   \
    uint32_t fragment[4];                                                        \
    ldmatrix_x4(fragment, SB(stage) + swizzled_offset(                           \
      warp_n + nn * kMmaN + row_in_subgroup, b_k_offset));                      \
    b_fragment[0][nn][0] = fragment[0];                                         \
    b_fragment[0][nn][1] = fragment[1];                                         \
    b_fragment[1][nn][0] = fragment[2];                                         \
    b_fragment[1][nn][1] = fragment[3];                                         \
  }                                                                              \
} while (0)

#define MMA_SUBSTEP(buffer, kk) do {                                             \
  _Pragma("unroll")                                                             \
  for (int nn = 0; nn < kMmaPerWarpN; ++nn) {                                   \
    const bool forward = (((kk) * kMmaPerWarpN + nn) & 1) == 0;                 \
    _Pragma("unroll")                                                           \
    for (int iteration = 0; iteration < kMmaPerWarpM; ++iteration) {             \
      const int mm = forward ? iteration : (kMmaPerWarpM - 1 - iteration);      \
      mma_s8(accumulator[mm][nn], a_fragment[buffer][mm],                        \
             b_fragment[kk][nn]);                                                \
    }                                                                            \
  }                                                                              \
} while (0)

#define TILE_BODY(current, prefetch, next, tile_number) do {                    \
  ISSUE_STAGE(prefetch, (tile_number) + kStages - 1);                            \
  LOAD_A(1, current, 1);                                                        \
  MMA_SUBSTEP(0, 0);                                                            \
  cp_async_wait<kStages - 2>();                                                 \
  __syncthreads();                                                              \
  LOAD_A(0, next, 0);                                                           \
  MMA_SUBSTEP(1, 1);                                                            \
  LOAD_B(next);                                                                 \
} while (0)

#define WRITE_TRANSCRIPT(slot) do {                                              \
  uint32_t owned = 0;                                                           \
  _Pragma("unroll")                                                             \
  for (int hi = 0; hi < kHashTilesMPerWarp; ++hi) {                             \
    _Pragma("unroll")                                                           \
    for (int wi = 0; wi < kHashTilesNPerWarp; ++wi) {                           \
      uint32_t value = ACCUMULATOR_XOR(hi, 2 * wi, 2 * wi + 1);                 \
      value = __reduce_xor_sync(0xffffffffu, value);                            \
      if (lane == hi * kHashTilesNPerWarp + wi) owned = value;                  \
    }                                                                            \
  }                                                                              \
  if (lane < kHashTilesPerWarp) {                                               \
    transcript[slot][warp * kHashTilesPerWarp + lane] = owned;                  \
  }                                                                              \
} while (0)

    ISSUE_STAGE(0, 0);
    cp_async_wait<0>();
    __syncthreads();
    LOAD_B(0);
    LOAD_A(0, 0, 0);

    int tile_number = 0;
#pragma unroll 1
    for (int group = 0; group < kGroups; ++group) {
#pragma unroll
      for (int tile = 0; tile < kCadenceTiles; ++tile) {
        TILE_BODY(tile & 1, (tile + 1) & 1, (tile + 1) & 1, tile_number);
        ++tile_number;
      }
      WRITE_TRANSCRIPT(group);
    }

    cp_async_wait_all();
    __syncthreads();

    // Complete warps finalize the CTA's hash tiles. The branch is warp-uniform.
    if (warp < kHashTilesPerCta / 32) {
      const int job = tid;
      uint32_t message[16];
#pragma unroll
      for (int slot = 0; slot < 16; ++slot) message[slot] = transcript[slot][job];
      uint32_t local_key[8];
      uint32_t local_target[8];
#pragma unroll
      for (int word = 0; word < 8; ++word) {
        local_key[word] = key[word];
        local_target[word] = target[word];
      }
      uint32_t hash[8];
      b3_keyed_block(message, local_key, hash);
      if (hash_le_target(hash, local_target)) {
        const int owner_warp = job / kHashTilesPerWarp;
        const int owner_tile = job % kHashTilesPerWarp;
        const int owner_warp_m = owner_warp / kWarpsN;
        const int owner_warp_n = owner_warp % kWarpsN;
        const int local_tile_m =
            owner_warp_m * kHashTilesMPerWarp + owner_tile / kHashTilesNPerWarp;
        const int local_tile_n =
            owner_warp_n * kHashTilesNPerWarp + owner_tile % kHashTilesNPerWarp;
        const uint64_t row_tile = static_cast<uint64_t>(cta_m / kHashTile + local_tile_m);
        const uint64_t col_tile = static_cast<uint64_t>(cta_n / kHashTile + local_tile_n);
        const uint64_t ordinal = row_tile * static_cast<uint64_t>(n / kHashTile) + col_tile;
        if (ordinal >= ordinal_start && ordinal < ordinal_end) {
          atomicMin(reinterpret_cast<unsigned long long*>(winner),
                    static_cast<unsigned long long>(ordinal));
        }
      }
    }
    __syncthreads();

#undef WRITE_TRANSCRIPT
#undef TILE_BODY
#undef MMA_SUBSTEP
#undef LOAD_B
#undef LOAD_A
#undef ISSUE_STAGE
  }

#undef SB
#undef SA
}

extern "C" __global__
void ai_pow_v3_peak_debug_kernel(
    const int8_t* __restrict__ a,
    const int8_t* __restrict__ b,
    int n,
    uint64_t ordinal,
    const uint32_t* __restrict__ key,
    int32_t* __restrict__ state_out,
    uint32_t* __restrict__ hash_out) {
  __shared__ int32_t accumulator[256];
  __shared__ uint32_t reduction[256];
  __shared__ uint32_t state[16];
  const int tid = threadIdx.x;
  const uint64_t col_tiles = static_cast<uint64_t>(n / kHashTile);
  const uint64_t row_tile = ordinal / col_tiles;
  const uint64_t col_tile = ordinal - row_tile * col_tiles;
  const int row = tid / kHashTile;
  const int col = tid % kHashTile;
  const int8_t* a_row = a + (row_tile * kHashTile + row) * static_cast<uint64_t>(kK);
  const int8_t* b_col = b + (col_tile * kHashTile + col) * static_cast<uint64_t>(kK);
  accumulator[tid] = 0;
  if (tid < 16) state[tid] = 0;
  __syncthreads();

#pragma unroll
  for (int step = 0; step < kGroups; ++step) {
    int32_t delta = 0;
    const int offset = step * kRank;
#pragma unroll 8
    for (int index = 0; index < kRank; ++index) {
      delta += static_cast<int32_t>(a_row[offset + index]) *
               static_cast<int32_t>(b_col[offset + index]);
    }
    accumulator[tid] += delta;
    reduction[tid] = static_cast<uint32_t>(accumulator[tid]);
    __syncthreads();
#pragma unroll
    for (int stride = 128; stride != 0; stride >>= 1) {
      if (tid < stride) reduction[tid] ^= reduction[tid + stride];
      __syncthreads();
    }
    if (tid == 0) state[step] = reduction[0];
    __syncthreads();
  }

  if (tid < 16) state_out[tid] = static_cast<int32_t>(state[tid]);
  if (tid == 0) {
    uint32_t local_key[8];
#pragma unroll
    for (int word = 0; word < 8; ++word) local_key[word] = key[word];
    uint32_t hash[8];
    b3_keyed_block(state, local_key, hash);
#pragma unroll
    for (int word = 0; word < 8; ++word) hash_out[word] = hash[word];
  }
}

cudaError_t launch_peak_matrix_commitment(
    const int8_t* matrix,
    uint32_t chunk_count,
    const uint32_t* key,
    uint32_t* ping,
    uint32_t* pong,
    uint32_t* root,
    cudaStream_t stream) {
  const uint32_t chunk_blocks =
      (chunk_count + kPrepareThreads - 1) / kPrepareThreads;
  peak_matrix_chunk_kernel<<<chunk_blocks, kPrepareThreads, 0, stream>>>(
      reinterpret_cast<const uint8_t*>(matrix), chunk_count, key, ping);
  cudaError_t error = cudaGetLastError();
  if (error != cudaSuccess) return error;

  uint32_t count = chunk_count;
  uint32_t* input = ping;
  uint32_t* output = pong;
  while (count > 1) {
    const uint32_t parent_count = (count + 1) / 2;
    const uint32_t parent_blocks =
        (parent_count + kPrepareThreads - 1) / kPrepareThreads;
    peak_matrix_parent_kernel<<<parent_blocks, kPrepareThreads, 0, stream>>>(
        input, count, key, output);
    error = cudaGetLastError();
    if (error != cudaSuccess) return error;
    uint32_t* temporary = input;
    input = output;
    output = temporary;
    count = parent_count;
  }
  return cudaMemcpyAsync(root, input, 32, cudaMemcpyDeviceToDevice, stream);
}

}  // namespace

struct AiPowCudaPeakSession {
  uint32_t device_ordinal;
  uint32_t m;
  uint32_t n;
  uint64_t total_tickets;
  size_t a_bytes;
  size_t b_bytes;
  int grid_size;
  bool source_mode;
  bool prepared;
  cudaStream_t stream;
  cudaEvent_t start_event;
  cudaEvent_t commitment_event;
  cudaEvent_t end_event;
  int8_t* d_source_a;
  int8_t* d_source_b;
  int8_t* d_a;
  int8_t* d_b;
  uint8_t* d_transcript;
  uint32_t* d_kappa;
  uint32_t* d_h_a;
  uint32_t* d_h_b;
  uint32_t* d_key;
  uint32_t* d_s_b;
  uint32_t* d_cv_ping;
  uint32_t* d_cv_pong;
  int8_t* d_e_l;
  int8_t* d_f_r;
  uint32_t* d_e_positions;
  uint32_t* d_f_positions;
  uint32_t* d_target;
  uint64_t* d_winner;
  int32_t* d_debug_state;
  uint32_t* d_debug_hash;
};

extern "C" int ai_pow_cuda_peak_kernel_info(
    uint32_t device_ordinal,
    AiPowCudaPeakKernelInfo* info_out) {
  if (info_out == nullptr) return static_cast<int>(cudaErrorInvalidValue);
  cudaError_t error = cudaSetDevice(static_cast<int>(device_ordinal));
  if (error != cudaSuccess) return static_cast<int>(error);

  cudaDeviceProp properties{};
  error = cudaGetDeviceProperties(&properties, static_cast<int>(device_ordinal));
  if (error != cudaSuccess) return static_cast<int>(error);
  cudaFuncAttributes attributes{};
  error = cudaFuncGetAttributes(&attributes, ai_pow_v3_peak_kernel);
  if (error != cudaSuccess) return static_cast<int>(error);
  int active_ctas = 0;
  error = cudaFuncSetAttribute(ai_pow_v3_peak_kernel,
                               cudaFuncAttributeMaxDynamicSharedMemorySize,
                               kDynamicSmemBytes);
  if (error != cudaSuccess) return static_cast<int>(error);
  error = cudaOccupancyMaxActiveBlocksPerMultiprocessor(
      &active_ctas, ai_pow_v3_peak_kernel, kThreads, kDynamicSmemBytes);
  if (error != cudaSuccess) return static_cast<int>(error);

  info_out->sm_count = static_cast<uint32_t>(properties.multiProcessorCount);
  info_out->threads_per_cta = kThreads;
  info_out->active_ctas_per_sm = static_cast<uint32_t>(active_ctas);
  info_out->registers_per_thread = static_cast<uint32_t>(attributes.numRegs);
  info_out->static_shared_bytes =
      static_cast<uint64_t>(attributes.sharedSizeBytes);
  info_out->dynamic_shared_bytes = kDynamicSmemBytes;
  return static_cast<int>(cudaSuccess);
}

extern "C" int ai_pow_cuda_peak_session_create(
    uint32_t device_ordinal,
    uint32_t m,
    uint32_t n,
    uint32_t k,
    uint32_t rank,
    uint32_t tile,
    const int8_t* a_prime,
    const int8_t* b_prime,
    const uint8_t pow_key[32],
    AiPowCudaPeakSession** session_out) {
  if (session_out == nullptr || a_prime == nullptr || b_prime == nullptr ||
      pow_key == nullptr || m == 0 || n == 0 || k != kK || rank != kRank ||
      tile != kHashTile || m % kBm != 0 || n % kBn != 0) {
    return static_cast<int>(cudaErrorInvalidValue);
  }
  *session_out = nullptr;
  size_t a_bytes = 0;
  size_t b_bytes = 0;
  if (!checked_product(m, k, &a_bytes) || !checked_product(n, k, &b_bytes)) {
    return static_cast<int>(cudaErrorInvalidValue);
  }
  const uint64_t row_tiles = m / tile;
  const uint64_t col_tiles = n / tile;
  if (row_tiles > UINT64_MAX / col_tiles) return static_cast<int>(cudaErrorInvalidValue);

  auto* session = static_cast<AiPowCudaPeakSession*>(
      std::calloc(1, sizeof(AiPowCudaPeakSession)));
  if (session == nullptr) return static_cast<int>(cudaErrorMemoryAllocation);
  session->device_ordinal = device_ordinal;
  session->m = m;
  session->n = n;
  session->total_tickets = row_tiles * col_tiles;
  session->a_bytes = a_bytes;
  session->b_bytes = b_bytes;
  session->source_mode = false;
  session->prepared = true;

  cudaDeviceProp properties{};
  cudaError_t error = cudaSetDevice(static_cast<int>(device_ordinal));
  if (error != cudaSuccess) goto fail;
  error = cudaGetDeviceProperties(&properties, static_cast<int>(device_ordinal));
  if (error != cudaSuccess) goto fail;
  if (properties.major != 12 || properties.minor != 0) {
    error = cudaErrorNotSupported;
    goto fail;
  }
  session->grid_size = properties.multiProcessorCount * 2;
  error = cudaStreamCreateWithFlags(&session->stream, cudaStreamNonBlocking);
  if (error != cudaSuccess) goto fail;
  error = cudaEventCreate(&session->start_event);
  if (error != cudaSuccess) goto fail;
  error = cudaEventCreate(&session->end_event);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_a, a_bytes);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_b, b_bytes);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_key, 32);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_target, 32);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_winner, sizeof(uint64_t));
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_debug_state, 16 * sizeof(int32_t));
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_debug_hash, 32);
  if (error != cudaSuccess) goto fail;
  error = cudaMemcpyAsync(session->d_a, a_prime, a_bytes, cudaMemcpyHostToDevice,
                          session->stream);
  if (error != cudaSuccess) goto fail;
  error = cudaMemcpyAsync(session->d_b, b_prime, b_bytes, cudaMemcpyHostToDevice,
                          session->stream);
  if (error != cudaSuccess) goto fail;
  error = cudaMemcpyAsync(session->d_key, pow_key, 32, cudaMemcpyHostToDevice,
                          session->stream);
  if (error != cudaSuccess) goto fail;
  error = cudaFuncSetAttribute(ai_pow_v3_peak_kernel,
                               cudaFuncAttributeMaxDynamicSharedMemorySize,
                               kDynamicSmemBytes);
  if (error != cudaSuccess) goto fail;
  error = cudaStreamSynchronize(session->stream);
  if (error != cudaSuccess) goto fail;
  *session_out = session;
  return static_cast<int>(cudaSuccess);

fail:
  ai_pow_cuda_peak_session_destroy(session);
  return static_cast<int>(error);
}

extern "C" int ai_pow_cuda_peak_source_session_create(
    uint32_t device_ordinal,
    uint32_t m,
    uint32_t n,
    uint32_t k,
    uint32_t rank,
    uint32_t tile,
    const int8_t* a,
    const int8_t* b,
    AiPowCudaPeakSession** session_out) {
  if (session_out == nullptr || a == nullptr || b == nullptr ||
      m == 0 || n == 0 || k != kK || rank != kRank ||
      tile != kHashTile || m % kBm != 0 || n % kBn != 0) {
    return static_cast<int>(cudaErrorInvalidValue);
  }
  *session_out = nullptr;
  size_t a_bytes = 0;
  size_t b_bytes = 0;
  size_t e_l_bytes = 0;
  size_t f_r_bytes = 0;
  if (!checked_product(m, k, &a_bytes) ||
      !checked_product(n, k, &b_bytes) ||
      !checked_product(m, rank, &e_l_bytes) ||
      !checked_product(n, rank, &f_r_bytes)) {
    return static_cast<int>(cudaErrorInvalidValue);
  }
  const uint32_t a_chunks = static_cast<uint32_t>(a_bytes / kChunkBytes);
  const uint32_t b_chunks = static_cast<uint32_t>(b_bytes / kChunkBytes);
  if (a_bytes % kChunkBytes != 0 || b_bytes % kChunkBytes != 0 ||
      a_chunks == 0 || b_chunks == 0) {
    return static_cast<int>(cudaErrorNotSupported);
  }
  const uint32_t max_chunks = a_chunks > b_chunks ? a_chunks : b_chunks;
  size_t cv_words = 0;
  size_t cv_bytes = 0;
  size_t position_words = 0;
  size_t position_bytes = 0;
  if (!checked_product(max_chunks, 8, &cv_words) ||
      !checked_product(cv_words, sizeof(uint32_t), &cv_bytes) ||
      !checked_product(k, 2, &position_words) ||
      !checked_product(position_words, sizeof(uint32_t), &position_bytes)) {
    return static_cast<int>(cudaErrorInvalidValue);
  }
  const uint64_t row_tiles = m / tile;
  const uint64_t col_tiles = n / tile;
  if (row_tiles > UINT64_MAX / col_tiles) {
    return static_cast<int>(cudaErrorInvalidValue);
  }

  auto* session = static_cast<AiPowCudaPeakSession*>(
      std::calloc(1, sizeof(AiPowCudaPeakSession)));
  if (session == nullptr) return static_cast<int>(cudaErrorMemoryAllocation);
  session->device_ordinal = device_ordinal;
  session->m = m;
  session->n = n;
  session->total_tickets = row_tiles * col_tiles;
  session->a_bytes = a_bytes;
  session->b_bytes = b_bytes;
  session->source_mode = true;
  session->prepared = false;

  cudaDeviceProp properties{};
  cudaError_t error = cudaSetDevice(static_cast<int>(device_ordinal));
  if (error != cudaSuccess) goto fail;
  error = cudaGetDeviceProperties(&properties, static_cast<int>(device_ordinal));
  if (error != cudaSuccess) goto fail;
  if (properties.major != 12 || properties.minor != 0) {
    error = cudaErrorNotSupported;
    goto fail;
  }
  session->grid_size = properties.multiProcessorCount * 2;
  error = cudaStreamCreateWithFlags(&session->stream, cudaStreamNonBlocking);
  if (error != cudaSuccess) goto fail;
  error = cudaEventCreate(&session->start_event);
  if (error != cudaSuccess) goto fail;
  error = cudaEventCreate(&session->commitment_event);
  if (error != cudaSuccess) goto fail;
  error = cudaEventCreate(&session->end_event);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_source_a, a_bytes);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_source_b, b_bytes);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_a, a_bytes);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_b, b_bytes);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_transcript, kTranscriptBytes);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_kappa, 32);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_h_a, 32);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_h_b, 32);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_key, 32);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_s_b, 32);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_cv_ping, cv_bytes);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_cv_pong, cv_bytes);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_e_l, e_l_bytes);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_f_r, f_r_bytes);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_e_positions, position_bytes);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_f_positions, position_bytes);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_target, 32);
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_winner, sizeof(uint64_t));
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_debug_state, 16 * sizeof(int32_t));
  if (error != cudaSuccess) goto fail;
  error = cudaMalloc(&session->d_debug_hash, 32);
  if (error != cudaSuccess) goto fail;
  error = cudaMemcpyAsync(session->d_source_a, a, a_bytes,
                          cudaMemcpyHostToDevice, session->stream);
  if (error != cudaSuccess) goto fail;
  error = cudaMemcpyAsync(session->d_source_b, b, b_bytes,
                          cudaMemcpyHostToDevice, session->stream);
  if (error != cudaSuccess) goto fail;
  error = cudaFuncSetAttribute(ai_pow_v3_peak_kernel,
                               cudaFuncAttributeMaxDynamicSharedMemorySize,
                               kDynamicSmemBytes);
  if (error != cudaSuccess) goto fail;
  error = cudaStreamSynchronize(session->stream);
  if (error != cudaSuccess) goto fail;
  *session_out = session;
  return static_cast<int>(cudaSuccess);

fail:
  ai_pow_cuda_peak_session_destroy(session);
  return static_cast<int>(error);
}

extern "C" int ai_pow_cuda_peak_session_prepare(
    AiPowCudaPeakSession* session,
    const uint8_t sigma[76],
    const uint8_t mu[52],
    AiPowCudaPeakPrepareResult* result_out) {
  if (session == nullptr || sigma == nullptr || mu == nullptr ||
      result_out == nullptr || !session->source_mode) {
    return static_cast<int>(cudaErrorInvalidValue);
  }
  session->prepared = false;
  uint8_t transcript[kTranscriptBytes];
  std::memcpy(transcript, sigma, kSigmaBytes);
  std::memcpy(transcript + kSigmaBytes, mu, kMuBytes);
  cudaError_t error = cudaSetDevice(static_cast<int>(session->device_ordinal));
  if (error != cudaSuccess) return static_cast<int>(error);
  error = cudaMemcpyAsync(session->d_transcript, transcript, kTranscriptBytes,
                          cudaMemcpyHostToDevice, session->stream);
  if (error != cudaSuccess) return static_cast<int>(error);
  error = cudaEventRecord(session->start_event, session->stream);
  if (error != cudaSuccess) return static_cast<int>(error);
  peak_kappa_kernel<<<1, 1, 0, session->stream>>>(
      session->d_transcript, session->d_kappa);
  error = cudaGetLastError();
  if (error != cudaSuccess) return static_cast<int>(error);

  const uint32_t a_chunks =
      static_cast<uint32_t>(session->a_bytes / kChunkBytes);
  const uint32_t b_chunks =
      static_cast<uint32_t>(session->b_bytes / kChunkBytes);
  error = launch_peak_matrix_commitment(
      session->d_source_a, a_chunks, session->d_kappa,
      session->d_cv_ping, session->d_cv_pong, session->d_h_a,
      session->stream);
  if (error != cudaSuccess) return static_cast<int>(error);
  error = launch_peak_matrix_commitment(
      session->d_source_b, b_chunks, session->d_kappa,
      session->d_cv_ping, session->d_cv_pong, session->d_h_b,
      session->stream);
  if (error != cudaSuccess) return static_cast<int>(error);
  peak_seed_kernel<<<1, 1, 0, session->stream>>>(
      session->d_kappa, session->d_h_a, session->d_h_b,
      session->m, session->n, session->d_key, session->d_s_b);
  error = cudaGetLastError();
  if (error != cudaSuccess) return static_cast<int>(error);
  error = cudaEventRecord(session->commitment_event, session->stream);
  if (error != cudaSuccess) return static_cast<int>(error);

  const uint32_t e_hashes =
      static_cast<uint32_t>((size_t(session->m) * kRank) / 32);
  const uint32_t f_hashes =
      static_cast<uint32_t>((size_t(session->n) * kRank) / 32);
  const uint32_t e_blocks =
      (e_hashes + kPrepareThreads - 1) / kPrepareThreads;
  const uint32_t f_blocks =
      (f_hashes + kPrepareThreads - 1) / kPrepareThreads;
  peak_uniform_noise_kernel<<<e_blocks, kPrepareThreads, 0, session->stream>>>(
      e_hashes, false, session->d_key, session->d_e_l);
  error = cudaGetLastError();
  if (error != cudaSuccess) return static_cast<int>(error);
  peak_uniform_noise_kernel<<<f_blocks, kPrepareThreads, 0, session->stream>>>(
      f_hashes, true, session->d_s_b, session->d_f_r);
  error = cudaGetLastError();
  if (error != cudaSuccess) return static_cast<int>(error);
  constexpr uint32_t position_hashes = kK / 8;
  constexpr uint32_t position_blocks =
      (position_hashes + kPrepareThreads - 1) / kPrepareThreads;
  peak_position_kernel<<<position_blocks, kPrepareThreads, 0, session->stream>>>(
      position_hashes, false, session->d_key, session->d_e_positions);
  error = cudaGetLastError();
  if (error != cudaSuccess) return static_cast<int>(error);
  peak_position_kernel<<<position_blocks, kPrepareThreads, 0, session->stream>>>(
      position_hashes, true, session->d_s_b, session->d_f_positions);
  error = cudaGetLastError();
  if (error != cudaSuccess) return static_cast<int>(error);
  const uint32_t noise_blocks =
      static_cast<uint32_t>(session->grid_size) * 8;
  peak_apply_noise_kernel<<<noise_blocks, kPrepareThreads, 0, session->stream>>>(
      session->d_source_a, session->a_bytes, session->d_e_l,
      session->d_e_positions, session->d_a);
  error = cudaGetLastError();
  if (error != cudaSuccess) return static_cast<int>(error);
  peak_apply_noise_kernel<<<noise_blocks, kPrepareThreads, 0, session->stream>>>(
      session->d_source_b, session->b_bytes, session->d_f_r,
      session->d_f_positions, session->d_b);
  error = cudaGetLastError();
  if (error != cudaSuccess) return static_cast<int>(error);
  error = cudaEventRecord(session->end_event, session->stream);
  if (error != cudaSuccess) return static_cast<int>(error);
  error = cudaEventSynchronize(session->end_event);
  if (error != cudaSuccess) return static_cast<int>(error);
  error = cudaEventElapsedTime(&result_out->commitment_ms, session->start_event,
                               session->commitment_event);
  if (error != cudaSuccess) return static_cast<int>(error);
  error = cudaEventElapsedTime(&result_out->noise_ms,
                               session->commitment_event, session->end_event);
  if (error != cudaSuccess) return static_cast<int>(error);
#define COPY_TRANSCRIPT(field, source) do {                                     \
  error = cudaMemcpy((field), (source), 32, cudaMemcpyDeviceToHost);             \
  if (error != cudaSuccess) return static_cast<int>(error);                      \
} while (0)
  COPY_TRANSCRIPT(result_out->kappa, session->d_kappa);
  COPY_TRANSCRIPT(result_out->h_a, session->d_h_a);
  COPY_TRANSCRIPT(result_out->h_b, session->d_h_b);
  COPY_TRANSCRIPT(result_out->s_a, session->d_key);
  COPY_TRANSCRIPT(result_out->s_b, session->d_s_b);
#undef COPY_TRANSCRIPT
  session->prepared = true;
  return static_cast<int>(cudaSuccess);
}

extern "C" int ai_pow_cuda_peak_session_debug(
    AiPowCudaPeakSession* session,
    uint64_t ordinal,
    int32_t state_out[16],
    uint8_t jackpot_out[32]) {
  if (session == nullptr || state_out == nullptr || jackpot_out == nullptr ||
      !session->prepared || ordinal >= session->total_tickets) {
    return static_cast<int>(cudaErrorInvalidValue);
  }
  cudaError_t error = cudaSetDevice(static_cast<int>(session->device_ordinal));
  if (error != cudaSuccess) return static_cast<int>(error);
  ai_pow_v3_peak_debug_kernel<<<1, 256, 0, session->stream>>>(
      session->d_a, session->d_b, static_cast<int>(session->n), ordinal,
      session->d_key, session->d_debug_state, session->d_debug_hash);
  error = cudaGetLastError();
  if (error != cudaSuccess) return static_cast<int>(error);
  error = cudaMemcpyAsync(state_out, session->d_debug_state,
                          16 * sizeof(int32_t), cudaMemcpyDeviceToHost,
                          session->stream);
  if (error != cudaSuccess) return static_cast<int>(error);
  error = cudaMemcpyAsync(jackpot_out, session->d_debug_hash, 32,
                          cudaMemcpyDeviceToHost, session->stream);
  if (error != cudaSuccess) return static_cast<int>(error);
  return static_cast<int>(cudaStreamSynchronize(session->stream));
}

extern "C" int ai_pow_cuda_peak_session_search(
    AiPowCudaPeakSession* session,
    uint64_t ordinal_start,
    uint64_t ordinal_count,
    const uint8_t target[32],
    AiPowCudaPeakSearchResult* result_out) {
  if (session == nullptr || target == nullptr || result_out == nullptr ||
      !session->prepared || ordinal_count == 0 ||
      ordinal_start >= session->total_tickets ||
      ordinal_count > session->total_tickets - ordinal_start) {
    return static_cast<int>(cudaErrorInvalidValue);
  }
  cudaError_t error = cudaSetDevice(static_cast<int>(session->device_ordinal));
  if (error != cudaSuccess) return static_cast<int>(error);
  error = cudaMemsetAsync(session->d_winner, 0xff, sizeof(uint64_t), session->stream);
  if (error != cudaSuccess) return static_cast<int>(error);
  error = cudaMemcpyAsync(session->d_target, target, 32, cudaMemcpyHostToDevice,
                          session->stream);
  if (error != cudaSuccess) return static_cast<int>(error);
  error = cudaEventRecord(session->start_event, session->stream);
  if (error != cudaSuccess) return static_cast<int>(error);
  ai_pow_v3_peak_kernel<<<session->grid_size, kThreads, kDynamicSmemBytes,
                          session->stream>>>(
      session->d_a, session->d_b, static_cast<int>(session->m),
      static_cast<int>(session->n), session->d_target, session->d_key,
      ordinal_start, ordinal_start + ordinal_count, session->d_winner);
  error = cudaGetLastError();
  if (error != cudaSuccess) return static_cast<int>(error);
  error = cudaEventRecord(session->end_event, session->stream);
  if (error != cudaSuccess) return static_cast<int>(error);
  error = cudaEventSynchronize(session->end_event);
  if (error != cudaSuccess) return static_cast<int>(error);
  error = cudaEventElapsedTime(&result_out->kernel_ms, session->start_event,
                               session->end_event);
  if (error != cudaSuccess) return static_cast<int>(error);
  error = cudaMemcpy(&result_out->winner_ordinal, session->d_winner,
                     sizeof(uint64_t), cudaMemcpyDeviceToHost);
  if (error != cudaSuccess) return static_cast<int>(error);
  std::memset(result_out->jackpot, 0, sizeof(result_out->jackpot));
  if (result_out->winner_ordinal != kNoWinner) {
    int32_t state[16];
    return ai_pow_cuda_peak_session_debug(session, result_out->winner_ordinal,
                                          state, result_out->jackpot);
  }
  return static_cast<int>(cudaSuccess);
}

extern "C" int ai_pow_cuda_peak_session_destroy(AiPowCudaPeakSession* session) {
  if (session == nullptr) return static_cast<int>(cudaSuccess);
  cudaError_t first = cudaSetDevice(static_cast<int>(session->device_ordinal));
#define FREE_DEVICE(pointer) do {                                                \
  if ((pointer) != nullptr) {                                                    \
    const cudaError_t current = cudaFree(pointer);                               \
    if (first == cudaSuccess) first = current;                                   \
  }                                                                              \
} while (0)
  FREE_DEVICE(session->d_f_positions);
  FREE_DEVICE(session->d_e_positions);
  FREE_DEVICE(session->d_f_r);
  FREE_DEVICE(session->d_e_l);
  FREE_DEVICE(session->d_cv_pong);
  FREE_DEVICE(session->d_cv_ping);
  FREE_DEVICE(session->d_s_b);
  FREE_DEVICE(session->d_h_b);
  FREE_DEVICE(session->d_h_a);
  FREE_DEVICE(session->d_kappa);
  FREE_DEVICE(session->d_transcript);
  FREE_DEVICE(session->d_debug_hash);
  FREE_DEVICE(session->d_debug_state);
  FREE_DEVICE(session->d_winner);
  FREE_DEVICE(session->d_target);
  FREE_DEVICE(session->d_key);
  FREE_DEVICE(session->d_b);
  FREE_DEVICE(session->d_a);
  FREE_DEVICE(session->d_source_b);
  FREE_DEVICE(session->d_source_a);
#undef FREE_DEVICE
  if (session->end_event != nullptr) {
    const cudaError_t current = cudaEventDestroy(session->end_event);
    if (first == cudaSuccess) first = current;
  }
  if (session->commitment_event != nullptr) {
    const cudaError_t current = cudaEventDestroy(session->commitment_event);
    if (first == cudaSuccess) first = current;
  }
  if (session->start_event != nullptr) {
    const cudaError_t current = cudaEventDestroy(session->start_event);
    if (first == cudaSuccess) first = current;
  }
  if (session->stream != nullptr) {
    const cudaError_t current = cudaStreamDestroy(session->stream);
    if (first == cudaSuccess) first = current;
  }
  std::free(session);
  return static_cast<int>(first);
}
