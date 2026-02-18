+++
version = "0.1.7"
status = "activated"
consensus_critical = true

activation_height = 16128
published = "2025-07-20"
activation_target = ""

authors = ["@nockchain-core"]
reviewers = ["@nockchain-core"]

supersedes = "0.1.6"
superseded_by = "0.1.8"
+++

# Legacy 008 - Checkpoint 16128

Checkpoint map expansion at height `16128`.

## Summary

This upgrade pinned height `16128` to a specific digest, extending the checkpoint schedule after the proof-version cutovers.

## Motivation

After two prover transitions (`%0 -> %1 -> %2`), adding a fresh checkpoint provided a hard post-transition anchor and reduced ambiguity in replay/resync across that period.

## Technical Specification

`checkpointed-digests` gained:

- `16128 -> ANjtb2YNFo3cAtLVkjkXXP2DJ2S5ZvByywpxgAa1UhxXM5f8YmiJLWX`

Validation semantics were unchanged from prior checkpoint upgrades:

- checkpointed heights require exact digest match,
- mismatch rejects with `%checkpoint-match-failed`.

Semantic delta:

- Before: most recent hard anchor at `4032`.
- After: most recent hard anchor at `16128`.

## Activation

- **Height**: `16128`
- **Coordination**: Upgrade before processing height `16128`.

## Migration

### Requirements

- Software version: commit `d85d78320` or newer

### Configuration

None.

### Data Migration

None.

### Steps

1. Upgrade nodes.
2. Confirm the checkpoint map includes `16128` and prior anchors.

### Rollback

Rollback can accept an alternative `16128` block that upgraded peers reject.

## Backward Compatibility

Not backward compatible at checkpointed heights.

## Security Considerations

Adds a hard checkpoint immediately after major proving-system transitions.

## Operational Impact

All nodes must run identical checkpoint data to avoid deterministic splits.

## Testing and Validation

- Canonical `16128` digest passes.
- Non-canonical digest at `16128` fails with `%checkpoint-match-failed`.

## Reference Implementation

- Commit: `d85d78320`
- Files:
  - `open/hoon/apps/dumbnet/lib/consensus.hoon`
