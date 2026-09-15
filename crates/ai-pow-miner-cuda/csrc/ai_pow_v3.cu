#include "ai_pow_gemm.h"

#include <cuda_runtime.h>

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <new>

namespace {

constexpr uint32_t kM = 64;
constexpr uint32_t kN = 64;
constexpr uint32_t kK = 1024;
constexpr uint32_t kRank = 64;
constexpr uint32_t kOpened = 8;
constexpr uint32_t kChunks = 64;
constexpr uint32_t kNoWinner = 0xffffffffu;

constexpr uint32_t kChunkStart = 1u << 0;
constexpr uint32_t kChunkEnd = 1u << 1;
constexpr uint32_t kParent = 1u << 2;
constexpr uint32_t kRoot = 1u << 3;
constexpr uint32_t kKeyed = 1u << 4;
__device__ __constant__ uint32_t kIv[8] = {
    0x6a09e667u, 0xbb67ae85u, 0x3c6ef372u, 0xa54ff53au,
    0x510e527fu, 0x9b05688cu, 0x1f83d9abu, 0x5be0cd19u,
};
__device__ __constant__ uint32_t kSaltA[8] = {
    0x6c404982u, 0x1615eda0u, 0x92f61696u, 0xf876f0fcu,
    0x2adbdb92u, 0x52b82370u, 0x1977d4f0u, 0x7b0190c3u,
};
__device__ __constant__ uint32_t kSaltB[8] = {
    0x32063011u, 0xca0163ecu, 0x71afe22bu, 0x4f4d3f8bu,
    0x39c6e91au, 0x04cce888u, 0x1d304448u, 0xa99ab871u,
};

__device__ __forceinline__ uint32_t rotr(uint32_t value, int shift) {
  return (value >> shift) | (value << (32 - shift));
}

#define G(a, b, c, d, mx, my) do { \
  (a) = (a) + (b) + (mx);                  \
  (d) = rotr((d) ^ (a), 16);               \
  (c) = (c) + (d);                         \
  (b) = rotr((b) ^ (c), 12);               \
  (a) = (a) + (b) + (my);                  \
  (d) = rotr((d) ^ (a), 8);                \
  (c) = (c) + (d);                         \
  (b) = rotr((b) ^ (c), 7);                \
} while (0)

template <int Round> struct Schedule;
template <> struct Schedule<0> { static __device__ __forceinline__ int at(int i) {
  const int s[16] = {0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15}; return s[i]; }};
template <> struct Schedule<1> { static __device__ __forceinline__ int at(int i) {
  const int s[16] = {2,6,3,10,7,0,4,13,1,11,12,5,9,14,15,8}; return s[i]; }};
template <> struct Schedule<2> { static __device__ __forceinline__ int at(int i) {
  const int s[16] = {3,4,10,12,13,2,7,14,6,5,9,0,11,15,8,1}; return s[i]; }};
template <> struct Schedule<3> { static __device__ __forceinline__ int at(int i) {
  const int s[16] = {10,7,12,9,14,3,13,15,4,0,11,2,5,8,1,6}; return s[i]; }};
template <> struct Schedule<4> { static __device__ __forceinline__ int at(int i) {
  const int s[16] = {12,13,9,11,15,10,14,8,7,2,5,3,0,1,6,4}; return s[i]; }};
template <> struct Schedule<5> { static __device__ __forceinline__ int at(int i) {
  const int s[16] = {9,14,11,5,8,12,15,1,13,3,0,10,2,6,4,7}; return s[i]; }};
template <> struct Schedule<6> { static __device__ __forceinline__ int at(int i) {
  const int s[16] = {11,15,5,0,1,9,8,6,14,10,2,12,3,4,7,13}; return s[i]; }};

template <int Round>
__device__ __forceinline__ void round(uint32_t v[16], const uint32_t m[16]) {
#define M(i) m[Schedule<Round>::at(i)]
  G(v[0],v[4],v[8],v[12],M(0),M(1)); G(v[1],v[5],v[9],v[13],M(2),M(3));
  G(v[2],v[6],v[10],v[14],M(4),M(5)); G(v[3],v[7],v[11],v[15],M(6),M(7));
  G(v[0],v[5],v[10],v[15],M(8),M(9)); G(v[1],v[6],v[11],v[12],M(10),M(11));
  G(v[2],v[7],v[8],v[13],M(12),M(13)); G(v[3],v[4],v[9],v[14],M(14),M(15));
#undef M
}

__device__ __forceinline__ void compress(
    const uint32_t block[16], const uint32_t cv[8], uint64_t counter,
    uint32_t block_len, uint32_t flags, uint32_t out[8]) {
  uint32_t v[16];
#pragma unroll
  for (int i = 0; i < 8; ++i) v[i] = cv[i];
  v[8]=kIv[0]; v[9]=kIv[1]; v[10]=kIv[2]; v[11]=kIv[3];
  v[12]=uint32_t(counter); v[13]=uint32_t(counter >> 32);
  v[14]=block_len; v[15]=flags;
  round<0>(v,block); round<1>(v,block); round<2>(v,block); round<3>(v,block);
  round<4>(v,block); round<5>(v,block); round<6>(v,block);
#pragma unroll
  for (int i = 0; i < 8; ++i) out[i] = v[i] ^ v[i+8];
}

__device__ __forceinline__ uint32_t load32(const uint8_t* p) {
  return uint32_t(p[0]) | (uint32_t(p[1]) << 8) |
         (uint32_t(p[2]) << 16) | (uint32_t(p[3]) << 24);
}

__device__ __forceinline__ void hash_bytes(
    const uint8_t* input, uint32_t length, const uint32_t key[8],
    uint32_t base_flags, uint32_t out[8]) {
  uint32_t cv[8];
#pragma unroll
  for (int i = 0; i < 8; ++i) cv[i] = key[i];
  const uint32_t blocks = (length + 63u) / 64u;
  for (uint32_t block_index = 0; block_index < blocks; ++block_index) {
    uint32_t block[16];
#pragma unroll
    for (int word = 0; word < 16; ++word) {
      uint32_t value = 0;
#pragma unroll
      for (int byte = 0; byte < 4; ++byte) {
        const uint32_t offset = block_index * 64 + word * 4 + byte;
        if (offset < length) value |= uint32_t(input[offset]) << (byte * 8);
      }
      block[word] = value;
    }
    const bool first = block_index == 0;
    const bool last = block_index + 1 == blocks;
    const uint32_t block_len = last ? length - block_index * 64u : 64u;
    uint32_t next[8];
    compress(block, cv, 0, block_len,
             base_flags | (first ? kChunkStart : 0) |
             (last ? (kChunkEnd | kRoot) : 0), next);
#pragma unroll
    for (int i = 0; i < 8; ++i) cv[i] = next[i];
  }
#pragma unroll
  for (int i = 0; i < 8; ++i) out[i] = cv[i];
}

__device__ __forceinline__ void single_block(
    const uint32_t block[16], const uint32_t key[8], uint32_t flags,
    uint32_t out[8]) {
  compress(block, key, 0, 64, flags | kChunkStart | kChunkEnd | kRoot, out);
}

__device__ __forceinline__ void hash_pair(
    const uint32_t left[8], const uint32_t right[8], uint32_t out[8]) {
  uint32_t block[16];
#pragma unroll
  for (int i=0;i<8;++i) { block[i]=left[i]; block[i+8]=right[i]; }
  single_block(block, kIv, 0, out);
}

__device__ __forceinline__ void chunk_cv(
    const uint8_t* bytes, uint32_t length, uint64_t counter,
    const uint32_t key[8], bool root, uint32_t out[8]) {
  uint32_t cv[8];
#pragma unroll
  for (int i=0;i<8;++i) cv[i]=key[i];
  const uint32_t blocks = (length + 63u) / 64u;
  for (uint32_t block_index=0; block_index<blocks; ++block_index) {
    uint32_t block[16];
#pragma unroll
    for (int word=0;word<16;++word) {
      uint32_t value=0;
#pragma unroll
      for (int byte=0;byte<4;++byte) {
        const uint32_t off=block_index*64+word*4+byte;
        if (off<length) value |= uint32_t(bytes[off]) << (byte*8);
      }
      block[word]=value;
    }
    const bool first=block_index==0;
    const bool last=block_index+1==blocks;
    const uint32_t block_len=last ? length-block_index*64u : 64u;
    uint32_t next[8];
    compress(block,cv,counter,block_len,kKeyed |
             (first?kChunkStart:0) | (last?kChunkEnd:0) |
             (last&&root?kRoot:0),next);
#pragma unroll
    for (int i=0;i<8;++i) cv[i]=next[i];
  }
#pragma unroll
  for (int i=0;i<8;++i) out[i]=cv[i];
}

__device__ __forceinline__ void parent_cv(
    const uint32_t left[8], const uint32_t right[8], const uint32_t key[8],
    bool root, uint32_t out[8]) {
  uint32_t block[16];
#pragma unroll
  for (int i=0;i<8;++i) { block[i]=left[i]; block[i+8]=right[i]; }
  compress(block,key,0,64,kKeyed|kParent|(root?kRoot:0),out);
}

__device__ __forceinline__ void random_hash(
    uint32_t index, bool b_side, const uint32_t key[8], uint32_t prepend,
    uint32_t out[8]) {
  uint32_t block[16]{};
  block[prepend]=index+1;
  block[8]=b_side ? 0x65745f42u : 0x65745f41u;
  block[9]=0x726f736eu;
  single_block(block,key,kKeyed,out);
}

__device__ __forceinline__ bool le_target(
    const uint32_t hash[8], const uint32_t target[8]) {
#pragma unroll
  for (int i=7;i>=0;--i) {
    if (hash[i]<target[i]) return true;
    if (hash[i]>target[i]) return false;
  }
  return true;
}

__global__ void commitments_kernel(
    uint32_t start, const int8_t* a, const int8_t* b,
    const uint8_t* sigma, const uint8_t* mu,
    const uint8_t* routing, uint32_t routing_len,
    const uint8_t* offsets, uint32_t offsets_len,
    uint32_t* kappas, uint32_t* h_as, uint32_t* h_bs,
    uint32_t* s_as, uint32_t* s_bs, bool capture_debug) {
  __shared__ uint32_t kappa[8];
  __shared__ uint32_t roots_a[kChunks][8];
  __shared__ uint32_t roots_b[kChunks][8];
  __shared__ uint32_t routing_root[8];
  __shared__ uint32_t offsets_hash[8];
  const uint32_t attempt=blockIdx.x;
  const uint32_t tid=threadIdx.x;
  if (tid==0) {
    uint8_t input[128];
#pragma unroll
    for (int i=0;i<76;++i) input[i]=sigma[i];
#pragma unroll
    for (int i=0;i<52;++i) input[76+i]=mu[i];
    const uint32_t timestamp=load32(input+68)+start+attempt;
    input[68]=uint8_t(timestamp); input[69]=uint8_t(timestamp>>8);
    input[70]=uint8_t(timestamp>>16); input[71]=uint8_t(timestamp>>24);
    hash_bytes(input,128,kIv,0,kappa);
    if (capture_debug) {
#pragma unroll
      for (int i=0;i<8;++i) kappas[attempt*8+i]=kappa[i];
    }
  }
  __syncthreads();
  if (tid<kChunks) {
    chunk_cv(reinterpret_cast<const uint8_t*>(a)+tid*1024,1024,tid,kappa,false,roots_a[tid]);
  } else if (tid<2*kChunks) {
    const uint32_t chunk=tid-kChunks;
    chunk_cv(reinterpret_cast<const uint8_t*>(b)+chunk*1024,1024,chunk,kappa,false,roots_b[chunk]);
  } else if (tid==2*kChunks) {
    chunk_cv(routing,1024,0,kappa,true,routing_root);
  } else if (tid==2*kChunks+1) {
    chunk_cv(offsets,1024,0,kappa,true,offsets_hash);
  }
  __syncthreads();
  for (uint32_t count=kChunks;count>1;count>>=1) {
    const uint32_t pairs=count>>1;
    const bool active=tid<2*pairs;
    const bool b_side=tid>=pairs;
    const uint32_t pair=b_side ? tid-pairs : tid;
    uint32_t parent[8];
    if (active) {
      const uint32_t (*roots)[8]=b_side ? roots_b : roots_a;
      parent_cv(roots[pair*2],roots[pair*2+1],kappa,pairs==1,parent);
    }
    __syncthreads();
    if (active) {
      uint32_t (*roots)[8]=b_side ? roots_b : roots_a;
#pragma unroll
      for (int i=0;i<8;++i) roots[pair][i]=parent[i];
    }
    __syncthreads();
  }
  if (tid==0) {
    (void)routing_len;
    (void)offsets_len;
    uint32_t message_a[16]{},message_b[16]{};
#pragma unroll
    for (int i=0;i<8;++i) { message_a[i]=roots_a[0][i]; message_b[i]=roots_b[0][i]; }
    message_a[8]=kM; message_b[8]=kN/2;
    uint32_t bound_a[8],bound_b[8];
    single_block(message_a,kSaltA,kKeyed,bound_a);
    single_block(message_b,kSaltB,kKeyed,bound_b);
    uint32_t hash_routing[8],activations[8],s_a[8],s_b[8];
    hash_pair(routing_root,offsets_hash,hash_routing);
    hash_pair(bound_a,hash_routing,activations);
    hash_pair(kappa,bound_b,s_b);
    hash_pair(s_b,activations,s_a);
#pragma unroll
    for (int i=0;i<8;++i) {
      if (capture_debug) {
        h_as[attempt*8+i]=roots_a[0][i];
        h_bs[attempt*8+i]=roots_b[0][i];
      }
      s_as[attempt*8+i]=s_a[i];
      s_bs[attempt*8+i]=s_b[i];
    }
  }
}

__device__ __forceinline__ int32_t dp4a_signed(
    const int8_t* a, const int8_t* b, int32_t accumulator) {
  uint32_t a_word, b_word;
  memcpy(&a_word,a,sizeof(a_word));
  memcpy(&b_word,b,sizeof(b_word));
  return __dp4a(int32_t(a_word),int32_t(b_word),accumulator);
}

__global__ void noise_kernel(
    const int8_t* a,const int8_t* b,const uint32_t* rows,const uint32_t* cols,
    const uint32_t* s_as,const uint32_t* s_bs,int8_t* open_a,int8_t* open_b) {
  __shared__ int8_t e_l[kOpened*kRank];
  __shared__ int8_t f_r[kOpened*kRank];
  __shared__ uint8_t e_plus[kK],e_minus[kK],f_plus[kK],f_minus[kK];
  const uint32_t attempt=blockIdx.x,tid=threadIdx.x;
  const uint32_t* s_a=s_as+attempt*8; const uint32_t* s_b=s_bs+attempt*8;
  for (uint32_t work=tid;work<288;work+=blockDim.x) {
    if (work<16) {
      const uint32_t opened=work/2,chunk=work%2;
      uint32_t hash[8]; random_hash(rows[opened]*2+chunk,false,s_a,0,hash);
#pragma unroll
      for (int word=0;word<8;++word) for(int byte=0;byte<4;++byte)
        e_l[work*32+word*4+byte]=int8_t(int((hash[word]>>(byte*8))&63)-32);
    } else if (work<32) {
      const uint32_t relative=work-16,opened=relative/2,chunk=relative%2;
      uint32_t hash[8]; random_hash(cols[opened]*2+chunk,true,s_b,0,hash);
#pragma unroll
      for (int word=0;word<8;++word) for(int byte=0;byte<4;++byte)
        f_r[relative*32+word*4+byte]=int8_t(int((hash[word]>>(byte*8))&63)-32);
    } else if (work<160) {
      const uint32_t chunk=work-32; uint32_t hash[8]; random_hash(chunk,false,s_a,1,hash);
#pragma unroll
      for(int slot=0;slot<8;++slot) { const uint32_t r=hash[slot]; const uint32_t p=r&63;
        e_plus[chunk*8+slot]=uint8_t(p); e_minus[chunk*8+slot]=uint8_t(p^(1+uint32_t((uint64_t(63)*r)>>32))); }
    } else {
      const uint32_t chunk=work-160; uint32_t hash[8]; random_hash(chunk,true,s_b,1,hash);
#pragma unroll
      for(int slot=0;slot<8;++slot) { const uint32_t r=hash[slot]; const uint32_t p=r&63;
        f_plus[chunk*8+slot]=uint8_t(p); f_minus[chunk*8+slot]=uint8_t(p^(1+uint32_t((uint64_t(63)*r)>>32))); }
    }
  }
  __syncthreads();
  const size_t stride=size_t(kOpened)*kK;
  int8_t* da=open_a+size_t(attempt)*stride; int8_t* db=open_b+size_t(attempt)*stride;
  for(uint32_t index=tid;index<stride;index+=blockDim.x) {
    const uint32_t opened=index/kK,l=index%kK;
    const int en=int(e_l[opened*kRank+e_plus[l]])-int(e_l[opened*kRank+e_minus[l]]);
    const int fn=int(f_r[opened*kRank+f_plus[l]])-int(f_r[opened*kRank+f_minus[l]]);
    da[index]=int8_t(int(a[size_t(rows[opened])*kK+l])+en);
    db[index]=int8_t(int(b[size_t(cols[opened])*kK+l])+fn);
  }
}

__global__ void tile_jackpot_kernel(
    const int8_t* open_a,const int8_t* open_b,const uint32_t* s_as,
    const uint32_t* target,int32_t* states,uint32_t* jackpots,uint32_t* winner,
    bool capture_debug) {
  __shared__ int32_t accum[64]; __shared__ int32_t reduction[64];
  __shared__ int32_t state[16];
  const uint32_t attempt=blockIdx.x,tid=threadIdx.x;
  const size_t stride=size_t(kOpened)*kK;
  const int8_t* a=open_a+size_t(attempt)*stride; const int8_t* b=open_b+size_t(attempt)*stride;
  if(tid<64) accum[tid]=0; if(tid<16) state[tid]=0; __syncthreads();
  for(uint32_t step=0;step<16;++step) {
    if(tid<64) {
      const uint32_t row=tid/8,col=tid%8; int32_t delta=0;
#pragma unroll
      for(uint32_t l=0;l<64;l+=4) {
        delta=dp4a_signed(a+row*kK+step*64+l,b+col*kK+step*64+l,delta);
      }
      accum[tid]+=delta; reduction[tid]=accum[tid];
    }
    __syncthreads();
    for(uint32_t shift=32;shift;shift>>=1) { if(tid<shift) reduction[tid]^=reduction[tid+shift]; __syncthreads(); }
    if(tid==0) {
      const uint32_t rotated=(uint32_t(state[step])<<13)|(uint32_t(state[step])>>(32-13));
      state[step]=int32_t(rotated)^reduction[0];
    }
    __syncthreads();
  }
  if(tid==0) {
    uint32_t block[16],hash[8];
#pragma unroll
    for(int i=0;i<16;++i) block[i]=uint32_t(state[i]);
    single_block(block,s_as+size_t(attempt)*8,kKeyed,hash);
#pragma unroll
    for(int i=0;i<8;++i) jackpots[size_t(attempt)*8+i]=hash[i];
    if(capture_debug) for(int i=0;i<16;++i) states[size_t(attempt)*16+i]=state[i];
    if(le_target(hash,target)) atomicMin(winner,attempt);
  }
}

__global__ void copy_winner_kernel(const uint32_t* winner,const uint32_t* jackpots,uint32_t* out) {
  if(threadIdx.x==0 && *winner!=kNoWinner) for(int i=0;i<8;++i) out[i]=jackpots[size_t(*winner)*8+i];
}

}  // namespace

struct AiPowCudaV3Session {
  uint32_t device_ordinal;
  uint32_t max_attempts;
  uint32_t routing_len;
  uint32_t offsets_len;
  cudaStream_t stream;
  int8_t *a,*b,*open_a,*open_b;
  uint8_t *sigma,*mu,*routing,*offsets;
  uint32_t *rows,*cols,*kappas,*h_as,*h_bs,*s_as,*s_bs,*jackpots,*target,*winner,*winner_hash;
  int32_t* states;
  uint32_t *host_winner,*host_hash;
};

namespace {
void destroy_v3(AiPowCudaV3Session* s) {
  if(!s) return;
  cudaSetDevice(static_cast<int>(s->device_ordinal));
#define FREE(field) if(s->field) cudaFree(s->field)
  FREE(winner_hash);FREE(winner);FREE(target);FREE(jackpots);FREE(states);FREE(s_bs);FREE(s_as);
  FREE(h_bs);FREE(h_as);FREE(kappas);FREE(cols);FREE(rows);FREE(offsets);FREE(routing);FREE(mu);FREE(sigma);
  FREE(open_b);FREE(open_a);FREE(b);FREE(a);
#undef FREE
  if(s->host_hash) cudaFreeHost(s->host_hash); if(s->host_winner) cudaFreeHost(s->host_winner);
  if(s->stream) cudaStreamDestroy(s->stream); delete s;
}
}

extern "C" int ai_pow_cuda_v3_session_create(
    uint32_t device_ordinal,uint32_t max_attempts,const int8_t* a_matrix,const int8_t* b_matrix,
    const uint8_t sigma[76],const uint8_t mu[52],const uint8_t* routing_data,
    uint32_t routing_data_len,const uint8_t* routing_offsets,uint32_t routing_offsets_len,
    const uint32_t row_indices[8],const uint32_t col_indices[8],AiPowCudaV3Session** out) {
  if(!out||!max_attempts||!a_matrix||!b_matrix||!sigma||!mu||!routing_data||!routing_data_len||
     routing_data_len>1024||!routing_offsets||!routing_offsets_len||routing_offsets_len>1024||!row_indices||!col_indices)
    return int(cudaErrorInvalidValue);
  *out=nullptr; auto* s=new(std::nothrow) AiPowCudaV3Session{}; if(!s) return int(cudaErrorMemoryAllocation);
  cudaError_t e=cudaSetDevice(static_cast<int>(device_ordinal));if(e!=cudaSuccess){delete s;return int(e);}
  s->device_ordinal=device_ordinal;
  const size_t hb=size_t(max_attempts)*32;
  const size_t strips=size_t(max_attempts)*8192;
  s->max_attempts=max_attempts;s->routing_len=routing_data_len;s->offsets_len=routing_offsets_len;
  e=cudaStreamCreateWithFlags(&s->stream,cudaStreamNonBlocking);if(e!=cudaSuccess)goto fail;
#define ALLOC(field,bytes) do{e=cudaMalloc(reinterpret_cast<void**>(&s->field),(bytes));if(e!=cudaSuccess)goto fail;}while(0)
  ALLOC(a,65536);ALLOC(b,65536);ALLOC(sigma,76);ALLOC(mu,52);ALLOC(routing,1024);ALLOC(offsets,1024);
  ALLOC(rows,32);ALLOC(cols,32);
  ALLOC(kappas,32);ALLOC(h_as,32);ALLOC(h_bs,32);ALLOC(s_as,hb);ALLOC(s_bs,hb);ALLOC(open_a,strips);ALLOC(open_b,strips);
  ALLOC(states,64);ALLOC(jackpots,hb);ALLOC(target,32);ALLOC(winner,4);ALLOC(winner_hash,32);
#undef ALLOC
  e=cudaMallocHost(reinterpret_cast<void**>(&s->host_winner),4);if(e!=cudaSuccess)goto fail;e=cudaMallocHost(reinterpret_cast<void**>(&s->host_hash),32);if(e!=cudaSuccess)goto fail;
  e=cudaMemsetAsync(s->routing,0,1024,s->stream);if(e!=cudaSuccess)goto fail;
  e=cudaMemsetAsync(s->offsets,0,1024,s->stream);if(e!=cudaSuccess)goto fail;
#define COPY(field,source,bytes) do{e=cudaMemcpyAsync(s->field,(source),(bytes),cudaMemcpyHostToDevice,s->stream);if(e!=cudaSuccess)goto fail;}while(0)
  COPY(a,a_matrix,65536);COPY(b,b_matrix,65536);COPY(sigma,sigma,76);COPY(mu,mu,52);COPY(routing,routing_data,routing_data_len);
  COPY(offsets,routing_offsets,routing_offsets_len);COPY(rows,row_indices,32);COPY(cols,col_indices,32);
#undef COPY
  e=cudaStreamSynchronize(s->stream);if(e!=cudaSuccess)goto fail;*out=s;return 0;
fail:destroy_v3(s);return int(e);
}

extern "C" int ai_pow_cuda_v3_session_search(
    AiPowCudaV3Session* s,uint32_t start,uint32_t attempts,const uint8_t target[32],
    uint32_t capture_debug,uint32_t* winner_local,uint8_t jackpot_out[32]) {
  if(!s||!attempts||attempts>s->max_attempts||!target||capture_debug>1||!winner_local||!jackpot_out||
     uint64_t(start)+attempts>(uint64_t(1)<<32)) return int(cudaErrorInvalidValue);
  cudaError_t e=cudaSetDevice(static_cast<int>(s->device_ordinal));if(e!=cudaSuccess)return int(e);
  e=cudaMemcpyAsync(s->target,target,32,cudaMemcpyHostToDevice,s->stream);if(e!=cudaSuccess)return int(e);
  e=cudaMemsetAsync(s->winner,0xff,4,s->stream);if(e!=cudaSuccess)return int(e);e=cudaMemsetAsync(s->winner_hash,0,32,s->stream);if(e!=cudaSuccess)return int(e);
  commitments_kernel<<<attempts,160,0,s->stream>>>(start,s->a,s->b,s->sigma,s->mu,s->routing,s->routing_len,s->offsets,s->offsets_len,
      s->kappas,s->h_as,s->h_bs,s->s_as,s->s_bs,capture_debug!=0);e=cudaGetLastError();if(e!=cudaSuccess)return int(e);
  noise_kernel<<<attempts,256,0,s->stream>>>(s->a,s->b,s->rows,s->cols,s->s_as,s->s_bs,s->open_a,s->open_b);e=cudaGetLastError();if(e!=cudaSuccess)return int(e);
  tile_jackpot_kernel<<<attempts,64,0,s->stream>>>(s->open_a,s->open_b,s->s_as,s->target,s->states,s->jackpots,s->winner,capture_debug!=0);
  e=cudaGetLastError();if(e!=cudaSuccess)return int(e);copy_winner_kernel<<<1,1,0,s->stream>>>(s->winner,s->jackpots,s->winner_hash);
  e=cudaGetLastError();if(e!=cudaSuccess)return int(e);e=cudaMemcpyAsync(s->host_winner,s->winner,4,cudaMemcpyDeviceToHost,s->stream);if(e!=cudaSuccess)return int(e);
  e=cudaMemcpyAsync(s->host_hash,s->winner_hash,32,cudaMemcpyDeviceToHost,s->stream);if(e!=cudaSuccess)return int(e);e=cudaStreamSynchronize(s->stream);if(e!=cudaSuccess)return int(e);
  *winner_local=*s->host_winner;std::memcpy(jackpot_out,s->host_hash,32);return 0;
}

extern "C" int ai_pow_cuda_v3_session_debug(
    AiPowCudaV3Session* s,uint32_t extranonce,uint8_t kappa[32],uint8_t h_a[32],uint8_t h_b[32],uint8_t s_a[32],uint8_t s_b[32],
    int8_t a_rows[8192],int8_t b_cols[8192],int32_t state[16],uint8_t jackpot[32]) {
  if(!s||!kappa||!h_a||!h_b||!s_a||!s_b||!a_rows||!b_cols||!state||!jackpot)return int(cudaErrorInvalidValue);
  uint8_t target[32];std::memset(target,0xff,32);uint32_t winner;
  int status=ai_pow_cuda_v3_session_search(s,extranonce,1,target,1,&winner,jackpot);if(status)return status;
#define GET(destination,source,bytes) do{cudaError_t e=cudaMemcpyAsync((destination),(source),(bytes),cudaMemcpyDeviceToHost,s->stream);if(e!=cudaSuccess)return int(e);}while(0)
  GET(kappa,s->kappas,32);GET(h_a,s->h_as,32);GET(h_b,s->h_bs,32);GET(s_a,s->s_as,32);GET(s_b,s->s_bs,32);
  GET(a_rows,s->open_a,8192);GET(b_cols,s->open_b,8192);GET(state,s->states,64);
#undef GET
  return int(cudaStreamSynchronize(s->stream));
}

extern "C" int ai_pow_cuda_v3_session_destroy(AiPowCudaV3Session* s) { destroy_v3(s);return 0; }
