#!/bin/bash
# Fund bridge nodes on Base Sepolia from the deployer account
set -e

# Required env vars
: "${BASE_SEPOLIA_RPC_URL:?BASE_SEPOLIA_RPC_URL not set}"
: "${BASE_SEPOLIA_DEPLOYER_KEY:?BASE_SEPOLIA_DEPLOYER_KEY not set}"

# Bridge node addresses.
ADDRS=(
  "${BASE_SEPOLIA_BRIDGE_NODE_ADDR_0:?BASE_SEPOLIA_BRIDGE_NODE_ADDR_0 not set}"
  "${BASE_SEPOLIA_BRIDGE_NODE_ADDR_1:?BASE_SEPOLIA_BRIDGE_NODE_ADDR_1 not set}"
  "${BASE_SEPOLIA_BRIDGE_NODE_ADDR_2:?BASE_SEPOLIA_BRIDGE_NODE_ADDR_2 not set}"
  "${BASE_SEPOLIA_BRIDGE_NODE_ADDR_3:?BASE_SEPOLIA_BRIDGE_NODE_ADDR_3 not set}"
  "${BASE_SEPOLIA_BRIDGE_NODE_ADDR_4:?BASE_SEPOLIA_BRIDGE_NODE_ADDR_4 not set}"
)

# Amount per node (in wei) - 0.0005 ETH = 500000000000000 wei
AMOUNT="${AMOUNT:-500000000000000}"
AMOUNT_ETH=$(echo "scale=6; $AMOUNT / 1000000000000000000" | bc)

echo "=== Base Sepolia Bridge Node Funder ==="
echo ""

# Check deployer balance
DEPLOYER_ADDR=$(cast wallet address --private-key "$BASE_SEPOLIA_DEPLOYER_KEY")
DEPLOYER_BAL=$(cast balance "$DEPLOYER_ADDR" --rpc-url "$BASE_SEPOLIA_RPC_URL")
DEPLOYER_BAL_ETH=$(echo "scale=6; $DEPLOYER_BAL / 1000000000000000000" | bc)

echo "Deployer: $DEPLOYER_ADDR"
echo "Balance:  $DEPLOYER_BAL wei ($DEPLOYER_BAL_ETH ETH)"
echo ""

TOTAL_NEEDED=$((AMOUNT * 5))
if [ "$DEPLOYER_BAL" -lt "$TOTAL_NEEDED" ]; then
  echo "WARNING: Deployer balance ($DEPLOYER_BAL) < total needed ($TOTAL_NEEDED)"
  echo "Will send what we can..."
fi

echo "Sending $AMOUNT wei ($AMOUNT_ETH ETH) to each node..."
echo ""

for i in "${!ADDRS[@]}"; do
  addr="${ADDRS[$i]}"
  echo "[$((i+1))/5] Funding node $i: $addr"
  
  # Check current balance
  bal=$(cast balance "$addr" --rpc-url "$BASE_SEPOLIA_RPC_URL")
  echo "  Current balance: $bal wei"
  
  # Send funds
  tx=$(cast send "$addr" --value "$AMOUNT" \
    --private-key "$BASE_SEPOLIA_DEPLOYER_KEY" \
    --rpc-url "$BASE_SEPOLIA_RPC_URL" \
    --json 2>&1) || {
    echo "  ERROR: Failed to send"
    echo "  $tx"
    continue
  }
  
  txhash=$(echo "$tx" | jq -r '.transactionHash')
  echo "  Sent: $txhash"
  
  # Check new balance
  new_bal=$(cast balance "$addr" --rpc-url "$BASE_SEPOLIA_RPC_URL")
  echo "  New balance: $new_bal wei"
  echo ""
done

echo "=== Final Balances ==="
for i in "${!ADDRS[@]}"; do
  addr="${ADDRS[$i]}"
  bal=$(cast balance "$addr" --rpc-url "$BASE_SEPOLIA_RPC_URL")
  bal_eth=$(echo "scale=6; $bal / 1000000000000000000" | bc)
  echo "Node $i ($addr): $bal wei ($bal_eth ETH)"
done

echo ""
DEPLOYER_BAL=$(cast balance "$DEPLOYER_ADDR" --rpc-url "$BASE_SEPOLIA_RPC_URL")
DEPLOYER_BAL_ETH=$(echo "scale=6; $DEPLOYER_BAL / 1000000000000000000" | bc)
echo "Deployer ($DEPLOYER_ADDR): $DEPLOYER_BAL wei ($DEPLOYER_BAL_ETH ETH)"
