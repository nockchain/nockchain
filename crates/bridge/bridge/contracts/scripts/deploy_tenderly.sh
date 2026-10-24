#!/usr/bin/env bash
set -euo pipefail

# Deploy bridge contracts to Tenderly network
# Usage: ./scripts/deploy_tenderly.sh [--dry-run] [EXTRA_FLAGS]

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONTRACTS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$CONTRACTS_DIR"

log() { echo "[deploy_tenderly] $*"; }
warn() { echo "[deploy_tenderly] WARN: $*" >&2; }

run_probe() {
    local label="$1"
    shift
    local output
    if output="$("$@" 2>&1)"; then
        log "$label: $output"
        return 0
    fi
    warn "$label failed: $output"
    return 1
}

capture_latest_nonce() {
    local addr="$1"
    cast nonce "$addr" --rpc-url "$TENDERLY_RPC_URL" 2>/dev/null
}

capture_pending_nonce() {
    local addr="$1"
    cast rpc --rpc-url "$TENDERLY_RPC_URL" eth_getTransactionCount "$addr" pending 2>/dev/null
}

print_rpc_diagnostics() {
    local stage="$1"
    local addr="$2"
    log "RPC diagnostics ($stage)"
    run_probe "chain-id" cast chain-id --rpc-url "$TENDERLY_RPC_URL" || true
    run_probe "latest-block" cast rpc --rpc-url "$TENDERLY_RPC_URL" eth_getBlockByNumber latest false || true
    run_probe "deployer-balance" cast balance "$addr" --rpc-url "$TENDERLY_RPC_URL" || true
    run_probe "deployer-latest-nonce" capture_latest_nonce "$addr" || true
    run_probe "deployer-pending-nonce" capture_pending_nonce "$addr" || true
}

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
    "TENDERLY_TEST_PRIVATE_KEY"
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

command -v cast >/dev/null 2>&1 || {
    echo "Error: cast is required for deployment diagnostics" >&2
    exit 1
}

ACTUAL_DEPLOYER_ADDRESS="$(cast wallet address --private-key "$TENDERLY_TEST_PRIVATE_KEY")"
if [ "$DEPLOYER_ADDRESS" = "0x0000000000000000000000000000000000000000" ]; then
    export DEPLOYER_ADDRESS="$ACTUAL_DEPLOYER_ADDRESS"
fi

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
    --private-key "$TENDERLY_TEST_PRIVATE_KEY"
    --broadcast
    --slow
    --skip-simulation
    --timeout 120
)
if [ ${#EXTRA_ARGS[@]} -gt 0 ]; then
    FORGE_CMD+=("${EXTRA_ARGS[@]}")
fi

log "Deployer: $ACTUAL_DEPLOYER_ADDRESS"
log "RPC URL: $TENDERLY_RPC_URL"

LATEST_NONCE_BEFORE="$(capture_latest_nonce "$ACTUAL_DEPLOYER_ADDRESS" || true)"
PENDING_NONCE_BEFORE="$(capture_pending_nonce "$ACTUAL_DEPLOYER_ADDRESS" || true)"
print_rpc_diagnostics "before forge script" "$ACTUAL_DEPLOYER_ADDRESS"

set +e
"${FORGE_CMD[@]}"
FORGE_STATUS=$?
set -e

if [ "$FORGE_STATUS" -ne 0 ]; then
    LATEST_NONCE_AFTER="$(capture_latest_nonce "$ACTUAL_DEPLOYER_ADDRESS" || true)"
    PENDING_NONCE_AFTER="$(capture_pending_nonce "$ACTUAL_DEPLOYER_ADDRESS" || true)"
    print_rpc_diagnostics "after failed forge script" "$ACTUAL_DEPLOYER_ADDRESS"
    if [ -n "$LATEST_NONCE_BEFORE" ] && [ -n "$LATEST_NONCE_AFTER" ]; then
        log "latest nonce before forge:  $LATEST_NONCE_BEFORE"
        log "latest nonce after forge:   $LATEST_NONCE_AFTER"
    fi
    if [ -n "$PENDING_NONCE_BEFORE" ] && [ -n "$PENDING_NONCE_AFTER" ]; then
        log "pending nonce before forge: $PENDING_NONCE_BEFORE"
        log "pending nonce after forge:  $PENDING_NONCE_AFTER"
    fi
    if [ -n "$PENDING_NONCE_BEFORE" ] && [ -n "$PENDING_NONCE_AFTER" ] && [ "$PENDING_NONCE_BEFORE" != "$PENDING_NONCE_AFTER" ]; then
        warn "Pending nonce changed during forge run; at least one deploy transaction was likely broadcast."
    elif [ -n "$LATEST_NONCE_BEFORE" ] && [ -n "$LATEST_NONCE_AFTER" ] && [ "$LATEST_NONCE_BEFORE" != "$LATEST_NONCE_AFTER" ]; then
        warn "Latest nonce changed during forge run; at least one deploy transaction was accepted onchain."
    else
        warn "No nonce change detected during forge run; forge likely failed before any deploy transaction was accepted."
    fi
    exit "$FORGE_STATUS"
fi

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
