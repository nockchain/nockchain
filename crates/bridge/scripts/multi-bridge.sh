#!/bin/bash
set -e

# Spawn 5 bridge nodes in zellij panes for proposal signing tests
# Usage: ./multi-bridge.sh [--new] [--start] [--base-start-height N] [--nockchain-start-height N]
#                         [--deposit-nonce-epoch-base N] [--deposit-nonce-epoch-start-height N]
#                         [--deposit-nonce-epoch-start-tx-id BASE58]
#
# Options:
#   --new                      Start with fresh bridge state for all nodes
#   --start                    Send a %start poke to clear kernel stop state
#   --base-start-height N      Override Base chain start height (default: 33387036)
#   --nockchain-start-height N Override Nockchain start height (default: 1)
#   --deposit-nonce-epoch-base N
#                             Override deposit nonce epoch base (optional)
#   --deposit-nonce-epoch-start-height N
#                             Override deposit nonce epoch start height (optional)
#   --deposit-nonce-epoch-start-tx-id BASE58
#                             Override deposit nonce epoch start tx id (base58, optional)
#
# Prerequisites:
#   - zellij installed
#   - Bridge binary built: cargo build --release -p bridge
#   - Node running (or use run-node-only.sh first)
#   - Environment sourced (optional): source environments/virtual-testnet.generated.env
#
# Each bridge gets:
#   - Unique node_id (0-4)
#   - Unique ingress port (8002-8006)
#   - Unique data directory

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/layout.sh
source "$SCRIPT_DIR/lib/layout.sh"
bridge_resolve_layout

BIN_DIR="$BRIDGE_BIN_DIR"
TEST_DATA_DIR="${BRIDGE_DIR}/test_run_data"

NODE_PRIVATE_GRPC_PORT="${NODE_PRIVATE_GRPC_PORT:-5002}"

to_ws_url() {
    local url="$1"
    url="${url/https:\/\//wss://}"
    url="${url/http:\/\//ws://}"
    echo "$url"
}

# Environment configuration
# Supports two environments:
#   - virtual-testnet: Tenderly VNet (default, uses TENDERLY_* env vars)
#   - base-sepolia: Real Base Sepolia testnet (uses BASE_SEPOLIA_* env vars)
BRIDGE_ENV="${BRIDGE_ENV:-virtual-testnet}"
AUTO_LOADED_ENV_FILE=""

# In virtual-testnet mode, auto-load generated env from tenderly-vnet-deploy.sh
# when no explicit BASE_WS_URL is provided.
if [ "$BRIDGE_ENV" = "virtual-testnet" ] && [ -z "${BASE_WS_URL:-}" ]; then
    GENERATED_ENV_FILE="${SCRIPT_DIR}/environments/virtual-testnet.generated.env"
    if [ -f "$GENERATED_ENV_FILE" ]; then
        # shellcheck disable=SC1090
        source "$GENERATED_ENV_FILE"
        AUTO_LOADED_ENV_FILE="$GENERATED_ENV_FILE"
    fi
fi

# If only BASE_RPC_URL is present, derive WS URL from it.
if [ -z "${BASE_WS_URL:-}" ] && [ -n "${BASE_RPC_URL:-}" ]; then
    BASE_WS_URL="$(to_ws_url "$BASE_RPC_URL")"
fi

# Guard against accidental HTTP URL being passed as BASE_WS_URL.
case "${BASE_WS_URL:-}" in
    http://*|https://*)
        echo "WARN: BASE_WS_URL uses HTTP scheme; converting to websocket URL"
        BASE_WS_URL="$(to_ws_url "$BASE_WS_URL")"
        ;;
esac

if [ "$BRIDGE_ENV" = "base-sepolia" ]; then
    : "${BASE_SEPOLIA_WS_URL:?BASE_SEPOLIA_WS_URL must be set for base-sepolia mode.}"
    : "${BASE_SEPOLIA_INBOX_PROXY:?BASE_SEPOLIA_INBOX_PROXY must be set for base-sepolia mode.}"
    : "${BASE_SEPOLIA_NOCK:?BASE_SEPOLIA_NOCK must be set for base-sepolia mode.}"
    BASE_WS_URL="$BASE_SEPOLIA_WS_URL"
    INBOX_CONTRACT_ADDRESS="$BASE_SEPOLIA_INBOX_PROXY"
    NOCK_CONTRACT_ADDRESS="$BASE_SEPOLIA_NOCK"

    BRIDGE_ETH_KEYS=(
        "${BASE_SEPOLIA_BRIDGE_NODE_KEY_0:?BASE_SEPOLIA_BRIDGE_NODE_KEY_0 must be set for base-sepolia mode.}"
        "${BASE_SEPOLIA_BRIDGE_NODE_KEY_1:?BASE_SEPOLIA_BRIDGE_NODE_KEY_1 must be set for base-sepolia mode.}"
        "${BASE_SEPOLIA_BRIDGE_NODE_KEY_2:?BASE_SEPOLIA_BRIDGE_NODE_KEY_2 must be set for base-sepolia mode.}"
        "${BASE_SEPOLIA_BRIDGE_NODE_KEY_3:?BASE_SEPOLIA_BRIDGE_NODE_KEY_3 must be set for base-sepolia mode.}"
        "${BASE_SEPOLIA_BRIDGE_NODE_KEY_4:?BASE_SEPOLIA_BRIDGE_NODE_KEY_4 must be set for base-sepolia mode.}"
    )

    # Nockchain keys - same for both environments (fakenet keys)
    BRIDGE_NOCK_KEYS=(
        "5KZuFKrctV5iUburT54Z9fhpf3V3hv2sPf9GRQnjFR8T"  # node 0
        "5KZuFKrctV5iUburT54Z9fhpf3V3hv2sPf9GRQnjFR8U"  # node 1
        "5KZuFKrctV5iUburT54Z9fhpf3V3hv2sPf9GRQnjFR8V"  # node 2
        "5KZuFKrctV5iUburT54Z9fhpf3V3hv2sPf9GRQnjFR8W"  # node 3
        "5KZuFKrctV5iUburT54Z9fhpf3V3hv2sPf9GRQnjFR8X"  # node 4
    )

    # Default start height for Base Sepolia (set to recent block to avoid long sync)
    # Current block ~35M as of Dec 2026
    BASE_START_HEIGHT="${BASE_START_HEIGHT:-40982896}"
else
    # Tenderly Virtual Testnet (default)
    : "${BASE_WS_URL:?BASE_WS_URL must be set; source scripts/environments/virtual-testnet.generated.env or an environment profile.}"
    : "${INBOX_CONTRACT_ADDRESS:?INBOX_CONTRACT_ADDRESS must be set.}"
    : "${NOCK_CONTRACT_ADDRESS:?NOCK_CONTRACT_ADDRESS must be set.}"

    # Keys for Tenderly VNet (deterministic test keys)
    BRIDGE_ETH_KEYS=(
        "${BRIDGE_NODE_KEY_0:-0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318}"
        "${BRIDGE_NODE_KEY_1:-0x5c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362319}"
        "${BRIDGE_NODE_KEY_2:-0x6c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f36231a}"
        "${BRIDGE_NODE_KEY_3:-0x7c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f36231b}"
        "${BRIDGE_NODE_KEY_4:-0x8c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f36231c}"
    )

    # Nockchain keys (fakenet keys)
    BRIDGE_NOCK_KEYS=(
        "5KZuFKrctV5iUburT54Z9fhpf3V3hv2sPf9GRQnjFR8T"  # node 0
        "5KZuFKrctV5iUburT54Z9fhpf3V3hv2sPf9GRQnjFR8U"  # node 1
        "5KZuFKrctV5iUburT54Z9fhpf3V3hv2sPf9GRQnjFR8V"  # node 2
        "5KZuFKrctV5iUburT54Z9fhpf3V3hv2sPf9GRQnjFR8W"  # node 3
        "5KZuFKrctV5iUburT54Z9fhpf3V3hv2sPf9GRQnjFR8X"  # node 4
    )

    # Default start height for Tenderly VNet
    BASE_START_HEIGHT="${BASE_START_HEIGHT:-36417335}"
fi

# Ingress ports for each bridge
INGRESS_PORTS=(8002 8003 8004 8005 8006)

# Configurable start heights (can be overridden via CLI)
BASE_START_HEIGHT="${BASE_START_HEIGHT:-40982896}"
NOCKCHAIN_START_HEIGHT="${NOCKCHAIN_START_HEIGHT:-1}"

# Driver-side finality configuration (confirmation depths)
BASE_CONFIRMATION_DEPTH="${BASE_CONFIRMATION_DEPTH:-1}"
NOCKCHAIN_CONFIRMATION_DEPTH="${NOCKCHAIN_CONFIRMATION_DEPTH:-1}"
DEPOSIT_NONCE_EPOCH_BASE="${DEPOSIT_NONCE_EPOCH_BASE:-}"
DEPOSIT_NONCE_EPOCH_START_HEIGHT="${DEPOSIT_NONCE_EPOCH_START_HEIGHT:-}"
DEPOSIT_NONCE_EPOCH_START_TX_ID_BASE58="${DEPOSIT_NONCE_EPOCH_START_TX_ID_BASE58:-}"

NEW_FLAG=""
START_FLAG=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --new)
            NEW_FLAG="--new"
            echo "Cleaning up old bridge data..."
            for i in {0..4}; do
                rm -rf "${TEST_DATA_DIR}/bridge-${i}"
            done
            shift
            ;;
        --start)
            START_FLAG="--start"
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
        --deposit-nonce-epoch-base)
            DEPOSIT_NONCE_EPOCH_BASE="$2"
            shift 2
            ;;
        --deposit-nonce-epoch-start-height)
            DEPOSIT_NONCE_EPOCH_START_HEIGHT="$2"
            shift 2
            ;;
        --deposit-nonce-epoch-start-tx-id)
            DEPOSIT_NONCE_EPOCH_START_TX_ID_BASE58="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: ./multi-bridge.sh [--new] [--start] [--base-start-height N] [--nockchain-start-height N] [--deposit-nonce-epoch-base N] [--deposit-nonce-epoch-start-height N] [--deposit-nonce-epoch-start-tx-id BASE58]"
            exit 1
            ;;
    esac
done

# Check prerequisites
if [ ! -f "$BIN_DIR/bridge" ]; then
    echo "Error: bridge binary not found. Run: cargo build --release -p bridge"
    exit 1
fi

if ! command -v zellij &> /dev/null; then
    echo "Error: zellij not found. Install with: cargo install zellij"
    exit 1
fi

echo "============================================"
echo "Spawning 5 Bridge Nodes in Zellij"
echo "Environment: $BRIDGE_ENV"
if [ -n "$AUTO_LOADED_ENV_FILE" ]; then
    echo "Loaded env:  $AUTO_LOADED_ENV_FILE"
fi
echo "Base WS URL: ${BASE_WS_URL:0:60}..."
echo "Inbox:       $INBOX_CONTRACT_ADDRESS"
echo "Nock:        $NOCK_CONTRACT_ADDRESS"
echo "Base Start:  $BASE_START_HEIGHT"
echo "Nock Start:  $NOCKCHAIN_START_HEIGHT"
if [ -n "$DEPOSIT_NONCE_EPOCH_BASE" ]; then
    echo "Deposit Epoch Base:  $DEPOSIT_NONCE_EPOCH_BASE"
fi
if [ -n "$DEPOSIT_NONCE_EPOCH_START_HEIGHT" ]; then
    echo "Deposit Epoch Start: $DEPOSIT_NONCE_EPOCH_START_HEIGHT"
fi
if [ -n "$DEPOSIT_NONCE_EPOCH_START_TX_ID_BASE58" ]; then
    echo "Deposit Epoch TxId:  $DEPOSIT_NONCE_EPOCH_START_TX_ID_BASE58"
fi
echo "============================================"
echo ""

# ETH addresses for each environment (derived from keys)
if [ "$BRIDGE_ENV" = "base-sepolia" ]; then
    BRIDGE_ETH_ADDRS=(
        "${BASE_SEPOLIA_BRIDGE_NODE_ADDR_0:?BASE_SEPOLIA_BRIDGE_NODE_ADDR_0 must be set for base-sepolia mode.}"
        "${BASE_SEPOLIA_BRIDGE_NODE_ADDR_1:?BASE_SEPOLIA_BRIDGE_NODE_ADDR_1 must be set for base-sepolia mode.}"
        "${BASE_SEPOLIA_BRIDGE_NODE_ADDR_2:?BASE_SEPOLIA_BRIDGE_NODE_ADDR_2 must be set for base-sepolia mode.}"
        "${BASE_SEPOLIA_BRIDGE_NODE_ADDR_3:?BASE_SEPOLIA_BRIDGE_NODE_ADDR_3 must be set for base-sepolia mode.}"
        "${BASE_SEPOLIA_BRIDGE_NODE_ADDR_4:?BASE_SEPOLIA_BRIDGE_NODE_ADDR_4 must be set for base-sepolia mode.}"
    )
else
    # Tenderly VNet addresses (derived from deterministic test keys)
    BRIDGE_ETH_ADDRS=(
        "0x2c7536E3605D9C16a7a3D7b1898e529396a65c23"
        "0x0EE156f080d9cB3BaA3C0DB53D07f13D69CEf4C9"
        "0x274BD645de480C325D618c60c661F11275eB77F1"
        "0x6dc59eb20f7928935c47A391e35545a2CEC51013"
        "0xcaB10dA05fC0aDBb7e91Eadc30f224bcDF601375"
    )
fi

# Nock PKHs (public key hashes, ~52 chars base58)
# Fake test PKHs (valid format placeholders for local testing)
BRIDGE_NOCK_PKHS=(
    "2222222222222222222222222222222222222222222222222222"  # test node 0
    "3333333333333333333333333333333333333333333333333333"  # test node 1
    "4444444444444444444444444444444444444444444444444444"  # test node 2
    "5555555555555555555555555555555555555555555555555555"  # test node 3
    "6666666666666666666666666666666666666666666666666666"  # test node 4
)

# Generate config for each bridge
generate_bridge_config() {
    local node_id=$1
    local ingress_port=$2
    local data_dir=$3
    local eth_key=$4
    local nock_key=$5

    mkdir -p "$data_dir"
    local config_file="${data_dir}/bridge-conf.toml"

    cat > "$config_file" << EOF
node_id = ${node_id}
# Environment: ${BRIDGE_ENV}
base_ws_url = "${BASE_WS_URL}"
inbox_contract_address = "${INBOX_CONTRACT_ADDRESS}"
nock_contract_address = "${NOCK_CONTRACT_ADDRESS}"
my_eth_key = "${eth_key}"
my_nock_key = "${nock_key}"
grpc_address = "http://127.0.0.1:${NODE_PRIVATE_GRPC_PORT}"
base_confirmation_depth = ${BASE_CONFIRMATION_DEPTH}
nockchain_confirmation_depth = ${NOCKCHAIN_CONFIRMATION_DEPTH}
ingress_listen_address = "127.0.0.1:${ingress_port}"
withdrawal_activation_nock_next_height = ${NOCKCHAIN_START_HEIGHT}
EOF

    if [ -n "$DEPOSIT_NONCE_EPOCH_BASE" ]; then
        cat >> "$config_file" << EOF
deposit_nonce_epoch_base = ${DEPOSIT_NONCE_EPOCH_BASE}
EOF
    fi
    if [ -n "$DEPOSIT_NONCE_EPOCH_START_HEIGHT" ]; then
        cat >> "$config_file" << EOF
deposit_nonce_epoch_start_height = ${DEPOSIT_NONCE_EPOCH_START_HEIGHT}
EOF
    fi
    if [ -n "$DEPOSIT_NONCE_EPOCH_START_TX_ID_BASE58" ]; then
        cat >> "$config_file" << EOF
deposit_nonce_epoch_start_tx_id_base58 = "${DEPOSIT_NONCE_EPOCH_START_TX_ID_BASE58}"
EOF
    fi

    cat >> "$config_file" << EOF

[[nodes]]
ip = "127.0.0.1:8002"
eth_pubkey = "${BRIDGE_ETH_ADDRS[0]}"
nock_pkh = "${BRIDGE_NOCK_PKHS[0]}"

[[nodes]]
ip = "127.0.0.1:8003"
eth_pubkey = "${BRIDGE_ETH_ADDRS[1]}"
nock_pkh = "${BRIDGE_NOCK_PKHS[1]}"

[[nodes]]
ip = "127.0.0.1:8004"
eth_pubkey = "${BRIDGE_ETH_ADDRS[2]}"
nock_pkh = "${BRIDGE_NOCK_PKHS[2]}"

[[nodes]]
ip = "127.0.0.1:8005"
eth_pubkey = "${BRIDGE_ETH_ADDRS[3]}"
nock_pkh = "${BRIDGE_NOCK_PKHS[3]}"

[[nodes]]
ip = "127.0.0.1:8006"
eth_pubkey = "${BRIDGE_ETH_ADDRS[4]}"
nock_pkh = "${BRIDGE_NOCK_PKHS[4]}"

# Bridge constants for local testing
[constants]
min_signers = 3
total_signers = 5
minimum_event_nocks = 1000       # Lower for testing (prod: 1_000_000)
nicks_fee_per_nock = 195
base_blocks_chunk = 1
base_start_height = ${BASE_START_HEIGHT}
nockchain_start_height = ${NOCKCHAIN_START_HEIGHT}
EOF

    echo "$config_file"
}

# Generate all configs first
echo "Generating bridge configs..."
CONFIG_FILES=()
for i in {0..4}; do
    data_dir="${TEST_DATA_DIR}/bridge-${i}"
    config_file=$(generate_bridge_config $i ${INGRESS_PORTS[$i]} "$data_dir" "${BRIDGE_ETH_KEYS[$i]}" "${BRIDGE_NOCK_KEYS[$i]}")
    CONFIG_FILES+=("$config_file")
    echo "  Bridge $i: $config_file (port ${INGRESS_PORTS[$i]})"
done
echo ""

# Create a temporary script for each bridge that zellij will run
create_bridge_runner() {
    local node_id=$1
    local config_file=$2
    local data_dir=$3
    local runner_script="${data_dir}/run.sh"

    cat > "$runner_script" << EOF
#!/bin/bash

echo "============================================"
echo "Bridge Node $node_id"
echo "Config: $config_file"
echo "Data:   $data_dir"
echo "============================================"
echo ""

export RUST_LOG=debug,h2=warn,hyper=warn,tower=warn,tonic=info
exec "$BIN_DIR/bridge" \\
    $NEW_FLAG \\
    $START_FLAG \\
    --config-path "$config_file" \\
    --data-dir "$data_dir"
EOF
    chmod +x "$runner_script"
    echo "$runner_script"
}

# Create runner scripts
echo "Creating runner scripts..."
RUNNER_SCRIPTS=()
for i in {0..4}; do
    data_dir="${TEST_DATA_DIR}/bridge-${i}"
    runner=$(create_bridge_runner $i "${CONFIG_FILES[$i]}" "$data_dir")
    RUNNER_SCRIPTS+=("$runner")
done
echo ""

# Create zellij layout file
LAYOUT_FILE="${TEST_DATA_DIR}/bridges.kdl"
cat > "$LAYOUT_FILE" << 'EOF'
layout {
    pane split_direction="vertical" {
        pane split_direction="horizontal" {
            pane {
                name "Bridge 0"
                command "bash"
                args "-c" "RUNNER_SCRIPT_0"
            }
            pane {
                name "Bridge 1"
                command "bash"
                args "-c" "RUNNER_SCRIPT_1"
            }
        }
        pane split_direction="horizontal" {
            pane {
                name "Bridge 2"
                command "bash"
                args "-c" "RUNNER_SCRIPT_2"
            }
            pane {
                name "Bridge 3"
                command "bash"
                args "-c" "RUNNER_SCRIPT_3"
            }
            pane {
                name "Bridge 4"
                command "bash"
                args "-c" "RUNNER_SCRIPT_4"
            }
        }
    }
}
EOF

# Replace placeholders with actual script paths
for i in {0..4}; do
    sed -i.bak "s|RUNNER_SCRIPT_${i}|${RUNNER_SCRIPTS[$i]}|g" "$LAYOUT_FILE"
done
rm -f "${LAYOUT_FILE}.bak"

echo "Starting zellij with 5 bridge panes..."
echo "Layout file: $LAYOUT_FILE"
echo ""
echo "============================================"
echo "Bridge Ports:"
for i in {0..4}; do
    echo "  Bridge $i: http://127.0.0.1:${INGRESS_PORTS[$i]}"
done
echo ""
echo "All bridges connect to node gRPC: http://127.0.0.1:$NODE_PRIVATE_GRPC_PORT"
echo "============================================"
echo ""
echo "TUI (separate terminal, pick any bridge):"
echo "  $BIN_DIR/nockchain-bridge-tui --server \"http://127.0.0.1:${INGRESS_PORTS[0]}\""
echo ""
echo "Press Ctrl+Q to exit zellij (all bridges will stop)"
echo ""

# Start zellij with the layout
exec zellij --layout "$LAYOUT_FILE"
