# `ai-pow-zk`

`ai-pow-zk` is the Plonky3 proof stack for Nockchain AI-PoW. It proves the Pearl-compatible useful-work statement supplied by `ai-pow` and produces the compact recursive certificate embedded in `%ai-pow` blocks.

The crate is internal and unpublished. Its AIR layout and certificate format are consensus-sensitive.

## Place in the system

`ai-pow` constructs the canonical statement and witness. `ai-pow-zk` proves and verifies that statement. `ai-pow-miner` serializes the compact certificate, while `ai-pow-jets` selects verifier-owned setup and invokes verification from Hoon consensus.

The dependency direction is intentional: `ai-pow` depends on `ai-pow-zk`; this crate does not depend back on `ai-pow`. Bridge callers translate protocol types into `ZkParams`, trace rows, public inputs, and the canonical program.

## Production proof route

1. **Layer 0:** a pinned composite batch STARK proves the useful-work trace over Goldilocks. Selector constraints and seven LogUp buses bind BLAKE3 operations, committed/noised operands, matmul, routing where present, tile-state fold, and jackpot public inputs.
2. **Layer 1:** a recursive Tip5-friendly verifier circuit verifies Layer 0 and exposes a statement digest bound to the canonical Layer-0 program and public values.
3. **Layer 2:** a native BLAKE3 batch STARK proves the Layer-1 verifier execution and is path-pruned into the compact body carried on wire.
4. **Verifier context:** the node supplies the expected setup, metadata, public-value layout, FRI shape, and verifier-key digest. The proof supplies none of these as trusted data.

The older non-compact checkpoint is retained only behind regression/test surfaces. Raw Layer-0 proofs, plain `MatmulProof`, and prover-returned verifier contexts are not block artifacts.

## Maintained invariants

- The verifier rebuilds the canonical Layer-0 program from the trusted statement; it never accepts the program encoded by the prover as authority.
- The compact certificate binds all public values needed to reconstruct the useful-work claim, including matrix commitments, nonce/job keys, fold state, jackpot key, and jackpot hash.
- Trace height and proof profile are recomputed from the statement and must match the certificate route.
- Dense and MoE schedules use the same canonical-program discipline. Routing, routed rows, expert-local columns, and grouped matmul are constrained as part of the proof.
- LogUp multiplicities close every cross-chip producer/consumer bus used by the production route.
- Setup and verifier-key digests are proof-independent and verifier-owned. Prover-side caches may improve proving time but cannot alter the verifier's accepted relation.
- Changing an AI-PoW extranonce changes attempt-dependent witness data. Reusing cached noised matrices or tile states across attempts is forbidden.
- Certificate decode has a single canonical encoding and explicit resource limits at the node boundary.

## Soundness dependencies

The proof claim relies on completeness of the composite AIR and canonical program, correct LogUp multiplicities, Goldilocks arithmetic, FRI query soundness, Fiat-Shamir transcript ordering/domain separation, Tip5 and BLAKE3 security in their respective layers, and sound recursive verification. See [`docs/SECURITY.md`](docs/SECURITY.md) for the exact trust model.

No proof-system property establishes chain activation, ASERT, fork choice, coinbase routing, or transaction validity. Those remain Hoon consensus responsibilities.

## Public surface

Production callers should normally use `ai_pow::zk_bridge`. Lower-level bridge code uses the `_sx` compact certificate builders and `recursion::verify_compact_batch_recursive_certificate_with_context`. Dev-only unpinned or non-compact routes must not be wired into block acceptance.

## Documentation and validation

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — layers, statements, setup, and wire boundary.
- [`docs/SECURITY.md`](docs/SECURITY.md) — proof assumptions and adversarial invariants.

```sh
cargo test -p ai-pow-zk --all-features
cargo check -p ai-pow-zk --all-features
```

Expensive production-scale proof and setup tests are opt-in; run them in release mode with the repository's normal shared caches.
