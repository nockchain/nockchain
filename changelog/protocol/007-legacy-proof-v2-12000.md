+++
version = "0.1.6"
status = "activated"
consensus_critical = true

activation_height = 12000
published = "2025-07-01"
activation_target = ""

authors = ["@nockchain-core"]
reviewers = ["@nockchain-core"]

supersedes = "0.1.5"
superseded_by = "0.1.7"
+++

# Legacy 007 - Proof Version 2 at 12000

Proof version `%2` becomes mandatory at height `12000`.

## Summary

This upgrade extended proof-version gating to a third phase. At `height >= 12000`, consensus requires `%2` proofs.

## Motivation

`0.1.5` established version-gated proving, but only for `%0/%1`. A second prover cutover needed the same deterministic boundary rule to avoid mixed-version block acceptance.

## Technical Specification

`open/hoon/apps/dumbnet/lib/consensus.hoon` updated `height-to-proof-version`:

- `height >= 12000` -> `%2`
- `6750 <= height < 12000` -> `%1`
- `height < 6750` -> `%0`

and added:

- `proof-version-2-start = 12000`

Semantic delta:

- Before: highest enforced proof version was `%1`.
- After: `%2` is required from `12000` onward.

This upgrade changed acceptance policy only, not checkpoint data and not transaction rules.

## Activation

- **Height**: `12000`
- **Coordination**: Miners and nodes had to switch in lockstep at the boundary.

## Migration

### Requirements

- Software version: commit `e4ada66c3` or newer

### Configuration

None.

### Data Migration

None.

### Steps

1. Deploy node release before `12000`.
2. Ensure miners emit `%2` proofs at `>=12000`.

### Rollback

Rollback across `12000` can split consensus due to `%1/%2` mismatch.

## Backward Compatibility

Not backward compatible across the activation boundary.

## Security Considerations

Maintains strict prover-generation separation by block height.

## Operational Impact

Release timing around `11999/12000` is critical for mining infrastructure.

## Testing and Validation

- `%1` valid below `12000`, invalid at and after `12000`.
- `%2` invalid below `12000`, valid at and after `12000`.

## Reference Implementation

- Commit: `e4ada66c3`
- Files:
  - `open/hoon/apps/dumbnet/lib/consensus.hoon`
