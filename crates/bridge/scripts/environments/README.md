# Bridge Script Environments

Status: Active
Owner: Nockchain Maintainers
Last Reviewed: 2026-02-20
Canonical/Legacy: Legacy (profile and variable map for `crates/bridge/scripts/*`)

Use this guide to select and load environment profiles consumed by bridge helper scripts.

## Scope

- In scope: profile files under `scripts/environments`, variable wiring used by script entrypoints.
- Out of scope: contract deployment internals, full operator provisioning, and incident response.

## Profile Setup

```bash
cd crates/bridge/scripts

# Virtual Testnet profile
cp environments/virtual-testnet.env.example environments/virtual-testnet.env
source environments/virtual-testnet.env

# Base Sepolia profile
cp environments/base-sepolia.env.example environments/base-sepolia.env
source environments/base-sepolia.env
```

Load one profile per shell session to keep script behavior deterministic.

## Profile Files

| File                                        | Purpose                                                                |
| ------------------------------------------- | ---------------------------------------------------------------------- |
| `environments/virtual-testnet.env.example`  | Single-node helper defaults for virtual testnet runs                   |
| `environments/base-sepolia.env.example`     | Base Sepolia script profile (expects `BASE_SEPOLIA_*` values in shell) |
| `environments/test-bridge-keys.env.example` | Deterministic local key/address bundle for test workflows              |

Security warning: resolved `.env` files can contain secrets; do not commit them.

## Variables By Script

### `run-node-and-bridge.sh`

Reads:
- `BRIDGE_ENV`
- `BASE_WS_URL`
- `INBOX_CONTRACT_ADDRESS`
- `NOCK_CONTRACT_ADDRESS`
- `BRIDGE_ETH_KEY`
- `BRIDGE_NOCK_KEY`

### `run-bridge-only.sh`

Reads:
- `BRIDGE_ENV`
- `BASE_WS_URL`
- `INBOX_CONTRACT_ADDRESS`
- `NOCK_CONTRACT_ADDRESS`
- `BRIDGE_ETH_KEY`
- `BRIDGE_NOCK_KEY`

The script also requires a bridge ETH address in generated config and currently uses its built-in default when unset.

### `multi-bridge.sh`

`virtual-testnet` mode reads:
- `BRIDGE_ENV` (set to `virtual-testnet`)
- `BASE_WS_URL`
- `INBOX_CONTRACT_ADDRESS`
- `NOCK_CONTRACT_ADDRESS`
- `BRIDGE_NODE_KEY_0..4` (optional overrides; script has defaults)

`base-sepolia` mode reads:
- `BRIDGE_ENV` (set to `base-sepolia`)
- `BASE_SEPOLIA_WS_URL`
- `BASE_SEPOLIA_INBOX_PROXY`
- `BASE_SEPOLIA_NOCK`
- `BASE_SEPOLIA_BRIDGE_NODE_KEY_0..4`
- `BASE_SEPOLIA_BRIDGE_NODE_ADDR_0..4`

### `fund-bridge-nodes.sh`

Requires:
- `BASE_SEPOLIA_RPC_URL`
- `BASE_SEPOLIA_DEPLOYER_KEY`

Optional overrides:
- `BASE_SEPOLIA_BRIDGE_NODE_ADDR_0..4`
- `AMOUNT` (wei sent per node)

## Base Sepolia Live Reference

| Contract             | Address                                      |
| -------------------- | -------------------------------------------- |
| MessageInbox (Proxy) | `0x9b1becA13c39b9Be10dB616F1bE10C3CeF9Dfb36` |
| Nock Token           | `0xA9cd4087D9B050D8B35727AAf810296CA957c7B3` |

- Basescan: https://sepolia.basescan.org/address/0x9b1becA13c39b9Be10dB616F1bE10C3CeF9Dfb36
- Deployment detail: [`../../contracts/environments/base-sepolia-testnet-accounts.md`](../../contracts/environments/base-sepolia-testnet-accounts.md)

## URL Reference

Virtual testnet:

```text
wss://virtual.base-sepolia.<region>.rpc.tenderly.co/<virtual-testnet-id>
```

Base Sepolia node RPC:

```text
wss://base-sepolia.gateway.tenderly.co/<access-key>
```

Access key source: Tenderly Dashboard -> Node RPCs.

## Handoff

- Next for bootstrap flow: [`../../QUICKSTART.md`](../../QUICKSTART.md)
- Next for operator provisioning: [`../../OPERATOR-SETUP.md`](../../OPERATOR-SETUP.md)
- Next for runtime operations/incidents: [`../../docs/node-runbook.md`](../../docs/node-runbook.md)
