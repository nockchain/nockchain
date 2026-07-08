#!/usr/bin/env bash
# scripts/mainnet-sync-compat.sh
#
# Runs a mainnet sync against the default backbone peer to validate
# that the ai-pow-integration kernel still accepts every pre-95000
# (and post-95000 ZK) block identically to the unmodified kernel.
# Exercises the kernel-state 9->10 migration on boot, consensus.hoon
# compute-target paths, accept-block, and derived-state population.
#
# Logs:   /tmp/mainnet-sync-compat.log (or $LOG_FILE)
# Data:   ./.data.mainnet-sync-compat  (or $DATA_DIR)
# Bind:   udp/31000 quic-v1            (or $BIND_PORT)

set -euo pipefail

LOG_FILE="${LOG_FILE:-/tmp/mainnet-sync-compat.log}"
DATA_DIR="${DATA_DIR:-./.data.mainnet-sync-compat}"
BIND_PORT="${BIND_PORT:-31000}"
NODE_BIN="${NODE_BIN:-./target/release/nockchain}"

if [[ ! -x "$NODE_BIN" ]]; then
    echo "error: $NODE_BIN not built. Run: cargo build --release -p nockchain --bin nockchain" >&2
    exit 1
fi

mkdir -p "$DATA_DIR"

echo "== mainnet-sync-compat =="
echo "  NODE_BIN  = $NODE_BIN"
echo "  DATA_DIR  = $DATA_DIR"
echo "  LOG_FILE  = $LOG_FILE"
echo "  BIND_PORT = $BIND_PORT"
echo

# Background the node, write PID to a file alongside the data dir
# so it's easy to kill later.
PID_FILE="$DATA_DIR/node.pid"

nohup env \
    RUST_LOG="info,nockchain=info,nockapp::kernel::form=info,nockapp::kernel::boot=info,libp2p=warn" \
    MINIMAL_LOG_FORMAT=true \
    "$NODE_BIN" \
        --data-dir "$DATA_DIR" \
        --identity-path "$DATA_DIR/.nockchain_identity" \
        --bind "/ip4/0.0.0.0/udp/$BIND_PORT/quic-v1" \
        --fast-sync \
        --gc-interval 900 \
    > "$LOG_FILE" 2>&1 &

NODE_PID=$!
echo "$NODE_PID" > "$PID_FILE"

echo "node pid = $NODE_PID  (also in $PID_FILE)"
echo
echo "Tail progress with:"
echo "  tail -f $LOG_FILE | grep --line-buffered -E 'added to validated blocks at|kernel state|migration|panic|ERROR|reject|invalid'"
echo
echo "Stop the node with:"
echo "  kill \$(cat $PID_FILE)"
