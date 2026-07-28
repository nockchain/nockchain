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

## Security and distributed-systems properties

Transport decoding, service dispatch, and backpressure must be bounded because requests and public clients are untrusted. gRPC and protobuf do not provide consensus validity. Every block, transaction, proof, and noun still passes the kernel's normal validation path.

`WatchEffects` is a low-latency notification channel, not durable storage. Consumers recover by accepting replacement candidates, reconnecting after stream loss, or querying current state; they must not require exactly-once delivery.

Authentication, authorization, TLS termination, request quotas, and Internet exposure are deployment concerns unless a specific service implements them. The private endpoint should normally bind only to a trusted local interface.

## Validation

```sh
cargo test -p nockapp-grpc
cargo test -p nockapp-grpc --all-features
```
