+++
version = "0.1.5"
status = "activated"
consensus_critical = true

activation_height = 6750
published = "2025-06-14"
activation_target = ""

authors = ["@nockchain-core"]
reviewers = ["@nockchain-core"]

supersedes = "0.1.4"
superseded_by = "0.1.6"
+++

# Legacy 006 - Proof Version 1 at 6750

Proof version `%1` becomes mandatory at height `6750`.

## Summary

This upgrade introduced explicit proof versioning and a height gate for verifier acceptance. At `height >= 6750`, pages must carry proof version `%1`; below `6750`, they must carry `%0`.

## Motivation

The proving pipeline had moved beyond a single implicit format. Without an explicit version gate, miners and nodes could drift across proof encodings and still exchange structurally similar blocks, causing deterministic disagreement.

## Technical Specification

### Versioned Proof Types

`open/hoon/common/ztd/four.hoon` introduced a tagged `proof` form and `proof-version` domain including `%0` and `%1`.

### Prover Input Contract

`open/hoon/common/stark/prover.hoon`, `open/hoon/common/nock-prover.hoon`, and `open/hoon/common/pow.hoon` switched from positional proof inputs to `prover-input` carrying:

- `version`
- `header`
- `nonce`
- `pow-len`

Mining effects and miner causes were updated to carry versioned prover input.

### Consensus Gate

`open/hoon/apps/dumbnet/lib/consensus.hoon` added:

- `proof-version-1-start = 6750`
- `height-to-proof-version(height)` mapping
- validation in `validate-page-without-txs`:
  - expected version computed from page height,
  - mismatch rejects with `%proof-version-invalid`.

Semantic delta:

- Before: proof version was not height-enforced by consensus.
- After: proof version is a consensus rule keyed to block height.

## Activation

- **Height**: `6750`
- **Coordination**: Nodes and miners needed synchronized rollout at the boundary.

## Migration

### Requirements

- Software version: commit `af3a12d19` or newer

### Configuration

None.

### Data Migration

Kernel-state upgrade paths were added so persisted runtime state remained loadable after introducing versioned mining/proof types.

### Steps

1. Upgrade nodes.
2. Upgrade miners to emit `%1` proofs at and after `6750`.
3. Confirm boundary behavior in staging around `6749/6750`.

### Rollback

Rollback across the boundary creates proof-version validity disagreements and can fork consensus.

## Backward Compatibility

Not backward compatible across the activation boundary.

## Security Considerations

Prevents acceptance of proofs from the wrong prover generation at a given height.

## Operational Impact

Boundary monitoring is required. A miner producing `%0` at `>=6750` is rejected immediately.

## Testing and Validation

- `%0` accepted below `6750`, rejected at and after `6750`.
- `%1` rejected below `6750`, accepted at and after `6750`.

## Reference Implementation

- Commit: `af3a12d19`
- Files:
  - `open/hoon/apps/dumbnet/lib/consensus.hoon`
  - `open/hoon/common/stark/prover.hoon`
  - `open/hoon/common/pow.hoon`
  - `open/hoon/common/ztd/four.hoon`
