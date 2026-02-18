+++
version = "0.1.8"
status = "superseded"
consensus_critical = true

activation_height = 37350
published = "2025-10-15"
activation_target = ""

authors = ["@nockchain-core"]
reviewers = ["@nockchain-core"]

supersedes = "0.1.7"
superseded_by = "0.1.9"
+++

# Legacy 009 - Segwit Cutover Initial

Initial v0/v1 consensus split with a planned phase boundary at `37350`.

## Summary

This upgrade introduced a dual-engine transaction model and an explicit height-gated cutover from v0 to v1 transaction and coinbase rules. It added v1 witness/note semantics while preserving a deterministic bridge path from v0 notes.

## Motivation

The pre-upgrade engine mixed legacy assumptions (sig-keyed coinbase, v0 note model, older spend semantics) with emerging segwit-style requirements. The protocol needed explicit versioning and a hard cutover height so every node evaluated the same rule set at the same height.

## Technical Specification

### Engine and Type Split

`open/hoon/common/tx-engine.hoon` was refactored into a versioned facade over:

- `tx-engine-0` (legacy rules)
- `tx-engine-1` (new rules)

Consensus-visible types became tagged unions, including:

- `page`
- `coinbase-split`
- `raw-tx`
- `tx`
- output/input forms

### Activation Constant

`tx-engine-1` introduced `v1-phase` in blockchain constants with default:

- `v1-phase = 37350`

### Height-Gated Rule Matrix

At `height < v1-phase`:

- v0 raw transactions are allowed.
- v1 raw transactions are rejected (`%v1-tx-before-activation`).
- v0 coinbase format is required.

At `height >= v1-phase`:

- v1 raw transactions are required.
- v0 raw transactions are rejected (`%v0-tx-after-cutoff`).
- v1 coinbase format is required.

### v1 Transaction Semantics Added

The v1 path introduced:

- `spend-0` bridge spends (v0 note -> v1 output)
- `spend-1` native v1 spends with witness checks (`check-context`, lock-merkle-proof validation)
- note-data limits per seed
- fee floor based on encoded word counts with:
  - `base-fee = 2^15`
  - `data.min-fee = 256`
- v1 coinbase split keyed by lock hash roots rather than legacy signature keys

### State Migration

Runtime/kernel state was upgraded to v6 to carry versioned consensus objects and preserve legacy entries as tagged v0 values.

Semantic delta:

- Before: one implicit transaction/coinbase regime.
- After: two explicit regimes with deterministic height-gated selection.

## Activation

- **Height**: `37350` (initial plan)
- **Coordination**: Full node, miner, and tx-builder rollout required.

## Migration

### Requirements

- Software version: commit `985350734` or newer

### Configuration

`v1-phase` had to be uniform across the network.

### Data Migration

Persistent node state was upgraded to kernel-state v6 with version-tagged transaction/page data.

### Steps

1. Upgrade nodes and miners.
2. Verify `v1-phase` value and v0/v1 rule matrix in staging.
3. Confirm wallet/builders can construct v1 spends and witnesses.

### Rollback

Rollback during cutover windows can produce deterministic version-rule splits.

## Backward Compatibility

Not backward compatible across the activation boundary.

## Security Considerations

This removed version ambiguity by making transaction-family selection explicit and height-bound.

## Operational Impact

Runbooks had to track `v1-phase` precisely and coordinate across nodes, miners, and transaction producers.

## Testing and Validation

- Validate v0/v1 transaction acceptance around the phase boundary.
- Validate v0/v1 coinbase acceptance around the phase boundary.
- Validate v0-to-v1 bridge spends and v1 witness checks.

## Reference Implementation

- Commit: `985350734`
- Files:
  - `open/hoon/common/tx-engine.hoon`
  - `open/hoon/common/tx-engine-0.hoon`
  - `open/hoon/common/tx-engine-1.hoon`
  - `open/hoon/apps/dumbnet/lib/consensus.hoon`
  - `open/hoon/apps/dumbnet/lib/types.hoon`
  - `open/hoon/apps/dumbnet/inner.hoon`
