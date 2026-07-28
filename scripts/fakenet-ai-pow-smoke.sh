#!/usr/bin/env bash
# scripts/fakenet-ai-pow-smoke.sh
#
# Full AI-PoW production flow on a fakenet, self-contained (no external Pearl
# Gateway). Boots a `nockchain` node with AI-PoW active from a low height and
# the gateway-free `ai-pow-mine --canonical` CPU miner, and shows the node<->miner
# communication end to end:
#
#   node  --%mine-ai (WatchEffects gRPC stream)-->  ai-pow-mine
#   node  <--%pow %ai-pow (unary poke gRPC)-------  ai-pow-mine
#
#   * the node emits a %mine-ai candidate (the AI-PoW work effect) post-activation;
#   * the miner subscribes, proves a CANONICAL AI-PoW block bound to the
#     candidate's block commitment on the CPU (~30s), and pokes %pow %ai-pow;
#   * the node's do-pow verifies the certificate against the boot-generated
#     verifier setup (the mandatory ai-pow-verify jet) and admits the block.
#
# NOTE ON TIMING: the canonical CPU prove is ~30s, far slower than the default
# fakenet candidate refresh. We set --fakenet-update-candidate-interval-secs
# LARGER than the prove time so the candidate commitment stays stable long enough
# for the proved certificate to still bind when it lands.
#
# NOTE ON FIRST BOOT: the node GENERATES the AI-PoW verifier-setup table on
# first use and caches it under data-dir/ai-pow. Each smoke run uses fresh node
# state and copies only that setup cache in/out, so stale PMA cannot affect boot.

set -euo pipefail

PRIV_PORT="${PRIV_PORT:-25557}"
FAKENET_POW_LEN="${FAKENET_POW_LEN:-2}"
FAKENET_LOG_DIFF="${FAKENET_LOG_DIFF:-1}"
# Activate AI-PoW at height 1: genesis (height 0) is auto-injected on fakenet, and
# the node then emits %mine-ai from height 1 — so the AI miner alone produces the
# whole chain (no ZK miner needed for pre-activation heights). Heights below
# zk-asert.phase are pre-asert (AI blocks use the epoch target, like the passing
# ai_pow_accept_e2e test); the AI ASERT retarget path is unit-tested separately.
FAKENET_AI_ACTIVATION="${FAKENET_AI_ACTIVATION:-1}"
# Candidate refresh interval: must exceed the ~30s canonical prove time.
CANDIDATE_INTERVAL="${CANDIDATE_INTERVAL:-120}"
# Generous timeout: covers first-boot verifier-setup generation + one prove.
BOOT_TIMEOUT_SECS="${BOOT_TIMEOUT_SECS:-1200}"
MINE_TIMEOUT_SECS="${MINE_TIMEOUT_SECS:-600}"
MIN_HEIGHT="${MIN_HEIGHT:-1}"
MINING_PKH="${MINING_PKH:-9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV}"

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# Persist only verifier setup; node PMA/event state is per-run.
SETUP_CACHE_DIR="${AI_POW_VERIFIER_SETUP_CACHE_DIR:-$REPO_ROOT/.fakenet-ai-pow-data/ai-pow}"

echo "== fakenet-ai-pow-smoke =="
echo "  PRIV_PORT             = $PRIV_PORT"
echo "  FAKENET_AI_ACTIVATION = $FAKENET_AI_ACTIVATION"
echo "  CANDIDATE_INTERVAL    = ${CANDIDATE_INTERVAL}s (must exceed the ~30s prove)"
echo "  SETUP_CACHE_DIR       = $SETUP_CACHE_DIR"
echo "  MIN_HEIGHT            = $MIN_HEIGHT"
echo "  MINING_PKH            = $MINING_PKH"

echo
echo "[build] nockchain + ai-pow-mine ..."
cargo build --release -p nockchain --bin nockchain
cargo build --release -p ai-pow-miner --features node --bin ai-pow-mine

WORK_DIR="$(mktemp -d -t fakenet-ai-pow-smoke.XXXXXX)"
NODE_LOG="$WORK_DIR/node.log"
MINER_LOG="$WORK_DIR/miner.log"
echo "[setup] work_dir=$WORK_DIR  logs: node.log, miner.log"
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
    [[ -n "$MINER_PID" ]] && { kill "$MINER_PID" 2>/dev/null; wait "$MINER_PID" 2>/dev/null; }
    [[ -n "$NODE_PID" ]]  && { kill "$NODE_PID"  2>/dev/null; wait "$NODE_PID"  2>/dev/null; }
    if [[ -d "$DATA_DIR/ai-pow" ]]; then
        mkdir -p "$SETUP_CACHE_DIR"
        cp -R "$DATA_DIR/ai-pow/." "$SETUP_CACHE_DIR/"
    fi
    echo "[cleanup] logs preserved at $WORK_DIR"
    if [[ "$EXIT_CODE" -ne 0 ]]; then
        echo; echo "===== node.log (tail) ====="; tail -80 "$NODE_LOG" 2>/dev/null || true
        echo; echo "===== miner.log (tail) ====="; tail -60 "$MINER_LOG" 2>/dev/null || true
    fi
    exit "$EXIT_CODE"
}
trap cleanup EXIT INT TERM

NODE_BIN="$REPO_ROOT/target/release/nockchain"
MINER_BIN="$REPO_ROOT/target/release/ai-pow-mine"

echo
echo "[boot ] starting node (AI activation=$FAKENET_AI_ACTIVATION); first boot GENERATES the"
echo "        verifier-setup table (multi-minute one-time stall) unless already cached ..."
pushd "$WORK_DIR" >/dev/null
RUST_LOG="${NODE_RUST_LOG:-info}" \
    "$NODE_BIN" \
    --fakenet \
    --data-dir "$DATA_DIR" \
    --bind-private-grpc-port "$PRIV_PORT" \
    --fakenet-pow-len "$FAKENET_POW_LEN" \
    --fakenet-log-difficulty "$FAKENET_LOG_DIFF" \
    --fakenet-ai-pow-activation-height "$FAKENET_AI_ACTIVATION" \
    --fakenet-update-candidate-interval-secs "$CANDIDATE_INTERVAL" \
    --no-default-peers \
    --bind /ip4/127.0.0.1/udp/0/quic-v1 \
    >"$NODE_LOG" 2>&1 &
NODE_PID=$!
popd >/dev/null
echo "[boot ] node pid=$NODE_PID; waiting for %born (up to ${BOOT_TIMEOUT_SECS}s for setup gen) ..."

DEADLINE=$(( SECONDS + BOOT_TIMEOUT_SECS ))
while (( SECONDS < DEADLINE )); do
    if grep -aq "handle-command: born" "$NODE_LOG" 2>/dev/null; then
        echo "[boot ] node reached %born (verifier setup ready)"
        break
    fi
    if grep -aqi "badly formatted cause" "$NODE_LOG" 2>/dev/null; then
        echo "[fail ] node rejected the fakenet %set-constants poke ('badly formatted cause')"
        EXIT_CODE=8; exit 8
    fi
    if ! kill -0 "$NODE_PID" 2>/dev/null; then
        echo "[fail ] node died before %born"; EXIT_CODE=2; exit 2
    fi
    sleep 3
done
if ! grep -aq "handle-command: born" "$NODE_LOG" 2>/dev/null; then
    echo "[fail ] timeout waiting for %born"; EXIT_CODE=2; exit 2
fi

sleep 2
echo
echo "[boot ] starting ai-pow-mine --canonical (gateway-free CPU miner) ..."
RUST_LOG="${MINER_RUST_LOG:-info}" \
    "$MINER_BIN" \
    --node-addr "http://127.0.0.1:$PRIV_PORT" \
    --mining-pkh "$MINING_PKH" \
    --canonical \
    >"$MINER_LOG" 2>&1 &
MINER_PID=$!
echo "[boot ] miner pid=$MINER_PID"

echo
echo "[wait ] polling for an accepted AI block h>=$MIN_HEIGHT (timeout ${MINE_TIMEOUT_SECS}s) ..."
DEADLINE=$(( SECONDS + MINE_TIMEOUT_SECS ))
SAW_BLOCK=0
PATTERN="added to validated blocks at ([${MIN_HEIGHT}-9]|[1-9][0-9]+)"
while (( SECONDS < DEADLINE )); do
    if grep -aE -q "$PATTERN" "$NODE_LOG" 2>/dev/null; then SAW_BLOCK=1; break; fi
    if ! kill -0 "$NODE_PID" 2>/dev/null; then echo "[fail ] node died"; EXIT_CODE=3; exit 3; fi
    if ! kill -0 "$MINER_PID" 2>/dev/null; then echo "[fail ] miner died"; EXIT_CODE=4; exit 4; fi
    sleep 3
done

echo
echo "===== node<->miner communication (observed) ====="
echo "--- node: %mine-ai emission + block acceptance ---"
grep -aE "mine-ai|added to validated blocks at|do-pow|Generating the AI-PoW|verifier" "$NODE_LOG" 2>/dev/null | tail -20 || true
echo "--- miner: subscribe -> prove -> submit ---"
grep -aE "subscribed|candidate|proving canonical|proved|submission|acked" "$MINER_LOG" 2>/dev/null | tail -20 || true

if (( SAW_BLOCK == 1 )); then
    echo
    echo "[ok   ] node ACCEPTED a mined AI-PoW block at height >= $MIN_HEIGHT"
    grep -aE "added to validated blocks at" "$NODE_LOG" | head -8
    if grep -aq "proof-version-invalid\|failed-pow-check\|page-target-invalid\|page-heaviness-invalid" "$NODE_LOG" 2>/dev/null; then
        echo "[warn ] node also logged a rejection reason (inspect node.log)"
    fi
    EXIT_CODE=0
else
    echo
    echo "[fail ] timeout waiting for an accepted AI block at height >= $MIN_HEIGHT"
    EXIT_CODE=5
fi
