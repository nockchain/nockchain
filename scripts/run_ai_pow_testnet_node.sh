#!/usr/bin/env bash
set -euo pipefail

# All consensus parameters below must match the AI-PoW testnet.
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
node_bin="${NOCKCHAIN_BIN:-$repo_root/target/release/nockchain}"
data_dir="${NOCKCHAIN_TESTNET_DATA_DIR:-$repo_root/nock_fake}"

if [[ ! -x "$node_bin" ]]; then
    printf 'nockchain binary is missing or not executable: %s\n' "$node_bin" >&2
    printf 'Build it with: cargo build --release --bin nockchain\n' >&2
    exit 1
fi

mkdir -p "$data_dir"

exec env RUST_LOG="${RUST_LOG:-info}" "$node_bin" \
    --data-dir "$data_dir" \
    --identity-path "$data_dir/identity.key" \
    --bind /ip4/0.0.0.0/udp/3400/quic-v1 \
    --bind-private-grpc-port 5555 \
    --no-default-peers \
    --force-peer /ip4/216.82.192.27/udp/3400/quic-v1 \
    --fakenet \
    --fakenet-log-difficulty 1 \
    --fakenet-pow-len 2 \
    --fakenet-asert-phase 1 \
    --fakenet-asert-anchor-height 0 \
    --fakenet-asert-anchor-target-bex 291 \
    --fakenet-ai-pow-activation-height 1 \
    --fakenet-ai-asert-phase 1 \
    --fakenet-ai-asert-anchor-height 0 \
    --fakenet-ai-asert-anchor-target-bex 232 \
    --fakenet-update-candidate-interval-secs 120
