+++
version = "0.1.1"
status = "activated"
consensus_critical = true

activation_height = 720
published = "2025-05-30"
activation_target = ""

authors = ["@nockchain-core"]
reviewers = ["@nockchain-core"]

supersedes = "0.1.0"
superseded_by = "0.1.2"
+++

# Legacy 002 - Checkpoints Genesis and 720

Checkpoint map expansion for genesis and block `720`.

## Summary

This upgrade extended the checkpoint set from one anchor to three. It added hard digest pins for height `0` (genesis) and `720`, while retaining the existing `144` pin.

## Motivation

`0.1.0` anchored one point, but left large unpinned spans before and after it. Two additional anchors improved deterministic replay and reduced ambiguity when nodes reconstructed chain history from peers.

## Technical Specification

`checkpointed-digests` in `consensus.hoon` was expanded to:

- `0 -> 7pR2bvzoMvfFcxXaHv4ERm8AgEnExcZLuEsjNgLkJziBkqBLidLg39Y`
- `144 -> 3rbqdep8HLqwwkW4YvZazVPYZpbqsFbqHCfEKGt13GVUUzA9ToDCsxT`
- `720 -> C4vJRnFNHCLHKHVRJGiYeoiYXS7CyTGrVk2ibEv95HQiZoxRvtr5SRQ`

No new validation branch was added. The existing checkpoint rule from `0.1.0` now applies at all three heights.

Semantic delta:

- Before: one fixed history anchor (`144`).
- After: three fixed anchors (`0`, `144`, `720`).

## Activation

- **Height**: `720`
- **Coordination**: Nodes needed this map before crossing `720`.

## Migration

### Requirements

- Software version: commit `0ab51fed3` or newer

### Configuration

None.

### Data Migration

None.

### Steps

1. Upgrade node software.
2. Verify checkpoint entries for `0`, `144`, and `720`.

### Rollback

Rollback can reintroduce acceptance of historical branches rejected by upgraded nodes.

## Backward Compatibility

Not backward compatible at checkpointed heights.

## Security Considerations

Adding a genesis digest pin removes dependence on inferred genesis identity during historical replay.

## Operational Impact

Bootstrap and resync paths become more deterministic, but only if all nodes run the same checkpoint table.

## Testing and Validation

- Canonical blocks at `0` and `720` pass.
- Any digest mismatch at `0`, `144`, or `720` fails with `%checkpoint-match-failed`.

## Reference Implementation

- Commit: `0ab51fed3`
- Files:
  - `open/hoon/apps/dumbnet/lib/consensus.hoon`
