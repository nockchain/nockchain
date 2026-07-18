#!/usr/bin/env bash
# scripts/fakenet-zk-pow-post-ai-smoke.sh
#
# Variant of fakenet-zk-pow-smoke.sh that activates AI-PoW EARLY
# (--fakenet-ai-pow-activation-height 2) so the ZK ASERT regime-2
# (post-AI) compute path is exercised in fakenet, including:
#   - the lazy population of the cached-zk-asert-post-ai-anchor by
#     accept-block when the first block at height >= 2 lands
#   - subsequent compute-target-zk-asert calls READING from the cache
#     to produce mineable targets without a runtime ancestry walk
#
# Success criterion: chain produces blocks past the activation height
# (height >= 4 means: genesis at 0, last pre-activation at 1, first
# post-activation at 2, first cache-using at 3, and we want one more
# to confirm steady-state). A timeout failure here indicates either
# (a) the cache was not populated (compute-target-zk-asert crashes
# with %zk-asert-post-ai-anchor-cache-empty), or (b) the regime-2 ZK
# ASERT produces unmineable difficulty (e.g. anchor-target wrong,
# placeholder values not patched up by cache).

set -euo pipefail

PRIV_PORT="${PRIV_PORT:-25556}"
FAKENET_POW_LEN="${FAKENET_POW_LEN:-2}"
FAKENET_LOG_DIFF="${FAKENET_LOG_DIFF:-1}"
FAKENET_AI_ACTIVATION="${FAKENET_AI_ACTIVATION:-2}"
NUM_THREADS="${NUM_THREADS:-1}"
TIMEOUT_SECS="${TIMEOUT_SECS:-360}"
MIN_HEIGHT="${MIN_HEIGHT:-4}"
MINING_PKH="${MINING_PKH:-9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV}"

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

echo "== fakenet-zk-pow-post-ai-smoke =="
echo "  PRIV_PORT             = $PRIV_PORT"
echo "  FAKENET_POW_LEN       = $FAKENET_POW_LEN"
echo "  FAKENET_LOG_DIFF      = $FAKENET_LOG_DIFF"
echo "  FAKENET_AI_ACTIVATION = $FAKENET_AI_ACTIVATION (chain crosses regime-2 at this height)"
echo "  NUM_THREADS           = $NUM_THREADS"
echo "  TIMEOUT_SECS          = $TIMEOUT_SECS"
echo "  MIN_HEIGHT            = $MIN_HEIGHT (must exceed activation by at least 2)"
echo "  MINING_PKH            = $MINING_PKH"

echo
echo "[build] nockchain + zk-pow-mine ..."
cargo build --release -p nockchain --bin nockchain
cargo build --release -p zk-pow-miner --bin zk-pow-mine

WORK_DIR="$(mktemp -d -t fakenet-zk-pow-post-ai-smoke.XXXXXX)"
NODE_LOG="$WORK_DIR/node.log"
MINER_LOG="$WORK_DIR/miner.log"
echo "[setup] work_dir=$WORK_DIR"

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

NODE_BIN="$REPO_ROOT/target/release/nockchain"
MINER_BIN="$REPO_ROOT/target/release/zk-pow-mine"

echo
echo "[boot ] starting node with AI activation at $FAKENET_AI_ACTIVATION ..."
pushd "$WORK_DIR" >/dev/null
RUST_LOG="${NODE_RUST_LOG:-info}" \
    "$NODE_BIN" \
    --fakenet \
    --bind-private-grpc-port "$PRIV_PORT" \
    --fakenet-pow-len "$FAKENET_POW_LEN" \
    --fakenet-log-difficulty "$FAKENET_LOG_DIFF" \
    --fakenet-ai-pow-activation-height "$FAKENET_AI_ACTIVATION" \
    --no-default-peers \
    --bind /ip4/127.0.0.1/udp/0/quic-v1 \
    >"$NODE_LOG" 2>&1 &
NODE_PID=$!
popd >/dev/null
echo "[boot ] node pid=$NODE_PID; waiting for born..."

DEADLINE=$(( SECONDS + 60 ))
while (( SECONDS < DEADLINE )); do
    if grep -q "born" "$NODE_LOG" 2>/dev/null; then
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
echo "[wait ] polling for accepted block h>=$MIN_HEIGHT (timeout ${TIMEOUT_SECS}s) ..."
DEADLINE=$(( SECONDS + TIMEOUT_SECS ))
SAW_BLOCK=0
PATTERN="added to validated blocks at ([${MIN_HEIGHT}-9]|[1-9][0-9]+)"
while (( SECONDS < DEADLINE )); do
    if grep -E -q "$PATTERN" "$NODE_LOG" 2>/dev/null; then
        SAW_BLOCK=1
        break
    fi
    if ! kill -0 "$NODE_PID" 2>/dev/null; then
        echo "[fail ] node died before producing block $MIN_HEIGHT"
        EXIT_CODE=3
        exit 3
    fi
    if ! kill -0 "$MINER_PID" 2>/dev/null; then
        echo "[fail ] miner died before producing block $MIN_HEIGHT"
        EXIT_CODE=4
        exit 4
    fi
    sleep 2
done

if (( SAW_BLOCK == 1 )); then
    echo "[ok   ] node accepted a mined block at height >= $MIN_HEIGHT"
    echo "[ok   ] full block-accept log:"
    grep -E "added to validated blocks at" "$NODE_LOG" | head -8
    # Sanity check: kernel must NOT have logged the empty-cache panic.
    if grep -q "zk-asert-post-ai-anchor-cache-empty" "$NODE_LOG" 2>/dev/null; then
        echo "[fail ] kernel logged cache-empty panic — cache population is broken"
        EXIT_CODE=6
        exit 6
    fi
    echo "[ok   ] no cache-empty panic in node log — cache populated + read correctly"
    # Stage 6 adversarial gate: kernel must NOT reject any block with
    # %proof-version-invalid. The predicate accepts the height-derived
    # legacy ZK version OR %3 (AI) post-activation. If anything else
    # were accepted by the miner, this catches it.
    if grep -q "proof-version-invalid" "$NODE_LOG" 2>/dev/null; then
        echo "[fail ] kernel rejected block(s) with proof-version-invalid — predicate is wrong"
        grep -E "proof-version-invalid" "$NODE_LOG" | head -5
        EXIT_CODE=7
        exit 7
    fi
    echo "[ok   ] no proof-version-invalid rejections — Stage 6 version-validity predicate intact"
    EXIT_CODE=0
else
    echo "[fail ] timeout waiting for accepted block at height >= $MIN_HEIGHT"
    # Diagnostic: show what blocks did land + any cache-related errors.
    echo "===== blocks accepted ====="
    grep -E "added to validated blocks at" "$NODE_LOG" 2>/dev/null | tail -10 || true
    echo "===== any cache errors ====="
    grep -i "cache\|asert" "$NODE_LOG" 2>/dev/null | tail -10 || true
    EXIT_CODE=5
fi
