# Base Sepolia Deployment Reference

Status: Active
Owner: Nockchain Maintainers
Last Reviewed: 2026-08-25
Canonical/Legacy: Legacy (operational reference for Base Sepolia bridge contracts and script accounts)

Bridge contracts deployed to real Base Sepolia network through Tenderly Node RPC.

## Scope

- In scope: live contract addresses, account/env wiring used by current deploy/funding scripts.
- Out of scope: full deployment procedure, incident response, and routine operator runtime.

For full deployment flow, see [`../DEPLOYMENT.md`](../DEPLOYMENT.md).

## Funding

The test deployer account used by contracts deployment (`TENDERLY_TEST_PRIVATE_KEY`) needs Base Sepolia ETH.

Faucets:
1. Alchemy: https://www.alchemy.com/faucets/base-sepolia
2. QuickNode: https://faucet.quicknode.com/base/sepolia
3. GetBlock: https://getblock.io/faucet/base-sepolia/

If your environment exports `BASE_SEPOLIA_DEPLOYER_ADDRESS`, quick balance check:

```bash
cast balance "$BASE_SEPOLIA_DEPLOYER_ADDRESS" --rpc-url "$BASE_SEPOLIA_RPC_URL"
```

## Environment Variables By Consumer

### Contracts deployment (`crates/bridge/contracts/scripts/deploy_tenderly.sh`)

Required:
- `TENDERLY_RPC_URL`
- `TENDERLY_TEST_PRIVATE_KEY`
- `NOCK_NAME`
- `NOCK_SYMBOL`
- `BRIDGE_NODE_0`
- `BRIDGE_NODE_1`
- `BRIDGE_NODE_2`
- `BRIDGE_NODE_3`
- `BRIDGE_NODE_4`

Common optional:
- `DEPLOY_TARGET_NETWORK` (typically `base-sepolia`)
- `DEPLOYER_ADDRESS`
- `TENDERLY_ACCESS_KEY` (for verification)

### Bridge helper scripts (`crates/bridge/scripts/*.sh`)

Common Base Sepolia inputs:
- `BASE_SEPOLIA_RPC_URL`
- `BASE_SEPOLIA_WS_URL`
- `BASE_SEPOLIA_DEPLOYER_KEY`
- `BASE_SEPOLIA_DEPLOYER_ADDRESS`
- `BASE_SEPOLIA_BRIDGE_NODE_ADDR_0..4`
- `BASE_SEPOLIA_BRIDGE_NODE_KEY_0..4`

`scripts/fund-bridge-nodes.sh` requires `BASE_SEPOLIA_RPC_URL` and `BASE_SEPOLIA_DEPLOYER_KEY`.

## Deploy (Contracts)

```bash
cd crates/bridge/contracts
cp environments/base-sepolia.example .env
# edit .env values
make deploy
```

`make deploy` auto-loads `.env` if present.

## Fund Bridge Nodes (Scripts)

```bash
cd crates/bridge
./scripts/fund-bridge-nodes.sh
```

## Live Deployment (2025-12-17)

The canonical public deployment identity is
[`../../e2e/environments/base-sepolia.json`](../../e2e/environments/base-sepolia.json).
It records the pinned finalized block, proxy and implementation distinction,
runtime code hashes, deployment receipts, pristine ownership and signer state,
reciprocal pairing, verified compiler artifacts, ABI hashes, and withdrawal
protocol/policy identifiers.

Read the current values without duplicating them:

```bash
MANIFEST=../../e2e/environments/base-sepolia.json
jq '.source_chain.fork_block' "$MANIFEST"
jq '.contracts' "$MANIFEST"
jq '.pristine_state' "$MANIFEST"
```

## Refresh Procedure

1. Select a Base Sepolia block reported as `finalized`; never certify `latest`.
2. Record the exact block number, hash, and timestamp.
3. At that block, read the ERC-1967 implementation slot, all runtime bytecode,
   both owners, five bridge nodes, threshold, withdrawal gate, and reciprocal
   contract pairing.
4. Repeat every read through a second independent RPC provider. Stop on any
   disagreement or empty runtime bytecode.
5. Compute Keccak-256 over the exact runtime bytecode returned for the proxy,
   implementation, and Nock token.
6. Confirm deployment transaction hashes, block numbers, block hashes, status,
   and created addresses from chain receipts and an independent explorer.
7. Fetch the explorer-verified compiler artifacts and ABIs. Recompute hashes
   using the manifest's declared canonicalization and SHA-256 scheme.
8. Update all manifest facts together, run
   `cargo test -p bridge --test base_sepolia_manifest`, and review every block,
   address, state, artifact, ABI, and code-hash change before committing.

RPC endpoints and credentials remain external inputs and must not be added to
the manifest.

## Verification

Preferred contract validation path:

```bash
cd crates/bridge/contracts
make verify DEPLOYMENTS_PATH=deployments/base-sepolia.json
```
