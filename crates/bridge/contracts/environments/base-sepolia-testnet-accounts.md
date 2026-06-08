# Base Sepolia Deployment Reference

Status: Active
Owner: Nockchain Maintainers
Last Reviewed: 2026-02-20
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

| Contract                      | Address                                      | Basescan                                                                             |
| ----------------------------- | -------------------------------------------- | ------------------------------------------------------------------------------------ |
| MessageInbox (Proxy)          | `0x9b1becA13c39b9Be10dB616F1bE10C3CeF9Dfb36` | https://sepolia.basescan.org/address/0x9b1becA13c39b9Be10dB616F1bE10C3CeF9Dfb36#code |
| MessageInbox (Implementation) | `0x7627Db3A99596668c9d42693efa352Ca69F089e3` | https://sepolia.basescan.org/address/0x7627Db3A99596668c9d42693efa352Ca69F089e3#code |
| Nock Token                    | `0xA9cd4087D9B050D8B35727AAf810296CA957c7B3` | https://sepolia.basescan.org/address/0xA9cd4087D9B050D8B35727AAf810296CA957c7B3#code |

Tenderly dashboard: https://dashboard.tenderly.co/zorp/bridge/contracts

## Verification

Preferred path for this repo:

```bash
cd crates/bridge/contracts
make verify DEPLOYMENTS_PATH=deployments/base-sepolia.json
```
