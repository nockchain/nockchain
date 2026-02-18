+++
version = "0.1.9"
status = "activated"
consensus_critical = true

activation_height = 39000
published = "2025-10-16"
activation_target = ""

authors = ["@nockchain-core"]
reviewers = ["@nockchain-core"]

supersedes = "0.1.8"
superseded_by = "0.1.10"
+++

# Legacy 010 - V1 Phase 39000

Move segwit phase boundary from `37350` to `39000`.

## Summary

This upgrade did not change v0/v1 mechanics, only the activation boundary. It changed `v1-phase` so the mandatory v1 switch happened `1,650` blocks later than originally specified.

## Motivation

The initial cutover height in `0.1.8` was superseded during rollout coordination. The semantic goal was to preserve one global boundary while giving operators and producers additional runway before v1-only enforcement.

## Technical Specification

`open/hoon/common/tx-engine-1.hoon` changed default constants:

- `v1-phase: 37350 -> 39000`

All rules keyed to `v1-phase` moved together:

- v0 raw-tx cutoff
- v1 raw-tx start
- v0 coinbase cutoff
- v1 coinbase start

Semantic delta:

- Before: boundary at `37350`.
- After: identical boundary logic at `39000`.

Consensus compatibility window:

- Nodes on `0.1.8` and `0.1.9` disagree in `37350..38999`.
  - `0.1.8` expects v1 there.
  - `0.1.9` still treats that range as pre-cutover.

## Activation

- **Height**: `39000`
- **Coordination**: All nodes and miners had to adopt the same constant before either boundary height was reached.

## Migration

### Requirements

- Software version: commit `eedbe8dd9` or newer

### Configuration

No extra flags on mainnet defaults, but any custom constants had to match `v1-phase=39000`.

### Data Migration

None.

### Steps

1. Upgrade all nodes before entering `37350..39000`.
2. Verify effective constants report `v1-phase = 39000`.

### Rollback

Rollback can create immediate version-rule divergence in the rescheduled window.

## Backward Compatibility

Not backward compatible with nodes still enforcing `37350`.

## Security Considerations

The security property is unchanged (single deterministic cutover), but only if all peers share the same boundary value.

## Operational Impact

Release timing, monitoring, and incident playbooks had to be updated to the new boundary.

## Testing and Validation

- Verify boundary behavior at `38999` and `39000`.
- Verify v0/v1 rule matrix no longer flips at `37350`.

## Reference Implementation

- Commit: `eedbe8dd9`
- Files:
  - `open/hoon/common/tx-engine-1.hoon`
