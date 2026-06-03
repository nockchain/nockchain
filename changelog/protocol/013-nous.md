+++
version = "1.0.0"
status = "final"
consensus_critical = false

activation_height = 0
published = "2026-02-25"
activation_target = "2026-Q2"

authors = ["@ryjm"]
reviewers = ["@nockchain-core"]

supersedes = "0.1.11"
superseded_by = ""
+++

# Nous

LibP2P request-response generation 2 (`req-res gen2`) adds batched transport requests, batched transport responses, bundled block+transaction response shapes, catch-up range prefetch, and protocol-order fallback to generation 1.

## Summary

Nous is a networking upgrade for the libp2p request-response path. It reduces round-trip overhead by carrying many request items in one libp2p request-response exchange (one request message and one response message). It also adds optional block+transaction bundle requests and optional catch-up range prefetch over the same gen2 transport. During rollout, nodes interoperate by negotiating `gen1` or `gen2` from outbound protocol ordering.

## Motivation

Gen1 request-response is singleton-oriented and pays round-trip overhead per item. During sync or missing-data recovery, this amplifies latency and redundant outbound request traffic.

Nous addresses this by adding batched transport while preserving rollout safety in mixed networks. The design keeps consensus semantics stable, keeps gen1 wire compatibility intact, and makes fallback behavior explicit allowing operators to upgrade incrementally.

## Technical Specification

### Scope and Invariants

In scope:
- Add `gen2` protocol ID and dual registration (`gen1` + `gen2`).
- Add batch request/response transport message schema.
- Add per-item response envelopes for singleton facts, block+transaction bundles, and block-range bundles.
- Add optional `%block-with-txs` request vocabulary and read-only Hoon scry paths for bundle/range response assembly.
- Add batch limits, queue pressure controls, and per-peer inflight caps.
- Add protocol-order-based send routing and fallback behavior.
- Add capability-aware catch-up prefetch peer selection for range requests.
- Add per-peer range capability, range outcome, and cooldown state for requester routing.
- Add singleton fallback and per-height backoff behavior for failed catch-up range requests.
- Add observability and a validation matrix to support rollout decisions.

Out of scope:
- Any batching semantic change in kernel execution (kernel continues to process singleton events).
- Any data migration.
- Any hard cutover that requires all peers to upgrade at once.
- Any PoW retuning or PoW-based abuse-defense redesign.

Normative invariants:
- Batch formation, batch routing, response unpacking, and fallback are implemented in Rust networking/driver code.
- Hoon changes are read-only scry support for `heavy-txs` and `heaviest-chain-blocks-range`; they do not add new consensus state transitions.
- Batch request items are executed in wire order on the responder read path.
- Batch result items are routed in response order on the requester; bundled transactions are routed before their block fact.
- If one item fails, previously completed items are not rolled back.
- Gen2 is additive. Gen1 protocol IDs, constants, and encodings remain byte-for-byte unchanged.
- Catch-up range prefetch is a transport optimization only. It MUST NOT change consensus semantics or payload validation rules.

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
- The gen1 transport enum remains `Request`/`Gossip` and `Result`/`Ack`; gen1 CBOR vectors remain stable.

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
- A Nous-capable node MUST always register `gen1`.
- If either `req_res_gen2_accept_enabled=true` or `req_res_gen2_send_enabled=true`, node MUST register `gen2` with a support mode consistent with those flags.
- If `req_res_gen2_send_enabled=false`, outbound protocol ordering MUST prefer `gen1`, and implementation MUST NOT advertise `gen2` as outbound-capable.
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
- When `req_res_gen2_send_enabled=false`, implementation MUST either:
  - advertise `gen1` first and configure `gen2` as inbound-only when `req_res_gen2_accept_enabled=true`, or
  - omit `gen2` entirely when `req_res_gen2_accept_enabled=false`.
- Outbound-only `gen2` support MAY exist for explicit experiments, but it is not part of the shipped rollout default.

### Gen2 Transport Message Schema

`gen2` adds batched transport containers. The inner request vocabulary accepted by the driver now includes classic singleton requests plus the optional bundle/range shapes shown below.

```rust
enum NockchainDataRequest {
    BlockByHeight(u64),
    EldersById(String, PeerId, NounSlab),
    RawTransactionById(String, NounSlab),
    BlockWithTxsByHeight(u64),
    BlockRangeWithTxs { start_height: u64, len: u8 },
}

const BLOCK_RANGE_REQUEST_MAX_LEN: u8 = u8::MAX;

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

enum EnvelopeKind {
    HeardBlock,
    HeardTx,
    HeardElders,
    HeardBlockWithTxs,
    HeardBlockRangeWithTxs,
}

struct ResponseEnvelope {
    kind: EnvelopeKind,
    block_id: Option<String>,
    tx_id: Option<String>,
    message: ByteBuf,
    tx_envelopes: Option<Vec<BundledTxEnvelope>>,
    unincluded_tx_ids: Option<Vec<String>>,
    range_blocks: Option<Vec<BundledBlockWithTxs>>,
}

struct BundledTxEnvelope {
    tx_id: String,
    message: ByteBuf,
}

struct BundledBlockWithTxs {
    block_id: String,
    block_message: ByteBuf,
    tx_envelopes: Vec<BundledTxEnvelope>,
    unincluded_tx_ids: Vec<String>,
}
```

Request item wire nouns:
- Classic block: `[%request [%block [%by-height height]]]`
- Elders: `[%request [%block [%elders block-id peer-id]]]`
- Raw transaction: `[%request [%raw-tx [%by-id tx-id]]]`
- Block bundle: `[%request [%block-with-txs [%by-height height]]]`
- Block range bundle: `[%request [%block-with-txs [%by-range start_height len]]]`; `len` must be `1..=BLOCK_RANGE_REQUEST_MAX_LEN`.

Envelope strictness matrix:
- `HeardBlock`: `block_id` required, `tx_id`, `tx_envelopes`, `unincluded_tx_ids`, and `range_blocks` absent.
- `HeardTx`: `tx_id` required, `block_id`, `tx_envelopes`, `unincluded_tx_ids`, and `range_blocks` absent.
- `HeardElders`: `block_id`, `tx_id`, `tx_envelopes`, `unincluded_tx_ids`, and `range_blocks` absent.
- `HeardBlockWithTxs`: `block_id` required, `tx_id` absent, `message` is the `heard-block` fact, `tx_envelopes` and `unincluded_tx_ids` required, `range_blocks` absent.
- `HeardBlockRangeWithTxs`: `range_blocks` required and non-empty, `message` empty, `block_id`, `tx_id`, `tx_envelopes`, and `unincluded_tx_ids` absent.
- Within each `HeardBlockWithTxs` or range block bundle, bundled tx IDs MUST be non-empty and unique, unincluded tx IDs MUST be non-empty, and a tx ID MUST NOT appear in both bundled and unincluded lists for the same block.
- `height` is not part of any response envelope and MUST NOT be included.

Normative requirements:
- `item_id` MUST be unique within each batch.
- Batch response correlation MUST use `item_id`.
- If `status=Result`, `envelope` MUST be populated and `error` MUST be `None`.
- If `status=Error`, `error` MUST be populated.
- If `status=Error`, `envelope` MUST be `None`.
- If `status=Ack` or `status=NotFound`, both `error` and `envelope` MUST be `None`.
- Envelope metadata is routing/dedupe metadata only and MUST NOT replace payload validation.
- Unknown variants MUST fail without panic.

Response payload granularity:
- A classic `BlockByHeight` item returns one `HeardBlock` fact and does not carry raw transaction blobs.
- A `RawTransactionById` item returns one `HeardTx` fact.
- A `BlockWithTxsByHeight` item returns one `HeardBlockWithTxs` envelope containing the block fact, any raw transaction facts that fit under the per-item cap, and `unincluded_tx_ids` for the remainder.
- A `BlockRangeWithTxs` item returns one `HeardBlockRangeWithTxs` envelope containing a contiguous prefix of block bundles; the responder may return fewer blocks than requested when the chain ends, the range has a gap, or the response budget fills.
- Batching changes transport shape. Bundle/range request shapes intentionally change response granularity while preserving validation of every enclosed fact.

Hoon scry interface used by bundle/range responders:
- `heavy_txs_scry_slab(height)` builds `[%heavy-txs height ~]`.
- `block_range_with_txs_scry_slab(start_height, len)` rejects `len = 0`, computes inclusive `end_height = start_height + len - 1` with overflow checking, and builds `[%heaviest-chain-blocks-range start_height end_height ~]`.
- The Hoon `%heavy-txs` arm returns `(unit (unit heavy-txs))`.
- The Hoon `%heaviest-chain-blocks-range` arm returns `(unit (unit (list [page-number block-id page (z-map tx-id raw-tx)])))`.
- The Rust range responder converts the Hoon list into a contiguous prefix starting at `start_height`.

### Batch Processing and Control Flow

Inbound control flow:
1. Receive `BatchRequest`.
2. Validate duplicate `item_id` absence and enforce top-level item-count and encoded-payload limits.
3. Verify the batch PoW.
4. Admit the request under the per-peer inbound request-response inflight cap.
5. Estimate each item's response envelope before execution when possible.
6. Execute items in wire order through cache/peek responder paths.
7. Stop before or during execution when the encoded response budget would be exceeded.
8. Emit one `BatchResult` with per-item outcomes when execution reaches the batch path.

Execution semantics:
- Partial success is valid (`Result`/`Ack`/`NotFound`/`Error` mixed in one batch).
- Per-item decode errors do not abort sibling items.
- No batch rollback is attempted after prior item success.
- If response budget estimation or actual encoding would overflow, already completed items keep their outcomes and the unexecuted tail is represented as `Error(Backpressure)`, with the current item reported as `Error(TooLarge)` only when the item cannot fit by itself.
- Top-level malformed, over-limit, invalid-PoW, or inflight-admission failures reject the request before a `BatchResult` is sent.

Effect deduplication requirements:
- Pending gen2 batches deduplicate exact request message bytes per peer before assigning another item.
- Active outbound requests deduplicate exact request message bytes per peer before enqueueing new wire work.
- Bundle response routing sends bundled tx facts before the block fact, which avoids immediate duplicate raw-tx follow-up requests from the kernel when the txs were already included in the bundle.
- Driver MUST preserve non-network side effects and item-local state transitions.
- Suppression is only for redundant wire requests, not for semantic facts.

### Driver-Side Outbound Coalescing

Batch sources:
- Driver-side coalescing of singleton outbound requests (required).
- Catch-up prefetch emits one `BlockRangeWithTxs` request message, which may travel as a one-item gen2 batch.
- Gossip traffic remains singleton unless a future extension defines batching.

Coalescing policy:
- Flush at `gen2_batch_max_items`.
- Flush at `gen2_batch_coalesce_window_ms`.
- Flush when payload bytes approach `gen2_batch_max_bytes`.
- Flush when estimated response bytes approach `min(gen2_block_batch_max_response_bytes, gen2_batch_max_bytes)` for response-budgeted request kinds.
- Raw-transaction-only batches wait two coalescing ticks; any batch containing non-raw-tx items flushes on the first tick.
- A one-item pending batch containing only classic `BlockByHeight` is demoted to a gen1 singleton request at flush time.
- Retry construction splits repeated failed batches with bounded exponential backoff and jitter.

Determinism requirements:
- Preserve request ordering in per-peer queue.
- Generate stable `item_id` sequence within each batch.

Primary insertion points:
- `open/crates/nockchain-libp2p-io/src/behaviour.rs` for dual protocol registration and codec byte caps.
- `open/crates/nockchain-libp2p-io/src/config.rs` for protocol IDs and batch knobs.
- `open/crates/nockchain-libp2p-io/src/driver.rs`:
  - swarm loop action dispatch,
  - `handle_effect_with_dispatcher` for block-by-height bundle upgrades and prefetch issuance.
- `open/crates/nockchain-libp2p-io/src/driver/gen2.rs` and `open/crates/nockchain-libp2p-io/src/driver/gen2/batch.rs` for outbound coalescing, retry, fallback, and sizing helpers.
- `open/crates/nockchain-libp2p-io/src/driver/gen2/inbound.rs` for inbound batch admission and `BatchResult` construction.
- `open/crates/nockchain-libp2p-io/src/driver/gen2/request_exec.rs` for singleton, bundle, and range responder execution.
- `open/crates/nockchain-libp2p-io/src/driver/gen2/outbound.rs` and `open/crates/nockchain-libp2p-io/src/driver/gen2/responses.rs` for batch result unpacking and fact routing.
- `open/crates/nockchain-libp2p-io/src/p2p_state.rs` for per-peer inflight accounting, active request dedupe, range capability state, deferred prefetch coverage, and response-size hints.

### Capability-Aware Catch-Up Prefetch Peer Selection

Problem statement:
- A catching-up requester may know a backbone peer supports `gen2`, but the current prefetch peer chooser can select an unrelated connected peer first.
- If that selected peer is not known to support `gen2`, the driver suppresses the full range shape and falls back to a classic `BlockByHeight` singleton for the requested start height.
- If that peer does not complete the range request, stable peer ordering can pin the catch-up loop to the same ineffective peer.
- The desired behavior is to route catch-up range work only to peers with the protocol and shape capability needed to serve it, then adapt quickly when observed behavior contradicts the current belief.

Request classes:
- `BlockRangeWithTxs` is the catch-up prefetch range request shape.
- `BlockRangeWithTxs` is issued only when `prefetch_enabled=true`, the request height is nonzero, the sync mode is `Cold` or `CatchingUp` or kernel demand is deep enough, and the prefetch window knobs are nonzero.
- `BlockRangeWithTxs` is carried over gen2 batching when the selected peer advertises inbound gen2 and outbound gen2 send is enabled.
- `BlockRangeWithTxs` MUST NOT be sent as a blind `gen1` range request to a peer whose range capability is `Unknown` or `Unsupported`.
- `gen1` block-by-height singleton fallback remains valid and MUST stay available when no range-capable `gen2` peer is eligible.
- Gossip remains outside this policy and continues to use the existing generation-selection rules.

Peer capability state:

```rust
enum RangeCapability {
    Unknown,
    Supported,
    Unsupported,
}

struct PrefetchPeerScore {
    generation: PeerReqResGeneration,
    range_capability: RangeCapability,
    range_success_count: u64,
    range_failure_count: u64,
    range_timeout_count: u64,
    range_inflight_count: u8,
    range_response_bytes_ewma: Option<f64>,
    range_round_trip_ms_ewma: Option<f64>,
    range_cooldown_until: Option<Instant>,
}
```

Capability update rules:
- Identify-derived `gen2` support is necessary but not sufficient for `BlockRangeWithTxs`.
- A peer with `generation != Gen2` is ineligible for `BlockRangeWithTxs` prefetch.
- A successful `BlockRangeWithTxs` response marks `range_capability=Supported`.
- An unsupported-protocol failure on a gen2 `BlockRangeWithTxs` request or an explicit `Error(Decode|TooLarge)` for the range item marks `range_capability=Unsupported`.
- Transport timeout, connection close, and I/O failure do not mark `Unsupported`; those outcomes update failure counters and cooldown only.
- Reconnect does not clear the local `Supported`/`Unsupported` range capability memo in the current implementation.
- Unknown peers MAY be probed only with a bounded range probe as described below.

Eligibility gate:

A peer is eligible for normal catch-up range prefetch only if all of the following are true:
- The peer is currently connected.
- The peer is known as `Gen2`.
- `range_capability == Supported`.
- The peer is not in range cooldown.
- The peer is under `prefetch_max_inflight_per_peer`.
- The peer is under `prefetch_bandwidth_cap_per_peer_bytes_per_min`.

If no peer passes the normal eligibility gate:
- The requester MAY select one `Gen2 + Unknown` peer for a probe.
- The requester MUST cap the probe to `len <= 2`.
- If no probe peer is available, the requester MUST fall back to singleton `BlockByHeight` requests across the existing diverse peer selection path.

Weighted rendezvous ranking:

Among eligible peers, selection MUST avoid lexicographic pinning. The requester uses weighted rendezvous ranking keyed by `start_height`, selected `window`, and candidate peer:

```text
success_probability =
    (range_success_count + 2) /
    (range_success_count + range_failure_count + 4)

rtt_weight =
    clamp(500 / max(range_round_trip_ms_ewma, 1), 0.25, 4.0)

payload_weight =
    clamp(log2(max(range_response_bytes_ewma, 1)) / 20, 0.5, 2.0)

inflight_penalty =
    (1 + range_inflight_count) ^ 1.5

peer_weight =
    success_probability *
    rtt_weight *
    payload_weight /
    inflight_penalty

u =
    hash_to_unit_interval(
        "nockchain-prefetch-peer-v2",
        start_height,
        window,
        candidate_peer_id
    )

rank =
    -ln(u) / max(peer_weight, 0.01)
```

Selection rule:
- Pick the peer with the smallest `rank`.
- Tie-break by peer ID base58 ordering.
- A peer without range stats starts with weight `1.0`; missing RTT or payload EWMA contributes `1.0` for that term.
- `PREFETCH_PEER_ACTIVE_REQUEST_BIAS` is `1.5`.
- The implementation clamps the weight floor at `0.01`.

Rationale:
- The hard gate prevents impossible or unsupported requests from entering the fast path.
- Weighted rendezvous gives deterministic spread by height range without a single sorted peer winning every request.
- The smoothed success, RTT, and payload-size terms bias traffic toward peers that have actually served range data.
- The inflight penalty prevents one strong peer from absorbing all catch-up demand.

Probe behavior:
- A probe is a normal `BlockRangeWithTxs` request with a reduced `len`.
- Probe success marks `Supported` and updates the same EWMA and success counters as a normal range response.
- Probe decode or unsupported-protocol failure marks `Unsupported`.
- Probe transient failure opens cooldown but leaves capability `Unknown`.
- Probe traffic increments `prefetch_peer_probe_total`.

Retry and cooldown behavior:
- A failed `BlockRangeWithTxs` request MUST clear its prefetch coverage before another request can cover the same heights.
- Timeout on a range request records range failure state, opens cooldown, and queues singleton `BlockByHeight` retry work for the start height through the alternate-peer path.
- Unsupported-protocol range failure marks the peer `Gen1` and `Unsupported` for range capability, then queues gen1 singleton fallback for the start height to the same peer.
- `Error(Decode|TooLarge)` on a range batch item marks the peer `Unsupported` for range capability and queues singleton `BlockByHeight` for the start height.
- Cooldown uses exponential backoff without jitter:
  - initial: `2s`
  - multiplier: `2`
  - maximum: `60s`
- Successful range response resets cooldown and reduces the effective failure penalty.
- Per-height stuck accounting remains in force and still bounds amplification.

Fallback behavior:
- If no `Gen2 + Supported` peer exists and no safe probe is available, the requester MUST issue singleton `BlockByHeight` requests through the existing fallback path.
- Singleton fallback SHOULD prefer peers that recently advertised or served the target height.
- Singleton fallback MUST preserve bounded retry and attempted-peer tracking.
- The requester MUST NOT repeatedly issue full range requests to peers that have never demonstrated range capability.

Implementation constraints:
- The selector belongs in Rust driver state, not Hoon/kernel code.
- The selector consumes only local observations and static config.
- No peer score is shared over the network.
- The scoring state is advisory and MUST NOT be trusted for payload validity.
- All payloads received through a selected peer continue through the existing validation path.
- The selector is deterministic for a fixed local state snapshot, candidate set, start height, window, and Rust hasher behavior.

Configuration keys and constants:

| Key | Default | Purpose |
| --- | --- | --- |
| `prefetch_enabled` | `false` | Enable catch-up range prefetch and the capability-aware selector |
| `prefetch_window_initial` | `4` | Minimum requested range window |
| `prefetch_window_max` | `64` | Maximum requested range window |
| `prefetch_behind_threshold` | `8` | Deferred-buffer threshold and kernel-demand threshold used by prefetch eligibility |
| `prefetch_peer_observed_threshold` | `32` | Peer-observed height gap threshold for catch-up mode |
| `prefetch_hysteresis_ms` | `30000` | Catch-up exit hysteresis |
| `prefetch_max_inflight_per_peer` | `2` | Per-peer range prefetch inflight cap |
| `prefetch_height_failure_budget` | `8` | Per-height singleton retry budget before stuck backoff |
| `prefetch_stuck_backoff_secs` | `20` | Per-height stuck backoff |
| `prefetch_bandwidth_cap_per_peer_bytes_per_min` | `209715200` | Per-peer prefetch byte cap over a 60s window |
| `PREFETCH_RANGE_PROBE_LEN` | `2` | Compile-time probe range cap |
| `PREFETCH_PEER_COOLDOWN_INITIAL` | `2s` | Compile-time initial range failure cooldown |
| `PREFETCH_PEER_COOLDOWN_MAX` | `60s` | Compile-time maximum range failure cooldown |
| `PREFETCH_PEER_ACTIVE_REQUEST_BIAS` | `1.5` | Compile-time inflight penalty exponent |
| `PREFETCH_PEER_EWMA_ALPHA` | `0.2` | Compile-time EWMA update factor for range response bytes and RTT |

The shipped default for `prefetch_enabled` remains `false`.

### Backpressure and Flow Control

### Receiver Behavior

Admission outcomes:
- **Accept full batch**: top-level checks pass and execution slot is available.
- **Process partially with per-item backpressure**: execution starts, then downstream queue pressure occurs; already-processed items keep their outcomes, remaining items return `Error(Backpressure)`.
- **Reject wholesale**:
  - malformed top-level batch or duplicate `item_id`,
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
- `gen2_max_inflight_per_peer` is a hard cap on outstanding inbound request-response work per peer.
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
- `gen2_block_batch_max_response_bytes`
- `gen2_max_inflight_per_peer`

Tuning requirements:
- Defaults MUST be finalized by benchmark data before `status = final`.
- `gen2_batch_max_items` and `gen2_batch_max_bytes` MUST be tuned together.
- Benchmark suite MUST include mixed request types, cache hit/miss mixes, and queue pressure cases.
- Safety upper bounds MUST remain in place even if benchmark results suggest larger values.
- Request-response CBOR codec max request/response sizes MUST be configured in lockstep with `gen2_batch_max_bytes`, keeping transport and application limits aligned.

Responder sizing prerequisites:
- Implementation MUST use response-size hints and cold estimates before adding response-budgeted items to outbound batches.
- Responders MUST estimate each batch result item before execution when possible and stop before response encoding exceeds `gen2_batch_max_bytes`.
- `BlockRangeWithTxs` responders MUST return a contiguous prefix that fits within `min(gen2_block_batch_max_response_bytes, gen2_batch_max_bytes)`, except the first block may be returned alone under the transport cap when it exceeds the tighter block-response budget.

Target tuning ranges (starting envelope):
- `gen2_batch_max_items`: 32 to 128
- `gen2_batch_max_bytes`: 1 MiB to 4 MiB
- `gen2_item_max_bytes`: 128 KiB to 2 MiB

Shipped defaults in `LibP2PConfig`:
- `gen2_batch_max_items = 64`
- `gen2_batch_max_bytes = 4_194_304`
- `gen2_item_max_bytes = 2_097_152`
- `gen2_block_batch_max_response_bytes = 2_097_152`
- `gen2_batch_coalesce_window_ms = 10`
- `gen2_max_inflight_per_peer = 32`
- `gen2_swarm_action_queue_capacity = 1000`

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
| `req_res_gen2_accept_enabled`       | `false`         | Accept gen2 requests after explicit rollout opt-in        |
| `req_res_gen2_send_enabled`         | `false`         | Enable gen2 send only after rollout gate is satisfied     |
| `req_res_gen2_bundle_enabled`       | `false`         | Upgrade eligible block-by-height effects to `%block-with-txs` |
| `gen2_batch_max_items`              | `64`            | Hard item cap                                             |
| `gen2_batch_max_bytes`              | `4194304`       | Hard batch byte cap and CBOR request/response cap         |
| `gen2_item_max_bytes`               | `2097152`       | Hard per-item byte cap                                    |
| `gen2_block_batch_max_response_bytes` | `2097152`     | Response budget for block bundle/range batches            |
| `gen2_batch_coalesce_window_ms`     | `10`            | Coalescing window                                         |
| `gen2_max_inflight_per_peer`        | `32`            | Per-peer inflight req-res cap                             |
| `gen2_swarm_action_queue_capacity`  | `1000`          | Bounded driver queue size for swarm actions               |
| `prefetch_enabled`                  | `false`         | Enable catch-up range prefetch                            |
| `prefetch_window_initial`           | `4`             | Initial range prefetch window                             |
| `prefetch_window_max`               | `64`            | Maximum range prefetch window                             |
| `prefetch_behind_threshold`         | `8`             | Deferred backlog and kernel-demand threshold              |
| `prefetch_peer_observed_threshold`  | `32`            | Peer-observed height threshold for catch-up mode          |
| `prefetch_hysteresis_ms`            | `30000`         | Catch-up exit hysteresis                                  |
| `prefetch_max_inflight_per_peer`    | `2`             | Per-peer range prefetch inflight cap                      |
| `prefetch_height_failure_budget`    | `8`             | Per-height retry budget before stuck backoff              |
| `prefetch_stuck_backoff_secs`       | `20`            | Per-height stuck backoff                                  |
| `prefetch_bandwidth_cap_per_peer_bytes_per_min` | `209715200` | Per-peer range prefetch byte cap over 60s |

### Failure Handling

Required handling paths:
- Unsupported protocol on outbound gen2 MUST:
  - trigger only on `OutboundFailure::UnsupportedProtocols`,
  - retry non-range gen2 requests via gen1 when gen1 fallback contexts can be built,
  - retry range gen2 requests as singleton `BlockByHeight(start_height)` fallback,
  - record the peer as gen1 for generation routing after gen2 unsupported protocol,
  - mark a range request peer as range-unsupported when the failed request was `BlockRangeWithTxs`,
  - emit fallback metrics/logs.
- Decode failure of one item: mark that item `Error(Decode)` and continue siblings when decoding context remains valid.
- Top-level batch decode failure: reject entire batch and count failure.
- Invalid PoW: block the peer and return no `BatchResult`.
- Over-limit top-level batch: reject entire batch and count failure.
- Backpressure before item execution: reject entire batch.
- Backpressure during execution: keep prior item outcomes, mark remaining items `Error(Backpressure)`.
- Timeouts remain observable per generation.
- `BlockRangeWithTxs` transient failure: clear prefetch coverage, record range failure state, apply peer cooldown, and queue singleton `BlockByHeight` retry work for the start height through the alternate-peer path.
- `BlockRangeWithTxs` decode, too-large, or unsupported-protocol failure: mark the peer range-unsupported and queue singleton `BlockByHeight` fallback for the start height.

Deterministic per-item error classification:
- `Error(Decode)`: malformed item payload; sender MUST NOT retry unchanged item.
- `Error(TooLarge)`: item or container exceeds configured limits; sender MUST NOT retry unchanged item.
- `Error(InvalidPow)`: classification exists in the enum, but top-level invalid PoW currently blocks the peer before a per-item response can be sent.
- `Error(Backpressure)`: transient saturation; sender MAY retry with bounded backoff.
- `Error(Internal)`: implementation/internal failure; sender MAY retry with bounded backoff.

### Observability Requirements

Required transport metrics:
- `gen1_outbound_failures`
- `gen2_outbound_failures`
- `gen1_outbound_timeouts`
- `gen2_outbound_timeouts`
- `gen2_batch_requests_sent`
- `gen2_batch_requests_received`
- `gen2_batch_items_sent`
- `gen2_batch_items_received`
- `gen2_batch_rejected_malformed`
- `gen2_batch_rejected_too_many_items`
- `gen2_batch_rejected_too_many_bytes`
- `gen2_batch_rejected_backpressure`
- `gen2_batch_item_error_decode`
- `gen2_batch_item_error_backpressure`
- `gen2_batch_item_error_too_large`
- `gen2_batch_item_error_invalid_pow`
- `gen2_batch_item_error_internal`
- `gen2_batch_result_unexpected_item_id`
- `req_res_fallback_total`
- `req_res_block_by_height_gen1_routed`
- request/response cache hit/miss counters:
  - `block_request_cache_hits`
  - `tx_request_cache_hits`
  - `block_seen_cache_hits`
  - `tx_seen_cache_hits`
  - `block_request_cache_misses`
  - `block_request_cache_negative`
  - `tx_request_cache_misses`
  - `block_seen_cache_misses`
  - `tx_seen_cache_misses`
- `req_res_inflight_total`
- `req_res_inflight_max_per_peer`
- `gen2_batch_pending_items`
- `gen2_batch_pending_peers`
- `req_res_retry_scheduled_total`
- `req_res_effect_dedup_suppressed`
- catch-up/prefetch metrics:
  - `sync_mode`
  - `sync_mode_transitions_total`
  - `behind_tip_estimate`
  - `deferred_blocks_above_frontier`
  - `peer_observed_max_height`
  - `prefetch_cache_hits_total`
  - `prefetch_cache_misses_total`
  - `prefetch_buffer_size`
  - `prefetch_issued_total`
  - `prefetch_singleton_suppressed_total`
  - `prefetch_no_eligible_peer_total`
  - `prefetch_invalidated_total`
  - `prefetch_height_stuck_total`
  - `prefetch_throttled_total`
- `prefetch_peer_selected_total`
- `prefetch_peer_probe_total`
- `prefetch_peer_capability_supported_total`
- `prefetch_peer_capability_unsupported_total`
- `prefetch_peer_cooldown_total`
- `prefetch_peer_no_gen2_range_peer_total`

Implementation note:
- This generation uses fixed counters/gauges for reject reasons and item error classes. Bounded cardinality keeps dashboards stable and avoids a combinatorial label party.
- The bounded `SwarmAction` channel currently applies async backpressure. The implementation tracks pending gen2 batch depth and backpressure/reject outcomes; no separate `queue_full_total` counter exists for this path.
- Prefetch peer-selection metrics are fixed counters/gauges without dynamic labels; raw peer IDs stay in logs.

Required logs:
- generation selected per peer
- fallback decisions
- batch reject reason
- batch response item/failure counts
- queue saturation decisions (reject/defer)
- dedupe suppression decisions (request key and reason)
- catch-up prefetch issuance decisions, including selected peer, request range, mode, response estimates, and probe flag
- suppression of range prefetch when only `gen1` peers or no eligible range peers are available
- range, bundle, and singleton fallback decisions

Gate condition:
- A node MUST NOT ship with `req_res_gen2_send_enabled=true` unless above metrics/logs are in place, interoperability and pressure tests pass, and the rollout proof bundle is green on representative operator hardware.
- The operator-facing validation entrypoints in this generation are:
  - `nix develop -c ./scripts/run_testnet_full_validation.sh` as the canonical recurring readiness gate; inspect `target/test-logs/testnet_full_validation/<timestamp>/report.json` and require `compose_network_snapshot_status == "captured"` unless `--skip-compose` was an explicit choice for that run.
  - `nix develop -c ./scripts/run_testnet_gen2_validation.sh` as the focused staged-send rehearsal; inspect `target/test-logs/testnet_gen2_validation/<timestamp>/compose/consensus.log` plus `target/test-logs/testnet_gen2_validation/<timestamp>/scenario-run/`.
  - `nix develop -c ./scripts/validate_req_res_gen2_rollout.rs --json-out target/test-logs/req_res_gen2_rollout_readiness/latest.json` as the cargo-only artifact gate over the benchmark and transcript bundle; inspect `target/test-logs/req_res_gen2_rollout_readiness/latest.json` plus `target/benchmarks/req_res_gen2/latest.json`.
  - `nix develop -c ./scripts/run_nous_mixed_generation_e2e.sh` with `NOCKCHAIN_BIN_OLD` set to the last pre-Nous release as the one-time pre-testnet validation for real old/new fallback.
- The underlying cargo-only proof bundle consumed by `validate_req_res_gen2_rollout.rs` is:
  - `nix develop -c make bench-req-res-gen2`
  - `nix develop -c ./scripts/test-req-res-gen2-e2e.sh --cargo-only`
  - `nix develop -c ./scripts/test-req-res-gen2-limits.sh --cargo-only`
- Required rollout evidence includes:
  - transport benchmark and request payload-fit artifacts
  - responder payload-fit artifacts with stop reasons
  - requester processing-cost artifacts
  - current-runtime resident-set artifacts for the checkpoint-backed runtime
  - two-peer driver-path latency artifacts
  - transcript-grade e2e and codec-boundary logs
- The rollout proof bundle uses `--cargo-only` (config-based interop tests), not recurring real old/new binary tests. Rationale: protocol negotiation is protocol-ID-based via libp2p multistream-select, not binary-version-based. The shipped default (`req_res_gen2_accept_enabled=false`, `req_res_gen2_send_enabled=false`) is pre-Nous-equivalent and omits `gen2` entirely. The explicit accept-only stage (`req_res_gen2_accept_enabled=true`, `req_res_gen2_send_enabled=false`) keeps outbound on `gen1` while registering `gen2` as inbound-only. The config-based E2E tests exercise those states alongside the rollout-critical negotiation paths (gen2 <-> gen2, gen2 -> gen1 fallback, mixed-config, renegotiation, rollback) without requiring old binary artifact maintenance in CI.
- Pre-testnet one-time validation: before enabling gen2 send on testnet, operators SHOULD run `scripts/run_nous_mixed_generation_e2e.sh` with `NOCKCHAIN_BIN_OLD` pointing to the last released pre-Nous binary. This validates real old/new fallback once; it is not a recurring CI gate.

### Rollout Model

Dual-stack operating model:
- Nous-capable nodes with `req_res_gen2_accept_enabled=true` register both protocol IDs.
- Shipped defaults register `gen1` only.
- The explicit accept-only rollout stage uses `gen1` as full support plus `gen2` as inbound-only support until send is explicitly enabled.
- Gen1 fallback remains available during migration.

Compatibility matrix:
- Old node <-> old node: `gen1` only.
- New default node <-> old node: `gen1` only.
- New accept-only node <-> old node: new node stays on `gen1` outbound while accepting inbound `gen2` from upgraded peers.
- New enabled node <-> old node: identify-derived peer support normally routes outbound traffic over `gen1`; stale gen2 attempts recover through unsupported-protocol gen1 fallback.
- Old node <-> new node: old node speaks `gen1`; new node accepts `gen1`.
- New node <-> new node: prefer `gen2` when local send is enabled and the remote advertises inbound gen2; otherwise negotiate according to support mode, commonly `gen1` outbound plus inbound-only `gen2` during staged rollout.

Independent node rollout:
- Nodes can be upgraded independently.
- Mixed-generation networks are supported by protocol-order negotiation and fallback.
- Shipped defaults keep gen2 accept, send, bundle upgrade, and prefetch disabled until rollout gate conditions are met in representative environments; operators may then enable accept-only, send, bundle, and prefetch stages explicitly.

## Activation

- **Height**: TBD (`activation_height = 0` in frontmatter).
- **Coordination**: staged opt-in dual-stack rollout; shipped defaults advertise `gen1` only, nodes use gen2 only when local send is enabled and the peer advertises inbound gen2, and gen1 fallback remains available during migration.

## Migration

### Operator Steps

1. Upgrade to Nous-capable node build.
2. Verify node advertises only the gen1 request-response protocol.
3. Keep `req_res_gen2_accept_enabled=false`.
4. Keep `req_res_gen2_send_enabled=false` while collecting rollout proof.
5. Keep `req_res_gen2_bundle_enabled=false` and `prefetch_enabled=false` until the staged send path is healthy.
6. Run `nix develop -c ./scripts/run_testnet_full_validation.sh` as the canonical recurring readiness gate before each staged rollout checkpoint.
   - Treat the run as incomplete unless `report.json` records `compose_network_snapshot_status == "captured"` when compose validation was enabled.
7. If you want the narrower staged-send rehearsal that mirrors the operator enablement sequence, run `nix develop -c ./scripts/run_testnet_gen2_validation.sh`.
8. If you need the cargo-only proof bundle on local hardware, run:
   - `nix develop -c make bench-req-res-gen2`
   - `nix develop -c ./scripts/test-req-res-gen2-e2e.sh --cargo-only`
   - `nix develop -c ./scripts/test-req-res-gen2-limits.sh --cargo-only`
   - `nix develop -c ./scripts/validate_req_res_gen2_rollout.rs --json-out target/test-logs/req_res_gen2_rollout_readiness/latest.json`
9. Before the first testnet enablement with `req_res_gen2_send_enabled=true`, run `nix develop -c ./scripts/run_nous_mixed_generation_e2e.sh` once with `NOCKCHAIN_BIN_OLD` pointing to the last released pre-Nous binary.
10. Inspect the integrated outputs, not only raw artifact directories:
   - `target/test-logs/testnet_full_validation/<timestamp>/report.json`
   - `target/test-logs/req_res_gen2_rollout_readiness/latest.json`
   - `target/benchmarks/req_res_gen2/latest.json`
11. Enable `req_res_gen2_accept_enabled=true`, then `req_res_gen2_send_enabled=true`, gradually only after representative hardware and traffic show acceptable latency, RSS, and fallback behavior.
12. Enable `req_res_gen2_bundle_enabled=true` after gen2 send is stable.
13. Enable `prefetch_enabled=true` after bundle/range behavior is validated for the target peer set.
14. Monitor generation, fallback, dedupe, bundle, prefetch, and backpressure metrics.

### Rollback Steps

1. Set `req_res_gen2_send_enabled=false`.
2. Set `req_res_gen2_bundle_enabled=false`.
3. Set `prefetch_enabled=false`.
4. Set `req_res_gen2_accept_enabled=false` to return to the shipped gen1-only baseline.
5. Continue on `gen1` transport path.

### Data Migration

- None.

## Backward Compatibility

This upgrade is transport-level and additive:
- Existing gen1 protocol IDs and encoding are preserved byte-for-byte.
- New nodes can communicate with old nodes via gen1 fallback.
- Old nodes continue operating on gen1 without protocol-level crashes from gen2 rollout.
- `%block-with-txs` request shapes are sent only from upgraded code paths and have classic `BlockByHeight` fallback on decode, too-large, unsupported-protocol, or no-range-peer paths.

This upgrade does not change transaction formats or consensus semantics, but it is liveness-critical transport:
- Transactions created by old software remain valid.
- Transactions created by new software remain valid.

## Security Considerations

Security-relevant points in this generation:
- PoW is unchanged and explicitly not treated as sufficient DoS protection.
- Abuse resistance relies on hard message/item limits, bounded queues, and per-peer inflight caps.
- Envelope metadata is advisory for routing/dedupe and MUST NOT be trusted over payload validation.
- Bundle envelopes cross-check decoded block IDs and tx IDs against envelope metadata before routing.
- Dedupe suppresses only redundant outbound wire requests, and MUST NOT suppress semantic side effects.
- Capability-aware peer selection is a local routing optimization. It MUST NOT weaken payload validation, fork detection, liar-block handling, or PoW verification.
- Unknown peers MUST be probed with bounded range length to limit amplification against the requester and responder.
- Peer scoring MUST remain local. Nodes MUST NOT accept remote claims about third-party range capability.

## Operational Impact

Operator-facing impact:
- Lower round-trip overhead for high-item request workloads when peers are gen2-capable.
- More explicit flow-control outcomes (admission reject vs partial item backpressure).
- Additional metrics/logs are required for rollout safety (generation selection, fallback rates, dedupe suppressions, queue pressure).
- Optional `BlockWithTxsByHeight` bundle requests can carry a block fact plus raw transaction facts, bounded by `gen2_item_max_bytes`.
- Optional `BlockRangeWithTxs` prefetch can carry a contiguous prefix of bundled blocks, bounded by `gen2_block_batch_max_response_bytes`.
- Catch-up prefetch should stop pinning to the first lexicographic peer and should prefer peers that have demonstrated `gen2` range service.
- Networks with no range-capable `gen2` peers should degrade to singleton block fetch rather than repeating full range attempts against incapable peers.

Rollout risk and mitigation:
- Mixed-version networks remain supported through gen1 fallback.
- Operators can disable gen2 send (`req_res_gen2_send_enabled=false`) as a rollback lever while still accepting gen2 if needed.
- Operators can disable bundle upgrades with `req_res_gen2_bundle_enabled=false`.
- Operators can disable catch-up range prefetch and its selector with `prefetch_enabled=false` while leaving the rest of Nous enabled.

## Testing and Validation

### Integrated Operator Entry Points

| Entry point | Scope | Operator cadence | Output to inspect |
| --- | --- | --- | --- |
| `nix develop -c ./scripts/run_testnet_full_validation.sh` | Broad recurring Nous validation, including steady-state gen2 transaction-path, soak, multi-sender, and partition/reorg coverage | Canonical recurring readiness gate before rollout checkpoints | `target/test-logs/testnet_full_validation/<timestamp>/report.json` (`compose_network_snapshot_status` must be `captured` unless compose was intentionally skipped) |
| `nix develop -c ./scripts/run_testnet_gen2_validation.sh` | Focused compose-plus-scenario rehearsal of the staged `nous_testnet_gen2_send` enablement path | Run when validating the specific staged-send operator flow or debugging that path | `target/test-logs/testnet_gen2_validation/<timestamp>/compose/consensus.log` plus `target/test-logs/testnet_gen2_validation/<timestamp>/scenario-run/` |
| `nix develop -c ./scripts/validate_req_res_gen2_rollout.rs --json-out target/test-logs/req_res_gen2_rollout_readiness/latest.json` | Reduce the cargo-only benchmark, E2E, and limits artifacts into one machine-readable readiness decision | Run whenever using the cargo-only proof bundle on representative operator hardware | `target/test-logs/req_res_gen2_rollout_readiness/latest.json` and `target/benchmarks/req_res_gen2/latest.json` |
| `nix develop -c ./scripts/run_nous_mixed_generation_e2e.sh` | Real old/new binary fallback proof against the last pre-Nous release | One-time pre-testnet validation before the first gen2-send enablement | `target/test-logs/nous_mixed_generation/<timestamp>.log` |

Relationship note:
- `run_testnet_full_validation.sh` is the broad recurring gate.
- `run_testnet_gen2_validation.sh` is the narrower staged-send drill built around the `nous_testnet_gen2_send` scenario.
- `validate_req_res_gen2_rollout.rs` is the integrated reducer for the cargo-only benchmark/transcript bundle.
- At the first checkpoint that folds in the one-time old/new proof, pass `--require-old-new-fallback`; the recurring report then fails review unless the companion cargo-only readiness JSON carries the embedded `old_new_fallback_proof` summary.
- `run_nous_mixed_generation_e2e.sh` is the one-time real old/new fallback proof and is not the recurring CI/operator gate.

### Validation Commands and Artifacts

Primary cargo-only artifact producer:

```sh
nix develop -c make bench-req-res-gen2
```

This writes a timestamped artifact directory under:

```text
target/benchmarks/req_res_gen2/<timestamp>/
```

Stable pointers:
- `target/benchmarks/req_res_gen2/latest.json`
- `target/benchmarks/req_res_gen2/<timestamp>/manifest.json`

Expected summary sidecars in the default rollout benchmark run:
- `recovery-path-summary.json`
- `responder-payload-fit-summary.json`
- `requester-cost-summary.json`
- `resident-set-summary.json`
- `two-peer-latency-summary.json`

Default rollout benchmark scenarios that write logs but no summary sidecar:
- `transport.log`
- `payload-fit.log`

Additional sidecars from `make bench-req-res-gen2-checkpoint`:
- `checkpoint-sizing-summary.json`
- `checkpoint-requester-cost-summary.json`
- `checkpoint-requester-profile-summary.json`

Expected log files in a default rollout benchmark run:
- `transport.log`
- `payload-fit.log`
- `recovery-path.log`
- `responder-payload-fit.log`
- `requester-cost.log`
- `resident-set.log`
- `two-peer-latency.log`

Supplemental proof commands:

```sh
nix develop -c ./scripts/test-req-res-gen2-e2e.sh --cargo-only
nix develop -c ./scripts/test-req-res-gen2-limits.sh --cargo-only
```

Those scripts emit timestamped transcript logs under:
- `target/test-logs/req_res_gen2_e2e/`
- `target/test-logs/req_res_gen2_limits/`

### Speed-of-Light Benchmark Gate

Use two complementary checks for the current implementation:

- Cargo-only latency gate: run `nix develop -c make bench-req-res-gen2`, then `nix develop -c ./scripts/validate_req_res_gen2_rollout.rs --json-out target/test-logs/req_res_gen2_rollout_readiness/latest.json`. The validator treats `two-peer-latency-summary.json` as the speed-of-light benchmark and currently requires `gen2-batch-4 >= 1.5x`, `gen2-batch-32 >= 10x`, and `gen2-batch-128-large >= 10x` versus the paired gen1 driver-path workloads.
- Staged-send rehearsal: run `nix develop -c ./scripts/run_testnet_gen2_validation.sh`. The `nous_testnet_gen2_send` scenario emits `scenario-run/reports/nous-peer-speedup.json`, asserts the mixed-generation fallback marker `Queued gen1 fallback after gen2 unsupported-protocol failure`, asserts a peer-specific `full-b -> full-a` completed gen2 exchange, and only passes when the peer-stats sampler observes at least `1.10x` gen2-over-gen1 throughput over a `5000 ms` sample window.

Measurement method:
- The cargo-only gate compares paired gen1 and gen2 workloads over the real request-response driver responder path, using `total_ms` from `two-peer-latency-summary.json`.
- The staged-send rehearsal samples live peer stats from the mixed-generation network and compares gen1 versus gen2 throughput over the same window used by the scenario assertion.

Scope: these gates verify the transport-layer batching primitive, the "supply side" of the gen2 crossover curve, on pre-formed batches. Whether a given workload realizes a given ratio in practice depends on the requester's emission pattern and is a property of batch formation, not of the transport. See `### D. Performance` for per-workload expectations. End-to-end sync throughput is measured separately by the `NB-01`/`NB-03` harness (`docs/NOUS-SYNC-BENCHMARK-HARNESS.md`) and is not a rollout gate in this generation.

### Requirement-to-Proof Matrix

| Requirement | Primary code path | Proof command / script | Proof artifact or test |
| --- | --- | --- | --- |
| Gen1 wire compatibility remains byte-stable | `src/messages.rs`, `src/cbor_tests.rs`, `testdata/req_res_gen1_cbor_vectors.json` | `nix develop -c cargo test -p nockchain-libp2p-io --lib -- --nocapture` | gen1 vector checks in `src/cbor_tests.rs` |
| Shipped defaults omit gen2, and `req_res_gen2_send_enabled=false` disables outbound gen2 | `src/behaviour.rs`, `src/driver.rs` | `nix develop -c ./scripts/test-req-res-gen2-e2e.sh --cargo-only` | behavior tests plus e2e transcripts showing gen1-only, gen1-first, or inbound-only negotiation |
| Opportunistic multi-item checkpoint-backed `BlockByHeight` batches use outbound gen2 send under the configured block-response budget, while singleton block fetches demote to gen1 | `src/config.rs`, `src/driver.rs`, `tests/req_res_gen2_e2e.rs` | `nix develop -c ./scripts/test-req-res-gen2-e2e.sh --cargo-only` | `req_res_block_by_height_batch_round_trip_uses_gen2` (E2E); supplemental helper/config coverage in `test_should_batch_request_respects_item_and_batch_limits`, `test_outbound_request_generation_respects_peer_support_for_singletons`, `test_gen2_block_batch_max_response_bytes_from_env` |
| Gen2 negotiation and fallback remain correct in mixed networks | `src/behaviour.rs`, `src/driver.rs`, `tests/req_res_gen2_e2e.rs` | `nix develop -c ./scripts/test-req-res-gen2-e2e.sh --cargo-only` | `req_res_gen2_four_node_rolling_rollback_stays_peer_scoped` (staged 4-node rollback stays peer-scoped), `req_res_multi_peer_restart_isolation_preserves_modern_gen2` (multi-peer restart isolation), `req_res_multi_peer_mixed_generation_fallback_stays_peer_scoped` (per-peer gen fallback), `req_res_gen2_sender_gen1_only_responder_reconnects_cleanly`, `req_res_protocol_renegotiation_upgrades_after_responder_restart`, `req_res_protocol_renegotiation_upgrades_after_requester_restart`, `req_res_gen2_rollback_reverts_outbound_to_gen1` |
| Oversize codec boundaries reject cleanly | `src/behaviour.rs`, `tests/req_res_limits.rs` | `nix develop -c ./scripts/test-req-res-gen2-limits.sh --cargo-only` | codec-boundary logs and limit tests |
| Receiver respects bounded partial-tail contract | `src/driver.rs`, `src/p2p_state.rs` | `nix develop -c cargo test -p nockchain-libp2p-io req_res_driver_ -- --nocapture` | driver unit coverage for bounded tail, retry, and inflight behavior |
| Generation-aware metrics and logs exist for rollout decisions | `src/metrics.rs`, `src/driver.rs` | `nix develop -c make bench-req-res-gen2` and `nix develop -c ./scripts/test-req-res-gen2-e2e.sh --cargo-only` | rollout logs, e2e transcripts, and metric declarations in `src/metrics.rs` |
| Request-side batching materially reduces round trips | `src/driver.rs` | `nix develop -c make bench-req-res-gen2` | `transport.log`, request transport benchmark rows in console output |
| Request-side container limits still fit current defaults | `src/driver.rs` | `nix develop -c make bench-req-res-gen2` | `payload-fit.log` |
| Responder-side fit heuristics stop with explicit reasons | `src/driver.rs`, `src/p2p_state.rs` | `nix develop -c make bench-req-res-gen2` | `responder-payload-fit-summary.json` |
| Requester-side batch result processing cost is measured, and timeout outcomes fail rollout readiness | `src/driver.rs` | `nix develop -c make bench-req-res-gen2` | `requester-cost-summary.json` |
| Current runtime RSS impact is measured on the checkpoint-backed runtime | `src/driver.rs` | `nix develop -c make bench-req-res-gen2` | `resident-set-summary.json` |
| Real two-peer req-res latency is measured over the driver responder path and held to explicit speed-of-light ratios | `src/driver.rs`, `scripts/validate_req_res_gen2_rollout.rs` | `nix develop -c make bench-req-res-gen2` then `nix develop -c ./scripts/validate_req_res_gen2_rollout.rs --json-out target/test-logs/req_res_gen2_rollout_readiness/latest.json` | `two-peer-latency-summary.json`; readiness check `two-peer-latency-speed-of-light` |
| Repeated backpressure reduces sender retry batch size | `src/driver.rs` | `nix develop -c cargo test -p nockchain-libp2p-io req_res_driver_retry_ -- --nocapture` | `test_build_retry_request_contexts_splits_batches_after_repeated_failures`, `req_res_driver_retry_respects_bounded_backoff_and_split_batches` |
| Duplicate batch `item_id` values are rejected before transport execution | `src/messages.rs`, `src/driver.rs` | `nix develop -c cargo test -p nockchain-libp2p-io --lib -- --nocapture` | `test_new_batch_request_rejects_duplicate_item_ids`, `test_batch_request_validation_rejects_duplicate_item_ids` |
| Unexpected extra `item_id` values in a `BatchResult` are skipped without corrupting retry accounting | `src/driver.rs` | `nix develop -c cargo test -p nockchain-libp2p-io --lib -- --nocapture` | `test_unexpected_batch_result_item_ids_returns_extra_ids`, `test_unexpected_batch_result_item_ids_returns_empty_when_all_expected`, `test_unexpected_batch_result_item_ids_with_mixed_expected_missing_and_extra`, `test_unexpected_item_ids_excluded_from_observed_set_and_retry_accounting`, `test_unexpected_item_ids_do_not_suppress_missing_item_retries` |
| Unknown gen2 result enums fail decode without panic | `src/cbor_tests.rs` | `nix develop -c cargo test -p nockchain-libp2p-io --lib -- --nocapture` | `test_unknown_batch_result_status_fails_without_panic`, `test_unknown_batch_error_class_fails_without_panic` |
| A fully disabled node remains gen1-only inside a gen2-active network | `tests/req_res_gen2_e2e.rs` | `nix develop -c ./scripts/test-req-res-gen2-e2e.sh --cargo-only` | `req_res_full_disable_peer_stays_gen1_only_in_gen2_network` |
| Gossip traffic remains singleton while gen2 batching is active | `tests/req_res_gen2_e2e.rs` | `nix develop -c ./scripts/test-req-res-gen2-e2e.sh --cargo-only` | `req_res_gen2_batching_keeps_gossip_singleton` |
| Gen2 PoW preimage uses explicit domain separation and canonical batch bytes | `src/messages.rs`, `src/cbor_tests.rs` | `nix develop -c cargo test -p nockchain-libp2p-io --lib -- --nocapture` | `test_gen2_pow_preimage_matches_spec_layout`, `test_batch_request_pow_verification_roundtrip` |
| Catch-up range prefetch selects only range-capable gen2 peers, probes unknown peers with bounded range length, and falls back to singleton block fetch when no safe range peer exists | `src/driver.rs`, `src/p2p_state.rs`, `src/metrics.rs`, `tests/req_res_gen2_e2e.rs` | `nix develop -c cargo test -p nockchain-libp2p-io prefetch_ -- --nocapture` and `nix develop -c ./scripts/test-req-res-gen2-e2e.sh --cargo-only` | selector unit tests for eligibility, weighted rendezvous ranking, cooldown, probe bounds, and singleton fallback; e2e transcript showing no full range request to gen1-only peers |

Edge-case audit note (2026-03-24):
- The request vocabulary now includes singleton requests plus `BlockWithTxsByHeight` and `BlockRangeWithTxs { start_height, len }`. `BlockRangeWithTxs` is the only count-bearing request shape, and the responder may return a contiguous prefix shorter than `len` when the chain ends, a gap is found, or the response budget fills. Bounds, prefix behavior, and singleton fallback are covered by driver and gen2 request-execution tests.

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

**Scope note: transport layer vs. end-to-end throughput.** Gen2 is a transport-layer batching primitive. Its speedup relative to gen1 depends on realized batch size, response shape, and whether the requester can form a batch within the coalescing window.

Current code behavior:
- A one-item pending batch containing only classic `BlockByHeight` demotes to gen1 at flush time.
- Multi-item classic block batches remain gen2 when outbound gen2 send is enabled and the peer supports gen2.
- Raw transaction and elders requests can batch up to `gen2_batch_max_items = 64` or `gen2_batch_max_bytes = 4_194_304`.
- Bundle/range responses are constrained by `gen2_item_max_bytes = 2_097_152` and `gen2_block_batch_max_response_bytes = 2_097_152`.
- Range prefetch estimates the window from response-size hints. Cold estimates use `128 KiB` per block, target `31/32` of the block response budget, and clamp to `prefetch_window_initial..=prefetch_window_max`.
- Gossip remains singleton.

Benchmark and validation harness:
- `make bench-req-res-gen2` runs the default rollout scenarios: `transport`, `payload-fit`, `responder-payload-fit`, `requester-cost`, `recovery-path`, `resident-set`, and `two-peer-latency`.
- `make bench-req-res-gen2-checkpoint` runs checkpoint-backed `checkpoint-sizing`, `checkpoint-requester-cost`, and `checkpoint-requester-profile` scenarios under `target/benchmarks/req_res_gen2_checkpoint`.
- `make bench-req-res-gen2-transport-tuning` runs the transport tuning matrix.
- `scripts/validate_req_res_gen2_rollout.rs` requires the seven default scenarios by default, can require the checkpoint suite with `--require-checkpoint-suite`, and can validate a checkpoint-only manifest with `--checkpoint-suite-only`.
- The validator's current speed-of-light checks require `gen2-batch-4 >= 1.5x`, `gen2-batch-32 >= 10x`, and `gen2-batch-128-large >= 10x` versus their paired gen1 workloads in `two-peer-latency-summary.json`.
- Checkpoint target fit is optional unless `--checkpoint-target-must-fit` is supplied.

Performance rollout checklist:
- default rollout benchmark manifest includes all seven default scenarios;
- readiness reducer passes against `target/benchmarks/req_res_gen2/latest.json`;
- no timeout outcome is accepted by requester-cost validation;
- recovery-path validation observes fallback behavior and replacement success;
- responder payload-fit validation records explicit stop reasons;
- current-runtime resident-set scenarios complete;
- two-peer speed-of-light benchmark meets the validator thresholds;
- checkpoint suite passes when that suite is required for the rollout checkpoint.

### Benchmark-tuning finalization

Defaults in `LibP2PConfig`:
- `gen2_batch_max_items = 64`
- `gen2_batch_max_bytes = 4_194_304`
- `gen2_item_max_bytes = 2_097_152`
- `gen2_block_batch_max_response_bytes = 2_097_152`
- `gen2_batch_coalesce_window_ms = 10`
- `gen2_max_inflight_per_peer = 32`

Rationale captured in `config.rs`:
- `gen2_batch_max_items = 64`: transport tuning on recovery/fan-out shapes found that 64-item batches retain almost all request reduction from 128-item batches with lower tail delay under the current 10 ms coalescing window.
- `gen2_batch_max_bytes = 4_194_304`: the full chain-history sweep observed a maximum individual raw transaction of 1.2 MiB and a maximum block-plus-transactions bundle of 1.34 MiB. The cap gives roughly 1.5x to 3x headroom above observed extremes while staying below consensus `max-block-size` at 8 MiB.
- `gen2_item_max_bytes = 2_097_152`: per-item responses remain bounded independently of the whole batch cap.
- `gen2_block_batch_max_response_bytes = 2_097_152`: response-budgeted block bundle/range batches stay on a tighter requester replay budget.
- CBOR request and response codec caps derive from `gen2_batch_max_bytes` in `behaviour.rs`.

### E. Abuse and Pressure Behavior
- over-limit batch rejection
- bad-PoW rejection
- per-item malformed payload isolation
- queue saturation/backpressure behavior without panic or deadlock
- per-peer inflight cap enforcement
- sender retry/backoff behavior under repeated backpressure

### F. Catch-Up Peer Selection
- gen2-supported peer beats lexicographically earlier gen1 peer for `BlockRangeWithTxs`
- `Gen2 + Supported` peer beats `Gen2 + Unknown` peer for normal range prefetch
- unknown-peer probe uses `PREFETCH_RANGE_PROBE_LEN = 2` and updates capability on success or decode failure
- transient range failure opens cooldown and queues singleton retry work for the start height through the alternate-peer path
- unsupported-protocol, decode, or too-large range failure marks the peer range-unsupported
- no eligible range peer triggers singleton `BlockByHeight` fallback
- weighted rendezvous ranking is stable for a fixed local peer, request range, candidate set, and score snapshot
- inflight and bandwidth caps exclude peers before scoring
- unknown and unsupported peers cannot receive full range prefetch requests through the normal eligibility gate

## Implementation File Map

Primary implementation files:
- `open/crates/nockchain-libp2p-io/src/messages.rs`
- `open/crates/nockchain-libp2p-io/src/driver.rs`
- `open/crates/nockchain-libp2p-io/src/driver/gen2.rs`
- `open/crates/nockchain-libp2p-io/src/driver/gen2/batch.rs`
- `open/crates/nockchain-libp2p-io/src/driver/gen2/frontier.rs`
- `open/crates/nockchain-libp2p-io/src/driver/gen2/inbound.rs`
- `open/crates/nockchain-libp2p-io/src/driver/gen2/outbound.rs`
- `open/crates/nockchain-libp2p-io/src/driver/gen2/request_exec.rs`
- `open/crates/nockchain-libp2p-io/src/driver/gen2/responses.rs`
- `open/crates/nockchain-libp2p-io/src/driver/gen2/routing.rs`
- `open/crates/nockchain-libp2p-io/src/driver/kernel_io.rs`
- `open/crates/nockchain-libp2p-io/src/behaviour.rs`
- `open/crates/nockchain-libp2p-io/src/config.rs`
- `open/crates/nockchain-libp2p-io/src/p2p_state.rs`
- `open/crates/nockchain-libp2p-io/src/metrics.rs`
- `Makefile` (`bench-req-res-gen2` harness entrypoint)
- `scripts/run_req_res_gen2_rollout.rs`
- `scripts/validate_req_res_gen2_rollout.rs`
- `scripts/test-req-res-gen2-e2e.sh`
- `scripts/test-req-res-gen2-limits.sh`
- `scripts/run_nous_mixed_generation_e2e.sh`
- `open/crates/nockchain-libp2p-io/src/cbor_tests.rs`
- `open/hoon/apps/dumbnet/inner.hoon`

Notes:
- Hoon changes are current read-only scry support for `heavy-txs` and `heaviest-chain-blocks-range`; Hoon does not emit batch transport effects in this generation.
- Any future Hoon-side batching is a transport optimization and must preserve gen1 compatibility paths.

## Resolved Design Decisions

1. PoW policy for large batches
- Decision: keep current fixed-cost PoW policy and strict transport caps in this generation.
- Decision: include explicit PoW preimage domain/version separator (`nockchain:req-res:gen2:pow:v1`).
- Deferred: count-weighted/account-weighted PoW redesign.

2. Default gen2 flags in shipped config
- Decision: `req_res_gen2_accept_enabled=false`, `req_res_gen2_send_enabled=false`, `req_res_gen2_bundle_enabled=false`, and `prefetch_enabled=false` in shipped defaults until rollout gate stages are explicitly enabled.

3. Envelope strictness
- Decision: envelope strictness is keyed by `kind` plus the relevant ID, message, bundle, or range fields for that kind.
- `height` removed.

4. Batching source
- Decision: Rust driver coalescing is the primary batch-formation mechanism.
- Decision: current `BlockByHeight` batching comes from Rust driver coalescing only, using whatever independent height requests are present within one flush interval. A lone classic `BlockByHeight` pending batch demotes to gen1.
- Decision: Rust prefetch emits `BlockRangeWithTxs` under `prefetch_enabled`, and Rust bundle upgrade emits `BlockWithTxsByHeight` under `req_res_gen2_bundle_enabled`.
- Deferred: broader Hoon-side batch emitters for other request shapes.

5. Request-response behavior architecture
- Decision: single request-response behavior with deterministic protocol ordering.
- Decision: transport generation fallback remains peer-scoped and outcome-driven.
- Decision: catch-up range routing maintains local per-peer range capability and performance state when `prefetch_enabled=true`.

6. Backpressure response contract
- Decision: if execution has started, return partial per-item `Error(Backpressure)` for unprocessed tail items.
- Decision: if admission fails before execution, reject whole batch.

7. Queue pressure policy
- Decision: bounded queues, no unbounded wait, immediate reject on full admission queue.

8. Per-peer inflight default and enforcement mode
- Decision: enforce hard cap (`gen2_max_inflight_per_peer`) with immediate reject at cap.

9. Capability caching policy
- Decision: `gen2` identify support is necessary but not sufficient for `BlockRangeWithTxs`.
- Decision: range capability is tracked as local requester state: `Unknown`, `Supported`, or `Unsupported`.
- Decision: successful range responses mark `Supported`; decode and unsupported-protocol failures mark `Unsupported`; transient failures update cooldown without changing shape capability.
- Decision: unknown range capability may be probed only with `len <= 2`.
- Decision: full catch-up range prefetch is restricted to `Gen2 + Supported` peers; unknown peers receive only bounded probes, and unsupported peers receive no range request.

10. Catch-up prefetch peer ranking
- Decision: replace lexicographic first-peer selection for catch-up range prefetch with capability-gated weighted rendezvous ranking.
- Decision: ranking weight combines smoothed success probability, smoothed RTT, smoothed payload size, and active inflight penalty.
- Decision: transient range failure opens peer cooldown and queues singleton retry work for the start height through the alternate-peer path.
- Decision: absence of an eligible range peer falls back to singleton `BlockByHeight`, not full range requests to incapable peers.
