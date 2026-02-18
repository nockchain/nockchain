+++
version = "0.1.4"
status = "activated"
consensus_critical = true

activation_height = 4032
published = "2025-06-10"
activation_target = ""

authors = ["@nockchain-core"]
reviewers = ["@nockchain-core"]

supersedes = "0.1.3"
superseded_by = "0.1.5"
+++

# Legacy 005 - Checkpoint 4032

Checkpoint map expansion at height `4032`.

## Summary

This upgrade pinned block `4032` to a fixed digest, extending the enforced checkpoint frontier beyond `2448`.

## Motivation

The network had moved far enough beyond earlier anchors that another checkpoint was needed to keep recovery behavior deterministic and to bound acceptable alternate history windows.

## Technical Specification

`checkpointed-digests` gained:

- `4032 -> DhaVTgMz6CMy3ZG3vsci1z9U2Gg7WZL6y3g7bZzfJLUbus1rd8j4BQU`

Validation behavior is unchanged in form:

- for checkpointed heights, digest equality is mandatory;
- mismatch rejects with `%checkpoint-match-failed`.

Semantic delta:

- Before: highest enforced checkpoint was `2448`.
- After: highest enforced checkpoint is `4032`.

## Activation

- **Height**: `4032`
- **Coordination**: Nodes needed the new map before `4032`.

## Migration

### Requirements

- Software version: commit `b8cf14101` or newer

### Configuration

None.

### Data Migration

None.

### Steps

1. Upgrade nodes.
2. Verify checkpoint entry for `4032`.

### Rollback

Rollback may allow histories at `4032` that upgraded peers reject.

## Backward Compatibility

Not backward compatible at checkpointed heights.

## Security Considerations

Strengthens historical anchoring by tightening the latest accepted canonical point.

## Operational Impact

Node fleets with mixed checkpoint tables can split at `4032`.

## Testing and Validation

- Canonical block `4032` passes.
- Digest mismatch at `4032` fails with `%checkpoint-match-failed`.

## Reference Implementation

- Commit: `b8cf14101`
- Files:
  - `open/hoon/apps/dumbnet/lib/consensus.hoon`
