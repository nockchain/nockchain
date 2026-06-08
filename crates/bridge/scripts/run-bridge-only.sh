#!/bin/bash
set -e

# Run just the bridge (assumes node is already running)
# Usage: ./run-bridge-only.sh [--new] [--base-start-height N] [--nockchain-start-height N]
#
# Options:
#   --new                      Start with fresh bridge state
#   --base-start-height N      Override Base chain start height (default: 33387036)
#   --nockchain-start-height N Override Nockchain start height (default: 1)
#
# Environment setup:
#   source environments/virtual-testnet.env  # For Virtual Testnet (50 block limit)
#   source environments/base-sepolia.env     # For real Base Sepolia (unlimited)
#
# Before you start, run `make install` and `make deps` in the bridge contracts
# directory. Also make sure your wallet, bridge, and nockchain binaries are up to date.
#
# To run the TUI client, use:
#   $BIN_DIR/nockchain-bridge-tui --server "http://$BRIDGE_INGRESS"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/layout.sh
source "$SCRIPT_DIR/lib/layout.sh"
bridge_resolve_layout

BIN_DIR="$BRIDGE_BIN_DIR"
TEST_DATA_DIR="${BRIDGE_DIR}/test_run_data"
BRIDGE_DATA_DIR="${TEST_DATA_DIR}/bridge"

NODE_PRIVATE_GRPC_PORT="${NODE_PRIVATE_GRPC_PORT:-5002}"
BRIDGE_INGRESS="127.0.0.1:8002"

# Environment configuration (set via environment variables or defaults to virtual testnet)
BRIDGE_ENV="${BRIDGE_ENV:-virtual-testnet}"
AUTO_LOADED_ENV_FILE=""
if [ "$BRIDGE_ENV" = "virtual-testnet" ] && [ -z "${BASE_WS_URL:-}" ]; then
    GENERATED_ENV_FILE="${SCRIPT_DIR}/environments/virtual-testnet.generated.env"
    if [ -f "$GENERATED_ENV_FILE" ]; then
        # shellcheck disable=SC1090
        source "$GENERATED_ENV_FILE"
        AUTO_LOADED_ENV_FILE="$GENERATED_ENV_FILE"
    fi
fi
: "${BASE_WS_URL:?BASE_WS_URL must be set; source scripts/environments/virtual-testnet.generated.env or an environment profile.}"
: "${INBOX_CONTRACT_ADDRESS:?INBOX_CONTRACT_ADDRESS must be set.}"
: "${NOCK_CONTRACT_ADDRESS:?NOCK_CONTRACT_ADDRESS must be set.}"
BRIDGE_ETH_KEY="${BRIDGE_ETH_KEY:-0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318}"
BRIDGE_ETH_ADDR="${BRIDGE_ETH_ADDR:-0x2c7536E3605D9C16a7a3D7b1898e529396a65c23}"
BRIDGE_NOCK_KEY="${BRIDGE_NOCK_KEY:-5KZuFKrctV5iUburT54Z9fhpf3V3hv2sPf9GRQnjFR8T}"

# Checkpoint save interval (ms). Lower values make restarts pick up faster.
BRIDGE_SAVE_INTERVAL_MS="${BRIDGE_SAVE_INTERVAL_MS:-5000}"

# Configurable start heights (can be overridden via CLI)
BASE_START_HEIGHT="${BASE_START_HEIGHT:-36417335}"
NOCKCHAIN_START_HEIGHT="${NOCKCHAIN_START_HEIGHT:-1}"

# Driver-side finality configuration (confirmation depths)
BASE_CONFIRMATION_DEPTH="${BASE_CONFIRMATION_DEPTH:-100}"
NOCKCHAIN_CONFIRMATION_DEPTH="${NOCKCHAIN_CONFIRMATION_DEPTH:-1}"

BRIDGE_ETH_ADDR="${BRIDGE_ETH_ADDR:-}"
if [ -z "$BRIDGE_ETH_ADDR" ]; then
    echo "Error: BRIDGE_ETH_ADDR must be set (the Ethereum address for BRIDGE_ETH_KEY)" >&2
    exit 1
fi

echo "============================================"
echo "Environment: $BRIDGE_ENV"
if [ -n "$AUTO_LOADED_ENV_FILE" ]; then
    echo "Loaded env:  $AUTO_LOADED_ENV_FILE"
fi
echo "Base WS URL: $BASE_WS_URL"
echo "Inbox:       $INBOX_CONTRACT_ADDRESS"
echo "Nock:        $NOCK_CONTRACT_ADDRESS"
echo "Base Start:  $BASE_START_HEIGHT"
echo "Nock Start:  $NOCKCHAIN_START_HEIGHT"
echo "Base Conf:   $BASE_CONFIRMATION_DEPTH"
echo "Nock Conf:   $NOCKCHAIN_CONFIRMATION_DEPTH"
echo "============================================"
echo ""

cleanup() {
    echo "Cleaning up..."
    [ -n "$BRIDGE_PID" ] && kill $BRIDGE_PID 2>/dev/null || true
    wait 2>/dev/null || true
    echo "Done."
}

trap cleanup EXIT INT TERM

NEW_FLAG=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --new)
            NEW_FLAG="--new"
            rm -rf "$BRIDGE_DATA_DIR"
            shift
            ;;
        --base-start-height)
            BASE_START_HEIGHT="$2"
            shift 2
            ;;
        --nockchain-start-height)
            NOCKCHAIN_START_HEIGHT="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: ./run-bridge-only.sh [--new] [--base-start-height N] [--nockchain-start-height N]"
            exit 1
            ;;
    esac
done

mkdir -p "$BRIDGE_DATA_DIR"

if [ ! -f "$BIN_DIR/bridge" ]; then
    echo "Error: bridge binary not found. Run: cargo build --release -p bridge"
    exit 1
fi

BRIDGE_CONFIG="${BRIDGE_DATA_DIR}/bridge-conf.toml"
cat > "$BRIDGE_CONFIG" << EOF
node_id = 0
# Environment: ${BRIDGE_ENV}
base_ws_url = "${BASE_WS_URL}"
inbox_contract_address = "${INBOX_CONTRACT_ADDRESS}"
nock_contract_address = "${NOCK_CONTRACT_ADDRESS}"
my_eth_key = "${BRIDGE_ETH_KEY}"
my_nock_key = "${BRIDGE_NOCK_KEY}"
grpc_address = "http://127.0.0.1:${NODE_PRIVATE_GRPC_PORT}"
base_confirmation_depth = ${BASE_CONFIRMATION_DEPTH}
nockchain_confirmation_depth = ${NOCKCHAIN_CONFIRMATION_DEPTH}
ingress_listen_address = "127.0.0.1:8001"

# Fake test data (node 0 address derived from BRIDGE_ETH_KEY)
[[nodes]]
ip = "localhost:8001"
eth_pubkey = "${BRIDGE_ETH_ADDR}"
nock_pkh = "2222222222222222222222222222222222222222222222222222"

[[nodes]]
ip = "127.0.0.1:8002"
eth_pubkey = "0x2222222222222222222222222222222222222222"
nock_pkh = "3333333333333333333333333333333333333333333333333333"

[[nodes]]
ip = "localhost:8003"
eth_pubkey = "0x3333333333333333333333333333333333333333"
nock_pkh = "4444444444444444444444444444444444444444444444444444"

[[nodes]]
ip = "localhost:8004"
eth_pubkey = "0x4444444444444444444444444444444444444444"
nock_pkh = "5555555555555555555555555555555555555555555555555555"

[[nodes]]
ip = "localhost:8005"
eth_pubkey = "0x5555555555555555555555555555555555555555"
nock_pkh = "6666666666666666666666666666666666666666666666666666"

# Bridge constants for local testing
[constants]
min_signers = 3
total_signers = 5
minimum_event_nocks = 1000       # Lower for testing (prod: 1_000_000)
nicks_fee_per_nock = 195
base_blocks_chunk = 100
base_start_height = ${BASE_START_HEIGHT}
nockchain_start_height = ${NOCKCHAIN_START_HEIGHT}
EOF

echo "Bridge config written to $BRIDGE_CONFIG"

echo "Starting bridge..."
# Filter noisy h2/hyper/tonic internal modules
RUST_LOG=debug,h2=warn,hyper=warn,tower=warn,tonic=info \
"$BIN_DIR/bridge" \
    $NEW_FLAG \
    --save-interval "$BRIDGE_SAVE_INTERVAL_MS" \
    --config-path "$BRIDGE_CONFIG" \
    --data-dir "$BRIDGE_DATA_DIR" \
    2>&1 | sed 's/^/[BRIDGE] /' &
BRIDGE_PID=$!

echo "Bridge started with PID $BRIDGE_PID"

echo ""
echo "============================================"
echo "Bridge running! [$BRIDGE_ENV]"
echo "============================================"
echo "Bridge: PID=$BRIDGE_PID"
echo "        Ingress:      http://$BRIDGE_INGRESS"
echo "        Config:       $BRIDGE_CONFIG"
echo "        Node gRPC:    http://127.0.0.1:$NODE_PRIVATE_GRPC_PORT"
echo "        Base WS:      ${BASE_WS_URL:0:50}..."
echo ""
echo "Data directory: $BRIDGE_DATA_DIR"
echo ""
echo "TUI (separate terminal):"
echo "  $BIN_DIR/nockchain-bridge-tui --server \"http://$BRIDGE_INGRESS\""
echo ""
echo "Press Ctrl+C to stop"
echo "============================================"

wait $BRIDGE_PID
