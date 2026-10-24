# `nockapp-grpc`

`nockapp-grpc` implements the private NockApp and public Nockchain gRPC services defined by `nockapp-grpc-proto`. It adapts RPC requests to NockApp peeks, pokes, effect streams, and chain-query drivers; it does not implement consensus.

## Place in the system

```text
wallet / miner / API client
           |
      protobuf + gRPC
           |
      nockapp-grpc
       /         \
private NockApp   public Nockchain services
       |                    |
  kernel handle       indexed/query state
```

The regular node uses the private service for trusted local control. `nockchain-api` additionally enables public services. External miner binaries use `WatchEffects` for candidates and `Poke` for submissions.

## Maintained invariants

- RPC noun payloads are canonical JAM on success and fail explicitly on decode errors.
- Private and public service implementations remain distinct. Private `Poke` exposes kernel command authority and is not safe as an unauthenticated public API.
- `WatchEffects` creates one fresh subscriber per client and preserves the order observed by that subscriber.
- The effect bus and per-client forwarding channel are bounded. A lagging subscriber receives a terminal stream error and must reconnect before acting on stateful work.
- Effect filtering compares only the raw head atom and does not parse or validate the effect tail.
- A successful `PokeResponse` reports delivery/acknowledgement, not consensus acceptance of any block or transaction produced by the command.
- Public query caches follow reported chain state but are not protocol authority. Callers must tolerate warm-up, reorganization, and stale-read windows documented by the API binary.
- Explorer range commits require contiguous heights and parent links, a matching adjacent cached boundary, matching seed/final tip identity where applicable, and the current chain generation. Reorg reconciliation atomically retains hash-verified common ancestry and replaces the orphan suffix using a tip-anchored seed. A decoded replacement with no proven common ancestor discards unverified history. Old-generation work cannot commit.
- Detected divergence quarantines retained cache state before replacement I/O: canonical cached positives, misses, prefixes and pagination return retryable `Unavailable` until validated reconciliation succeeds. Transient replacement-fetch failures retain indexes and backfill progress; old-generation reads/backfill cannot publish into or clear a recovered generation. Explicit consistency failures still discard unverified history.
- Both transaction endpoints classify a confirmation-index miss only when canonical history is complete: a present raw transaction returns `Pending`, while an encoded absent raw transaction returns `NotFound`. Incomplete indexing or a concurrent reorganization returns retryable `Unavailable`; malformed or failed scries remain errors.
- Historical backfill keeps one bounded range in flight, yields between successful chunks, and retains retry backoff when history is unavailable. Successful chunks have no mandatory fixed delay.
- Explorer consistency checks do not make reads linearizable with the kernel. Cached data can lag until a reorg is observed, and warm-up or recovery can leave gaps. Freshness is eventual without a fixed convergence deadline.

## Security and distributed-systems properties

Transport decoding, service dispatch, and backpressure must be bounded because requests and public clients are untrusted. gRPC and protobuf do not provide consensus validity. Every block, transaction, proof, and noun still passes the kernel's normal validation path.

`WatchEffects` is a low-latency notification channel, not durable storage. Consumers recover by accepting replacement candidates, reconnecting after stream loss, or querying current state; they must not require exactly-once delivery.

Authentication, authorization, TLS termination, request quotas, and Internet exposure are deployment concerns unless a specific service implements them. The private endpoint should normally bind only to a trusted local interface.

## Validation

```sh
cargo test -p nockapp-grpc
cargo test -p nockapp-grpc --all-features
```
