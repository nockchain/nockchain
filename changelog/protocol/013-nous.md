+++
version = "1.0.0"
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

Nous is the generation-2 libp2p request-response protocol. It batches data requests and responses, authenticates gossip with EquiX proof-of-work, supports bundled block data, and provides bounded catch-up prefetch.

## Summary

Nodes support exactly one request-response protocol ID:

- `/nockchain-2-req-res`

Generation 1 is not registered, negotiated, decoded, or used as a fallback. A peer that only supports `/nockchain-1-req-res` is incompatible and must upgrade before joining the network.

Every accepted request, including gossip, carries sender-bound proof-of-work and participates in replay protection.

## Motivation

The transport batches data requests under one proof-of-work solve and authenticates gossip with sender-bound proof-of-work.

Generation 2 provides:

- one proof-of-work solve for a bounded batch of data requests;
- one response containing independently classified results;
- authenticated gossip with no unauthenticated compatibility path;
- explicit item, batch, response, queue, and per-peer inflight bounds;
- optional bundled block-with-transactions and range-prefetch optimizations.

## Protocol Contract

### Registration

A node MUST register `/nockchain-2-req-res` as full inbound and outbound support.

A node MUST NOT:

- register `/nockchain-1-req-res`;
- advertise inbound-only or outbound-only rollout modes;
- select a protocol generation per peer;
- retry `UnsupportedProtocols` failures over generation 1;
- decode generation-1 `Request` or `Gossip` wire values.

The former `req_res_gen2_accept_enabled` and `req_res_gen2_send_enabled` configuration fields are removed. Generation 2 is unconditional.

### Request Schema

```rust
enum NockchainRequest {
    BatchRequest {
        pow: [u8; 16],
        nonce: u64,
        items: Vec<BatchRequestItem>,
    },
    AuthenticatedGossip {
        pow: [u8; 16],
        nonce: u64,
        message: ByteBuf,
    },
}

struct BatchRequestItem {
    item_id: u32,
    message: ByteBuf,
}
```

A singleton data request is encoded as a one-item `BatchRequest`; there is no singleton transport variant.

The inner data-request vocabulary includes:

```rust
enum NockchainDataRequest {
    BlockByHeight(u64),
    EldersById(String, PeerId, NounSlab),
    RawTransactionById(String, NounSlab),
    BlockWithTxsByHeight(u64),
    BlockRangeWithTxs { start_height: u64, len: u8 },
}
```

### Response Schema

```rust
enum NockchainResponse {
    BatchResult { results: Vec<BatchResultItem> },
    Ack { acked: bool },
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
    Invalid,
    Backpressure,
    Internal,
}
```

`NockchainResponse::Result` may exist as an internal construction type, but it MUST NOT serialize on the network. A `BatchRequest` MUST receive `BatchResult`; `AuthenticatedGossip` MUST receive `Ack`. Shape mismatches are protocol errors.

### Proof-of-Work and Replay Protection

Both request variants MUST carry valid EquiX proof-of-work.

The proof preimage binds:

- sender peer ID;
- receiver peer ID;
- request variant and full request payload;
- nonce.

Changing the peer order, nonce, item order, item ID, item payload, or gossip payload MUST invalidate the proof.

Inbound processing MUST verify proof-of-work before routing work to the kernel. Accepted proofs participate in bounded replay detection. Invalid or replayed requests are rejected without kernel execution.

### Batch Execution

- Outbound kernel requests enter the generation-2 batch queue, including singleton requests.
- Batch items are executed in wire order.
- Item IDs correlate results independently of payload contents.
- One item failure MUST NOT roll back completed items.
- Retry logic MUST requeue only retryable or missing items and MUST keep generation-2 request shapes.
- Response envelopes are validated before facts are routed to the kernel.
- Bundled transactions are routed before their containing block fact.
- Fair-queue scheduling recovers poisoned locks and stale scheduling tokens.

### Limits and Pressure Controls

Implementations MUST enforce configured bounds before allocating or executing untrusted work:

- `gen2_batch_max_items`;
- `gen2_batch_max_bytes`;
- `gen2_item_max_bytes`;
- `gen2_block_batch_max_response_bytes`;
- `gen2_max_inflight_per_peer`;
- `gen2_swarm_action_queue_capacity`;
- request and gossip admission buckets;
- CBOR decoding rejects more than 4,096 batch items, 262,144 items in one bundle list, 255 range blocks, or 4 MiB of aggregate container-element storage;
- jam decoding is capped at 1,000,000 encoded noun positions per network payload;
- every cue decoder validates declared atom sizes against the remaining input before allocation;
- owned-noun conversion of decoded values uses an iterative worklist with a budget of 2^20 nodes;
- noun re-homing after decode uses an iterative worklist;
- the tx-source hint order queue is capped by its own length, independently of live-map entries removed per ID or on disconnect;
- runtime commands follow a strict schema;
- noun digest hashing uses an iterative worklist;
- DAG-aware noun validation memoizes on source pointers, and logical-expansion walks are budgeted at 2^20 nodes;
- the `dor`/`gor`/`mor` order jets compare by value using a pointer fast path, cached-mug pre-filter, and budgeted structural walk; separately allocated equal atoms order identically to the pure `+dor` gate, preserving canonical sorted state (z-sets, z-maps) across jet configurations;
- IP exclusions, address cooldowns, peer cooldowns, IP history, and per-IP evidence are each capped by `max_exclusion_entries`; unrelated events cannot evict retained per-IP strikes;
- deferred heard-block buffering is capped at 4,096 entries and 64 MiB per peer, and at 65,536 entries and 256 MiB globally.

A request that cannot fit the item or batch caps is rejected rather than emitted through an alternate protocol.

Automatic configured startup, backbone, and recovery dialing uses endpoint-scoped
single-flight admission: bootstrap and backbone requests do not start another
configured attempt while one is pending. Actual terminal connection events or
connection establishment update pending-attempt tracking so normal retries can
proceed. Manual and force dialing remain distinct; this does not impose
single-flight behavior on all manual dials or broaden the exact configured
endpoint, expected-identity, or recovery exceptions. Other address/IP exclusions,
admission limits, and recovery-slot protection remain effective.

Connection health is tracked by authenticated connection ID. A successful ping
resets only that connection's consecutive-failure count, and reaching the
configured threshold closes only that connection. Closing a connection removes
its health state. Shared-IP admission, request quotas, and address hygiene remain
independent.

### Gossip

Outbound gossip is converted to `AuthenticatedGossip` only after a destination peer is selected, because the proof binds the receiver peer ID. The sender solves EquiX proof-of-work for that peer and immutable message payload.

Inbound gossip MUST pass:

1. request decoding;
2. proof-of-work verification;
3. replay protection;
4. admission controls;
5. message decoding and kernel routing.

There is no unauthenticated gossip compatibility mode.

### Bundles and Prefetch

`req_res_gen2_bundle_enabled` controls whether eligible block requests use block-with-transactions response envelopes. `prefetch_enabled` controls catch-up range prefetch. These are data-shape and scheduling optimizations; neither changes protocol generation or consensus rules.

Range prefetch MUST preserve:

- bounded range length and response byte budgets;
- per-peer capability and cooldown state;
- deterministic fallback to generation-2 singleton batches when a range cannot be used;
- normal block and transaction validation.

## Observability

Live peer statistics report generation 2. Generation-1 metric fields retained in public telemetry schemas are historical compatibility fields and are never incremented by networking logic.

Operators should monitor:

- authenticated gossip verification and rejection;
- request replay rejection;
- batch item and byte counts;
- queue pressure and per-peer inflight rejection;
- response-shape and envelope validation failures;
- request timeouts and retries;
- bundle and range-prefetch outcomes, including deferred-buffer entry and retained-byte gauges.

## Activation and Migration

- **Height**: no consensus-height trigger; this is a transport compatibility cutover.
- **Coordination**: all participating peers must run generation-2-capable software.
- **Data migration**: none.

Before deployment:

1. Upgrade every node that must remain reachable.
2. Remove `NOCKCHAIN_LIBP2P_REQ_RES_GEN2_ACCEPT_ENABLED` and `NOCKCHAIN_LIBP2P_REQ_RES_GEN2_SEND_ENABLED` from node configuration; they no longer exist.
3. Verify nodes advertise `/nockchain-2-req-res` only.
4. Verify authenticated gossip and one-item/multi-item batch exchanges between representative peers.
5. Keep bundle and prefetch tuning within the tested item, byte, and inflight bounds.

Downgrading one peer to generation 1 makes that peer protocol-incompatible. There is no runtime rollback flag or generation-1 fallback.

## Validation

The implementation is validated by:

- CBOR round trips and canonical generation-2 vectors;
- rejection of legacy `Gossip` CBOR;
- sender/receiver/payload proof-of-work binding tests;
- authenticated gossip two-peer round trips;
- batch result status and response-envelope validation tests;
- singleton and multi-item generation-2 request tests;
- timeout, retry, queue-pressure, and per-peer inflight tests;
- block-by-height, bundle, and range-prefetch tests.
