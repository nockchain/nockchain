#!/usr/bin/env bash
# Advance a Tenderly Virtual TestNet by N blocks
#
# Usage: ./tenderly-advance-blocks.sh <num_blocks> [rpc_url]
#
# Examples:
#   ./tenderly-advance-blocks.sh 100
#   ./tenderly-advance-blocks.sh 84 https://virtual.base.rpc.tenderly.co/your-vnet-id

set -euo pipefail

NUM_BLOCKS="${1:?Usage: $0 <num_blocks> [rpc_url]}"
RPC_URL="${2:-${TENDERLY_ADMIN_RPC_URL:-${TENDERLY_RPC_URL:-${TENDERLY_VIRTUAL_TESTNET_RPC_URL:-${BASE_WS_URL:-${BASE_RPC_URL:-}}}}}}"

if [[ -z "$RPC_URL" ]]; then
    echo "Error: No RPC URL provided."
    echo "Set TENDERLY_ADMIN_RPC_URL, TENDERLY_RPC_URL, TENDERLY_VIRTUAL_TESTNET_RPC_URL, or pass as second argument."
    exit 1
fi

# Convert ws:// to https:// if needed
RPC_URL="${RPC_URL/wss:\/\//https://}"
RPC_URL="${RPC_URL/ws:\/\//http://}"

echo "Advancing $NUM_BLOCKS blocks on $RPC_URL"

# Get current block number
CURRENT_BLOCK=$(curl -s -X POST "$RPC_URL" \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
    | jq -r '.result' | xargs printf "%d")

echo "Current block: $CURRENT_BLOCK"

# Mine blocks one at a time so the testnet produces contiguous intermediate
# blocks instead of jumping the visible tip forward in one bulk step.
for ((i=1; i<=NUM_BLOCKS; i++)); do
    RESULT=$(curl -s -X POST "$RPC_URL" \
        -H "Content-Type: application/json" \
        -d '{"jsonrpc":"2.0","method":"evm_mine","params":[],"id":1}')

    ERROR=$(echo "$RESULT" | jq -r '.error // empty')
    if [[ -n "$ERROR" ]]; then
        echo "Error from RPC while mining block $i: $ERROR"
        exit 1
    fi

    if ((i % 10 == 0)) || ((i == NUM_BLOCKS)); then
        echo "  Mined $i / $NUM_BLOCKS blocks..."
    fi
done

# Get new block number
NEW_BLOCK=$(curl -s -X POST "$RPC_URL" \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
    | jq -r '.result' | xargs printf "%d")

ADVANCED=$((NEW_BLOCK - CURRENT_BLOCK))
echo "New block: $NEW_BLOCK (advanced $ADVANCED blocks)"

if [[ $ADVANCED -lt $NUM_BLOCKS ]]; then
    echo "Warning: Only advanced $ADVANCED blocks, expected $NUM_BLOCKS"
    exit 1
fi

echo "Done!"
