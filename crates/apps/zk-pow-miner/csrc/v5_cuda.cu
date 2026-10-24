#include "v5_cuda.h"

#include <cuda_runtime.h>

#include <algorithm>
#include <climits>
#include <cstring>
#include <new>

namespace {

constexpr uint64_t kPrime = 0xffffffff00000001ULL;
constexpr uint64_t kMontOne = 0x00000000ffffffffULL;
constexpr uint64_t kR2 = 0xfffffffe00000001ULL;
constexpr int kStateSize = 16;
constexpr int kRate = 10;
constexpr int kRounds = 7;

__constant__ uint8_t kLookupTable[256] = {
    0,   7,   26,  63,  124, 215, 85,  254, 214, 228, 45,  185, 140, 173, 33,  240,
    29,  177, 176, 32,  8,   110, 87,  202, 204, 99,  150, 106, 230, 14,  235, 128,
    213, 239, 212, 138, 23,  130, 208, 6,   44,  71,  93,  116, 146, 189, 251, 81,
    199, 97,  38,  28,  73,  179, 95,  84,  152, 48,  35,  119, 49,  88,  242, 3,
    148, 169, 72,  120, 62,  161, 166, 83,  175, 191, 137, 19,  100, 129, 112, 55,
    221, 102, 218, 61,  151, 237, 68,  164, 17,  147, 46,  234, 203, 216, 22,  141,
    65,  57,  123, 12,  244, 54,  219, 231, 96,  77,  180, 154, 5,   253, 133, 165,
    98,  195, 205, 134, 245, 30,  9,   188, 59,  142, 186, 197, 181, 144, 92,  31,
    224, 163, 111, 74,  58,  69,  113, 196, 67,  246, 225, 10,  121, 50,  60,  157,
    90,  122, 2,   250, 101, 75,  178, 159, 24,  36,  201, 11,  243, 132, 198, 190,
    114, 233, 39,  52,  21,  209, 108, 238, 91,  187, 18,  104, 194, 37,  153, 34,
    200, 143, 126, 155, 236, 118, 64,  80,  172, 89,  94,  193, 135, 183, 86,  107,
    252, 13,  167, 206, 136, 220, 207, 103, 171, 160, 76,  182, 227, 217, 158, 56,
    174, 4,   66,  109, 139, 162, 184, 211, 249, 47,  125, 232, 117, 43,  16,  42,
    127, 20,  241, 25,  149, 105, 156, 51,  53,  168, 145, 247, 223, 79,  78,  226,
    15,  222, 82,  115, 70,  210, 27,  41,  1,   170, 40,  131, 192, 229, 248, 255,
};

__constant__ uint16_t kMdsFirstColumn[kStateSize] = {
    61402, 1108,  28750, 33823, 7454,  43244, 53865, 12034,
    56951, 27521, 41351, 40901, 12021, 59689, 26798, 17845,
};

__constant__ uint64_t kRoundConstants[kRounds][kStateSize] = {
    {6813007285744613222ULL, 9538108458283805344ULL, 8266796718228611711ULL,
     10279833152781686635ULL, 16136178252164695712ULL, 11678500968092896548ULL,
     18177224533631314584ULL, 8519208882353197867ULL, 15278933395031186751ULL,
     5605030382266121712ULL, 3266079902019342405ULL, 16977155338689078860ULL,
     575533378161618286ULL, 14008024146379968822ULL, 16952256074650551489ULL,
     10699818153468018415ULL},
    {16274322239097854776ULL, 16277174423203830480ULL, 15551543598572731978ULL,
     17734447432847017984ULL, 10634644696612177250ULL, 14223629804877666866ULL,
     9951585614956111842ULL, 13410522825507264153ULL, 15504271780310363158ULL,
     15788426030062030790ULL, 7247426745733321025ULL, 15545848059337170693ULL,
     7257327654080199927ULL, 2632620606461733813ULL, 5468949404670321892ULL,
     3408181798280532022ULL},
    {6407521478186447124ULL, 10532483258532500040ULL, 9962573180077511189ULL,
     14997058441937336819ULL, 8347291381276979462ULL, 1834710304424753372ULL,
     8919127106750279878ULL, 17952692726686580444ULL, 10425759383842794244ULL,
     3571063091305112274ULL, 11196674225031209104ULL, 17831978239644188755ULL,
     7386054759687923415ULL, 49233562557975441ULL, 15763370708992892484ULL,
     7042466268943341941ULL},
    {14925546578125121441ULL, 5737865664192390903ULL, 6112071640890712275ULL,
     7093386846491465789ULL, 12933769084390453308ULL, 840431699266909703ULL,
     2593502341286518015ULL, 805532971224672190ULL, 662776811092263083ULL,
     1082592850076858062ULL, 2260713232066289719ULL, 18161814497919979745ULL,
     11436170062534698819ULL, 9156670326168191466ULL, 13690674722453603930ULL,
     16450526946025880915ULL},
    {3443037901035637703ULL, 13512956751884108002ULL, 12765464435334038877ULL,
     16857582347068713433ULL, 4403818324750733470ULL, 16327824648413653612ULL,
     9624633671524957693ULL, 11798148227002487001ULL, 4806282851616964758ULL,
     13789375745913111929ULL, 8048230392833675591ULL, 15394445679479006170ULL,
     5381819560221452561ULL, 4546720664034456941ULL, 17286163612312122987ULL,
     16936562784938244714ULL},
    {11067749825657848638ULL, 5556080822347806028ULL, 3866118074743041663ULL,
     2201009632364155631ULL, 10808969316669713964ULL, 9312983943061336112ULL,
     17369380183573126906ULL, 12953586427039891533ULL, 16564382082196301935ULL,
     6117018641086235131ULL, 2379948990303454544ULL, 9900641007991965131ULL,
     14289331750432136160ULL, 12105488135916678431ULL, 14113550218116986428ULL,
     13441194625000926086ULL},
    {8346758232352358445ULL, 13109503806329090541ULL, 16233458644157342064ULL,
     3717000905522992223ULL, 4028024080310608291ULL, 16928904978228437531ULL,
     486523272751840851ULL, 17746229827600458028ULL, 4231774801891550196ULL,
     11401341037617516726ULL, 12004481761165906799ULL, 1880237553532135241ULL,
     7506757868197934780ULL, 1656439004520781315ULL, 7739084580576441604ULL,
     17945328382079677663ULL},
};

__device__ __forceinline__ uint64_t field_add(uint64_t left, uint64_t right) {
  const uint64_t complement = kPrime - right;
  uint64_t result = left - complement;
  if (left < complement) {
    result -= 0xffffffffULL;
  }
  return result;
}

__device__ __forceinline__ uint64_t mont_mul(uint64_t left, uint64_t right) {
  const uint64_t low = left * right;
  const uint64_t high = __umul64hi(left, right);
  const uint64_t x1 = low >> 32;
  const uint64_t sum = (low & 0xffffffffULL) + x1;
  const uint64_t c_low = sum << 32;
  const uint64_t d = (sum >> 32) == 0
                         ? c_low - x1
                         : c_low + (0xffffffffULL - x1);
  return high >= d ? high - d : (kPrime - d) + high;
}

__device__ __forceinline__ uint64_t montify(uint64_t value) {
  return mont_mul(value, kR2);
}

__device__ __forceinline__ uint64_t demontify(uint64_t value) {
  return mont_mul(value, 1);
}

__device__ __forceinline__ uint64_t reduce_small_u128(uint64_t low,
                                                       uint64_t high) {
  const uint64_t folded = high * 0xffffffffULL;
  uint64_t result = low + folded;
  if (result < low) {
    result += 0xffffffffULL;
  }
  if (result >= kPrime) {
    result -= kPrime;
  }
  return result;
}

__device__ __forceinline__ uint64_t lookup_sbox(uint64_t value,
                                                const uint8_t *table) {
  uint64_t result = 0;
#pragma unroll
  for (int byte = 0; byte < 8; ++byte) {
    result |= static_cast<uint64_t>(table[(value >> (byte * 8)) & 0xff])
              << (byte * 8);
  }
  return result;
}

__device__ __forceinline__ uint64_t pow7(uint64_t value) {
  const uint64_t square = mont_mul(value, value);
  const uint64_t fourth = mont_mul(square, square);
  return mont_mul(value, mont_mul(square, fourth));
}

__device__ __forceinline__ uint64_t subgroup_shuffle(uint32_t mask,
                                                      uint64_t value,
                                                      int source_lane) {
  return __shfl_sync(mask, value, source_lane, kStateSize);
}

__device__ __forceinline__ uint64_t permute(uint64_t state, int lane,
                                             uint32_t mask,
                                             const uint8_t *lookup) {
#pragma unroll
  for (int round = 0; round < kRounds; ++round) {
    const uint64_t nonlinear = lane < 4 ? lookup_sbox(state, lookup) : pow7(state);
    uint64_t low = 0;
    uint64_t high = 0;
#pragma unroll
    for (int input_lane = 0; input_lane < kStateSize; ++input_lane) {
      const uint64_t input = subgroup_shuffle(mask, nonlinear, input_lane);
      const uint64_t coefficient =
          kMdsFirstColumn[(lane + kStateSize - input_lane) & 15];
      const uint64_t product_low = input * coefficient;
      const uint64_t product_high = __umul64hi(input, coefficient);
      const uint64_t previous = low;
      low += product_low;
      high += product_high + static_cast<uint64_t>(low < previous);
    }
    state = field_add(reduce_small_u128(low, high),
                      kRoundConstants[round][lane]);
  }
  return state;
}

__device__ __forceinline__ uint64_t pair_hash(uint64_t left,
                                               uint64_t right,
                                               int lane, uint32_t mask,
                                               const uint8_t *lookup) {
  const int source_lane = lane >= 5 && lane < 10 ? lane - 5 : 0;
  const uint64_t right_limb =
      subgroup_shuffle(mask, right, source_lane);
  uint64_t state = 0;
  if (lane < 5) {
    state = left;
  } else if (lane < 10) {
    state = right_limb;
  } else {
    state = kMontOne;
  }
  return permute(state, lane, mask, lookup);
}

__device__ __forceinline__ uint64_t nonce_limb(const uint64_t start[5],
                                                uint64_t increment,
                                                int requested_limb) {
  uint64_t value = 0;
  uint64_t carry = increment;
#pragma unroll
  for (int limb = 0; limb < 5; ++limb) {
    uint64_t next;
    if (carry == 0) {
      next = start[limb];
    } else if (limb == 0) {
      if (start[limb] >= kPrime - carry) {
        next = start[limb] - (kPrime - carry);
        carry = 1;
      } else {
        next = start[limb] + carry;
        carry = 0;
      }
    } else if (start[limb] == kPrime - 1) {
      next = 0;
      carry = 1;
    } else {
      next = start[limb] + 1;
      carry = 0;
    }
    if (limb == requested_limb) {
      value = next;
    }
  }
  return value;
}

__device__ __forceinline__ void absorb_word(uint64_t &state, uint64_t word,
                                             int &position, int lane,
                                             uint32_t mask,
                                             const uint8_t *lookup) {
  if (lane == position) {
    state = word;
  }
  ++position;
  if (position == kRate) {
    state = permute(state, lane, mask, lookup);
    position = 0;
  }
}

__device__ __forceinline__ uint64_t v5_digest_limb(
    const ZkPowV5Job *job, const uint64_t start_nonce[5], uint64_t increment,
    int lane, uint32_t mask, const uint8_t *lookup) {
  const uint64_t nonce = lane < 5 ? nonce_limb(start_nonce, increment, lane) : 0;

  const int nonce_source = lane >= 5 && lane < 10 ? lane - 5 : 0;
  const uint64_t shuffled_nonce =
      subgroup_shuffle(mask, montify(nonce), nonce_source);
  uint64_t rng_state = 0;
  if (lane < 5) {
    rng_state = montify(job->commitment[lane]);
  } else if (lane < 10) {
    rng_state = shuffled_nonce;
  }
  rng_state = permute(rng_state, lane, mask, lookup);
  if (lane < kRate) {
    rng_state = lane == 0 ? kMontOne : 0;
  }
  rng_state = permute(rng_state, lane, mask, lookup);

  uint64_t leaves[4] = {0, 0, 0, 0};
  uint32_t produced = 0;
  while (produced < job->pow_len) {
    const uint32_t remaining = job->pow_len - produced;
    const int count =
        static_cast<int>(remaining < kRate ? remaining : kRate);
    const uint64_t output = lane < count ? demontify(rng_state) : 0;
#pragma unroll
    for (int index = 0; index < kRate; ++index) {
      if (index < count) {
        const uint32_t leaf_index = produced + index;
        const uint64_t word = subgroup_shuffle(mask, output, index);
        if (lane == static_cast<int>(leaf_index & 15)) {
          leaves[leaf_index >> 4] = word;
        }
      }
    }
    produced += count;
    rng_state = permute(rng_state, lane, mask, lookup);
  }

  uint64_t product_state = 0;
  int position = 0;
  absorb_word(product_state, montify(static_cast<uint64_t>(job->pow_len) + 1),
              position, lane, mask, lookup);
  for (uint32_t remaining = job->pow_len; remaining > 0; --remaining) {
    const uint32_t leaf_index = remaining - 1;
    const uint64_t local_word = leaves[leaf_index >> 4];
    const uint64_t word =
        subgroup_shuffle(mask, local_word, leaf_index & 15);
    absorb_word(product_state, montify(word), position, lane, mask, lookup);
  }

  absorb_word(product_state, 0, position, lane, mask, lookup);
  for (uint32_t index = 0; index < job->pow_len; ++index) {
    absorb_word(product_state, 0, position, lane, mask, lookup);
    absorb_word(product_state, kMontOne, position, lane, mask, lookup);
  }
  if (lane < kRate && lane >= position) {
    product_state = lane == position ? kMontOne : 0;
  }
  product_state = permute(product_state, lane, mask, lookup);

  uint64_t state = pair_hash(
      lane < 5 ? montify(job->length_hash[lane]) : 0, product_state, lane,
      mask, lookup);
  state = pair_hash(lane < 5 ? montify(nonce) : 0, state, lane, mask, lookup);
  state = pair_hash(lane < 5 ? montify(job->commitment[lane]) : 0, state,
                    lane, mask, lookup);
  state = pair_hash(lane < 5 ? montify(job->puzzle_tag_hash[lane]) : 0, state,
                    lane, mask, lookup);

  const uint64_t object_hash = state;
  if (lane < 5) {
    state = object_hash;
  } else if (lane < kRate) {
    state = lane == 5 ? kMontOne : 0;
  } else {
    state = 0;
  }
  state = permute(state, lane, mask, lookup);

  state = pair_hash(lane < 5 ? montify(job->pow_tag_hash[lane]) : 0, state,
                    lane, mask, lookup);
  return lane < 5 ? demontify(state) : 0;
}

__device__ __forceinline__ bool digest_meets_target(uint64_t digest,
                                                     const ZkPowV5Job *job,
                                                     int lane, uint32_t mask) {
  int comparison = 0;
  for (int limb = 4; limb >= 0; --limb) {
    const uint64_t value = subgroup_shuffle(mask, digest, limb);
    if (lane == 0 && comparison == 0) {
      if (value < job->target[limb]) {
        comparison = -1;
      } else if (value > job->target[limb]) {
        comparison = 1;
      }
    }
  }
  return comparison <= 0;
}

__global__ void search_kernel(const ZkPowV5Job *job,
                              const uint64_t *start_nonce, uint64_t attempts,
                              uint64_t *winner) {
  __shared__ uint8_t lookup[256];
  for (int index = threadIdx.x; index < 256; index += blockDim.x) {
    lookup[index] = kLookupTable[index];
  }
  __syncthreads();

  const int lane = threadIdx.x & 15;
  const uint32_t mask = (threadIdx.x & 16) == 0 ? 0x0000ffffU : 0xffff0000U;
  uint64_t attempt =
      (static_cast<uint64_t>(blockIdx.x) * blockDim.x + threadIdx.x) >> 4;
  const uint64_t stride =
      (static_cast<uint64_t>(gridDim.x) * blockDim.x) >> 4;
  for (; attempt < attempts; attempt += stride) {
    const uint64_t digest =
        v5_digest_limb(job, start_nonce, attempt, lane, mask, lookup);
    const bool meets_target =
        digest_meets_target(digest, job, lane, mask);
    if (lane == 0 && meets_target) {
      atomicMin(reinterpret_cast<unsigned long long *>(winner),
                static_cast<unsigned long long>(attempt));
    }
  }
}

__global__ void digest_kernel(const ZkPowV5Job *job,
                              const uint64_t *nonce,
                              uint64_t *digest_output) {
  __shared__ uint8_t lookup[256];
  for (int index = threadIdx.x; index < 256; index += blockDim.x) {
    lookup[index] = kLookupTable[index];
  }
  __syncthreads();
  if (threadIdx.x >= 16) {
    return;
  }
  const int lane = threadIdx.x;
  const uint64_t digest =
      v5_digest_limb(job, nonce, 0, lane, 0x0000ffffU, lookup);
  if (lane < 5) {
    digest_output[lane] = digest;
  }
}

void add_nonce_host(const uint64_t start[5], uint64_t increment,
                    uint64_t output[5]) {
  unsigned __int128 carry = increment;
  for (int limb = 0; limb < 5; ++limb) {
    const unsigned __int128 sum =
        static_cast<unsigned __int128>(start[limb]) + carry;
    output[limb] = static_cast<uint64_t>(sum % kPrime);
    carry = sum / kPrime;
  }
}

int check(cudaError_t error) { return static_cast<int>(error); }

}  // namespace

struct ZkPowCudaSession {
  uint32_t device;
  uint32_t blocks_per_sm;
  uint32_t threads_per_block;
  uint32_t multiprocessors;
  cudaStream_t stream;
  cudaEvent_t started;
  cudaEvent_t stopped;
  ZkPowV5Job *device_job;
  uint64_t *device_nonce;
  uint64_t *device_winner;
  uint64_t *device_digest;
};

extern "C" int zk_pow_cuda_device_count(uint32_t *count) {
  if (count == nullptr) {
    return check(cudaErrorInvalidValue);
  }
  int value = 0;
  const cudaError_t error = cudaGetDeviceCount(&value);
  if (error == cudaSuccess) {
    *count = static_cast<uint32_t>(value);
  }
  return check(error);
}

extern "C" int zk_pow_cuda_device_name(uint32_t device, char *name,
                                         uint32_t name_len) {
  if (name == nullptr || name_len == 0) {
    return check(cudaErrorInvalidValue);
  }
  cudaDeviceProp properties{};
  const cudaError_t error = cudaGetDeviceProperties(&properties, device);
  if (error == cudaSuccess) {
    std::strncpy(name, properties.name, name_len - 1);
    name[name_len - 1] = '\0';
  }
  return check(error);
}

extern "C" int zk_pow_cuda_session_create(uint32_t device,
                                            uint32_t blocks_per_sm,
                                            uint32_t threads_per_block,
                                            ZkPowCudaSession **output) {
  if (output == nullptr || blocks_per_sm == 0 || threads_per_block < 32 ||
      threads_per_block > 1024 || threads_per_block % 32 != 0) {
    return check(cudaErrorInvalidValue);
  }
  *output = nullptr;
  cudaError_t error = cudaSetDevice(device);
  if (error != cudaSuccess) {
    return check(error);
  }
  cudaDeviceProp properties{};
  error = cudaGetDeviceProperties(&properties, device);
  if (error != cudaSuccess) {
    return check(error);
  }
  int active_blocks = 0;
  error = cudaOccupancyMaxActiveBlocksPerMultiprocessor(
      &active_blocks, search_kernel, threads_per_block, 0);
  if (error != cudaSuccess) {
    return check(error);
  }
  if (active_blocks == 0) {
    return check(cudaErrorLaunchOutOfResources);
  }

  auto *session = new (std::nothrow) ZkPowCudaSession{};
  if (session == nullptr) {
    return check(cudaErrorMemoryAllocation);
  }
  session->device = device;
  session->blocks_per_sm = blocks_per_sm;
  session->threads_per_block = threads_per_block;
  session->multiprocessors = properties.multiProcessorCount;

#define CUDA_CREATE(call)                \
  do {                                   \
    error = (call);                      \
    if (error != cudaSuccess) {          \
      zk_pow_cuda_session_destroy(session); \
      return check(error);               \
    }                                    \
  } while (0)

  CUDA_CREATE(cudaStreamCreateWithFlags(&session->stream, cudaStreamNonBlocking));
  CUDA_CREATE(cudaEventCreate(&session->started));
  CUDA_CREATE(cudaEventCreate(&session->stopped));
  CUDA_CREATE(cudaMalloc(&session->device_job, sizeof(ZkPowV5Job)));
  CUDA_CREATE(cudaMalloc(&session->device_nonce, sizeof(uint64_t) * 5));
  CUDA_CREATE(cudaMalloc(&session->device_winner, sizeof(uint64_t)));
  CUDA_CREATE(cudaMalloc(&session->device_digest, sizeof(uint64_t) * 5));
#undef CUDA_CREATE

  *output = session;
  return check(cudaSuccess);
}

extern "C" void zk_pow_cuda_session_destroy(ZkPowCudaSession *session) {
  if (session == nullptr) {
    return;
  }
  cudaSetDevice(session->device);
  if (session->device_digest != nullptr) cudaFree(session->device_digest);
  if (session->device_winner != nullptr) cudaFree(session->device_winner);
  if (session->device_nonce != nullptr) cudaFree(session->device_nonce);
  if (session->device_job != nullptr) cudaFree(session->device_job);
  if (session->stopped != nullptr) cudaEventDestroy(session->stopped);
  if (session->started != nullptr) cudaEventDestroy(session->started);
  if (session->stream != nullptr) cudaStreamDestroy(session->stream);
  delete session;
}

extern "C" int zk_pow_cuda_search(ZkPowCudaSession *session,
                                    const ZkPowV5Job *job,
                                    const uint64_t start_nonce[5],
                                    uint64_t attempts,
                                    ZkPowCudaResult *result) {
  if (session == nullptr || job == nullptr || start_nonce == nullptr ||
      result == nullptr || attempts == 0 || attempts >= kPrime ||
      job->pow_len == 0 || job->pow_len > 64) {
    return check(cudaErrorInvalidValue);
  }
  cudaError_t error = cudaSetDevice(session->device);
  if (error != cudaSuccess) return check(error);

  error = cudaMemcpyAsync(session->device_job, job, sizeof(*job),
                          cudaMemcpyHostToDevice, session->stream);
  if (error != cudaSuccess) return check(error);
  error = cudaMemcpyAsync(session->device_nonce, start_nonce,
                          sizeof(uint64_t) * 5, cudaMemcpyHostToDevice,
                          session->stream);
  if (error != cudaSuccess) return check(error);
  error = cudaMemsetAsync(session->device_winner, 0xff, sizeof(uint64_t),
                          session->stream);
  if (error != cudaSuccess) return check(error);

  const uint64_t attempts_per_block = session->threads_per_block / 16;
  const uint64_t needed_blocks =
      (attempts + attempts_per_block - 1) / attempts_per_block;
  const uint64_t resident_blocks = static_cast<uint64_t>(session->multiprocessors) *
                                   session->blocks_per_sm;
  const uint32_t blocks = static_cast<uint32_t>(
      std::max<uint64_t>(1, std::min(needed_blocks, resident_blocks)));

  error = cudaEventRecord(session->started, session->stream);
  if (error != cudaSuccess) return check(error);
  search_kernel<<<blocks, session->threads_per_block, 0, session->stream>>>(
      session->device_job, session->device_nonce, attempts,
      session->device_winner);
  error = cudaGetLastError();
  if (error != cudaSuccess) return check(error);
  error = cudaEventRecord(session->stopped, session->stream);
  if (error != cudaSuccess) return check(error);

  uint64_t winner = ULLONG_MAX;
  error = cudaMemcpyAsync(&winner, session->device_winner, sizeof(winner),
                          cudaMemcpyDeviceToHost, session->stream);
  if (error != cudaSuccess) return check(error);
  error = cudaStreamSynchronize(session->stream);
  if (error != cudaSuccess) return check(error);

  float elapsed_ms = 0;
  error = cudaEventElapsedTime(&elapsed_ms, session->started, session->stopped);
  if (error != cudaSuccess) return check(error);

  std::memset(result, 0, sizeof(*result));
  result->attempts = attempts;
  result->kernel_ms = elapsed_ms;
  result->found = winner != ULLONG_MAX;
  if (result->found != 0) {
    add_nonce_host(start_nonce, winner, result->nonce);
  } else {
    add_nonce_host(start_nonce, attempts, result->nonce);
  }
  return check(cudaSuccess);
}

extern "C" int zk_pow_cuda_digest(ZkPowCudaSession *session,
                                    const ZkPowV5Job *job,
                                    const uint64_t nonce[5],
                                    uint64_t digest[5]) {
  if (session == nullptr || job == nullptr || nonce == nullptr ||
      digest == nullptr || job->pow_len == 0 || job->pow_len > 64) {
    return check(cudaErrorInvalidValue);
  }
  cudaError_t error = cudaSetDevice(session->device);
  if (error != cudaSuccess) return check(error);
  error = cudaMemcpyAsync(session->device_job, job, sizeof(*job),
                          cudaMemcpyHostToDevice, session->stream);
  if (error != cudaSuccess) return check(error);
  error = cudaMemcpyAsync(session->device_nonce, nonce, sizeof(uint64_t) * 5,
                          cudaMemcpyHostToDevice, session->stream);
  if (error != cudaSuccess) return check(error);
  digest_kernel<<<1, 256, 0, session->stream>>>(
      session->device_job, session->device_nonce, session->device_digest);
  error = cudaGetLastError();
  if (error != cudaSuccess) return check(error);
  error = cudaMemcpyAsync(digest, session->device_digest, sizeof(uint64_t) * 5,
                          cudaMemcpyDeviceToHost, session->stream);
  if (error != cudaSuccess) return check(error);
  return check(cudaStreamSynchronize(session->stream));
}

extern "C" const char *zk_pow_cuda_error_string(int error) {
  return cudaGetErrorString(static_cast<cudaError_t>(error));
}
