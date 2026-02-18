+++
version = "0.1.2"
status = "activated"
consensus_critical = true

activation_height = 2448
published = "2025-06-09"
activation_target = ""

authors = ["@nockchain-core"]
reviewers = ["@nockchain-core"]

supersedes = "0.1.1"
superseded_by = "0.1.3"
+++

# Legacy 003 - Checkpoint 2448

Checkpoint map expansion at height `2448`.

## Summary

This upgrade added a new hard checkpoint digest for block `2448`. It kept prior pins (`0`, `144`, `720`) unchanged.

## Motivation

As the chain grew, keeping only early anchors left a longer unpinned segment. Adding `2448` reduced the acceptable reorg/search space during recovery and made state reconstruction more deterministic.

## Technical Specification

`checkpointed-digests` gained:

- `2448 -> 9EChUtcNJumW5DDYgS6UP5UHfHtD6vFH7HoSqjmTuWP2Px6JdpxaR23`

Validation logic remained identical to `0.1.0`/`0.1.1`:

- checkpointed heights must match pinned digests,
- failures reject with `%checkpoint-match-failed`.

Semantic delta:

- Before: highest enforced checkpoint at `720`.
- After: highest enforced checkpoint at `2448`.

## Activation

- **Height**: `2448`
- **Coordination**: Upgrade before processing `2448`.

## Migration

### Requirements

- Software version: commit `3b825759e` or newer

### Configuration

None.

### Data Migration

None.

### Steps

1. Upgrade node software.
2. Confirm checkpoint table includes `2448` and previous entries.

### Rollback

Rollback can admit a `2448` history rejected by upgraded peers.

## Backward Compatibility

Not backward compatible at checkpointed heights.

## Security Considerations

Adds another hard anchor, reducing ambiguity in long-range historical replay.

## Operational Impact

Consensus safety still depends on checkpoint-table parity across nodes.

## Testing and Validation

- Canonical `2448` digest passes.
- Altered `2448` digest fails with `%checkpoint-match-failed`.

## Reference Implementation

- Commit: `3b825759e`
- Files:
  - `open/hoon/apps/dumbnet/lib/consensus.hoon`
