# MessageInbox Upgrade Guide

This guide covers upgrading `MessageInbox` behind the UUPS proxy.

## Pre-Upgrade Checklist

- [ ] New implementation tested (`make test`)
- [ ] Storage layout compatibility reviewed (`forge inspect MessageInbox storage-layout`)
- [ ] Owner key available (`INBOX_PRIVATE_KEY`)
- [ ] Correct deployment manifest selected (`DEPLOYMENTS_PATH`)
- [ ] Rollback plan prepared

## Fork Rehearsal

```bash
cd crates/bridge/bridge/contracts

# 1) Spawn fork and set env
tenderly fork spawn --network base-sepolia --project bridge-contracts
export TENDERLY_RPC_URL="<fork-rpc-url>"
export INBOX_PRIVATE_KEY="<owner-key>"
export DEPLOYMENTS_PATH="deployments/fork-test.json"

# 2) Execute upgrade (interactive prompt expects: yes)
make upgrade

# 3) Validate and run integration script
make validate DEPLOYMENTS_PATH=deployments/fork-test.json
make integration-test DEPLOYMENTS_PATH=deployments/fork-test.json
```

## Production Upgrade

```bash
cd crates/bridge/bridge/contracts
set -a; . ./.env; set +a
export DEPLOYMENTS_PATH="deployments/base-sepolia.json"
make upgrade
```

Notes:

- `make upgrade` runs `scripts/upgrade_tenderly.sh`, which prompts `Upgrade MessageInbox? (yes/no):`.
- The upgrade script deploys a new implementation and calls `upgradeTo(newImplementation)` on proxy.
- The script does **not** rewrite `messageInboxImplementation` in your deployment JSON. Update the manifest before `make validate`, or validation will fail with proxy mismatch.

## Post-Upgrade Validation

```bash
make validate DEPLOYMENTS_PATH=deployments/base-sepolia.json
make integration-test DEPLOYMENTS_PATH=deployments/base-sepolia.json
```

Confirm:

- Proxy implementation matches deployment manifest.
- Bridge node set remains intact.
- `nock.inbox()` still equals proxy.
- Ownership and withdrawals state are as expected.

## Emergency Rollback

If the new implementation is bad:

1. Optionally pause withdrawals:
```bash
make emergency-disable DEPLOYMENTS_PATH=deployments/base-sepolia.json
```
2. Roll proxy to previous implementation address:
```bash
export PROXY_ADDRESS="<messageInboxProxy>"
export OLD_IMPL_ADDRESS="<previous-implementation>"
cast send "$PROXY_ADDRESS" "upgradeTo(address)" "$OLD_IMPL_ADDRESS" \
  --rpc-url "$TENDERLY_RPC_URL" \
  --private-key "$INBOX_PRIVATE_KEY"
```
3. Update deployment manifest `messageInboxImplementation` to `OLD_IMPL_ADDRESS`.
4. Re-run validation/integration checks.
5. Re-enable withdrawals if paused:
```bash
make emergency-enable DEPLOYMENTS_PATH=deployments/base-sepolia.json
```

## Storage Layout Notes

Current custom storage layout in `MessageInbox.sol`:

- Slots `0..4`: `bridgeNodes` (`address[5]`)
- Slot `5`: `processedDeposits` mapping base slot
- Slot `6`: `lastDepositNonce` (`uint256`)
- Slot `7`: packed `withdrawalsEnabled` (`bool`) + `nock` (`address`)

Rules:

- Never reorder or delete existing storage variables.
- Append new variables at the end.
- Re-run `forge inspect MessageInbox storage-layout` before and after every storage change.

## Versioning

`VERSION` is a human-readable implementation tag in `MessageInbox`.  
Bump it when publishing a new implementation.
