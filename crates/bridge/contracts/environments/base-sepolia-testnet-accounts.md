# Base Sepolia Testnet Deployment

Bridge contracts deployed to real Base Sepolia network via Tenderly gateway RPC.

## Funding Instructions

The **deployer account** needs Base Sepolia ETH to deploy contracts. Use one of these faucets:

1. **Alchemy** (recommended): https://www.alchemy.com/faucets/base-sepolia
2. **QuickNode**: https://faucet.quicknode.com/base/sepolia
3. **GetBlock**: https://getblock.io/faucet/base-sepolia/ (requires 0.005 mainnet ETH)

Fund the deployer address: `$BASE_SEPOLIA_DEPLOYER_ADDRESS` (see devenv.nix)

## Accounts

All private keys and addresses are stored in `devenv.nix` under the `BASE_SEPOLIA_*` prefix.

### Environment Variables (from devenv.nix)

| Variable                             | Description                |
| ------------------------------------ | -------------------------- |
| `BASE_SEPOLIA_DEPLOYER_ADDRESS`      | Deployer account address   |
| `BASE_SEPOLIA_DEPLOYER_KEY`          | Deployer private key       |
| `BASE_SEPOLIA_BRIDGE_NODE_ADDR_0..4` | Bridge node addresses      |
| `BASE_SEPOLIA_BRIDGE_NODE_KEY_0..4`  | Bridge node private keys   |
| `BASE_SEPOLIA_RPC_URL`               | Tenderly gateway HTTPS URL |
| `BASE_SEPOLIA_WS_URL`                | Tenderly gateway WSS URL   |

## Check Balance

```bash
cast balance $BASE_SEPOLIA_DEPLOYER_ADDRESS --rpc-url $BASE_SEPOLIA_RPC_URL
```

## Quick Deploy (after funding)

```bash
cd open/crates/bridge/contracts
source environments/base-sepolia.env
make deploy
```

## Deployed Contracts (2025-12-17)

| Contract             | Address                                      | Basescan                                                                                     |
| -------------------- | -------------------------------------------- | -------------------------------------------------------------------------------------------- |
| MessageInbox (Proxy) | `0x9b1becA13c39b9Be10dB616F1bE10C3CeF9Dfb36` | [View](https://sepolia.basescan.org/address/0x9b1becA13c39b9Be10dB616F1bE10C3CeF9Dfb36#code) |
| MessageInbox (Impl)  | `0x7627Db3A99596668c9d42693efa352Ca69F089e3` | [View](https://sepolia.basescan.org/address/0x7627Db3A99596668c9d42693efa352Ca69F089e3#code) |
| Nock Token           | `0xA9cd4087D9B050D8B35727AAf810296CA957c7B3` | [View](https://sepolia.basescan.org/address/0xA9cd4087D9B050D8B35727AAf810296CA957c7B3#code) |

**Tenderly Dashboard**: https://dashboard.tenderly.co/zorp/bridge/contracts

## Verification

Contracts are verified on Basescan using Etherscan API V2. To verify new deployments:

```bash
forge verify-contract <address> <contract> --chain base-sepolia --verifier etherscan --watch
```

Requires `ETHERSCAN_API_KEY` environment variable (set in devenv.nix).
