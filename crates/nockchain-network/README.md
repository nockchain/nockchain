# `nockchain-network`

`nockchain-network` connects a NockApp kernel to Nockchain's peer-to-peer network. The shared core owns gossip, request/response semantics, catch-up requests, peer state, traffic prioritization, metrics, and peer/IP abuse controls; bounded transport actors provide libp2p or direct-only Iroh connectivity.

## Place in the system

The crate sits between untrusted peers and the node's kernel:

```text
remote peers <-> libp2p or Iroh <-> nockchain-network <-> NockApp effects/pokes
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

QUIC/TLS authenticates transport peers, not blocks. Block authenticity and validity come from consensus proofs, signatures, hashes, and the Hoon validation path. The libp2p Kademlia backend and Iroh's simplified Kademlia-style peer exchange are discovery mechanisms and may return adversarial data.

Network correctness assumes eventual access to at least one honest peer for synchronization. The implementation must remain safe under Byzantine peers, duplicated messages, partial responses, disconnects, reordering across connections, and local cache loss. Liveness policies must not create a second interpretation of protocol validity.

## Backend selection

libp2p remains the default. Select direct-only Iroh with `--network-backend iroh`. Iroh disables relays, port mapping, and external address lookup; bootstrap peers therefore require direct IP/UDP multiaddresses with trailing `/p2p/<node-id>` identities. Until direct Iroh backbone addresses are configured, pass `--no-default-peers` and supply at least one direct `--peer`. Use `--iroh-advertise <socket-address>` when the bind address is unspecified or differs from the address other peers can dial.

## Validation

```sh
cargo test -p nockchain-network --lib
cargo check -p nockchain-network
```

The integration harness under `test_support` is for tests and does not weaken production admission rules.
