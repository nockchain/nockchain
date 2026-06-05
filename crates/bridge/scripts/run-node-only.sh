#!/bin/bash
set -e

# Run just a nockchain node (useful for testing bridge connections separately)
# Usage: ./run-node-only.sh [--clean] [--new] [--v1-phase N] [--pow-len N] [--log-difficulty N] [--genesis-jam-path PATH]

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/layout.sh
source "$SCRIPT_DIR/lib/layout.sh"
bridge_resolve_layout

BIN_DIR="$BRIDGE_BIN_DIR"
TEST_DATA_DIR="${BRIDGE_DIR}/test_run_data"
NODE_DIR="${TEST_DATA_DIR}/node"
WALLET_DIR="${TEST_DATA_DIR}/wallet"

GENESIS_JAM="${GENESIS_JAM_PATH:-${BRIDGE_SOURCE_ROOT}/crates/nockchain/jams/fakenet-genesis-pow-2-bex-1.jam}"

NODE_BIND="/ip4/0.0.0.0/udp/3005/quic-v1"
NODE_PUBLIC_GRPC="127.0.0.1:5001"
NODE_PRIVATE_GRPC_PORT="5002"

CLEAN_FLAG="false"
NEW_FLAG="false"
V1_PHASE="${V1_PHASE:-20}"
POW_LEN=""
LOG_DIFFICULTY=""
GENESIS_JAM_PATH_OVERRIDE=""

cleanup() {
    echo "Cleaning up..."
    [ -n "$NODE_PID" ] && kill $NODE_PID 2>/dev/null || true
    wait 2>/dev/null || true
    echo "Done."
}

trap cleanup EXIT INT TERM

while [[ $# -gt 0 ]]; do
    case "$1" in
        --clean)
            CLEAN_FLAG="true"
            shift
            ;;
        --new)
            NEW_FLAG="true"
            shift
            ;;
        --v1-phase)
            V1_PHASE="$2"
            shift 2
            ;;
        --v1-phase=*)
            V1_PHASE="${1#*=}"
            shift
            ;;
        --pow-len)
            POW_LEN="$2"
            shift 2
            ;;
        --log-difficulty)
            LOG_DIFFICULTY="$2"
            shift 2
            ;;
        --genesis-jam-path)
            GENESIS_JAM_PATH_OVERRIDE="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: ./run-node-only.sh [--clean] [--new] [--v1-phase N] [--pow-len N] [--log-difficulty N] [--genesis-jam-path PATH]"
            exit 1
            ;;
    esac
done

if [ -n "$GENESIS_JAM_PATH_OVERRIDE" ]; then
    GENESIS_JAM="$GENESIS_JAM_PATH_OVERRIDE"
fi

if [ "$CLEAN_FLAG" = "true" ]; then
    rm -rf "$TEST_DATA_DIR"
elif [ "$NEW_FLAG" = "true" ]; then
    rm -rf "$NODE_DIR"
fi

mkdir -p "$NODE_DIR" "$WALLET_DIR"

if [ ! -f "$BIN_DIR/nockchain" ]; then
    echo "Error: nockchain not found. Run: cargo build --release -p nockchain"
    exit 1
fi
if [ ! -f "$GENESIS_JAM" ]; then
    echo "Error: genesis jam not found at $GENESIS_JAM"
    echo "Set GENESIS_JAM_PATH to override the default."
    exit 1
fi

# Get mining address
FAKENET_SEED="route run sing warrior light swamp clog flower agent ugly wasp fresh tube snow motion salt salon village raccoon chair demise neutral school confirm"
NOCK_DATA_DIR="$WALLET_DIR" "$BIN_DIR/nockchain-wallet" --fakenet import-keys \
    --seedphrase "$FAKENET_SEED" --version 1 2>/dev/null || true

MINING_ADDR=$( NOCK_DATA_DIR="$WALLET_DIR" "$BIN_DIR/nockchain-wallet"  --fakenet list-active-addresses 2>/dev/null | LC_ALL=C sed -n 's/.*- Address: //p' | head -1 )
[ -z "$MINING_ADDR" ] && MINING_ADDR="placeholder"

echo "Mining address: $MINING_ADDR"

echo "Starting nockchain node..."
cd "$NODE_DIR"

echo "Running command:"
echo "$BIN_DIR/nockchain \\"
echo "    --fakenet \\"
echo "    --fakenet-v1-phase $V1_PHASE \\"
echo "    --fakenet-genesis-jam-path $GENESIS_JAM \\"
if [ "$NEW_FLAG" = "true" ]; then
    echo "    --new \\"
fi
if [ -n "$POW_LEN" ]; then
    echo "    --fakenet-pow-len $POW_LEN \\"
fi
if [ -n "$LOG_DIFFICULTY" ]; then
    echo "    --fakenet-log-difficulty $LOG_DIFFICULTY \\"
fi
echo "    --mine \\"
echo "    --mining-pkh $MINING_ADDR \\"
echo "    --bind $NODE_BIND \\"
echo "    --bind-public-grpc-addr $NODE_PUBLIC_GRPC \\"
echo "    --bind-private-grpc-port $NODE_PRIVATE_GRPC_PORT"
echo ""

EXTRA_ARGS=()
if [ "$NEW_FLAG" = "true" ]; then
    EXTRA_ARGS+=(--new)
fi
if [ -n "$POW_LEN" ]; then
    EXTRA_ARGS+=(--fakenet-pow-len "$POW_LEN")
fi
if [ -n "$LOG_DIFFICULTY" ]; then
    EXTRA_ARGS+=(--fakenet-log-difficulty "$LOG_DIFFICULTY")
fi

"$BIN_DIR/nockchain" \
    --fakenet \
    --fakenet-v1-phase "$V1_PHASE" \
    --fakenet-genesis-jam-path "$GENESIS_JAM" \
    "${EXTRA_ARGS[@]}" \
    --mine \
    --mining-pkh "$MINING_ADDR" \
    --bind "$NODE_BIND" \
    --bind-public-grpc-addr "$NODE_PUBLIC_GRPC" \
    --bind-private-grpc-port "$NODE_PRIVATE_GRPC_PORT" \
    2>&1 | sed 's/^/[NODE] /' &
NODE_PID=$!

echo "Node started with PID $NODE_PID"
echo "Waiting for node to initialize..."
sleep 3

if ! kill -0 $NODE_PID 2>/dev/null; then
    echo "Error: Node failed to start"
    exit 1
fi

echo ""
echo "============================================"
echo "Node running!"
echo "============================================"
echo "Node:   PID=$NODE_PID"
echo "        Public gRPC:  http://$NODE_PUBLIC_GRPC"
echo "        Private gRPC: http://127.0.0.1:$NODE_PRIVATE_GRPC_PORT"
echo ""
echo "Data directory: $NODE_DIR"
echo ""
echo "Press Ctrl+C to stop"
echo "============================================"

wait $NODE_PID
