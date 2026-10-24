#!/bin/bash
# Wallet helper script - uses the same keys as run-node-only.sh
#
# Usage:
#   ./wallet.sh list-notes              # List spendable notes
#   ./wallet.sh list-active-addresses   # List addresses
#   ./wallet.sh show-balance            # Show wallet balance
#   ./wallet.sh create-tx ...           # Create a transaction (saves to ./txs/)
#   ./wallet.sh send-tx <file>          # Submit a transaction to the node
#   ./wallet.sh --public-api list-notes # Sync via public API instead of private
#   ./wallet.sh --new <command>         # Reset wallet state, re-import seed, then run
#   ./wallet.sh <any wallet command>    # Pass through to wallet
#
# ============================================================================
# BRIDGE DEPOSIT WORKFLOW
# ============================================================================
#
# 1. Check your balance and find spendable notes:
#
#    ./wallet.sh show-balance
#    ./wallet.sh list-notes
#
# 2. Create a bridge deposit transaction:
#    - Pick notes from list-notes output (need enough to cover amount + fee)
#    - The --names arg takes a Hoon list: "[<name1> <name2>]"
#
#    ./wallet.sh create-tx \
#      --names "[2naNCqw9F1VxLse3PBxZjTrzTSrEF8HbjgDogcrpXbGhvN6TwNsJAgV 4JjFAYZNzdZqpqZWqQpwaP4BSvXkVGmj6X6bsvJtKB38jDAkzkgub5w]" \
#      --fee 2654208 \
#      --recipient '{"kind":"bridge-deposit", "evm-address": "0x1111111111111111111111111111111111111111", "amount": 4000000000}'
#
#    This saves the transaction to ./txs/<tx-name>.tx
#
# 3. Submit the transaction to the nockchain node:
#
#    ./wallet.sh send-tx ./txs/<tx-name>.tx --public-grpc-server-addr "http://127.0.0.1:50052"
#
#    On success you'll see: "Validation for TX <id> passed. TX has been submitted to node."
#
# 4. The bridge will detect the deposit when it scans the block containing the tx.
#
# ============================================================================

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/layout.sh
source "$SCRIPT_DIR/lib/layout.sh"
bridge_resolve_layout

BIN_DIR="$BRIDGE_BIN_DIR"
TEST_DATA_DIR="${TEST_DATA_DIR:-${BRIDGE_DIR}/test_run_data}"
WALLET_DIR="${TEST_DATA_DIR}/wallet"

NODE_PRIVATE_GRPC_PORT="${NODE_PRIVATE_GRPC_PORT:-5002}"
NODE_PUBLIC_GRPC_SERVER_ADDR="${NODE_PUBLIC_GRPC_SERVER_ADDR:-http://127.0.0.1:5001}"
COMMON_WALLET_ARGS=(--data-dir "$WALLET_DIR" --fakenet)
if [[ -n "${BRIDGE_DEV_FAKENET_BYTHOS_PHASE:-}" ]]; then
    COMMON_WALLET_ARGS+=(--fakenet-bythos-phase "$BRIDGE_DEV_FAKENET_BYTHOS_PHASE")
fi
CLIENT_MODE="private"

# Same v1 seed as run-node-only.sh, plus the legacy v0 seed that matches
# the node's hardcoded pre-v1 coinbase pubkey.
FAKENET_V1_SEED="route run sing warrior light swamp clog flower agent ugly wasp fresh tube snow motion salt salon village raccoon chair demise neutral school confirm"
FAKENET_V0_SEED="farm step rhythm surprise math august panther pulse protect remain anger depend adjust sting enable poet describe stone essay blast click horse hair practice"
FAKENET_V1_ACTIVE_MASTER="9phXGACnW4238oqgvn2gpwaUjG3RAqcxq2Ash2vaKp8KjzSd3MQ56Jt"

NEW_WALLET=false
# Script-only flags.
PASSTHRU_ARGS=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --new)
            NEW_WALLET=true
            shift
            ;;
        --public-api)
            CLIENT_MODE="public"
            shift
            ;;
        --private-api)
            CLIENT_MODE="private"
            shift
            ;;
        --public-grpc-server-addr)
            if [[ $# -lt 2 ]]; then
                echo "Error: --public-grpc-server-addr requires a value"
                exit 1
            fi
            NODE_PUBLIC_GRPC_SERVER_ADDR="$2"
            shift 2
            ;;
        --public-grpc-server-addr=*)
            NODE_PUBLIC_GRPC_SERVER_ADDR="${1#*=}"
            shift
            ;;
        --private-grpc-server-port)
            if [[ $# -lt 2 ]]; then
                echo "Error: --private-grpc-server-port requires a value"
                exit 1
            fi
            NODE_PRIVATE_GRPC_PORT="$2"
            shift 2
            ;;
        --private-grpc-server-port=*)
            NODE_PRIVATE_GRPC_PORT="${1#*=}"
            shift
            ;;
        *)
            PASSTHRU_ARGS+=("$1")
            shift
            ;;
    esac
done

if [[ "$CLIENT_MODE" == "public" ]]; then
    CLIENT_ARGS=(
        --client public
        --public-grpc-server-addr "$NODE_PUBLIC_GRPC_SERVER_ADDR"
    )
else
    CLIENT_ARGS=(
        --client private
        --private-grpc-server-port "$NODE_PRIVATE_GRPC_PORT"
    )
fi

if [ ! -f "$BIN_DIR/nockchain-wallet" ]; then
    echo "Error: nockchain-wallet not found. Run: cargo build --release -p nockchain-wallet"
    exit 1
fi

if [ "$NEW_WALLET" = true ]; then
    rm -rf "$WALLET_DIR"
fi
mkdir -p "$WALLET_DIR"

if [ "$NEW_WALLET" = true ]; then
    # Reset wallet state and import deterministic fakenet keys.
    # Keep the v1 bridge address active by default after also importing the
    # legacy v0 key needed to spend pre-v1 coinbase notes.
    NOCKAPP_HOME="$TEST_DATA_DIR" "$BIN_DIR/nockchain-wallet" --new \
        "${COMMON_WALLET_ARGS[@]}" import-keys \
        --seedphrase "$FAKENET_V1_SEED" --version 1
    NOCKAPP_HOME="$TEST_DATA_DIR" "$BIN_DIR/nockchain-wallet" \
        "${COMMON_WALLET_ARGS[@]}" import-keys \
        --seedphrase "$FAKENET_V0_SEED" --version 0
    NOCKAPP_HOME="$TEST_DATA_DIR" "$BIN_DIR/nockchain-wallet" \
        "${COMMON_WALLET_ARGS[@]}" set-active-master-address "$FAKENET_V1_ACTIVE_MASTER"
fi

# Run the wallet command
export NOCKAPP_HOME="$TEST_DATA_DIR"
exec "$BIN_DIR/nockchain-wallet" \
    "${CLIENT_ARGS[@]}" \
    "${COMMON_WALLET_ARGS[@]}" \
    "${PASSTHRU_ARGS[@]}"
