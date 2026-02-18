+++
version = "0.1.3"
status = "activated"
consensus_critical = true

activation_height = 0
published = "2025-06-09"
activation_target = ""

authors = ["@nockchain-core"]
reviewers = ["@nockchain-core"]

supersedes = "0.1.2"
superseded_by = "0.1.4"
+++

# Legacy 004 - Sign Output Source

Spend signatures now commit to `output-source`.

## Summary

This upgrade changed spend signature hashing so `seed.output-source` is part of the signed message. It closed a gap where source-routing semantics could change without invalidating a signature.

## Motivation

In v0 transaction structure, `output-source` affected output semantics, but the spend signature path did not include it. That meant authorization and execution semantics were partially decoupled. The fix was to bind `output-source` directly into the signature hash path.

## Technical Specification

`open/hoon/common/tx-engine.hoon` added and wired new signed hash paths:

- `hashable-unit:source` to encode `(unit source)` deterministically.
- `sig-hashable:seed` now includes:
  - `output-source`
  - recipient lock
  - timelock intent
  - gift amount
  - parent hash
- `sig-hashable:seeds` recursively applies `sig-hashable:seed`.
- `sig-hash:spend` now hashes `sig-hashable:seeds` (previously used `hashable:seeds`).

Semantic delta:

- Before: signature validity did not commit to `output-source`.
- After: any post-signing `output-source` change invalidates the signature.

## Activation

- **Height**: `0` (rollout-gated, no block-height gate)
- **Coordination**: Nodes, wallets, and transaction producers had to upgrade together.

## Migration

### Requirements

- Software version: commit `28a496501` or newer

### Configuration

None.

### Data Migration

None.

### Steps

1. Upgrade nodes and transaction builders.
2. Rebuild and re-sign any pending transactions produced before this change.

### Rollback

Mixed fleets can disagree on validity of transactions whose `output-source` differs from what was originally signed.

## Backward Compatibility

Not backward compatible for signatures produced under pre-upgrade hashing rules.

## Security Considerations

Closes a signature-commitment gap around output source constraints.

## Operational Impact

Pre-upgrade mempool transactions may fail verification after rollout and require re-signing.

## Testing and Validation

- Mutating `output-source` after signing causes signature failure.
- Transactions with unchanged signed fields still verify.

## Reference Implementation

- Commit: `28a496501`
- Files:
  - `open/hoon/common/tx-engine.hoon`
