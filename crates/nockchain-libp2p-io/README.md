# `nockchain-libp2p-io`

`nockchain-libp2p-io` connects a NockApp kernel to Nockchain's peer-to-peer network. It implements libp2p behavior, gossip and request/response transport, catch-up requests, peer state, traffic prioritization, metrics, and peer/IP abuse controls.

## Place in the system

The crate sits between untrusted peers and the node's kernel:

```text
remote peers <-> QUIC/libp2p <-> nockchain-libp2p-io <-> NockApp effects/pokes
                                                           |
                                                     Hoon validation
```

It transports blocks and chain data but does not decide their validity or fork-choice weight.

## Peer protocol v3

The request/response protocol is `/nockchain-3-req-res` over the existing
libp2p QUIC transport. Each request and response has one four-byte big-endian
length prefix, one protobuf body, and EOF. The receiver finishes the frame
before decoding or dispatching it. There is no v2 fallback; deployment requires
the coordinated rollout described in the draft
[Ecclesia specification](../../changelog/protocol/018-ecclesia.md).

The [peer schemas](proto/README.md) describe complete transactions and both
page versions. Generated Prost messages stay inside `src/v3`: checked Rust
constructors enforce presence, variants, numeric domains, collection
uniqueness, and relationships before constructing consensus nouns. Arbitrary
consensus noun fields use a checked, flat node table. No peer field contains JAM.

The existing driver still accepts internal JAM messages during this migration.
The inbound adapter creates those bytes locally from checked values. The
outbound adapter checks that the typed representation reconstructs the exact
local message, so it cannot silently change consensus data or the request PoW
preimage. PoW retains sender/receiver binding and uses v3 domain separators;
it does not commit to protobuf serialization. Kernel consensus checks still
establish transaction and block validity.

Protobuf's normal merge and unknown-field rules apply. Validation sees the
completed DTO, and only the checked interpretation reaches the driver. The
build enforces the schema restrictions in `build_support/schema.rs`; schema
compatibility checks and domain tests must accompany future changes.

## Maintained invariants

- Peer input is untrusted and bounded before it can consume unbounded memory, work, or queue capacity.
- Request/response messages retain request identity, expected peer, range, and response-size constraints across retries and fallbacks.
- Duplicate suppression and batching may change transport work, never the logical block or transaction presented to the kernel.
- Catch-up prefetch is advisory. Missing, reordered, duplicated, or stale responses cannot bypass kernel validation.
- Peer IDs, connection IDs, and network addresses remain distinct identities. Address/IP exclusions are applied only when the address is actually resolved for the offending connection.
- Objective cryptographic abuse may escalate to peer and address/IP exclusion. Reasons that can arise from protocol-version disagreement remain peer-scoped so an upgrade boundary cannot incorrectly ban an address shared by honest peers.
- Slow or malicious peers cannot hold global driver state locks across outbound channel waits.
- Transport penalties and peer selection affect availability and resource use, not consensus acceptance.

## Security and distributed-systems dependencies

QUIC/TLS and libp2p authenticate transport peers, not blocks. Block authenticity and validity come from consensus proofs, signatures, hashes, and the Hoon validation path. Kademlia and gossip are discovery/distribution mechanisms and may return adversarial data.

Network correctness assumes eventual access to at least one honest peer for synchronization. The implementation must remain safe under Byzantine peers, duplicated messages, partial responses, disconnects, reordering across connections, and local cache loss. Liveness policies must not create a second interpretation of protocol validity.

## Validation

```sh
cargo test -p nockchain-libp2p-io --lib
cargo check -p nockchain-libp2p-io
```

The integration harness under `test_support` is for tests and does not weaken production admission rules.
