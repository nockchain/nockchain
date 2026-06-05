#!/usr/bin/env bash
set -euo pipefail

# Verify deployed contracts on Tenderly
# Usage: ./scripts/verify_contracts.sh [DEPLOYMENTS_PATH]

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONTRACTS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$CONTRACTS_DIR"

# Determine deployment path
DEPLOYMENTS_PATH="${1:-${DEPLOYMENTS_PATH:-}}"
if [ -z "$DEPLOYMENTS_PATH" ]; then
    DEPLOY_TARGET_NETWORK="${DEPLOY_TARGET_NETWORK:-tenderly-devnet}"
    DEPLOYMENTS_PATH="$CONTRACTS_DIR/deployments/${DEPLOY_TARGET_NETWORK}.json"
fi

if [ ! -f "$DEPLOYMENTS_PATH" ]; then
    echo "Error: Deployment file not found: $DEPLOYMENTS_PATH" >&2
    exit 1
fi

# Validate required environment variables
if [ -z "${TENDERLY_RPC_URL:-}" ]; then
    echo "Error: TENDERLY_RPC_URL not set" >&2
    exit 1
fi

if [ -z "${TENDERLY_ACCESS_KEY:-}" ]; then
    echo "Error: TENDERLY_ACCESS_KEY not set" >&2
    echo "Get your access key from: https://dashboard.tenderly.co/account/authorization" >&2
    exit 1
fi

# Read addresses from deployment file
NOCK_ADDRESS=$(jq -r '.nock' "$DEPLOYMENTS_PATH")
INBOX_IMPL_ADDRESS=$(jq -r '.messageInboxImplementation' "$DEPLOYMENTS_PATH")
INBOX_PROXY_ADDRESS=$(jq -r '.messageInboxProxy' "$DEPLOYMENTS_PATH")

echo "Verifying contracts from: $DEPLOYMENTS_PATH"
echo ""
echo "Addresses:"
echo "  Nock:                    $NOCK_ADDRESS"
echo "  MessageInbox (impl):     $INBOX_IMPL_ADDRESS"
echo "  MessageInbox (proxy):    $INBOX_PROXY_ADDRESS"
echo ""

# Build first to ensure artifacts are current
echo "Building contracts..."
forge build --force

# Construct verifier URL from RPC URL
VERIFIER_URL="${TENDERLY_RPC_URL}/verify/etherscan"

echo ""
echo "Using verifier URL: $VERIFIER_URL"
echo ""

# Verify Nock token
echo "=== Verifying Nock token ==="
forge verify-contract "$NOCK_ADDRESS" Nock.sol:Nock \
    --verifier-url "$VERIFIER_URL" \
    --etherscan-api-key "$TENDERLY_ACCESS_KEY" \
    --watch || echo "Warning: Nock verification failed or already verified"

echo ""

# Verify MessageInbox implementation
echo "=== Verifying MessageInbox implementation ==="
forge verify-contract "$INBOX_IMPL_ADDRESS" MessageInbox.sol:MessageInbox \
    --verifier-url "$VERIFIER_URL" \
    --etherscan-api-key "$TENDERLY_ACCESS_KEY" \
    --watch || echo "Warning: MessageInbox implementation verification failed or already verified"

echo ""

# Note: ERC1967Proxy is from OpenZeppelin and may need special handling
echo "=== Verifying ERC1967Proxy ==="
forge verify-contract "$INBOX_PROXY_ADDRESS" \
    lib/openzeppelin-contracts/contracts/proxy/ERC1967/ERC1967Proxy.sol:ERC1967Proxy \
    --verifier-url "$VERIFIER_URL" \
    --etherscan-api-key "$TENDERLY_ACCESS_KEY" \
    --watch || echo "Warning: Proxy verification failed or already verified"

echo ""
echo "Verification complete. Check Tenderly dashboard for results."
