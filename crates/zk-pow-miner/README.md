# `zk-pow-miner`

`zk-pow-miner` is the standalone miner for Nockchain's ZK-PoW `puzzle-nock` STARK. The `zk-pow-mine` binary connects to a running node, watches ZK mining candidates, proves them in a worker pool, and submits `%pow` solutions.

## Place in the system

```text
nockchain kernel --%mine-zk effect--> run loop --> worker pool
       ^                                             |
       |                                      miner.jam / STARK prover
       +---------------- %pow poke ------------------+
```

The node and miner are separate processes. `nockchain-mining-common` owns candidate decoding and private gRPC transport. Each worker runs the miner Hoon kernel from `kernels-open-miner` through a `SerfThread`. The Nockchain node independently validates submitted proofs and applies consensus rules.

## Maintained invariants

- Work is bound to the candidate's version, block commitment, target, and proof length.
- Workers receive immutable jobs. Replacement candidates cancel or supersede stale work rather than mutating a job in place.
- Worker results are associated with the job that produced them; a late result cannot be submitted as a solution to a newer commitment.
- The miner's `%pow` wire source remains distinct from `%ai-pow` and other kernel commands.
- The private effect stream is best-effort and bounded. Missing an effect can reduce miner liveness but cannot change node consensus.
- A submitted proof is never trusted because it came from the reference miner. The node checks proof version, exact target, block commitment, and STARK validity.

## Cryptographic and consensus dependencies

The miner relies on the Nock ZKVM/STARK prover and the proof-verification jets used by the node. Soundness depends on the Nock proof system, Fiat-Shamir transcript, hash functions, and Hoon/Rust noun agreement. Chain safety additionally depends on the Hoon kernel's ASERT, work accounting, fork choice, transaction validation, and coinbase rules; none are implemented here.

The miner is a liveness component, not a consensus authority. A faulty miner can waste its own work or submit invalid blocks but must not make a conforming node accept one.

## Validation

```sh
cargo test -p zk-pow-miner
cargo run --release -p zk-pow-miner --bin zk-pow-mine -- --help
```

Changes to `miner.hoon` require rebuilding `assets/miner.jam` before rebuilding this binary.
