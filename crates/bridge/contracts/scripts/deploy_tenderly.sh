#!/usr/bin/env bash
set -euo pipefail

# Deploy bridge contracts to Tenderly network
# Usage: ./scripts/deploy_tenderly.sh [--dry-run] [EXTRA_FLAGS]

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONTRACTS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$CONTRACTS_DIR"

# Parse arguments
DRY_RUN=false
EXTRA_ARGS=()
for arg in "$@"; do
    if [ "$arg" = "--dry-run" ]; then
        DRY_RUN=true
    else
        EXTRA_ARGS+=("$arg")
    fi
done

# Validate required environment variables
REQUIRED_VARS=(
    "TENDERLY_RPC_URL"
    "TENDERLY_PRIVATE_KEY"
    "NOCK_NAME"
    "NOCK_SYMBOL"
    "BRIDGE_NODE_0"
    "BRIDGE_NODE_1"
    "BRIDGE_NODE_2"
    "BRIDGE_NODE_3"
    "BRIDGE_NODE_4"
)

MISSING_VARS=()
for var in "${REQUIRED_VARS[@]}"; do
    if [ -z "${!var:-}" ]; then
        MISSING_VARS+=("$var")
    fi
done

if [ ${#MISSING_VARS[@]} -ne 0 ]; then
    echo "Error: Missing required environment variables:" >&2
    printf "  - %s\n" "${MISSING_VARS[@]}" >&2
    echo "" >&2
    echo "See .env.template for required variables." >&2
    exit 1
fi

# Set defaults for optional variables
export DEPLOY_TARGET_NETWORK="${DEPLOY_TARGET_NETWORK:-tenderly-devnet}"
export DEPLOYER_ADDRESS="${DEPLOYER_ADDRESS:-0x0000000000000000000000000000000000000000}"

# Determine deployment path
if [ -z "${DEPLOYMENTS_PATH:-}" ]; then
    DEPLOYMENTS_DIR="$CONTRACTS_DIR/deployments"
    mkdir -p "$DEPLOYMENTS_DIR"
    export DEPLOYMENTS_PATH="$DEPLOYMENTS_DIR/${DEPLOY_TARGET_NETWORK}.json"
fi

# Dry run mode - show what would happen
if [ "$DRY_RUN" = true ]; then
    echo "=== DRY RUN MODE ==="
    echo ""
    echo "Configuration:"
    echo "  Network:         $DEPLOY_TARGET_NETWORK"
    echo "  RPC URL:         ${TENDERLY_RPC_URL:0:50}..."
    echo "  Deployer:        $DEPLOYER_ADDRESS"
    echo "  Deployments:     $DEPLOYMENTS_PATH"
    echo ""
    echo "Token Configuration:"
    echo "  Name:            $NOCK_NAME"
    echo "  Symbol:          $NOCK_SYMBOL"
    echo ""
    echo "Bridge Nodes:"
    echo "  Node 0:          $BRIDGE_NODE_0"
    echo "  Node 1:          $BRIDGE_NODE_1"
    echo "  Node 2:          $BRIDGE_NODE_2"
    echo "  Node 3:          $BRIDGE_NODE_3"
    echo "  Node 4:          $BRIDGE_NODE_4"
    echo ""
    if [ -f "$DEPLOYMENTS_PATH" ]; then
        echo "WARNING: Existing deployment at $DEPLOYMENTS_PATH will be backed up"
    fi
    echo ""
    echo "Actions that would be performed:"
    echo "  1. Build contracts with forge"
    echo "  2. Deploy Nock token"
    echo "  3. Deploy MessageInbox implementation"
    echo "  4. Deploy ERC1967Proxy with MessageInbox"
    echo "  5. Link Nock to MessageInbox proxy"
    echo "  6. Write deployment to $DEPLOYMENTS_PATH"
    if [ -n "${TENDERLY_ACCESS_KEY:-}" ]; then
        echo "  7. Verify contracts on Tenderly (using forge verify-contract)"
    else
        echo "  7. Skip verification (TENDERLY_ACCESS_KEY not set)"
    fi
    echo ""
    echo "Run without --dry-run to execute."
    exit 0
fi

# Backup existing deployment if it exists
if [ -f "$DEPLOYMENTS_PATH" ]; then
    HISTORY_DIR="$CONTRACTS_DIR/deployments/history/${DEPLOY_TARGET_NETWORK}"
    mkdir -p "$HISTORY_DIR"
    TIMESTAMP=$(date +%Y%m%d-%H%M%S)
    BACKUP_PATH="$HISTORY_DIR/${TIMESTAMP}.json"
    cp "$DEPLOYMENTS_PATH" "$BACKUP_PATH"
    echo "Backed up existing deployment to: $BACKUP_PATH"
fi

forge build --force || {
    echo "Error: Build failed" >&2
    exit 1
}
FORGE_CMD=(
    forge script forge/Deploy.s.sol:Deploy
    --rpc-url "$TENDERLY_RPC_URL"
    --private-key "$TENDERLY_PRIVATE_KEY"
    --broadcast
    --slow
)
if [ ${#EXTRA_ARGS[@]} -gt 0 ]; then
    FORGE_CMD+=("${EXTRA_ARGS[@]}")
fi
"${FORGE_CMD[@]}"

if [ ! -f "$DEPLOYMENTS_PATH" ]; then
    echo "Error: Deployment file not created at $DEPLOYMENTS_PATH" >&2
    exit 1
fi

# Verify contracts if access key is available
if [ -n "${TENDERLY_ACCESS_KEY:-}" ]; then
    echo ""
    echo "Verifying contracts on Tenderly..."
    "$SCRIPT_DIR/verify_contracts.sh" "$DEPLOYMENTS_PATH" || {
        echo "Warning: Contract verification failed. Run 'make verify' to retry." >&2
    }
else
    echo ""
    echo "Skipping verification: TENDERLY_ACCESS_KEY not set"
    echo "To verify later, set TENDERLY_ACCESS_KEY and run: make verify"
fi
