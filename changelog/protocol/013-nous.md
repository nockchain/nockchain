+++
version = "0.1.12"
status = "draft"
consensus_critical = false

activation_height = 0
published = "2026-02-25"
activation_target = ""

authors = ["@nockchain-core"]
reviewers = ["@nockchain-core"]

supersedes = "0.1.11"
superseded_by = ""
+++

# Nous

LibP2P request-response generation 2 (`req-res gen2`) adds batched transport requests, batched transport responses, and protocol-order fallback to generation 1.

## Summary

Nous is a networking upgrade for the libp2p request-response path. It reduces round-trip overhead by carrying many request items in one libp2p request-response exchange (one request message and one response message). During rollout, nodes interoperate by negotiating `gen1` or `gen2` from outbound protocol ordering.

## Motivation

Gen1 request-response is singleton-oriented and pays round-trip overhead per item. During sync or missing-data recovery, this amplifies latency and redundant outbound request traffic.

Nous addresses this by adding batched transport while preserving rollout safety in mixed networks. The design keeps kernel/Hoon interfaces stable, keeps gen1 wire compatibility intact, and makes fallback behavior explicit so operators can upgrade incrementally.

## Technical Specification

### Scope and Invariants

In scope:
- Add `gen2` protocol ID and dual registration (`gen1` + `gen2`).
- Add batch request/response transport message schema.
- Add minimal per-item response envelope metadata for routing and dedupe keys.
- Add batch limits, queue pressure controls, and per-peer inflight caps.
- Add protocol-order-based send routing and fallback behavior.
- Add observability and a validation matrix to support rollout decisions.

Out of scope:
- Any kernel/Hoon interface change.
- Any batching semantic change in kernel execution (kernel continues to process singleton events).
- Any data migration.
- Any hard cutover that requires all peers to upgrade at once.
- Any PoW retuning or PoW-based abuse-defense redesign.

Normative invariants:
- Kernel and Hoon interfaces are unchanged in this generation.
- Batch handling is implemented only in Rust networking/driver code.
- A batch is executed as an ordered sequence of singleton kernel calls.
- If one item fails, previously applied items are not rolled back.
- Gen2 is additive. Gen1 protocol IDs, constants, and encodings remain byte-for-byte unchanged.

### Current Network Behavior (Gen1)

Current protocol ID:
- `/nockchain-1-req-res`

Current transport behavior:
- One request item per request-response exchange.
- Per-request EquiX PoW solve and verify.
- Request timeout defaults to 30 seconds.
- Randomized peer fanout for requests.
- No explicit per-peer transport generation state in driver routing.
- Kernel/network effects consumed by the driver are singleton-oriented.

Current transport message schema (simplified):

```rust
enum NockchainRequest {
    Request { pow: [u8; 16], nonce: u64, message: ByteBuf },
    Gossip { message: ByteBuf },
}

enum NockchainResponse {
    Result { message: ByteBuf },
    Ack { acked: bool },
}
```

### Target Network Behavior (Gen2)

### Protocol IDs, Compatibility, and Send Routing

Protocol IDs:
- `gen1`: `/nockchain-1-req-res`
- `gen2`: `/nockchain-2-req-res`

Compatibility invariants:
- Existing `gen1` protocol constant names and bytes MUST NOT change.
- Existing `gen1` CBOR encoding/decoding behavior MUST remain byte-for-byte stable.
- Gen2 implementation MUST ship compatibility tests that fail on any gen1 protocol ID or gen1 encoding drift.

Node behavior requirements:
- A Nous-capable node MUST register both IDs.
- If `req_res_gen2_send_enabled=false`, outbound protocol ordering MUST prefer `gen1`.
- If `req_res_gen2_send_enabled=true`, outbound protocol ordering MUST prefer `[gen2, gen1]`.
- If outbound `gen2` is unsupported for a request, sender MUST retry that request via `gen1` when available in the protocol family.
- `UnsupportedProtocols` outcomes SHOULD increment fallback metrics/logs.

Implementation constraint:
- In current libp2p request-response API, `send_request` does not take an explicit protocol argument.
- Generation choice depends on outbound protocol ordering plus remote support.
- This generation uses one request-response behavior with deterministic protocol ordering.

Libp2p integration requirements:
- Implementations SHOULD use one request-response behavior that advertises both protocol IDs as one protocol family.
- When `req_res_gen2_send_enabled=true`, outbound protocol ordering MUST prefer `[gen2, gen1]`.
- When `req_res_gen2_send_enabled=false`, implementation SHOULD either:
  - prefer outbound ordering `[gen1, gen2]`, or
  - configure gen2 as inbound-only via protocol support mode.

### Gen2 Transport Message Schema

`gen2` adds batched transport containers.

```rust
enum NockchainRequest {
    // Existing gen1
    Request { pow: [u8; 16], nonce: u64, message: ByteBuf },
    Gossip { message: ByteBuf },

    // New gen2
    BatchRequest {
        pow: [u8; 16],
        nonce: u64,
        items: Vec<BatchRequestItem>,
    },
}

struct BatchRequestItem {
    item_id: u32,
    message: ByteBuf,
}

enum NockchainResponse {
    // Existing gen1
    Result { message: ByteBuf },
    Ack { acked: bool },

    // New gen2
    BatchResult {
        results: Vec<BatchResultItem>,
    },
}

struct BatchResultItem {
    item_id: u32,
    status: BatchResultStatus,
    error: Option<BatchErrorClass>,
    envelope: Option<ResponseEnvelope>,
}

enum BatchResultStatus {
    Result,
    Ack,
    NotFound,
    Error,
}

enum BatchErrorClass {
    Decode,
    Backpressure,
    TooLarge,
    InvalidPow,
    Internal,
}

struct ResponseEnvelope {
    kind: EnvelopeKind,        // HeardBlock | HeardTx | HeardElders
    block_id: Option<String>,  // present when kind == HeardBlock
    tx_id: Option<String>,     // present when kind == HeardTx
    message: ByteBuf,
}
```

Envelope strictness matrix:
- `HeardBlock`: `block_id` required, `tx_id` absent.
- `HeardTx`: `tx_id` required, `block_id` absent.
- `HeardElders`: both IDs absent.
- `height` is not part of the envelope and MUST NOT be included.

Normative requirements:
- `item_id` MUST be unique within each batch.
- Batch response correlation MUST use `item_id`.
- If `status=Error`, `error` MUST be populated.
- If `status != Error`, `error` MUST be `None`.
- Envelope metadata is routing/dedupe metadata only and MUST NOT replace payload validation.
- Unknown variants MUST fail without panic.

Response payload granularity (preserved from existing behavior):
- A successful response item represents one requested object/fact payload, not a transitive bundle.
- A block request item returns a block/page fact payload (`HeardBlock`) and MUST NOT implicitly include all raw transaction blobs for that block.
- A raw transaction blob is returned only for explicit raw-transaction request items (`HeardTx` path).
- Batching changes transport shape only; callers that need block data plus transaction blobs SHOULD include both block and tx request items in the same batch.

### Batch Processing and Control Flow

Inbound control flow:
1. Receive `BatchRequest`.
2. Perform top-level checks: decode container, verify PoW, enforce hard size/count limits.
3. Decode and vet each item independently.
4. Execute vetted items in wire order, one-by-one, through existing singleton kernel call paths.
5. Collect per-item result statuses and envelopes.
6. Aggregate kernel/network effects generated during execution.
7. Suppress duplicate outbound network requests implied by items within the same batch before enqueueing to the wire.
8. Emit one `BatchResult` with per-item outcomes.

Execution semantics:
- Partial success is valid (`Result`/`Ack`/`NotFound`/`Error` mixed in one batch).
- Per-item decode errors do not abort sibling items.
- No batch rollback is attempted after prior item success.

Effect deduplication requirements:
- Dedup applies only to outbound network requests keyed by object identity:
  - `BlockRequest(block_id)`
  - `TxRequest(tx_id)`
- Driver MUST dedup requests implied by other items in the same batch.
- Dedup against queued or in-flight requests MAY be implemented as a non-normative optimization in a future generation.
- Driver MUST preserve non-network side effects and item-local state transitions.
- Suppression is only for redundant wire requests, not for semantic effects.

### Driver-Side Outbound Coalescing

Batch sources:
- Driver-side coalescing of singleton outbound requests (required).
- Optional pre-batched emitters from higher layers are deferred.
- Gossip traffic remains singleton unless a future extension defines batching.

Coalescing policy:
- Flush at `gen2_batch_max_items`.
- Flush at `gen2_batch_coalesce_window_ms`.
- Flush when payload bytes approach `gen2_batch_max_bytes`.
- Sender SHOULD adapt effective batch size below `gen2_batch_max_items` based on observed timeout and backpressure rates.

Determinism requirements:
- Preserve request ordering in per-peer queue.
- Generate stable `item_id` sequence within each batch.

Primary insertion points:
- `open/crates/nockchain-libp2p-io/src/behaviour.rs` (`NockchainBehaviour::pre_new`) for dual protocol registration.
- `open/crates/nockchain-libp2p-io/src/config.rs` for protocol IDs and batch knobs.
- `open/crates/nockchain-libp2p-io/src/driver.rs`:
  - swarm loop send branch for outbound coalescing/send,
  - `handle_effect` for staging outbound item flow,
  - `handle_request_response` for inbound batch execution and `BatchResult` construction.
- `open/crates/nockchain-libp2p-io/src/p2p_state.rs` for per-peer inflight accounting.

### Backpressure and Flow Control

### Receiver Behavior

Admission outcomes:
- **Accept full batch**: top-level checks pass and execution slot is available.
- **Process partially with per-item backpressure**: execution starts, then downstream queue pressure occurs; already-processed items keep their outcomes, remaining items return `Error(Backpressure)`.
- **Reject wholesale**:
  - malformed top-level batch,
  - invalid PoW,
  - hard limit violation,
  - no execution slot available before processing begins.

Receiver response contract:
- If wholesale rejection occurs before any item executes, receiver MAY close the request without per-item results.
- If execution has started, receiver MUST return `BatchResult` with outcomes for processed items and `Error(Backpressure)` for any unprocessed tail items.

Yield semantics:
- For request items that can return multiple units, requested count `N` is an upper bound, not a guarantee.
- Responders MAY yield fewer units `M` than requested (`M <= N`) for any valid operational reason, including payload byte limits, compute budget, or queue pressure.
- When at least one unit is available, responders MUST support fractional yield down to the minimum quantum of one valid unit for that request kind.
- Senders MUST treat fractional yield as valid behavior and continue via additional requests until completion criteria are met.

Queue pressure policy:
- Queueing is bounded.
- Receiver MUST NOT block indefinitely waiting for queue room.
- Queue-full at admission is immediate reject, not bounded wait/retry inside receiver.
- Request-response stream-level concurrency limits are necessary but not sufficient; implementation MUST preserve explicit driver-level queue admission semantics.

Per-peer inflight policy:
- `gen2_max_inflight_per_peer` is a hard cap on outstanding request-response work per peer.
- Hitting the cap causes immediate batch rejection with backpressure classification (when a batch response is available) or early request failure.

### Sender-Visible Semantics

Sender-observed outcomes:
- Transport-level failure before any `BatchResult` (timeout, decode failure, unsupported protocol, early reject): sender treats entire batch as failed and retries with bounded backoff.
- `BatchResult` with per-item `Error(Backpressure)`: sender retries only those failed items, preserving successful items.
- `BatchResult` with per-item `Error(Decode|TooLarge|InvalidPow)`: sender MUST NOT retry those items unchanged.

Retry/backoff requirements:
- Retries MUST use bounded budgets.
- Backpressure retries SHOULD use exponential backoff with jitter.
- Sender MUST NOT spin-retry immediately on repeated backpressure.
- Sender SHOULD reduce retry batch size after repeated transport failures or repeated `Error(Backpressure)` outcomes.

### Limits and Benchmark Tuning Strategy

Gen2 limits are configuration defaults, not protocol constants.

Configured limits:
- `gen2_batch_max_items`
- `gen2_batch_max_bytes`
- `gen2_item_max_bytes`
- `gen2_max_inflight_per_peer`

Tuning requirements:
- Defaults MUST be finalized by benchmark data before `status = final`.
- `gen2_batch_max_items` and `gen2_batch_max_bytes` MUST be tuned together.
- Benchmark suite MUST include mixed request types, cache hit/miss mixes, and queue pressure cases.
- Safety upper bounds MUST remain in place even if benchmark results suggest larger values.
- Request-response CBOR codec max request/response sizes MUST be configured in lockstep with `gen2_batch_max_bytes` so transport and application limits agree.

Responder sizing prerequisites:
- Implementation MUST include a weight index or heuristic (for example b-tree index, range heuristic, or precomputed historical sizing data) so responders can avoid overfetching units that will not fit in the response.
- Implementation MUST include a forward-size heuristic for unknown future block sizes so responders can stop before doing expensive serialization work for units that cannot fit.

Target tuning ranges (starting envelope):
- `gen2_batch_max_items`: 32 to 256
- `gen2_batch_max_bytes`: 256 KiB to 2 MiB
- `gen2_item_max_bytes`: 64 KiB to 256 KiB

Initial hard limits in this draft:
- `gen2_batch_max_items = 128`
- `gen2_batch_max_bytes = 1_048_576`
- `gen2_item_max_bytes = 131_072`

### PoW and Abuse Controls

PoW policy in this generation:
- PoW mechanism and tuning remain unchanged from existing request-response policy.
- Gen2 does not introduce count-weighted or account-weighted PoW.
- Current PoW is known to be under-tuned and MUST NOT be treated as sufficient DoS protection.

Abuse resistance for this generation relies on:
- strict hard limits,
- bounded queues,
- per-peer inflight caps,
- dedupe suppression of redundant outbound requests.

PoW retuning and stronger abuse controls are explicitly deferred to a future generation.

### PoW Input for Batched Payloads

Because batch payload bytes differ from singleton payload bytes, verifier input serialization is defined for interoperability:

Gen2 PoW preimage MUST include:
- fixed domain-separation bytes: ASCII `nockchain:req-res:gen2:pow:v1`
- `nonce`
- sender peer bytes
- receiver peer bytes
- canonical serialized batch item bytes

Canonical batch item bytes definition:
- Use batch item order exactly as transmitted in `BatchRequest.items` (do not sort).
- Encode item list as:
  - `item_count_le_u32`
  - for each item:
    - `item_id_le_u32`
    - `message_len_le_u32`
    - `message_bytes`

Reference preimage layout:
- `domain_sep || nonce_le_u64 || sender_peer_bytes || receiver_peer_bytes || canonical_batch_item_bytes`

### Configuration

Recommended key names:

| Key                                 | Shipped Default | Purpose                                                   |
| ----------------------------------- | --------------- | --------------------------------------------------------- |
| `req_res_gen2_accept_enabled`       | `true`          | Accept gen2 requests                                      |
| `req_res_gen2_send_enabled`         | `false`         | Enable gen2 send only after rollout gate is satisfied     |
| `gen2_batch_max_items`              | `128`           | Hard item cap (benchmark-tuned before final)              |
| `gen2_batch_max_bytes`              | `1048576`       | Hard batch byte cap (co-tuned with item cap)              |
| `gen2_item_max_bytes`               | `131072`        | Hard per-item byte cap                                    |
| `gen2_batch_coalesce_window_ms`     | `10`            | Coalescing window                                         |
| `gen2_max_inflight_per_peer`        | `32`            | Per-peer inflight req-res cap                             |
| `gen2_swarm_action_queue_capacity`  | `1000`          | Bounded driver queue size for swarm actions               |

### Failure Handling

Required handling paths:
- Unsupported protocol on outbound gen2 MUST:
  - trigger only on `OutboundFailure::UnsupportedProtocols`,
  - retry via `gen1` for that request when `gen1` is available in the outbound protocol family,
  - avoid persistent per-peer downgrade state in this generation,
  - emit fallback metrics/logs.
- Decode failure of one item: mark that item `Error(Decode)` and continue siblings when decoding context remains valid.
- Top-level batch decode failure: reject entire batch and count failure.
- Invalid PoW: reject entire batch and count failure.
- Over-limit batch: reject entire batch and count failure (`TooLarge` classification where representable).
- Backpressure before item execution: reject entire batch.
- Backpressure during execution: keep prior item outcomes, mark remaining items `Error(Backpressure)`.
- Timeouts remain observable per generation.

Deterministic per-item error classification:
- `Error(Decode)`: malformed item payload; sender MUST NOT retry unchanged item.
- `Error(TooLarge)`: item or container exceeds configured limits; sender MUST NOT retry unchanged item.
- `Error(InvalidPow)`: invalid proof classification where representable; sender MUST NOT retry unchanged item.
- `Error(Backpressure)`: transient saturation; sender MAY retry with bounded backoff.
- `Error(Internal)`: implementation/internal failure; sender MAY retry with bounded backoff.

### Observability Requirements

Required transport metrics:
- `gen2_batch_requests_sent`
- `gen2_batch_requests_received`
- `gen2_batch_item_count_histogram`
- `gen2_batch_rejected_total{reason=*}`
- `gen2_batch_item_errors_total{class=*}`
- `gen2_fallback_total{from,to,reason=*}`
- `req_res_failures_total{generation=*}`
- `req_res_timeouts_total{generation=*}`
- cache hit/miss counters in request and response paths
- `req_res_inflight_per_peer` gauge
- `req_res_swarm_action_queue_depth` gauge
- `req_res_swarm_action_queue_full_total`
- `req_res_kernel_backpressure_total`
- `req_res_effect_dedup_suppressed_total{reason=in_batch}`

Required logs:
- generation selected per peer
- fallback decisions
- batch reject reason
- per-item failure class counts
- queue saturation decisions (reject/defer)
- dedupe suppression decisions (request key and reason)

Gate condition:
- A node MUST NOT ship with `req_res_gen2_send_enabled=true` unless above metrics/logs, interoperability tests, and PMA RSS validation are in place, and required performance gates pass (two-peer speed-of-light benchmark, offline payload-size fit tests, requester compute profiling).

### Rollout Model

Dual-stack operating model:
- Nous-capable nodes register both protocol IDs and accept both generations.
- Gen1 fallback remains available during migration.

Compatibility matrix:
- Old node <-> old node: `gen1` only.
- New node <-> old node: new node prefers `gen2`; when unsupported for a request, retry `gen1`.
- Old node <-> new node: old node speaks `gen1`; new node accepts `gen1`.
- New node <-> new node: prefer `gen2`; negotiate `gen1` when peer configuration or support requires.

Independent node rollout:
- Nodes can be upgraded independently.
- Mixed-generation networks are supported by protocol-order negotiation and fallback.
- Shipped defaults keep gen2 send disabled until rollout gate conditions are met; operators may then enable `req_res_gen2_send_enabled=true`.

## Activation

- **Height**: TBD (`activation_height = 0` in frontmatter).
- **Coordination**: staged dual-stack rollout; nodes advertise and accept both generations, prefer gen2 when negotiated, and retain gen1 fallback during migration.

## Migration

### Operator Steps

1. Upgrade to Nous-capable node build.
2. Verify node advertises both protocol IDs.
3. Keep `req_res_gen2_accept_enabled=true`.
4. Run staged validation in representative traffic environments.
5. Confirm rollout gate metrics/tests are passing.
6. Enable `req_res_gen2_send_enabled=true` gradually.
7. Monitor generation, fallback, dedupe, and backpressure metrics.

### Rollback Steps

1. Set `req_res_gen2_send_enabled=false`.
2. Keep `req_res_gen2_accept_enabled=true` unless incident response requires full disable.
3. Continue on `gen1` transport path.

### Data Migration

- None.

## Backward Compatibility

This upgrade is transport-level and additive:
- Existing gen1 protocol IDs and encoding are preserved byte-for-byte.
- New nodes can communicate with old nodes via gen1 fallback.
- Old nodes continue operating on gen1 without protocol-level crashes from gen2 rollout.

This upgrade does not change transaction formats or consensus semantics, but it is liveness-critical transport:
- Transactions created by old software remain valid.
- Transactions created by new software remain valid.

## Security Considerations

Security-relevant points in this generation:
- PoW is unchanged and explicitly not treated as sufficient DoS protection.
- Abuse resistance relies on hard message/item limits, bounded queues, and per-peer inflight caps.
- Envelope metadata is advisory for routing/dedupe and MUST NOT be trusted over payload validation.
- Dedupe suppresses only redundant outbound wire requests, and MUST NOT suppress semantic side effects.

## Operational Impact

Operator-facing impact:
- Lower round-trip overhead for high-item request workloads when peers are gen2-capable.
- More explicit flow-control outcomes (admission reject vs partial item backpressure).
- Additional metrics/logs are required for rollout safety (generation selection, fallback rates, dedupe suppressions, queue pressure).

Rollout risk and mitigation:
- Mixed-version networks remain supported through gen1 fallback.
- Operators can disable gen2 send (`req_res_gen2_send_enabled=false`) as a rollback lever while still accepting gen2 if needed.

## Testing and Validation

### A. Serialization and Compatibility
- Gen1 round-trip unchanged.
- Gen1 protocol ID constants unchanged.
- Gen1 vector bytes unchanged against golden files.
- Gen2 batch round-trip for mixed item types.
- Malformed and truncated batch decode rejection without panic.
- Extend `open/crates/nockchain-libp2p-io/src/cbor_tests.rs` for `BatchRequest`/`BatchResult` round-trip and malformed input coverage.
- Maintain machine-readable conformance vectors at `open/crates/nockchain-libp2p-io/testdata/req_res_gen1_cbor_vectors.json`.
- Keep vector-driven tests executable in `open/crates/nockchain-libp2p-io/src/cbor_tests.rs`.

### B. Interop
- gen1 <-> gen1
- gen2 <-> gen2
- gen2 sender with gen1-only peer fallback
- mixed-network reconnect and protocol renegotiation churn
- integration tests around request-response behavior in `open/crates/nockchain-libp2p-io/src/driver.rs`

### C. Correctness
- per-item correlation by `item_id`
- mixed cache hit/miss batch behavior
- in-batch dedupe usage by item type
- deterministic per-peer queue ordering under fallback retries
- suppression of redundant outbound block/tx requests while preserving non-network side effects

### D. Performance
- 100-item fetch workload improves materially over gen1 baseline
- no unacceptable increase in timeout/error rate
- benchmark-derived defaults for batch item and byte caps
- add/update benchmark coverage to measure transport generation behavior (existing `peek_refresh` benchmark is not sufficient by itself)
- include PMA RSS harness scenarios for syncing peers (gen1 vs gen2, with and without native peek optimizations)
- include a two-peer speed-of-light benchmark (fat responder with full PMA data vs requester) and gate rollout on the result
- include offline payload-size tests using current checkpoint data (serialize `Vec<block+transactions>` and determine practical fit under max byte limits, including the nominal 128-block target)
- include requester compute-time profiling for poke + verification of a full 128-block payload

### E. Abuse and Pressure Behavior
- over-limit batch rejection
- bad-PoW rejection
- per-item malformed payload isolation
- queue saturation/backpressure behavior without panic or deadlock
- per-peer inflight cap enforcement
- sender retry/backoff behavior under repeated backpressure

## Implementation File Map

Primary implementation files:
- `open/crates/nockchain-libp2p-io/src/messages.rs`
- `open/crates/nockchain-libp2p-io/src/driver.rs`
- `open/crates/nockchain-libp2p-io/src/behaviour.rs`
- `open/crates/nockchain-libp2p-io/src/config.rs`
- `open/crates/nockchain-libp2p-io/src/p2p_state.rs`
- `open/crates/nockchain-libp2p-io/src/metrics.rs`
- `open/crates/nockchain-libp2p-io/src/cbor_tests.rs`
- `open/crates/nockchain-api/benches/peek_refresh.rs` (extend or add dedicated transport benchmark)

Optional future batching-emit producers:
- `open/hoon/apps/dumbnet/lib/types.hoon`
- `open/hoon/apps/dumbnet/inner.hoon`

Notes:
- Initial rollout does not require Hoon-side batch effect changes.
- Any future Hoon-side batching is a transport optimization and must preserve gen1 compatibility paths.

## Resolved Design Decisions

1. PoW policy for large batches
- Decision: keep current fixed-cost PoW policy and strict transport caps in this generation.
- Decision: include explicit PoW preimage domain/version separator (`nockchain:req-res:gen2:pow:v1`).
- Deferred: count-weighted/account-weighted PoW redesign.

2. Default send flag in shipped config
- Decision: `req_res_gen2_send_enabled=false` in shipped defaults until rollout gate is satisfied.

3. Envelope strictness
- Decision: minimal envelope is `kind + relevant ID + message`.
- `height` removed.

4. Future batching source
- Decision: Rust driver coalescing is required and sufficient for initial rollout.
- Deferred: Hoon-side batch emitters.

5. Request-response behavior architecture
- Decision: single request-response behavior with deterministic protocol ordering and no persistent per-peer capability pinning state in this generation.

6. Backpressure response contract
- Decision: if execution has started, return partial per-item `Error(Backpressure)` for unprocessed tail items.
- Decision: if admission fails before execution, reject whole batch.

7. Queue pressure policy
- Decision: bounded queues, no unbounded wait, immediate reject on full admission queue.

8. Per-peer inflight default and enforcement mode
- Decision: enforce hard cap (`gen2_max_inflight_per_peer`) with immediate reject at cap.

9. Capability caching policy
- Decision: no persistent per-peer capability downgrade TTL in this generation.
- Deferred: explicit capability state caching heuristics if future measurements show material benefit.

## Reference Implementation

TODO
