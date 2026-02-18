+++
version = "0.1.10"
status = "activated"
consensus_critical = true

activation_height = 0
published = "2025-11-07"
activation_target = ""

authors = ["@nockchain-core"]
reviewers = ["@nockchain-core"]

supersedes = "0.1.9"
superseded_by = "0.1.11"
+++

# Legacy 011 - LMP Axis Hotfix

Temporary witness lock-merkle-proof restriction: `axis` must be `1`.

## Summary

This hotfix narrowed witness lock-merkle-proof acceptance so only `axis = 1` remained valid. It also restored the legacy placeholder hash behavior in the witness hash path to prevent accidental commitment semantics that were not consistently enforced.

## Motivation

The v1 witness path was not yet ready to safely support arbitrary axis commitments as a consensus contract. Leaving multiple axis values admissible created semantic risk: different builders and nodes could reason about witness commitment scope differently. The safe short-term move was to freeze axis behavior until a fully versioned commitment scheme shipped.

## Technical Specification

`open/hoon/common/tx-engine-1.hoon` changed `lock-merkle-proof` hashing/checking:

- In `hashable`, replaced `leaf+axis` with fixed hash constant:
  - `6mhCSwJQDvbkbiPAUNjetJtVoo1VLtEhmEYoU4hmdGd6ep1F6ayaV4A`
- In `check`, added a hard guard:
  - if `axis != 1`, reject immediately (and log).

Semantic delta:

- Before: witness path accepted non-`1` axis values.
- After: consensus rejects every non-`1` axis witness path.

This was an intentionally restrictive compatibility patch, not a final witness-commitment design.

## Activation

- **Height**: `0` (rollout-gated)
- **Coordination**: Nodes and transaction builders needed synchronized deployment.

## Migration

### Requirements

- Software version: commit `cc17b1871` or newer

### Configuration

None.

### Data Migration

None.

### Steps

1. Upgrade nodes and builders.
2. Ensure all newly produced witness proofs use `axis = 1`.

### Rollback

Mixed deployments can disagree on validity of witness proofs carrying non-`1` axis values.

## Backward Compatibility

Not backward compatible for transactions using `axis != 1` witness paths.

## Security Considerations

Reduces under-committed witness surface by collapsing accepted axis space until explicit versioned commitments are available.

## Operational Impact

Wallet/builder software had to enforce `axis = 1` as a hard production rule.

## Testing and Validation

- Confirm non-`1` axis witnesses are rejected.
- Confirm axis guard triggers deterministically before deeper spend checks.

## Reference Implementation

- Commit: `cc17b1871`
- Files:
  - `open/hoon/common/tx-engine-1.hoon`
