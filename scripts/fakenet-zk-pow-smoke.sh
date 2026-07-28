#!/usr/bin/env bash
# scripts/fakenet-zk-pow-smoke.sh
#
# End-to-end fakenet smoke for the zk-pow-miner architecture:
#   1. Boot a fakenet `nockchain` node (no in-process miner; gRPC private port).
#   2. Boot a separate `zk-pow-mine` binary that connects to it.
#   3. Wait for the node to log "added to validated blocks at <h>" for h ≥ 1,
#      which proves the miner found a block AND the node accepted it.
#
# Verifier setup is mandatory node state even in this ZK-only smoke. First boot
# may spend several minutes generating the table; the script reuses only that
# proof-independent setup cache while keeping consensus PMA/event state fresh.
#
# Tunables via env vars:
#   PRIV_PORT         — node private gRPC port               (default: 25555)
#   FAKENET_POW_LEN   — fakenet pow-len                      (default: 2)
#   FAKENET_LOG_DIFF  — log target difficulty (2^N)          (default: 1)
#   NUM_THREADS       — miner pool size                      (default: 1)
#   TIMEOUT_SECS      — post-boot mining wait                (default: 180)
#   BOOT_TIMEOUT_SECS — verifier setup / born wait           (default: 1200)
#   MINING_PKH        — payout pkh (defaults to a valid stub)

set -euo pipefail

PRIV_PORT="${PRIV_PORT:-25555}"
FAKENET_POW_LEN="${FAKENET_POW_LEN:-2}"
FAKENET_LOG_DIFF="${FAKENET_LOG_DIFF:-1}"
NUM_THREADS="${NUM_THREADS:-1}"
TIMEOUT_SECS="${TIMEOUT_SECS:-180}"
BOOT_TIMEOUT_SECS="${BOOT_TIMEOUT_SECS:-1200}"
MINING_PKH="${MINING_PKH:-9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV}"

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# Persist only verifier setup; consensus PMA/event state is per-run.
SETUP_CACHE_DIR="${AI_POW_VERIFIER_SETUP_CACHE_DIR:-$REPO_ROOT/.fakenet-ai-pow-data/ai-pow}"

echo "== fakenet-zk-pow-smoke =="
echo "  PRIV_PORT       = $PRIV_PORT"
echo "  FAKENET_POW_LEN = $FAKENET_POW_LEN"
echo "  FAKENET_LOG_DIFF= $FAKENET_LOG_DIFF"
echo "  NUM_THREADS     = $NUM_THREADS"
echo "  TIMEOUT_SECS    = $TIMEOUT_SECS"
echo "  BOOT_TIMEOUT    = $BOOT_TIMEOUT_SECS"
echo "  SETUP_CACHE_DIR = $SETUP_CACHE_DIR"
echo "  MINING_PKH      = $MINING_PKH"

echo
echo "[build] nockchain + zk-pow-mine ..."
cargo build --release -p nockchain --bin nockchain
cargo build --release -p zk-pow-miner --bin zk-pow-mine

WORK_DIR="$(mktemp -d -t fakenet-zk-pow-smoke.XXXXXX)"
NODE_LOG="$WORK_DIR/node.log"
MINER_LOG="$WORK_DIR/miner.log"
echo "[setup] work_dir=$WORK_DIR"
DATA_DIR="$WORK_DIR/node-data"
mkdir -p "$DATA_DIR"
if [[ -d "$SETUP_CACHE_DIR" ]]; then
    mkdir -p "$DATA_DIR/ai-pow"
    cp -R "$SETUP_CACHE_DIR/." "$DATA_DIR/ai-pow/"
fi

NODE_PID=""
MINER_PID=""
EXIT_CODE=99

cleanup() {
    local rc=$?
    set +e
    echo
    echo "[cleanup] tearing down (rc=$rc)"
    if [[ -n "$MINER_PID" ]]; then
        kill "$MINER_PID" 2>/dev/null
        wait "$MINER_PID" 2>/dev/null
    fi
    if [[ -n "$NODE_PID" ]]; then
        kill "$NODE_PID" 2>/dev/null
        wait "$NODE_PID" 2>/dev/null
    fi
    if [[ -d "$DATA_DIR/ai-pow" ]]; then
        mkdir -p "$SETUP_CACHE_DIR"
        cp -R "$DATA_DIR/ai-pow/." "$SETUP_CACHE_DIR/"
    fi
    echo "[cleanup] logs preserved at $WORK_DIR"
    if [[ "$EXIT_CODE" -ne 0 ]]; then
        echo
        echo "===== node.log (tail) ====="
        tail -60 "$NODE_LOG" 2>/dev/null || true
        echo
        echo "===== miner.log (tail) ====="
        tail -40 "$MINER_LOG" 2>/dev/null || true
    fi
    exit "$EXIT_CODE"
}
trap cleanup EXIT INT TERM

# Run the node in $WORK_DIR so its .nockchain_identity etc. don't pollute the repo.
NODE_BIN="$REPO_ROOT/target/release/nockchain"
MINER_BIN="$REPO_ROOT/target/release/zk-pow-mine"

echo
echo "[boot ] starting node ..."
pushd "$WORK_DIR" >/dev/null
RUST_LOG="${NODE_RUST_LOG:-info}" \
    "$NODE_BIN" \
    --fakenet \
    --data-dir "$DATA_DIR" \
    --bind-private-grpc-port "$PRIV_PORT" \
    --fakenet-pow-len "$FAKENET_POW_LEN" \
    --fakenet-log-difficulty "$FAKENET_LOG_DIFF" \
    --no-default-peers \
    --bind /ip4/127.0.0.1/udp/0/quic-v1 \
    >"$NODE_LOG" 2>&1 &
NODE_PID=$!
popd >/dev/null
echo "[boot ] node pid=$NODE_PID; waiting for %born (up to ${BOOT_TIMEOUT_SECS}s for setup gen)..."

# Wait for the kernel's born command to run. The driver-level "born poke sent"
# line is not readiness: verifier setup can still be generating.
DEADLINE=$(( SECONDS + BOOT_TIMEOUT_SECS ))
while (( SECONDS < DEADLINE )); do
    if grep -aq "handle-command: born" "$NODE_LOG" 2>/dev/null; then
        echo "[boot ] node reached %born"
        break
    fi
    if ! kill -0 "$NODE_PID" 2>/dev/null; then
        echo "[fail ] node died before %born"
        EXIT_CODE=2
        exit 2
    fi
    sleep 1
done
if ! grep -aq "handle-command: born" "$NODE_LOG" 2>/dev/null; then
    echo "[fail ] timeout waiting for %born"
    EXIT_CODE=2
    exit 2
fi

# Brief settle.
sleep 2

echo
echo "[boot ] starting miner ..."
RUST_LOG="${MINER_RUST_LOG:-info}" \
    "$MINER_BIN" \
    --node-addr "http://127.0.0.1:$PRIV_PORT" \
    --mining-pkh "$MINING_PKH" \
    --num-threads "$NUM_THREADS" \
    >"$MINER_LOG" 2>&1 &
MINER_PID=$!
echo "[boot ] miner pid=$MINER_PID"

echo
echo "[wait ] polling for accepted block h>=1 (timeout ${TIMEOUT_SECS}s) ..."
DEADLINE=$(( SECONDS + TIMEOUT_SECS ))
SAW_BLOCK=0
# Require height >= 1 (post-genesis); genesis is at height 0 and lands
# pre-mining as the fakenet bootstrap. h>=1 proves the miner ran a
# real STARK and the node accepted the proof.
PATTERN='added to validated blocks at ([1-9][0-9]*|[1-9])'
while (( SECONDS < DEADLINE )); do
    if grep -E -q "$PATTERN" "$NODE_LOG" 2>/dev/null; then
        SAW_BLOCK=1
        break
    fi
    if ! kill -0 "$NODE_PID" 2>/dev/null; then
        echo "[fail ] node died before producing a block"
        EXIT_CODE=3
        exit 3
    fi
    if ! kill -0 "$MINER_PID" 2>/dev/null; then
        echo "[fail ] miner died before producing a block"
        EXIT_CODE=4
        exit 4
    fi
    sleep 2
done

if (( SAW_BLOCK == 1 )); then
    echo "[ok   ] node accepted a mined block (height>=1)"
    grep -E "$PATTERN" "$NODE_LOG" | tail -3
    EXIT_CODE=0
else
    echo "[fail ] timeout waiting for accepted block at height >= 1"
    EXIT_CODE=5
fi
