# Coordinated upgrade at height 154,500

Deploy the matching node, consensus kernel, verifier jets, AI and ZK miners,
wallet, and bridge release before block height **154,500**. This release updates
AI-PoW verification, difficulty scheduling, and deposit processing. Bridge
withdrawal processing must remain disabled.

## Activation and compatibility

The candidate block's height selects the consensus rules. Blocks below 154,500
retain their historical rules during synchronization, replay, and reorgs. Blocks
at or above 154,500 use the updated rules. Installation does not activate
height-gated rules early.

Both ASERT lanes re-anchor using the median-time-past of predecessor 154,499.
Historical work is unchanged. The exact schedule and work conversion are in the
[protocol specification](../changelog/protocol/017-anthropos.md#subsequent-reset-at-height-154500).
Pearl transaction inclusion and nonce limits are specified in
[Logos](../changelog/protocol/016-logos.md#pearl-transaction-commitment).

The node and AI miner must be upgraded together. The AI mining effect includes
candidate height; older miners cannot decode the updated effect, and updated
miners reject effects that omit height. The ZK mining effect and on-chain block
and proof encodings are unchanged. Deploy the consensus kernel and Rust verifier
from the same release.

## Before activation

1. Record the release revision and build all distributed binaries and kernels
   from that revision. Check the live tip and allow time to finish the rollout
   before activation.
2. Start upgraded nodes early enough for verifier setup initialization to finish.
   Missing or incompatible setup data is regenerated automatically before
   networking starts. Matching data is reused. No manual cache deletion or
   preparation is required. Initialization can require substantial CPU time and
   working memory; monitor startup completion and use the deployment's configured
   memory budget. Historical verifier setups remain available for replay.
3. Upgrade and restart miners before parent height 154,499. Confirm that they
   receive fresh candidates and advance under the new software.
4. Confirm `withdrawal_processing_enabled = false` in the rendered production
   bridge configuration. Existing signed withdrawals, if any, require separate
   reconciliation before any future withdrawal rollout.

## State loading and recovery

Loading the kernel refreshes the mining candidate and discards pending headers,
including when reloading the same state schema version. Accepted chain state
and retained raw transactions are preserved when the state is compatible;
retained transactions return to the mempool. Discarded blocks require fresh
admission when received again.

Stored post-activation history is checked on load. If retained history is
incompatible, the node refuses to load it. Preserve that state and restore a
verified pre-activation archive for replay with the matching release. An upgrade
does not automatically rewind or repair already accepted incompatible history.
Coordinate recovery if the node crossed the activation boundary under older
software, and do not roll back to a pre-upgrade binary after accepting blocks
under the new rules.

Release validation and deployment readiness must be assessed against the final
build. This operator note does not certify a full historical-chain replay,
production setup capacity, GPU performance, or deployed bridge behavior.
