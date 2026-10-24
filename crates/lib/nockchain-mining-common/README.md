# `nockchain-mining-common`

`nockchain-mining-common` contains the transport and configuration shared by Nockchain's external miner binaries. It does not implement a puzzle or decide whether a proof is valid.

## Place in the system

Both `zk-pow-miner` and `ai-pow-miner` run outside the node process. They use this crate to:

- parse mining reward-key configuration;
- decode `%mine-zk` and `%mine-ai` candidate effects;
- connect to the node's private `NockAppService` gRPC endpoint;
- configure mining through kernel pokes;
- subscribe to the live effect stream; and
- submit a puzzle-specific mined-block command.

Each miner retains its own wire source and proof vocabulary. This crate deliberately does not erase the distinction between ZK-PoW and AI-PoW submissions.

## Maintained invariants

- `MiningCandidate` preserves the puzzle variant, block commitment, target, and proof-length fields emitted by the kernel.
- Candidate decoding is structural and fallible. Malformed nouns produce errors rather than partially initialized jobs.
- Pokes are tagged with the caller's `WireRepr`; ZK and AI miners cannot silently submit under one shared source tag.
- Mining-key configuration distinguishes legacy public keys from v1 payee hashes and forwards the exact kernel command expected for each.
- The effect stream is a live, bounded broadcast, not a durable queue. Slow or disconnected miners may miss candidates and must treat a reconnect or new effect as replacement work.
- A gRPC acknowledgement confirms command delivery, not block validity. Consensus validation remains inside the node.

## Distributed-systems assumptions

The private gRPC service is trusted operator infrastructure and should not be exposed publicly. Network interruption, stream lag, duplicate candidates, and stale work are normal conditions. Miner implementations must make work cancellation idempotent and must never infer canonical-chain acceptance from local submission success.

The kernel is the source of truth for candidate identity and target. This crate does not cache chain state or perform fork choice.

## Validation

```sh
cargo test -p nockchain-mining-common
cargo check -p nockchain-mining-common
```
