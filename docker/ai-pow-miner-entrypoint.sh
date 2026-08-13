#!/usr/bin/env bash
set -euo pipefail

: "${MINING_PKH:?MINING_PKH must contain the v1 mining pubkey hash}"
: "${NODE_ADDR:?NODE_ADDR must contain the node private gRPC URL}"

args=(
  --gpu
  --node-addr "$NODE_ADDR"
  --mining-pkh "$MINING_PKH"
  --cuda-device "${CUDA_DEVICE:-0}"
  --gpu-batch-attempts "${GPU_BATCH_ATTEMPTS:-256}"
)

if [[ "${CANONICAL:-true}" == "true" ]]; then
  args+=(--canonical)
else
  : "${PEARL_GATEWAY:?PEARL_GATEWAY is required when CANONICAL is not true}"
  args+=(--pearl-gateway "$PEARL_GATEWAY")
fi

exec /usr/local/bin/ai-pow-mine "${args[@]}" "$@"
