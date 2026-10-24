#!/usr/bin/env bash
set -euo pipefail

: "${MINING_PKH:?MINING_PKH must contain the v1 mining pubkey hash}"
: "${NODE_ADDR:?NODE_ADDR must contain the node private gRPC URL}"

# NodeClient selects the restricted role when the miner credential is present.
# Keep tokens in the inherited environment, never arguments or log output.
if [ -z "${NOCKCHAIN_PRIVATE_GRPC_AUTH_TOKEN:-}" ] &&
   [ -z "${NOCKCHAIN_PRIVATE_GRPC_MINER_TOKEN:-}" ]; then
  case "${NOCKCHAIN_ALLOW_UNAUTHENTICATED_PRIVATE_GRPC:-}" in
    1|true|TRUE|yes|YES) ;;
    *)
      echo "Set NOCKCHAIN_PRIVATE_GRPC_AUTH_TOKEN or NOCKCHAIN_PRIVATE_GRPC_MINER_TOKEN; development-only tokenless mining requires NOCKCHAIN_ALLOW_UNAUTHENTICATED_PRIVATE_GRPC=1 and an explicitly opted-out loopback node." >&2
      exit 1
      ;;
  esac
fi

args=(
  --gpu
  --reference
  --node-addr "$NODE_ADDR"
  --mining-pkh "$MINING_PKH"
  --cuda-devices "${CUDA_DEVICES:-all}"
)

exec /usr/local/bin/ai-pow-mine "${args[@]}" "$@"
