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
