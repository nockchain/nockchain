# `ai-pow`

A Pearl-style proof-of-useful-work matrix-multiplication puzzle for
Nockchain. The crate implements the `(A + E)(B + F)` tiled matmul puzzle
from the Pearl Whitepaper (cited by name; PDF not in repo)
end-to-end: low-rank noise generation, tile-by-tile mining with an
iterative 512-bit accumulator state, shape-aware difficulty thresholds,
and a replication-style verifier for diagnostic/plain-proof checks.

This is **Phase 1** of the AI-PoW track. The production block artifact is
not the plain `MatmulProof`; it is the structured recursive certificate noun
built through the `ai-pow-zk` integration and consumed at the Rust/Hoon
boundary. Plain proofs are mining internals and diagnostics.
An earlier experimental verifiable-inference crate (`ai-pow-vi`) was
removed — it was offline tooling, not on the production path.

## Production certificate path

`ai-pow` owns the Pearl-compatible mineable work unit and plain-proof
diagnostics. The production recursive certificate is built and verified through
`ai-pow-zk` and the miner integration. The current production pipeline,
including every cryptographic binding and the verifier-key digest, is documented
in
[`../ai-pow-zk/docs/2026-06-07_COMPACT_RECURSIVE_PRODUCTION_PIPELINE.md`](../ai-pow-zk/docs/2026-06-07_COMPACT_RECURSIVE_PRODUCTION_PIPELINE.md).

The [`docs/`](docs/) directory intentionally keeps only a small current index.
Historical Pearl-audit and route-comparison notes remain available in git
history, but they are not part of the current production API guidance.

## Scope

What `ai-pow` provides:

- **Inputs**: caller-supplied INT8 matrices `A` (m × k row-major) and `B`
  (n × k column-major), each entry in `[-64, 64]` (Pearl §4.1).
- **Mining**: `mine(block_commitment, nonce, a, b, params, opts)` searches
  for a tile whose keyed-hash of the tile state falls below a shape-aware
  difficulty target `2^(256-b) · r · t²` (Pearl §4.5).
- **Nonce-bound attempt diagnostics**: `ai_pow::prover::BlockContext` exists
  for explicit diagnostics, tests, and miner internals. It is intentionally not
  re-exported from the crate root because it contains cached matmul state for
  exactly one nonce-bound attempt; normal callers should use `mine` or
  `mine_block`, which rebuild attempt state per nonce.
- **Plain-proof verification**: diagnostic and pre-ZKP callers use
  `ai_pow::verifier::verify_prod_at_target(block_commitment, nonce, params, target, proof)`
  to confirm that a mined `MatmulProof` hit the exact chain target under the
  production parameter envelope before recursive certificate generation.
  Lower-level test/tooling callers can use
  `ai_pow::verifier::verify_at_target(block_commitment, nonce, params, target, proof)`.
  These plain verifiers are intentionally not re-exported from the crate root
  and are not canonical block-acceptance APIs. The old `verifier::verify`
  helper derives its target from `params.difficulty_bits` and is not a
  consensus API.
- **Production certificate verification**: Nockchain block/persistence/wire
  boundaries must verify the structured recursive certificate noun and run the
  Pearl-compatible statement precheck. Recursive certificate statements derive
  canonical seeds from proof-bound chunk commitments. The Pearl-compatible
  protocol requirement set is Pearl's full periodic-pattern ticket model:
  canonical `MiningConfiguration`, row/column `PeriodicPattern` values,
  valid `t_rows`/`t_cols` offsets, shifted opened row/column sets, and
  Pearl's pattern-size target pricing. The current recursive prover supports
  square-contiguous Pearl tile tickets across multi-tile matrices and remains
  fail-closed for other Pearl-valid pattern shapes until proof support catches
  up to the spec.
- **Proof format**: 32-byte tile-state commitment `comm_m`, BLAKE3-keyed
  matrix commitments `H_A` and `H_B`, and per-tile openings (raw strips,
  m-path to `comm_m`, per-row/col paths to `H_A` / `H_B`).
- **Synth helper**: `synth_matrices(seed, params)` deterministically
  generates Pearl-valid `(A, B)` pairs for tests; real miners supply
  their own.

What `ai-pow` deliberately does **not** include:

- Plonky2 / STARKy zkSNARK block-opening proof (Pearl §4.7). This crate
  is the pre-SNARK reference; proof sizes scale with σ × t × k.
- Chain integration, mempool, RPC, block-header format.
- Hoon-side jets or consensus glue. Those live downstream.

## Pearl alignment

Cross-implementation byte-equivalence against the Pearl upstream is captured by
the fixture table in `tests/fixtures/pearl.rs` and exercised by
`tests/pearl_compat_fixtures.rs`:

| Section | Topic | Status |
|---|---|---|
| S0 | Protocol constants (`JACKPOT_SIZE`, `LROT_PER_TILE`, label seeds, chunk size) | byte-equivalent |
| S1 | `get_random_hash` PRNG byte stream (`prng::pearl_random_hash`) | byte-equivalent |
| S2 | Permutation pairs via XOR trick (`prng::pearl_permutation_pair`) | byte-equivalent |
| S3 | `generate_uniform_random_matrix` (`prng::fill_uniform_row`) | byte-equivalent |
| S4 | `matvec_sparse_perm` reconstruction | byte-equivalent |
| S5 | Tile loop `jackpot[16]` evolution | byte-equivalent |
| S6 | `compute_jackpot_hash` keyed BLAKE3 | byte-equivalent |
| S7 | Commitment-hash chain `kappa → s_b → s_a` | byte-equivalent |
| S8 | Matrix-commitment chunk-Merkle root (`commit::matrix_commitment`) | root byte-equivalent; per-strip proof format is a per-row Merkle (follow-on) |
| S9 | Shape-aware difficulty target in little-endian | byte-equivalent |

The Pearl ISC license is reproduced verbatim at
[`LICENSE-PEARL`](LICENSE-PEARL); see that file for the precise
list of `ai-pow`-side derived portions.

## Layout

| Path | Purpose |
|---|---|
| `src/lib.rs` | Public re-exports; intentionally omits verifier helpers and `BlockContext` |
| `src/params.rs` | `MatmulParams` and validation (Pearl §4.8 constraints) |
| `src/prng.rs` | Pearl-compatible PRNG building blocks (`pearl_random_hash`, uniform-noise, permutation, A/B synth) |
| `src/matmul.rs` | `BlockNoise`, `Matrices` (`A' = A + E`, `B' = B + F`), `TileState`, `compute_tile`, `compute_tile_from_slices` |
| `src/tile_hash.rs` | `difficulty_target`, `hash_le_target` (little-endian U256 semantics) |
| `src/commit.rs` | Tile-state Merkle (sentinel-padded) and Pearl `matrix_commitment` chunk-Merkle |
| `src/fiat_shamir.rs` | Pearl §4.3 commitment-hash chain helpers; Pearl-compatible mode derives from `sigma || mu`, while native diagnostics can derive from nonce-bound attempt state |
| `src/prover.rs` | native diagnostic `BlockContext`, `mine`, `mine_block` |
| `src/verifier.rs` | `verify` |
| `src/proof.rs` | `MatmulProof`, `TileOpening`, encode / decode |
| `src/synth.rs` | Deterministic `(A, B)` test synthesis |
| `tests/` | End-to-end, adversarial, soundness simulation, LLM-shape, Pearl-compat fixtures |
| `docs/README.md` | Current documentation index |

## Tests

`cargo test -p ai-pow` runs the crate's unit, integration, and fixture tests:

- 53 unit (params, prng, matmul, tile_hash, commit, fiat_shamir, proof, synth)
- 19 adversarial (every verifier rejection path exercised by tampering)
- nonce-grinding regressions: different nonces re-key commitments, change
  noise/tile states before final hashing, and stale attempt contexts are
  rejected
- 13 end-to-end (round-trip prove → verify)
- 5 LLM-shape (rectangular, non-pow-2 tile counts, Gemma 4 / Qwen 3.6 FFN profiles)
- 11 Pearl-compat fixtures (sections S0 – S9 above)
- 3 soundness Monte-Carlo (rejection rate vs `1 − (1 − f)^σ`)

Plus 1 `#[ignore]`-d `gen_fixtures` test that regenerates `tests/fixtures/pearl.rs`
from vendored Pearl reference code.

## Difficulty-bound parameters

Pearl §4.8 constraints (enforced by `MatmulParams::validate`):

- `noise_rank` must be a power of two with `r ≥ 2`
- `k` must be a multiple of `noise_rank`
- `k ≤ 2^16`
- `tile` must divide both `m` and `n`
- `spot_checks` must be > 0 and ≤ the total tile count
