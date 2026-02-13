# MessageInbox Upgrade Guide

This guide covers the process for upgrading the MessageInbox implementation via
UUPS proxy.

## Pre-Upgrade Checklist

Before upgrading, ensure:

- [ ] New implementation has been thoroughly tested
- [ ] Storage layout is compatible (no storage variable reordering)
- [ ] All critical functions work correctly in fork tests
- [ ] Gas usage remains acceptable (<200k for deposits)
- [ ] Owner private key is secure and accessible
- [ ] Backup of current deployment addresses

## Fork Testing Procedure

1. Fork the target network in Tenderly:
   ```bash
   tenderly fork spawn --network base-sepolia --project bridge-contracts
   ```

2. Deploy new implementation to fork:
   ```bash
   export TENDERLY_RPC_URL="<fork-rpc-url>"
   export INBOX_PRIVATE_KEY="<owner-key>"
   export DEPLOYMENTS_PATH="deployments/fork-test.json"

   # Deploy new implementation
   forge script forge/Upgrade.s.sol:Upgrade \
     --rpc-url "$TENDERLY_RPC_URL" \
     --private-key "$INBOX_PRIVATE_KEY" \
     --broadcast
   ```

3. Run validation:
   ```bash
   make validate DEPLOYMENTS_PATH=deployments/fork-test.json
   ```

4. Run integration tests:
   ```bash
   make integration-test DEPLOYMENTS_PATH=deployments/fork-test.json
   ```

5. Verify critical functions:
   - Deposit flow works correctly
   - Withdrawal flow works correctly
   - Bridge node updates work
   - Ownership is preserved

## Upgrade Execution Steps

1. **Prepare environment**:
   ```bash
   cd open/crates/bridge/contracts
   source .env  # or source environments/{network}.example
   ```

2. **Set deployment path** (if not using default):
   ```bash
   export DEPLOYMENTS_PATH="deployments/base-sepolia.json"
   ```

3. **Build contracts**:
   ```bash
   make build
   ```

4. **Execute upgrade**:
   ```bash
   make upgrade
   ```

   Or manually:
   ```bash
   ./scripts/upgrade_tenderly.sh deployments/base-sepolia.json
   ```

5. **Verify upgrade in Tenderly**:
   - Check transaction succeeded
   - Verify proxy implementation address updated
   - Review storage layout

6. **Run post-upgrade validation**:
   ```bash
   make validate
   ```

7. **Run integration tests**:
   ```bash
   make integration-test
   ```

## Post-Upgrade Validation

After upgrading, verify:

- [ ] Proxy points to new implementation
- [ ] All bridge nodes still configured correctly
- [ ] Nock token still connected to inbox
- [ ] Ownership unchanged
- [ ] Withdrawals still enabled (if expected)
- [ ] Deposit flow works end-to-end
- [ ] Withdrawal flow works end-to-end

Use the validation script:
```bash
make validate DEPLOYMENTS_PATH=deployments/{network}.json
```

## Emergency Rollback Procedure

If an upgrade introduces critical issues:

1. **Immediately disable withdrawals** (if needed):
   ```bash
   make emergency-disable DEPLOYMENTS_PATH=deployments/{network}.json
   ```

2. **Deploy previous implementation**:
   - Locate previous implementation address from deployment history
   - Deploy previous version code
   - Upgrade proxy back to previous implementation:
     ```bash
     # Set NEW_IMPL_ADDRESS to previous implementation
     forge script forge/Upgrade.s.sol:Upgrade \
       --rpc-url "$TENDERLY_RPC_URL" \
       --private-key "$INBOX_PRIVATE_KEY" \
       --broadcast \
       --sig "upgradeTo(address)" "$NEW_IMPL_ADDRESS"
     ```

3. **Verify rollback**:
   ```bash
   make validate
   make integration-test
   ```

4. **Re-enable withdrawals** (if disabled):
   ```bash
   make emergency-enable DEPLOYMENTS_PATH=deployments/{network}.json
   ```

## Storage Layout Compatibility

**CRITICAL**: Never reorder storage variables or change types in a way that
shifts storage slots. The UUPS pattern allows upgrades, but storage layout
must remain compatible.

### MessageInbox Storage Layout (v1.0.0)

The contract inherits from OpenZeppelin's upgradeable contracts which use
namespaced storage (EIP-7201). Our custom storage starts after the inherited
slots.

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ INHERITED STORAGE (OpenZeppelin Upgradeable Contracts)                       │
├──────────────────────────────────────────────────────────────────────────────┤
│ Initializable:       Uses EIP-7201 namespaced storage                        │
│ OwnableUpgradeable:  Uses EIP-7201 namespaced storage (owner address)        │
│ UUPSUpgradeable:     Uses EIP-7201 namespaced storage                        │
└──────────────────────────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────────────────────────┐
│ MESSAGEINBOX CUSTOM STORAGE                                                  │
├──────┬──────────────────────┬────────────────────────────┬───────────────────┤
│ Slot │ Variable             │ Type                       │ Size (bytes)      │
├──────┼──────────────────────┼────────────────────────────┼───────────────────┤
│ 0-4  │ bridgeNodes          │ address[5]                 │ 160 (5 slots)     │
│ 5    │ processedDeposits    │ mapping(bytes32 => bool)   │ 32 (slot only)    │
│ 6    │ withdrawalsEnabled   │ bool                       │ 1 (offset 0)      │
│ 6    │ nock                 │ address (Nock contract)    │ 20 (offset 1)     │
└──────┴──────────────────────┴────────────────────────────┴───────────────────┘
```

**Storage Notes:**
- `bridgeNodes` occupies slots 0-4 (5 addresses × 32 bytes/slot)
- `processedDeposits` mapping uses slot 5 as its base slot; actual values are
  stored at `keccak256(key . slot)`
- `withdrawalsEnabled` and `nock` are packed into slot 6 (1 byte + 20 bytes = 21 bytes)
- Constants (`VERSION`, `THRESHOLD`, `SECP256K1_N`, `SECP256K1_HALF_N`) do NOT
  use storage slots

**Regenerate this layout:**
```bash
forge inspect MessageInbox storage-layout
```

### Safe Upgrade Rules

When adding new storage variables:
- Add them at the end (after slot 7)
- Never remove or reorder existing variables
- Use storage gaps if removing variables
- New variables will start at slot 8

**Example of safe addition:**
```solidity
// SAFE: Adding at the end
uint256 public newVariable;  // Will be slot 8

// UNSAFE: Inserting between existing variables
// This would shift all subsequent slots and corrupt storage!
```

### Upgrade Checklist for Storage

- [ ] Run `forge inspect MessageInbox storage-layout` before and after changes
- [ ] Verify no existing slots have changed position
- [ ] Verify no types have changed in existing slots
- [ ] New variables are added at the end only
- [ ] Increment VERSION constant

## Version Tracking

The `VERSION` constant in MessageInbox tracks the implementation version.
Increment it when deploying a new implementation:

```solidity
string public constant VERSION = "1.1.0";  // Increment for new version
```

## Notes

- Upgrades are irreversible once executed (except by upgrading again)
- Always test upgrades on forks before production
- Keep deployment records for all upgrades
- Document any breaking changes or migration requirements
