#!/usr/bin/env bash
# scripts/fakenet-dual-pow-smoke.sh
#
# Dual-puzzle fakenet smoke: ONE nockchain node with BOTH a ZK-PoW miner and an
# AI-PoW miner attached simultaneously, proving the two puzzles' end-to-end flow
# runs in tandem (node emits %mine + %mine-ai; each miner subscribes to its own
# effect; both submit %pow pokes the node validates against its own puzzle path).
#
#   node --%mine     (WatchEffects gRPC)--> zk-pow-mine  --%pow %dumb-zkpow-->  node
#   node --%mine-ai  (WatchEffects gRPC)--> ai-pow-mine  --%pow %ai-pow------>  node
#
# Both miners set the SAME mining pkh (idempotent — no key conflict) and watch
# DIFFERENT effect labels ("mine" vs "mine-ai"), so they coexist cleanly.
#
# PREREQUISITE — miner.jam must be current: zk-pow-mine embeds assets/miner.jam
# (kernels-open-miner::KERNEL). A stale miner.jam emits an old %pow the dual-puzzle
# node rejects as "badly formatted cause", so ZK blocks silently never land. If in
# doubt: `HOONC=target/release/hoonc make assets/miner.jam && cargo build --release
# -p zk-pow-miner --bin zk-pow-mine`.
#
# RACING DYNAMIC + a known height-1 STALL (empirically observed on this laptop):
# the canonical AI prove has a ~27s hard floor (grind + certificate). Equal-weight
# heaviness makes it a first-to-tip race:
#   * ZK faster than ~27s -> ZK wins every height; AI proofs arrive stale and the
#     node rejects them ("%ai-pow certificate failed verification").
#   * ZK slower than ~27s -> the chain tends to STALL after height 1 (the ZK worker
#     is superseded by candidate refresh before completing its long search, and the
#     AI's post-bootstrap ASERT target hardens out of reach). A balanced ~50/50 live
#     chain is NOT achievable by difficulty alone here — it needs a faster AI prove
#     or decoupling candidate regeneration from the slow AI cycle. See the
#     fakenet-dual-miner-tuning memory. The retargeting MATH is validated separately
#     by the tandem unit tests (roswell test-dumb: test-tandem-asert-*).
#
# Laptop-normalized difficulty (see the memory): the ZK ASERT anchor target (via
# --fakenet-asert-* below) governs ZK block time; the AI first block is always
# bootstrap-trivial (no AI ancestor) and prove-bound (~27s). Use a huge
# --fakenet-asert-half-life so the ~2^63 wall-clock timestamps are absorbed
# (constant difficulty), OR regenerate the fakenet genesis with a current timestamp
# for natural retargeting.
#
# NOTE ON TIMING / FIRST BOOT: identical to fakenet-ai-pow-smoke.sh — the node
# GENERATES the ~6GB AI-PoW verifier-setup table on first boot (one-time, cached
# under the data dir); the AI candidate refresh interval must exceed the ~30s prove.

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

# Persistent data dir so the verifier-setup table is generated once and reused.
DATA_DIR="${AI_POW_FAKENET_DATA_DIR:-$REPO_ROOT/.fakenet-ai-pow-data}"

echo "== fakenet-dual-pow-smoke =="
echo "  PRIV_PORT             = $PRIV_PORT"
echo "  FAKENET_AI_ACTIVATION = $FAKENET_AI_ACTIVATION"
echo "  CANDIDATE_INTERVAL    = ${CANDIDATE_INTERVAL}s (must exceed the ~30s AI prove)"
echo "  NUM_THREADS (ZK)      = $NUM_THREADS"
echo "  ZK_ASERT handicap     = $ZK_ASERT (bex=$ZK_ANCHOR_TARGET_BEX, half-life=${ZK_HALF_LIFE}s)"
echo "  DATA_DIR              = $DATA_DIR"

echo
echo "[build] nockchain + zk-pow-mine + ai-pow-mine ..."
cargo build --release -p nockchain --bin nockchain
cargo build --release -p zk-pow-miner --bin zk-pow-mine
cargo build --release -p ai-pow-miner --features node --bin ai-pow-mine

WORK_DIR="$(mktemp -d -t fakenet-dual-pow-smoke.XXXXXX)"
NODE_LOG="$WORK_DIR/node.log"
ZK_LOG="$WORK_DIR/zk-miner.log"
AI_LOG="$WORK_DIR/ai-miner.log"
echo "[setup] work_dir=$WORK_DIR  logs: node.log, zk-miner.log, ai-miner.log"
mkdir -p "$DATA_DIR"

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
    grep -q "born" "$NODE_LOG" 2>/dev/null && { echo "[boot ] node reached %born (verifier setup ready)"; break; }
    if grep -qi "badly formatted cause" "$NODE_LOG" 2>/dev/null; then
        echo "[fail ] node rejected the fakenet %set-constants poke"; EXIT_CODE=8; exit 8; fi
    kill -0 "$NODE_PID" 2>/dev/null || { echo "[fail ] node died before %born"; EXIT_CODE=2; exit 2; }
    sleep 3
done
grep -q "born" "$NODE_LOG" 2>/dev/null || { echo "[fail ] timeout waiting for %born"; EXIT_CODE=2; exit 2; }

sleep 2
echo
echo "[boot ] starting zk-pow-mine (num-threads=$NUM_THREADS) ..."
RUST_LOG="${ZK_RUST_LOG:-info}" "$ZK_BIN" \
    --node-addr "http://127.0.0.1:$PRIV_PORT" --mining-pkh "$MINING_PKH" \
    --num-threads "$NUM_THREADS" >"$ZK_LOG" 2>&1 &
ZK_PID=$!
echo "[boot ] zk-pow-mine pid=$ZK_PID"

echo "[boot ] starting ai-pow-mine --canonical ..."
RUST_LOG="${AI_RUST_LOG:-info}" "$AI_BIN" \
    --node-addr "http://127.0.0.1:$PRIV_PORT" --mining-pkh "$MINING_PKH" \
    --canonical >"$AI_LOG" 2>&1 &
AI_PID=$!
echo "[boot ] ai-pow-mine pid=$AI_PID"

echo
echo "[wait ] polling for accepted blocks from BOTH puzzles (timeout ${MINE_TIMEOUT_SECS}s) ..."
ZK_PAT='added to validated blocks at .* with proof version'
AI_PAT='added to validated blocks at .* with ai-pow certificate'
DEADLINE=$(( SECONDS + MINE_TIMEOUT_SECS ))
while (( SECONDS < DEADLINE )); do
    ZK_N=$(grep -Ec "$ZK_PAT" "$NODE_LOG" 2>/dev/null || echo 0)
    AI_N=$(grep -Ec "$AI_PAT" "$NODE_LOG" 2>/dev/null || echo 0)
    (( ZK_N >= 1 && AI_N >= 1 )) && break
    kill -0 "$NODE_PID" 2>/dev/null || { echo "[fail ] node died"; EXIT_CODE=3; exit 3; }
    kill -0 "$ZK_PID"   2>/dev/null || { echo "[warn ] zk-pow-mine died"; }
    kill -0 "$AI_PID"   2>/dev/null || { echo "[warn ] ai-pow-mine died"; }
    sleep 3
done

ZK_N=$(grep -Ec "$ZK_PAT" "$NODE_LOG" 2>/dev/null || echo 0)
AI_N=$(grep -Ec "$AI_PAT" "$NODE_LOG" 2>/dev/null || echo 0)
echo
echo "===== result ====="
echo "  ZK blocks accepted : $ZK_N"
echo "  AI blocks accepted : $AI_N"
echo "--- node: recent acceptances ---"
grep -E "added to validated blocks at" "$NODE_LOG" 2>/dev/null | tail -12 || true

if (( ZK_N >= 1 && AI_N >= 1 )); then
    echo "[ok   ] BOTH puzzles produced accepted blocks with both miners running simultaneously"
    EXIT_CODE=0
elif (( ZK_N >= 1 || AI_N >= 1 )); then
    echo "[partial] both miners ran + coexisted, but only one puzzle landed blocks in the window."
    echo "          (Expected on trivial ZK difficulty — raise it with ZK_ASERT=1 to let AI win races.)"
    EXIT_CODE=6
else
    echo "[fail ] no blocks accepted from either puzzle within the window"
    EXIT_CODE=5
fi
