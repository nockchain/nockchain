+++
version = "0.1.0"
status = "activated"
consensus_critical = true

activation_height = 144
published = "2025-05-25"
activation_target = ""

authors = ["@nockchain-core"]
reviewers = ["@nockchain-core"]

supersedes = ""
superseded_by = "0.1.1"
+++

# Legacy 001 - Checkpoint 144

Hard checkpoint at block 144.

## Summary

This upgrade introduced the first explicit checkpoint map in consensus and pinned block `144` to a specific digest. From this point on, any candidate page at height `144` had to match that digest exactly, regardless of otherwise-valid PoW or ancestry.

## Motivation

At launch, early history selection depended on normal chain validation and accumulated work only. That was enough for steady-state operation, but it left room for alternate early histories during bootstrap and resync. A fixed checkpoint converted height `144` into a deterministic anchor.

## Technical Specification

`open/hoon/apps/dumbnet/lib/consensus.hoon` added `checkpointed-digests`:

- `144 -> 3rbqdep8HLqwwkW4YvZazVPYZpbqsFbqHCfEKGt13GVUUzA9ToDCsxT`

`validate-page-without-txs` gained a hard rule:

- If `height` is not in `checkpointed-digests`, continue normal validation.
- If `height` is present, require `digest == checkpointed-digests[height]`.
- On mismatch, reject with `%checkpoint-match-failed`.

Semantic delta:

- Before: height `144` could be valid if structural and PoW checks passed.
- After: height `144` is valid only for one canonical digest.

## Activation

- **Height**: `144`
- **Coordination**: Nodes had to deploy before syncing through `144` so they shared the same anchor.

## Migration

### Requirements

- Software version: commit `8f4b65633` or newer

### Configuration

None.

### Data Migration

None.

### Steps

1. Upgrade node software.
2. Confirm checkpoint map contains height `144` with the pinned digest.

### Rollback

Rolling back can make a node accept a height-`144` history that upgraded peers reject.

## Backward Compatibility

Not backward compatible at checkpointed heights.

## Security Considerations

This closes a practical early-history ambiguity by pinning one canonical block at `144`.

## Operational Impact

Operators must keep checkpoint maps identical across nodes. Mismatched maps are consensus splits.

## Testing and Validation

- Canonical block `144` passes checkpoint validation.
- Same-height block with any other digest fails with `%checkpoint-match-failed`.

## Reference Implementation

- Commit: `8f4b65633`
- Files:
  - `open/hoon/apps/dumbnet/lib/consensus.hoon`
