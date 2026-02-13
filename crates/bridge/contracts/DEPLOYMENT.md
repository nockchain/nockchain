# Bridge Contract Deployment Guide

This guide covers deploying, upgrading, and managing the bridge contracts
(`MessageInbox` behind an ERC-1967 proxy plus the `Nock` ERC-20) on Tenderly
networks (devnets, simulations, or proxied mainnets).

## Quick Start

For a quick deployment to a new Tenderly devnet:

```bash
cd open/crates/bridge/contracts

# 1. Install dependencies
make install

# 2. Configure environment
cp .env.template .env
# Edit .env with your values

# 3. Spawn Tenderly devnet
tenderly devnet spawn --network base-sepolia --project bridge-contracts

# 4. Update .env with RPC URL from above

# 5. Deploy
make deploy
```

## Table of Contents

1. [Prerequisites](#1-prerequisites)
2. [Environment Configuration](#2-environment-configuration)
3. [Deployment](#3-deployment)
4. [Post-Deployment Validation](#4-post-deployment-validation)
5. [Upgrading Contracts](#5-upgrading-contracts)
6. [Operational Procedures](#6-operational-procedures)
7. [Multi-Network Deployment Tracking](#7-multi-network-deployment-tracking)
8. [Testing](#8-testing)
9. [Troubleshooting](#9-troubleshooting)

## 1. Prerequisites

### Install Tools

```bash
cd open/crates/bridge/contracts
make install
```

This installs:

- Foundry (forge, cast)
- Tenderly CLI
- Contract dependencies (forge-std, OpenZeppelin)

### Authenticate Tenderly

One-time setup per machine:

```bash
tenderly login
tenderly project link littel_wolfur/bridge-contracts
```

## 2. Environment Configuration

### Option A: Use .env file (Recommended)

1. Copy the template:

   ```bash
   cp .env.template .env
   ```

2. Edit `.env` with your values (see `.env.template` for all options)

3. The Makefile automatically sources `.env` for all commands

### Option B: Use environment files

Pre-configured examples are in `environments/`:

```bash
source environments/devnet.example
# or
source environments/base-sepolia.example
```

### Option C: Export variables manually

```bash
export TENDERLY_RPC_URL="<rpc-url>"
export TENDERLY_PRIVATE_KEY="0x..."
export BRIDGE_NODE_0="0x..."
# ... etc
```

### Required Variables

- `TENDERLY_RPC_URL` - RPC endpoint from Tenderly
- `TENDERLY_PRIVATE_KEY` - Deployer account private key (hex, with 0x)
- `BRIDGE_NODE_0` through `BRIDGE_NODE_4` - Five bridge node addresses
- `NOCK_NAME` - Token name (e.g., "Nock")
- `NOCK_SYMBOL` - Token symbol (e.g., "NOCK")

### Optional Variables

- `DEPLOY_TARGET_NETWORK` - Network identifier (default: "tenderly-devnet")
- `DEPLOYER_ADDRESS` - Deployer address for metadata (default: zero address)
- `DEPLOYMENTS_PATH` - Custom deployment file path
- `TENDERLY_ACCOUNT_ID` - For automatic verification
- `TENDERLY_PROJECT_SLUG` - For automatic verification
- `INBOX_PRIVATE_KEY` - Owner key for upgrades/admin operations
- `TEST_ACCOUNT_PRIVATE_KEY` - For integration tests
- `BRIDGE_NODE_KEY_*` - Bridge node private keys for testing

## 3. Deployment

### Spawn or Select Tenderly Network

For a devnet fork:

```bash
tenderly devnet spawn --network base-sepolia --project bridge-contracts
```

Copy the returned `rpc_url`. For live networks proxied through Tenderly, use
the RPC URL shown in the Tenderly dashboard.

### Fund Test Accounts (Devnets Only)

Tenderly devnets start with zero balances. Before running integration tests,
fund the bridge node and test accounts using the `tenderly_setBalance` RPC:

```bash
# Fund bridge node 0 and test account with 10 ETH each
curl -X POST "$TENDERLY_RPC_URL" \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "method": "tenderly_setBalance",
    "params": [
      ["0xBridgeNode0Address", "0xTestAccountAddress"],
      "0x8AC7230489E80000"
    ],
    "id": 1
  }'
```

The hex value `0x8AC7230489E80000` equals 10 ETH (10 × 10^18 wei). You can
fund multiple addresses in a single call by adding them to the array.

**Note:** This is only needed for devnets. Mainnet forks inherit real balances.

### Preview Deployment (Dry Run)

Before deploying, preview what will happen:

```bash
make deploy EXTRA_FLAGS="--dry-run"
```

This shows:

- Network and RPC configuration
- Token configuration (name, symbol)
- All 5 bridge node addresses
- Deployment file path
- Whether an existing deployment will be backed up

### Deploy Contracts

```bash
make deploy
```

This performs:

1. Validates required environment variables
2. Builds contracts (`forge build`)
3. Deploys via `forge/Deploy.s.sol`
4. Saves addresses to `deployments/{network}.json`
5. Optionally verifies contracts on Tenderly

### Custom Deployment Path

```bash
export DEPLOYMENTS_PATH="deployments/my-custom-network.json"
make deploy
```

### Additional Forge Flags

```bash
make deploy EXTRA_FLAGS="--skip-simulation --gas-price 1000000"
```

### Manual Deployment

For full control, call forge directly:

```bash
  forge script forge/Deploy.s.sol:Deploy \
  --rpc-url "$TENDERLY_RPC_URL" \
  --private-key "$TENDERLY_PRIVATE_KEY" \
  --broadcast \
  --slow
```

## 4. Post-Deployment Validation

Always validate after deployment:

```bash
make validate
```

Or with custom path:

```bash
make validate DEPLOYMENTS_PATH=deployments/base-sepolia.json
```

The validation script checks:

- Proxy points to correct implementation
- Bridge nodes are configured
- Nock token connected to inbox
- Ownership is set
- Initial state is correct (withdrawals enabled, zero burns)

### Inspect Deployment

- Review `deployments/{network}.json` for addresses
- Open Tenderly dashboard to inspect transactions
- Check contract verification status

### Contract Verification

Contracts are automatically verified during deployment if `TENDERLY_ACCESS_KEY` is set.
To verify contracts manually (e.g., if deployment succeeded but verification failed):

```bash
make verify
```

Or with custom deployment path:

```bash
make verify DEPLOYMENTS_PATH=deployments/base-sepolia.json
```

**Required for verification:**

- `TENDERLY_ACCESS_KEY` - Get from [Tenderly Authorization](https://dashboard.tenderly.co/account/authorization) (API Keys tab)
- `TENDERLY_RPC_URL` - Same RPC URL used for deployment

The verification script verifies all three contracts:

- Nock token
- MessageInbox implementation
- ERC1967Proxy

**Note:** `cbor_metadata = true` must be set in `foundry.toml` for verification to succeed.
This is already configured in the project.

## 5. Upgrading Contracts

The MessageInbox uses UUPS (Universal Upgradeable Proxy Standard) for upgrades.

### Pre-Upgrade Checklist

- [ ] New implementation tested thoroughly
- [ ] Storage layout compatible (no reordering)
- [ ] Fork tests pass
- [ ] Gas usage acceptable (<200k for deposits)
- [ ] Owner key secure and accessible

See [UPGRADE_GUIDE.md](docs/UPGRADE_GUIDE.md) for detailed upgrade procedures.

### Quick Upgrade

```bash
# Set INBOX_PRIVATE_KEY in .env (must be owner)
make upgrade
```

### Upgrade Steps

1. **Test on fork** (recommended):

   ```bash
   tenderly fork spawn --network base-sepolia
   export TENDERLY_RPC_URL="<fork-rpc-url>"
   export DEPLOYMENTS_PATH="deployments/fork-test.json"
   make upgrade
   make validate
   make integration-test
   ```

2. **Upgrade production**:

   ```bash
   export DEPLOYMENTS_PATH="deployments/base-sepolia.json"
   make upgrade
   ```

3. **Validate upgrade**:
   ```bash
   make validate
   make integration-test
   ```

## 6. Operational Procedures

### List All Deployments

```bash
make list-deployments
```

Shows all deployment files in `deployments/` directory.

### Rotate Bridge Node

Update a bridge node address:

```bash
# Set all bridge node addresses (only target index changes)
export BRIDGE_NODE_ADDR_0="0x..."
export BRIDGE_NODE_ADDR_1="0x..."
# ... etc

# Rotate node at index 0
./scripts/rotate_bridge_node.sh 0 0xNewAddress deployments/base-sepolia.json
```

Or use the SetBridgeNodes script directly:

```bash
forge script forge/SetBridgeNodes.s.sol:SetBridgeNodes \
  --rpc-url "$TENDERLY_RPC_URL" \
  --private-key "$INBOX_PRIVATE_KEY" \
  --broadcast
```

### Emergency: Disable Withdrawals

If critical issues are detected:

```bash
make emergency-disable
```

This immediately disables withdrawals. Users cannot burn tokens until
re-enabled. Requires typing "DISABLE" to confirm.

### Emergency: Re-enable Withdrawals

After resolving the emergency:

```bash
make emergency-enable
```

Requires typing "ENABLE" to confirm.

### Transfer Ownership

Transfer ownership of MessageInbox to a new address (e.g., multisig):

```bash
make transfer-ownership NEW_OWNER=0x1234...abcd
```

Preview first with dry-run:

```bash
./scripts/transfer_ownership.sh --dry-run 0x1234...abcd
```

**Important:** This initiates a two-step transfer. The new owner must call
`acceptOwnership()` on the MessageInbox contract to complete the transfer.
Until then, the original owner retains control.

## 7. Multi-Network Deployment Tracking

Deployments are tracked per network in `deployments/{network}.json`.

### Default Behavior

- If `DEPLOYMENTS_PATH` is not set, defaults to `deployments/{DEPLOY_TARGET_NETWORK}.json`
- Each network gets its own file
- Legacy `deployments.json` is still supported

### Example Structure

```
deployments/
  ├── tenderly-devnet.json
  ├── base-sepolia.json
  ├── base-mainnet.json
  └── history/
      ├── tenderly-devnet/
      │   ├── 20241209-143500.json
      │   └── 20241209-150000.json
      └── base-sepolia/
          └── 20241210-120000.json
```

### Deployment Backup

Every deployment automatically backs up the previous deployment file before
overwriting. Backups are stored in `deployments/history/{network}/` with
timestamps.

To restore a previous deployment:

```bash
cp deployments/history/tenderly-devnet/20241209-143500.json deployments/tenderly-devnet.json
```

### List Deployments

```bash
make list-deployments
```

Shows all current deployment files.

To include deployment history:

```bash
make list-deployments HISTORY=1
```

This shows all current deployments plus historical backups with their proxy
addresses.

## 8. Testing

### Unit Tests

```bash
make test
```

Runs Foundry tests with gas reporting. Every bridge contract change must keep
deposit flow under 200k gas.

### Integration Tests

Test against deployed contracts:

```bash
# Set TEST_ACCOUNT_PRIVATE_KEY and BRIDGE_NODE_KEY_* in .env
make integration-test
```

Or with custom deployment:

```bash
export DEPLOYMENTS_PATH="deployments/base-sepolia.json"
make integration-test
```

**Note for devnets:** Bridge node and test accounts need ETH for gas. See
[Fund Test Accounts](#fund-test-accounts-devnets-only) above to fund them
using `tenderly_setBalance`.

The integration test:

- Tests full deposit + burn cycle
- Validates gas usage
- Verifies state changes

### Gas Baselines

Every bridge contract change must keep the deposit flow under 200k gas. Run
tests with gas report:

```bash
make test
```

The Foundry tests use `vm.pauseGasMetering()`/`vm.resumeGasMetering()` to
exclude signature fabrication overhead, ensuring measured gas approximates
on-chain execution. Treat any regression above 200k gas as a release blocker.

## 9. Troubleshooting

### Deployment Fails: Missing Variables

Error: `Missing required environment variables`

Solution: Check `.env` file or export variables. See `.env.template` for all
required variables.

### Validation Fails: Proxy Mismatch

Error: `Proxy implementation mismatch`

Solution: Verify the implementation address in deployments.json matches the
proxy's current implementation. Check Tenderly dashboard.

### Upgrade Fails: Not Owner

Error: Transaction reverts with access control error

Solution: Ensure `INBOX_PRIVATE_KEY` is set to the owner address. Check
ownership:

```bash
cast call $PROXY_ADDRESS "owner()" --rpc-url "$TENDERLY_RPC_URL"
```

### Integration Test Fails: Signature Mismatch

Error: `Bridge node key mismatch`

Solution: Ensure `BRIDGE_NODE_KEY_*` variables match the addresses in
`BRIDGE_NODE_*`. The keys must correspond to the bridge node addresses.

### Script Not Found

Error: `./scripts/deploy_tenderly.sh: No such file`

Solution: Ensure you're in the `contracts/` directory and scripts are
executable:

```bash
chmod +x scripts/*.sh
```

### Deployment File Not Found

Error: `Deployment file not found`

Solution: Check `DEPLOYMENTS_PATH` is set correctly. Default is
`deployments/{DEPLOY_TARGET_NETWORK}.json`. Ensure the directory exists:

```bash
mkdir -p deployments
```

## Deployment Checklist

Before deploying to production:

- [ ] All tests pass (`make test`)
- [ ] Gas usage under 200k for deposits
- [ ] Environment variables configured
- [ ] Bridge node addresses verified (all 5 unique)
- [ ] Bridge node private keys match addresses (for testing)
- [ ] Owner key secure and backed up
- [ ] Deployment tested on fork/devnet
- [ ] Test accounts funded (devnets only)
- [ ] Post-deployment validation passes
- [ ] Integration tests pass
- [ ] Contracts verified on Tenderly
- [ ] Deployment addresses recorded

## Architecture Notes

- `MessageInbox` is deployed behind an `ERC1967Proxy` (UUPS upgradeable)
- `Nock` token is non-upgradeable for immutability
- Nock token is connected to inbox proxy via `updateInbox`
- All bridge node addresses must be provided at deployment
- Bridge nodes can be rotated via `updateBridgeNode` after deployment
- The deployment script is network-agnostic: works with any Tenderly RPC

## Manual Operations

### Direct Contract Calls

Use `cast` for manual operations:

```bash
# Check owner
cast call $PROXY_ADDRESS "owner()" --rpc-url "$TENDERLY_RPC_URL"

# Check bridge node
cast call $PROXY_ADDRESS "bridgeNodes(uint256)" 0 --rpc-url "$TENDERLY_RPC_URL"

# Update bridge node (requires owner)
cast send $PROXY_ADDRESS "updateBridgeNode(uint256,address)" 0 0xNewAddress \
  --rpc-url "$TENDERLY_RPC_URL" \
  --private-key "$INBOX_PRIVATE_KEY"
```

## Additional Resources

- [UPGRADE_GUIDE.md](docs/UPGRADE_GUIDE.md) - Detailed upgrade procedures
- [tenderly.env.example](tenderly.env.example) - Legacy env example
- [environments/](environments/) - Pre-configured environment examples
