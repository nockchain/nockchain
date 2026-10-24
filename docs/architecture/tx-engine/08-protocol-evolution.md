# Protocol Evolution: Upgrade Mechanics and History

## Upgrade Philosophy

Nockchain uses **height-gated hard cutovers** for consensus-critical upgrades. Unlike Bitcoin's BIP 9/BIP 8 miner-signaling soft fork mechanism, Nockchain upgrades activate at a predetermined block height with no signaling period. All nodes must upgrade before the activation height or risk forking.

This approach is documented in `docs/protocol/upgrades/SPECIFICATION.md` and reflected in the protocol changelog entries.

### Comparison: Bitcoin BIP Activation vs Nockchain Height-Gating

| Aspect | Bitcoin (BIP 9/8) | Nockchain |
|---|---|---|
| Activation trigger | Miner signaling threshold | Fixed block height |
| Signaling period | ~2 weeks per retarget | None |
| Backward compatibility | Soft fork (old nodes accept) | Hard cutover (old nodes may fork) |
| Coordination | Miners signal readiness | All operators must upgrade |
| Rollback | Possible before lock-in | Only safe before activation |
| Upgrade timeline | Months (signaling + lock-in) | Days to weeks (deploy before height) |

## Protocol Changelog

All consensus-critical changes are documented in `docs/protocol/upgrades/`, with a structured frontmatter format:

```toml
version = "0.1.11"
status = "final"            # draft | final | superseded
consensus_critical = true
activation_height = 54000
published = "2026-01-19"
activation_target = "2026-03-01"
```

## Version History

### V0 Genesis (Block 0)

The original transaction engine, defined entirely in `hoon/common/tx-engine-0.hoon` (~2471 lines).

**Characteristics:**
- Single engine, no version tagging
- Simple M-of-N multisig via `Lock` (keys_required + set of Schnorr pubkeys)
- Signature directly embedded in `Spend` structure
- Coinbase keyed by signer pubkeys
- Notes carry full lock, source, and timelock data
- No note-data (no eUTXO datum)

### Protocol 009: SegWit Cutover (Block 37350)

**File**: `docs/protocol/upgrades/009-legacy-segwit-cutover-initial.md`

The most significant upgrade — introduced the V0/V1 split. This is Nockchain's "SegWit moment."

**Changes:**
1. **Dual engine architecture**: `tx-engine.hoon` became a versioned facade dispatching to `tx-engine-0.hoon` or `tx-engine-1.hoon`
2. **Witness separation**: V1 `Spend1` carries a separate `Witness` struct
3. **Lock trees**: V1 replaces M-of-N locks with Merkle trees of `SpendCondition` branches
4. **Lock primitives**: `Pkh`, `Tim`, `Hax`, `Burn` replace the monolithic lock
5. **Note data**: V1 notes carry `NoteData` (eUTXO-inspired datum)
6. **Bridge spends**: `Spend0` allows V0 notes to transition to V1 outputs
7. **Coinbase format**: Changed from sig-keyed to lock-hash-rooted
8. **Fee floor**: Word-count-based pricing with `base-fee = 2^15`

**Height-gated rule matrix:**

| Block Height | V0 Tx | V1 Tx | Coinbase |
|---|---|---|---|
| `< 37350` | Allowed | Rejected | V0 format |
| `≥ 37350` | Rejected | Required | V1 format |

**State migration**: Kernel state upgraded to v6 to carry versioned consensus objects.

### Protocol 010: V1-Phase Finalization (Block 39000)

**File**: `docs/protocol/upgrades/010-legacy-v1-phase-39000.md`

Finalized the V1-phase boundary. The initial plan (Protocol 009) set `v1-phase = 37350`, and Protocol 010 confirmed the transition period ended cleanly at block 39000.

### Protocol 011: LMP Axis Hotfix

**File**: `docs/protocol/upgrades/011-legacy-lmp-axis-hotfix.md`

A targeted fix for a lock merkle proof issue related to axis handling — addressed a specific vulnerability or edge case before the Bythos comprehensive fix.

### Protocol 012: Bythos (Block 54000)

**File**: `docs/protocol/upgrades/012-bythos.md`

A consensus-critical upgrade addressing two issues discovered after the initial SegWit cutover.

**Lock Merkle Proof Versioning:**
- Problem: Stub proofs used a hardcoded placeholder instead of committing to the axis field, meaning the witness hash didn't bind to which branch was executed
- Solution: Introduced `lock-merkle-proof-full` with axis in the hashable
- Backward compatibility: Both stub and full formats accepted after activation; stub proofs only before activation
- Format gating by note origin page, not current height

**Fee Rebalancing:**
- Base fee halved: `2^15 → 2^14`
- Input/witness discount: `input-fee-divisor = 4` (inputs charged at 1/4 output rate)
- Separate seed/witness word counting
- Note-data size validation moved to per-output (after merge by lock-root)

**Context-Aware Mempool Admission:**
- V1 transactions now validated with `validate-with-context` at mempool receipt
- Transactions failing context checks (expired timelocks, invalid lock proofs, oversized note-data) dropped immediately

### Protocol 013: Nous (Draft, Non-Consensus)

**File**: `docs/protocol/upgrades/013-nous.md`

A networking-layer upgrade (not tx-engine). Adds batched transport requests for the libp2p request-response protocol. Included here for completeness — it does not change transaction formats or consensus rules.

## The Versioned Facade

The Hoon facade (`hoon/common/tx-engine.hoon`) is the authoritative dispatch layer:

```hoon
/=  v0  /common/tx-engine-0
/=  v1  /common/tx-engine-1
...
|_  blockchain-constants
+*  v0  ~(. ^v0 +63:+<)
```

It wraps types as tagged unions for consensus-visible structures:

```hoon
++  coinbase-split
  =<  form
  |%
  +$  form
    $%  [%0 coinbase-split:v0]
        [%1 coinbase-split:v1]
    ==
```

This pattern applies to `page`, `raw-tx`, `local-page`, `coinbase-split`, and other consensus types. The facade checks version tags and routes to the appropriate engine.

## Kernel State Migration

Protocol upgrades sometimes require kernel state migration. The kernel state carries the UTXO balance, chain tip, and other consensus data. When the state format changes:

1. The kernel state version is bumped (e.g., v5 → v6 for Protocol 009)
2. Old state is migrated to the new format on first load
3. Legacy entries are tagged with version markers (e.g., V0 notes in the balance)

The Hoon kernel manages this migration during node startup. The Rust networking layer is agnostic to the internal state format — it only handles serialized nouns for transport.

## Upgrade Coordination Model

Nockchain upgrades follow a predictable pattern:

1. **Protocol spec published** in `docs/protocol/upgrades/` with all technical details
2. **Software updated** to include new rules, gated by activation height
3. **Operators deploy** updated software before activation
4. **Activation height reached** — new rules take effect network-wide
5. **Old software** may fork or reject valid blocks

There is no soft-fork compatibility layer. This is a deliberate choice:
- Simpler reasoning about consensus rules at any given height
- No ambiguity about which rules apply
- No "anyone can spend" risks from soft fork semantics
- Clear upgrade timeline for operators

The trade-off is that upgrades require full network coordination. Late upgraders will fork.
