# `ai-pow-zk`

`ai-pow-zk` is the Plonky3 proof stack for Nockchain AI proof-of-work.
The selected production direction is the compact final-layer batch-STARK
certificate:

- Layer 0 proves the useful-work AI-PoW statement.
- Layer 1 recursively verifies Layer 0 with a Tip5-friendly verifier circuit.
- Layer 2 is a native BLAKE3 final STARK over the Layer-1 proof.
- The wire artifact is a compact recursive certificate plus an explicit
  verifier-key/setup digest.

> **Important:** the current source of truth is
> [`2026-06-07_COMPACT_RECURSIVE_PRODUCTION_PIPELINE.md`](docs/2026-06-07_COMPACT_RECURSIVE_PRODUCTION_PIPELINE.md).
> Read it before changing recursive proof shape, FRI parameters, certificate
> serialization, verifier-key digest binding, or miner artifact wiring.
>
> Open proof-path test work is tracked in
> [`2026-06-07_OPEN_TEST_ISSUES.md`](docs/2026-06-07_OPEN_TEST_ISSUES.md).

## Current Status

The compact route meets the relaxed production target:

| Gate | Current measurement |
|---|---:|
| Full jammed `%ai-pow` artifact | `125,382` bytes |
| Compact recursive certificate inside that artifact | `124,570` bytes |
| Cold artifact build wall time | `31.837s` |
| Crate-level compact certificate | `122,597` bytes |
| Crate-level recursive proof wall time after chain-verified Layer 0 | `22.006s` |

Soundness is 60 FRI query bits without proof-system PoW grinding:

| Layer | Hash / commitment | FRI shape |
|---|---|---|
| Layer 0 useful-work STARK | Tip5 MMCS / transcript | `lb=4,nq=15,pow=0` |
| Layer 1 recursive proof | Tip5 MMCS / transcript | `lb=3,nq=20,cap=4,pow=0` |
| Layer 2 final compact proof | BLAKE3 MMCS / transcript | `lb=5,nq=12,lfp=2,mla=3,cap=4,pow=0` |

Tip5 remains the recursive/circuit-friendly hash. BLAKE3 is used only for the
native final Layer-2 STARK commitments/transcript and for the AI-PoW data path
proved by the BLAKE3 AIR.

## Production API

Production callers should use the compact recursive path through `ai-pow`:

- `ai_pow::zk_bridge::prove_pearl_merge_compact_recursive_certificate`
- `ai_pow::zk_bridge::prove_pearl_merge_compact_recursive_certificate_with_prover_cache`
- `ai_pow::zk_bridge::prove_ai_pow_compact_recursive_certificate`
- `ai_pow::zk_bridge::prove_ai_pow_compact_recursive_certificate_with_prover_cache`

The lower-level `ai-pow-zk` entrypoints are for the bridge after it has already
verified the Layer-0 statement against chain-owned data:

- `recursion::prove_compact_batch_recursive_certificate_from_chain_verified_composite_proof`
- `recursion::prove_compact_batch_recursive_certificate_from_chain_verified_composite_proof_with_prover_cache`
- `recursion::verify_compact_batch_recursive_certificate_with_context`
- `recursion::encode_compact_batch_recursive_certificate`
- `recursion::decode_compact_batch_recursive_certificate`

Verification must use verifier-owned context and the expected verifier-key/setup
digest. A miner must not supply trusted metadata, setup, FRI shape, or verifier
context. The compact certificate carries only the digest and compact final proof
body.

## Not Production APIs

- Raw Layer-0 proofs are intermediate prover inputs, not block artifacts.
- Plain `MatmulProof` remains a diagnostic/pre-ZKP target-hit check.
- The full batch-STARK recursive checkpoint is retained only for regression and
  soundness debugging; it is too large for production wire use.
- Native terminal compression experiments have been removed from the AI-PoW API.
  The vendored native-terminal backend and its dedicated tests were also
  removed. Measurements remain in git history and older documents.

## Historical Docs

Historical roadmap, terminal-compression, proof-size, and route-investigation
documents were removed from this branch and remain available in git history.
The active implementation guide is the current compact pipeline document linked
above.

## Validation

For proof-path changes, prefer release/native measurements:

```sh
RUSTFLAGS="-C target-cpu=native" cargo check -p ai-pow-zk --features recursion
RUSTFLAGS="-C target-cpu=native" cargo check -p ai-pow --features zk
RUSTFLAGS="-C target-cpu=native" cargo check -p ai-pow-miner --features node
RUSTFLAGS="-C target-cpu=native" cargo test -p ai-pow-miner --release --features node \
  real_compact_pearl_merge_artifact_jam_size_for_selected_route -- --ignored --nocapture
```

Always run `cargo fmt --check` and `git diff --check` before committing.

See also
[`docs/2026-06-07_OPEN_TEST_ISSUES.md`](docs/2026-06-07_OPEN_TEST_ISSUES.md)
for release/prover regressions that are not yet automated.
