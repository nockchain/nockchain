#!/usr/bin/env bash
# scripts/fakenet-zk-pow-smoke.sh
#
# End-to-end fakenet smoke for the zk-pow-miner architecture:
#   1. Boot a fakenet `nockchain` node (no in-process miner; gRPC private port).
#   2. Boot a separate `zk-pow-mine` binary that connects to it.
#   3. Wait for the node to log "added to validated blocks at <h>" for h ≥ 1,
#      which proves the miner found a block AND the node accepted it.
#
# KNOWN RESIDUAL (2026-05-26): this smoke currently fails to produce a block
# because of a pre-existing bug in `nockchain` that lives outside the
# zk-pow-miner scope. The fakenet `set-constants` poke (built in
# `crates/nockchain/src/setup.rs::poke::PokeFakenetConstants` from a
# `nockchain_types::BlockchainConstants` noun) is silently rejected by the
# kernel with "Error: badly formatted cause, should never occur." The kernel
# therefore keeps the mainnet defaults (pow-len=64) instead of the fakenet
# values (pow-len=2). The bundled fakenet genesis jam
# (`jams/fakenet-genesis-pow-2-bex-1.jam`) was built for pow-len=2, so when
# the node tries to validate it, `check-pow-valid` / `check-target` /
# `check-work` all fail and the kernel emits
# `liar-effect: ATTN: received a bad genesis block`. No heaviest-block ⇒
# no `%mine` effect emission ⇒ the miner correctly stays idle.
#
# Evidence that the *miner* itself works end-to-end up to that point:
#   miner.log:
#     "zk-pow-mine: starting node=... threads=1"
#     "zk-pow-miner: pool ready; entering main loop"
#     "zk-pow-miner: subscribed + mining enabled; awaiting candidates"
#   node.log:
#     "handle-command: set-mining-key-advanced"  ← miner-side poke landed
#     "handle-command: enable-mining"            ← miner-side poke landed
#
# This smoke flips green automatically once the BlockchainConstants noun
# encoding (or the Hoon-side blockchain-constants:v1 schema soft) is
# repaired. Until then it is a useful diagnostic — it correctly identifies
# the chain bring-up bug and preserves both logs at `WORK_DIR` for triage.
#
# Tunables via env vars:
#   PRIV_PORT      — node private gRPC port               (default: 25555)
#   FAKENET_POW_LEN — fakenet pow-len                     (default: 2)
#   FAKENET_LOG_DIFF — log target difficulty (2^N)        (default: 1)
#   NUM_THREADS    — miner pool size                      (default: 1)
#   TIMEOUT_SECS   — overall wait                         (default: 180)
#   MINING_PKH     — payout pkh (defaults to a valid stub)

set -euo pipefail

PRIV_PORT="${PRIV_PORT:-25555}"
FAKENET_POW_LEN="${FAKENET_POW_LEN:-2}"
FAKENET_LOG_DIFF="${FAKENET_LOG_DIFF:-1}"
NUM_THREADS="${NUM_THREADS:-1}"
TIMEOUT_SECS="${TIMEOUT_SECS:-180}"
MINING_PKH="${MINING_PKH:-9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV}"

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

echo "== fakenet-zk-pow-smoke =="
echo "  PRIV_PORT       = $PRIV_PORT"
echo "  FAKENET_POW_LEN = $FAKENET_POW_LEN"
echo "  FAKENET_LOG_DIFF= $FAKENET_LOG_DIFF"
echo "  NUM_THREADS     = $NUM_THREADS"
echo "  TIMEOUT_SECS    = $TIMEOUT_SECS"
echo "  MINING_PKH      = $MINING_PKH"

echo
echo "[build] nockchain + zk-pow-mine ..."
cargo build --release -p nockchain --bin nockchain
cargo build --release -p zk-pow-miner --bin zk-pow-mine

WORK_DIR="$(mktemp -d -t fakenet-zk-pow-smoke.XXXXXX)"
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

# Run the node in $WORK_DIR so its .nockchain_identity etc. don't pollute the repo.
NODE_BIN="$REPO_ROOT/target/release/nockchain"
MINER_BIN="$REPO_ROOT/target/release/zk-pow-mine"

echo
echo "[boot ] starting node ..."
pushd "$WORK_DIR" >/dev/null
RUST_LOG="${NODE_RUST_LOG:-info}" \
    "$NODE_BIN" \
    --fakenet \
    --bind-private-grpc-port "$PRIV_PORT" \
    --fakenet-pow-len "$FAKENET_POW_LEN" \
    --fakenet-log-difficulty "$FAKENET_LOG_DIFF" \
    --no-default-peers \
    --bind /ip4/127.0.0.1/udp/0/quic-v1 \
    >"$NODE_LOG" 2>&1 &
NODE_PID=$!
popd >/dev/null
echo "[boot ] node pid=$NODE_PID; waiting for born..."

# Wait for the node to print %born so we know the kernel is past init.
DEADLINE=$(( SECONDS + 60 ))
while (( SECONDS < DEADLINE )); do
    if grep -q "born" "$NODE_LOG" 2>/dev/null || grep -q "born poke sent" "$NODE_LOG" 2>/dev/null; then
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
