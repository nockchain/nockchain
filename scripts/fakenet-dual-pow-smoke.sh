#!/usr/bin/env bash
# scripts/fakenet-dual-pow-smoke.sh
#
# Dual-puzzle fakenet smoke: one node accepts an AI block, a ZK block extending
# it, then an AI block extending the interleaved ZK tip. Running one miner at a
# time avoids turning the test into a nondeterministic stale-proof race while
# still exercising both production binaries, effect streams, proof paths,
# branch-local puzzle lineage, and shared fork choice end to end.
#
#   node --%mine-ai  --> ai-pow-mine --%pow %ai-pow------> node
#   node --%mine     --> zk-pow-mine --%pow %dumb-zkpow--> node
#
# Success requires exact consecutive acceptance heights (AI -> ZK -> AI) and no
# dangling jet-hint registration diagnostics.
#
# `make assets/miner.jam` must precede rebuilding `zk-pow-mine` after miner Hoon
# changes. This script rebuilds the Rust binaries but does not rebuild jams.
#
# Exact independent-ASERT direction and interleaving invariants are covered by
# `scripts/test-dual-pow-retarget.sh`; this live smoke prints each accepted tip's
# target as additional end-to-end evidence.
#
# NOTE ON TIMING / FIRST BOOT: verifier setup generation is a one-time,
# multi-minute operation. The script reuses only the proof-independent setup
# cache; each run gets fresh consensus state so an old PMA cannot mask or block
# a kernel migration.

set -euo pipefail

PRIV_PORT="${PRIV_PORT:-25559}"
FAKENET_POW_LEN="${FAKENET_POW_LEN:-2}"
FAKENET_LOG_DIFF="${FAKENET_LOG_DIFF:-1}"
FAKENET_AI_ACTIVATION="${FAKENET_AI_ACTIVATION:-1}"
CANDIDATE_INTERVAL="${CANDIDATE_INTERVAL:-120}"
NUM_THREADS="${NUM_THREADS:-1}"
BOOT_TIMEOUT_SECS="${BOOT_TIMEOUT_SECS:-1200}"
MINE_TIMEOUT_SECS="${MINE_TIMEOUT_SECS:-600}"
MINING_PKH="${MINING_PKH:-9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV}"

# Optional ZK-difficulty handicap so the AI miner can win some races. When ZK_ASERT
# is set, apply --fakenet-asert-* (phase 2 / anchor 1 / anchor-target-bex chosen by
# ZK_ANCHOR_TARGET_BEX) + an optional short half-life.
ZK_ASERT="${ZK_ASERT:-0}"
ZK_ANCHOR_TARGET_BEX="${ZK_ANCHOR_TARGET_BEX:-250}"
ZK_HALF_LIFE="${ZK_HALF_LIFE:-600}"

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# Persist only the verifier setup. Consensus PMA/event state is per-run.
SETUP_CACHE_DIR="${AI_POW_VERIFIER_SETUP_CACHE_DIR:-$REPO_ROOT/.fakenet-ai-pow-data/ai-pow}"

echo "== fakenet-dual-pow-smoke =="
echo "  PRIV_PORT             = $PRIV_PORT"
echo "  FAKENET_AI_ACTIVATION = $FAKENET_AI_ACTIVATION"
echo "  CANDIDATE_INTERVAL    = ${CANDIDATE_INTERVAL}s (must exceed the ~30s AI prove)"
echo "  NUM_THREADS (ZK)      = $NUM_THREADS"
echo "  ZK_ASERT handicap     = $ZK_ASERT (bex=$ZK_ANCHOR_TARGET_BEX, half-life=${ZK_HALF_LIFE}s)"
echo "  SETUP_CACHE_DIR       = $SETUP_CACHE_DIR"

echo
echo "[build] nockchain + zk-pow-mine + ai-pow-mine ..."
cargo build --release \
    -p nockchain --bin nockchain \
    -p zk-pow-miner --bin zk-pow-mine \
    -p ai-pow-miner --features ai-pow-miner/node --bin ai-pow-mine

WORK_DIR="$(mktemp -d -t fakenet-dual-pow-smoke.XXXXXX)"
NODE_LOG="$WORK_DIR/node.log"
ZK_LOG="$WORK_DIR/zk-miner.log"
AI_LOG="$WORK_DIR/ai-miner.log"
echo "[setup] work_dir=$WORK_DIR  logs: node.log, zk-miner.log, ai-miner.log"
DATA_DIR="$WORK_DIR/node-data"
mkdir -p "$DATA_DIR" "$SETUP_CACHE_DIR"
ln -s "$SETUP_CACHE_DIR" "$DATA_DIR/ai-pow"

NODE_PID=""; ZK_PID=""; AI_PID=""
EXIT_CODE=99

cleanup() {
    local rc=$?
    set +e
    echo
    echo "[cleanup] tearing down (rc=$rc)"
    [[ -n "$ZK_PID" ]]   && { kill "$ZK_PID"   2>/dev/null; wait "$ZK_PID"   2>/dev/null; }
    [[ -n "$AI_PID" ]]   && { kill "$AI_PID"   2>/dev/null; wait "$AI_PID"   2>/dev/null; }
    [[ -n "$NODE_PID" ]] && { kill "$NODE_PID" 2>/dev/null; wait "$NODE_PID" 2>/dev/null; }
    echo "[cleanup] logs preserved at $WORK_DIR"
    if [[ "$EXIT_CODE" -ne 0 ]]; then
        echo; echo "===== node.log (tail) ====="; tail -60 "$NODE_LOG" 2>/dev/null || true
        echo; echo "===== zk-miner.log (tail) ====="; tail -30 "$ZK_LOG" 2>/dev/null || true
        echo; echo "===== ai-miner.log (tail) ====="; tail -30 "$AI_LOG" 2>/dev/null || true
    fi
    exit "$EXIT_CODE"
}
trap cleanup EXIT INT TERM

NODE_BIN="$REPO_ROOT/target/release/nockchain"
ZK_BIN="$REPO_ROOT/target/release/zk-pow-mine"
AI_BIN="$REPO_ROOT/target/release/ai-pow-mine"

NODE_ARGS=(
    --fakenet
    --data-dir "$DATA_DIR"
    --bind-private-grpc-port "$PRIV_PORT"
    --fakenet-pow-len "$FAKENET_POW_LEN"
    --fakenet-log-difficulty "$FAKENET_LOG_DIFF"
    --fakenet-ai-pow-activation-height "$FAKENET_AI_ACTIVATION"
    --fakenet-update-candidate-interval-secs "$CANDIDATE_INTERVAL"
    --no-default-peers
    --bind /ip4/127.0.0.1/udp/0/quic-v1
)
if [[ "$ZK_ASERT" != "0" ]]; then
    NODE_ARGS+=(
        --fakenet-asert-phase 2
        --fakenet-asert-anchor-height 1
        --fakenet-asert-anchor-target-bex "$ZK_ANCHOR_TARGET_BEX"
        --fakenet-asert-half-life "$ZK_HALF_LIFE"
    )
fi

echo
echo "[boot ] starting node (AI activation=$FAKENET_AI_ACTIVATION); first boot GENERATES the"
echo "        verifier-setup table (multi-minute one-time stall) unless already cached ..."
pushd "$WORK_DIR" >/dev/null
RUST_LOG="${NODE_RUST_LOG:-info}" "$NODE_BIN" "${NODE_ARGS[@]}" >"$NODE_LOG" 2>&1 &
NODE_PID=$!
popd >/dev/null
echo "[boot ] node pid=$NODE_PID; waiting for %born (up to ${BOOT_TIMEOUT_SECS}s for setup gen) ..."

DEADLINE=$(( SECONDS + BOOT_TIMEOUT_SECS ))
while (( SECONDS < DEADLINE )); do
    grep -aq "born" "$NODE_LOG" 2>/dev/null && { echo "[boot ] node reached %born (verifier setup ready)"; break; }
    if grep -aqi "badly formatted cause" "$NODE_LOG" 2>/dev/null; then
        echo "[fail ] node rejected the fakenet %set-constants poke"; EXIT_CODE=8; exit 8; fi
    kill -0 "$NODE_PID" 2>/dev/null || { echo "[fail ] node died before %born"; EXIT_CODE=2; exit 2; }
    sleep 3
done
grep -aq "born" "$NODE_LOG" 2>/dev/null || { echo "[fail ] timeout waiting for %born"; EXIT_CODE=2; exit 2; }

sleep 2
ZK_PAT='added to validated blocks at [1-9][0-9]* with proof version'
AI_PAT='added to validated blocks at [1-9][0-9]* with ai-pow certificate'

count_acceptances() {
    grep -aEc "$1" "$NODE_LOG" 2>/dev/null || true
}

wait_for_acceptances() {
    local pattern=$1
    local wanted=$2
    local label=$3
    local deadline=$(( SECONDS + MINE_TIMEOUT_SECS ))
    local count
    while (( SECONDS < deadline )); do
        count=$(count_acceptances "$pattern")
        (( count >= wanted )) && return 0
        kill -0 "$NODE_PID" 2>/dev/null || return 1
        sleep 0.1
    done
    echo "[fail ] timeout waiting for $label"
    return 1
}

acceptance_height() {
    local pattern=$1
    local occurrence=$2
    grep -aE "$pattern" "$NODE_LOG" 2>/dev/null \
        | sed -E 's/.* at ([0-9]+) with.*/\1/' \
        | sed -n "${occurrence}p"
}

stop_miner() {
    local pid=$1
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
}

echo
echo "[phase] mine AI block 1 ..."
RUST_LOG="${AI_RUST_LOG:-info}" "$AI_BIN" \
    --node-addr "http://127.0.0.1:$PRIV_PORT" --mining-pkh "$MINING_PKH" \
    --canonical >"$AI_LOG" 2>&1 &
AI_PID=$!
wait_for_acceptances "$AI_PAT" 1 "first AI block" || { EXIT_CODE=5; exit 5; }
stop_miner "$AI_PID"
AI_PID=""

echo "[phase] mine a ZK block extending the AI tip ..."
RUST_LOG="${ZK_RUST_LOG:-info}" "$ZK_BIN" \
    --node-addr "http://127.0.0.1:$PRIV_PORT" --mining-pkh "$MINING_PKH" \
    --num-threads "$NUM_THREADS" >"$ZK_LOG" 2>&1 &
ZK_PID=$!
wait_for_acceptances "$ZK_PAT" 1 "ZK block after AI" || { EXIT_CODE=6; exit 6; }
stop_miner "$ZK_PID"
ZK_PID=""

echo "[phase] mine AI block 2 across the interleaved ZK block ..."
RUST_LOG="${AI_RUST_LOG:-info}" "$AI_BIN" \
    --node-addr "http://127.0.0.1:$PRIV_PORT" --mining-pkh "$MINING_PKH" \
    --canonical >>"$AI_LOG" 2>&1 &
AI_PID=$!
wait_for_acceptances "$AI_PAT" 2 "second AI block" || { EXIT_CODE=7; exit 7; }
stop_miner "$AI_PID"
AI_PID=""

AI_1_HEIGHT=$(acceptance_height "$AI_PAT" 1)
ZK_HEIGHT=$(acceptance_height "$ZK_PAT" 1)
AI_2_HEIGHT=$(acceptance_height "$AI_PAT" 2)
ZK_N=$(count_acceptances "$ZK_PAT")
AI_N=$(count_acceptances "$AI_PAT")

echo
echo "===== result ====="
echo "  accepted sequence    : AI@$AI_1_HEIGHT -> ZK@$ZK_HEIGHT -> AI@$AI_2_HEIGHT"
echo "  ZK blocks accepted   : $ZK_N"
echo "  AI blocks accepted   : $AI_N"
echo "--- node: recent heaviest-chain targets ---"
grep -aE "new_heaviest_chain block_height=" "$NODE_LOG" 2>/dev/null | tail -6 || true

if (( ZK_HEIGHT != AI_1_HEIGHT + 1 || AI_2_HEIGHT != ZK_HEIGHT + 1 )); then
    echo "[fail ] puzzle blocks did not extend one linear consensus chain"
    EXIT_CODE=9
elif grep -aq "could not match parent battery at given axis" "$NODE_LOG"; then
    echo "[fail ] dangling jet-hint ancestry detected"
    EXIT_CODE=10
else
    echo "[ok   ] AI -> ZK -> AI blocks extended one linear heaviest chain"
    EXIT_CODE=0
fi
