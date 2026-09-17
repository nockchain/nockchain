#pragma once

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct ZkPowCudaSession ZkPowCudaSession;

typedef struct {
  uint64_t commitment[5];
  uint64_t target[5];
  uint64_t puzzle_tag_hash[5];
  uint64_t length_hash[5];
  uint64_t pow_tag_hash[5];
  uint32_t pow_len;
} ZkPowV5Job;

typedef struct {
  uint64_t nonce[5];
  uint64_t attempts;
  float kernel_ms;
  uint32_t found;
} ZkPowCudaResult;

int zk_pow_cuda_device_count(uint32_t *count);
int zk_pow_cuda_device_name(uint32_t device, char *name, uint32_t name_len);
int zk_pow_cuda_session_create(uint32_t device, uint32_t blocks_per_sm,
                               uint32_t threads_per_block,
                               ZkPowCudaSession **session);
void zk_pow_cuda_session_destroy(ZkPowCudaSession *session);
int zk_pow_cuda_search(ZkPowCudaSession *session, const ZkPowV5Job *job,
                       const uint64_t start_nonce[5], uint64_t attempts,
                       ZkPowCudaResult *result);
int zk_pow_cuda_digest(ZkPowCudaSession *session, const ZkPowV5Job *job,
                       const uint64_t nonce[5], uint64_t digest[5]);
const char *zk_pow_cuda_error_string(int error);

#ifdef __cplusplus
}
#endif
