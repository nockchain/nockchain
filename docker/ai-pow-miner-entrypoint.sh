#!/usr/bin/env bash
set -euo pipefail

: "${MINING_PKH:?MINING_PKH must contain the v1 mining pubkey hash}"
: "${NODE_ADDR:?NODE_ADDR must contain the node private gRPC URL}"

args=(
  --gpu
  --canonical
  --node-addr "$NODE_ADDR"
  --mining-pkh "$MINING_PKH"
  --cuda-devices "${CUDA_DEVICES:-all}"
)

exec /usr/local/bin/ai-pow-mine "${args[@]}" "$@"
