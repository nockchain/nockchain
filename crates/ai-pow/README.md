# `ai-pow`

`ai-pow` defines Nockchain's Pearl-compatible AI proof-of-useful-work statement. It implements the dense and grouped-GEMM INT8 matrix-multiplication ticket, commitment-keyed low-rank noise, tile-state evolution, jackpot calculation, Pearl transcript compatibility, and the bridge into Nockchain's compact recursive certificate.

This crate owns the mineable work unit. It does not own chain activation, ASERT, fork choice, coinbase rules, peer handling, or the mandatory consensus jet.

## Place in the system

```text
Pearl-compatible model/ticket + Nockchain candidate commitment
                              |
                           ai-pow
                  statement, attempt, precheck
                         /             \
                 plain diagnostics    ai-pow-zk proof
                                             |
                                     ai-pow-miner noun
                                             |
                              Hoon + ai-pow-jets verification
```

- `ai-pow-miner` drives attempts and packages the block artifact.
- `ai-pow-zk` proves the selected statement.
- `ai-pow-jets` performs mandatory native verification.
- The Hoon kernel independently enforces activation, target, fork choice, and block validity.

## Production and diagnostic paths

The production block artifact is a versioned `%ai-pow` noun containing an opaque nonce/statement envelope and a compact recursive certificate. Production callers normally enter through `zk_bridge` and the miner's certificate-noun layer.

`MatmulProof` and the plain verifier are diagnostics and Pearl Gateway plumbing. They are useful for target-hit prechecks and compatibility tests, but they are not canonical Nockchain block-acceptance APIs.

## Maintained invariants

- **One useful-work instance per attempt.** The extranonce is upstream of `kappa`, `H_A`, `H_B`, `s_A`, `s_B`, noise, noised matrices, tile state, and jackpot. Changing it requires fresh matrix work; cached nonce-only jackpot grinding is invalid.
- **Commitment binding.** The Nockchain block commitment is included in the Pearl-compatible attempt through exactly one auxiliary tag. A proof cannot authorize two Nockchain commitments.
- **Matrix binding.** Miner-supplied dense or MoE matrices are authenticated by their commitments and bound to the in-circuit opened schedule. The verifier does not substitute prover-selected public parameters or setup.
- **MoE binding.** Routing commitments, expert-local dimensions, selected experts, routed rows, and grouped-matmul columns are mutually constrained. Expert-local columns cannot bleed into another expert.
- **Difficulty binding.** The jackpot is a 256-bit little-endian value checked against the exact chain target after Pearl's shape adjustment. Shape parameters that price work are statement-bound and range-checked.
- **Canonical wire behavior.** Pearl-compatible hashes, PRNG output, matrix layouts, ticket patterns, and proof metadata use their specified byte order and domain labels.
- **Fail-closed parsing.** Parameter, proof, routing, opening, and certificate limits reject malformed or oversized inputs before unbounded work or allocation.

## Pearl compatibility

Compatibility is at the mineable-work layer: the same `sigma`, `mu`, matrix commitments, noise seeds, ticket state, and jackpot can serve Pearl and Nockchain submission. The chains retain independent targets, commitments, acceptance rules, and proof formats. Nockchain does not accept Pearl's proof object as its consensus certificate, and Hoon does not interpret Pearl Gateway concepts.

Known-answer fixtures under `tests/fixtures/pearl.rs` pin cross-implementation byte behavior. Merge-mining tests cover dense and MoE success, wrong commitments, malformed aux data, invalid schedules, routing tampering, and independent target outcomes.

## Module map

| Module | Responsibility |
|---|---|
| `params` | Dense matrix and ticket parameter validation |
| `prng` | Pearl-compatible random-hash, noise, permutation, and synthesis helpers |
| `matmul` | Noised dense/grouped matmul and tile-state transitions |
| `tile_hash` | Shape-adjusted target and little-endian jackpot comparison |
| `commit` / `blake3_tree` | Matrix, tile-state, and selective-opening commitments |
| `pearl_compat` | Pearl parameter, ticket, aux-inclusion, and wire compatibility |
| `pearl_moe_routing` | MoE routing and expert-local binding |
| `prover` / `verifier` | Plain diagnostic mining and verification |
| `zk_bridge` | Canonical statement construction and compact-certificate integration |

## Soundness dependencies

The work claim depends on BLAKE3's keyed-hash and collision resistance, deterministic transcript derivation, the inability to shortcut the committed noised matmul, and the `ai-pow-zk` AIR/FRI/recursion soundness described in [`docs/SECURITY.md`](docs/SECURITY.md). Consensus additionally depends on exact Hoon/Rust statement agreement and puzzle-specific target enforcement.


## Documentation and validation

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — data flow and crate boundaries.
- [`docs/SECURITY.md`](docs/SECURITY.md) — invariants, assumptions, and attack classes.
- [`docs/2026-07-17_DUAL_PUZZLE_CONSENSUS_AUDIT.md`](docs/2026-07-17_DUAL_PUZZLE_CONSENSUS_AUDIT.md) — dated audit record and remediation evidence.

```sh
cargo test -p ai-pow
cargo test -p ai-pow --all-features
```
