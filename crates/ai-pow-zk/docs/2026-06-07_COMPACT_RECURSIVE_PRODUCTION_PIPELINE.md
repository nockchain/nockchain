# Compact Recursive Production Pipeline

Date: 2026-06-07
Status: current AI-PoW recursive proof path

This is the authoritative description of the selected AI-PoW recursive proof
pipeline. Older roadmap, terminal-compression, and route-investigation
documents are historical unless this document links to them for background.

## Summary

The production proof artifact is a compact recursive certificate, not a raw
Layer-0 proof and not the old full recursive checkpoint envelope.

The selected route is:

1. Build and verify a Layer-0 AI-PoW batch STARK for one chain-derived work
   statement.
2. Build a Tip5 Layer-1 verifier circuit that recursively verifies that Layer-0
   proof and exposes a statement digest as public values.
3. Build a native BLAKE3 Layer-2 STARK that recursively verifies the Layer-1
   proof and binds the Layer-1 statement digest in final public values.
4. Compact the Layer-2 proof by removing verifier-deterministic setup material
   and carrying only a verifier-key/setup digest plus the compact final proof
   body.

Current release/native measurements:

| Measurement | Value |
|---|---:|
| Full jammed `%ai-pow` artifact | `125,382` bytes |
| Compact recursive certificate inside the artifact | `124,570` bytes |
| Full cold artifact build wall time | `31.837s` |
| Crate-level compact recursive certificate | `122,597` bytes |
| Crate-level recursive proof wall after chain-verified Layer 0 | `22.006s` |

The accepted relaxed target is about `150 KB`, 60 bits of proof-system
soundness without proof-system PoW grinding, and about `32s` cold proof build
time. The current route meets that target.

## Public API Shape

Production mining and Pearl-merge-compatible submission should call the compact
bridge APIs in `ai-pow`:

- `prove_pearl_merge_compact_recursive_certificate`
- `prove_pearl_merge_compact_recursive_certificate_with_prover_cache`
- `prove_ai_pow_compact_recursive_certificate`
- `prove_ai_pow_compact_recursive_certificate_with_prover_cache`

The `ai-pow-zk` recursive entrypoint is lower-level. It is only for callers
that already verified the Layer-0 proof against chain-owned statement data:

- `prove_compact_batch_recursive_certificate_from_chain_verified_composite_proof`
- `prove_compact_batch_recursive_certificate_from_chain_verified_composite_proof_with_prover_cache`
- `verify_compact_batch_recursive_certificate_with_context`
- `encode_compact_batch_recursive_certificate`
- `decode_compact_batch_recursive_certificate`

The old native terminal experiment is no longer an AI-PoW API. The old full
batch-STARK checkpoint remains a regression/diagnostic path, but it is too large
for production wire use.

## Layer 0: Useful-Work STARK

Layer 0 proves that the miner performed the selected AI-PoW matrix work for the
chain-derived attempt.

### Attempt Commitments

The prover starts from chain-owned and miner-owned attempt data:

- Nockchain block commitment
- opaque AI-PoW nonce
- AI-PoW parameters
- committed matrix bytes `A` and `B`
- selected tile index `found_idx`
- Nockchain target

The attempt derives:

- `kappa`: nonce-bound job key from `(block_commitment, nonce, params_tag)`.
- `HASH_A`: BLAKE3 chunk-Merkle commitment to the committed `A` bytes.
- `HASH_B`: BLAKE3 chunk-Merkle commitment to the committed `B` bytes.
- `s_b`: canonical noise seed derived from `kappa` and `HASH_B`.
- `s_a`: canonical noise seed derived from `s_b` and `HASH_A`.
- jackpot key:
  - Pearl-compatible path: `s_a`
  - native AI-PoW path: `pow_key_for_nonce(s_a, nonce)`

Changing the nonce changes the attempt state before commitments, noise, tile
state, jackpot preimage, and proof witness data are built. Reusing witness data
across nonce attempts would be a soundness bug.

### Layer-0 Public Inputs

The Layer-0 proof exposes the 60-field-element
`CompositePublicInputs` vector:

- `cumsum`: 4 limbs
- `jackpot`: 16 limbs
- `hash_a`: 8 limbs
- `hash_b`: 8 limbs
- `job_key`: 8 limbs
- `commitment_hash`: 8 limbs
- `hash_jackpot`: 8 limbs

For Pearl-compatible merge mining:

- `job_key` binds `kappa`.
- `commitment_hash` binds `s_a`.
- `hash_a` binds `HASH_A`.
- `hash_b` binds `HASH_B`.
- `jackpot` binds the final tile state.
- `hash_jackpot` binds `BLAKE3(tile_state, key=s_a)`.

The verifier derives the expected public inputs from trusted attempt data and
rejects mismatches before constructing the recursive certificate input.

### Work Binding

Layer 0 uses `CompositeFullAirWithLookupsPinned` and
`composite_prove_pinned_logup`.

The soundness-critical bindings are:

- The canonical program is derived by the verifier from public parameters and
  the selected strip schedule, not supplied by the prover.
- BLAKE3 strip-opening rows authenticate the opened `A` and `B` strips to
  `HASH_A` and `HASH_B`.
- The `noised_packed` LogUp bus binds matmul reads to the committed plain bytes
  plus verifier-derived noise.
- The useful-work chain runs the matmul sweep, StripeXor reduction, fold chain,
  final tile-state construction, and jackpot BLAKE3 block.
- The old off-circuit tile-computation fallback was removed; the in-circuit
  sweep is the only matmul path.

A malicious miner cannot fabricate `x_steps`, swap cheaper strips, or skip the
matrix multiplication without breaking the Layer-0 AIR or LogUp relations.

### Layer-0 Soundness

Production Layer 0 uses:

- `log_blowup = 4`
- `num_queries = 15`
- `commit_pow_bits = 0`
- `query_pow_bits = 0`

This gives `4 * 15 = 60` FRI query bits. Proof-system PoW grinding contributes
zero bits to the claimed proof-system soundness.

## Chain-Verified Boundary

The bridge creates `ChainVerifiedCompositeProof` only after checking the
Layer-0 proof against the chain-derived statement.

That check covers:

- production parameter envelope
- nonce and block commitment binding
- canonical matrix commitments
- selected tile index and strip schedule
- trace height
- target hit
- canonical Layer-0 program
- Layer-0 public input equality
- native Layer-0 proof verification

The `ChainVerifiedCompositeProof` constructor is unsafe because the type cannot
prove the caller did all of those checks. Production callers should use the
safe bridge APIs in `ai-pow`, not construct it directly.

## Layer 1: Tip5 Recursive Proof

Layer 1 proves that the Layer-0 proof verified inside a recursive verifier
circuit.

The Layer-1 verifier circuit:

- uses the in-circuit Tip5 permutation
- verifies the Layer-0 batch-STARK proof
- verifies the Layer-0 FRI/MMCS transcript shape
- computes a Tip5 digest over the Layer-0 public values
- exposes that statement digest as Layer-1 public binding lanes

The statement digest is the public-value binding carried forward to Layer 2.
It prevents a compact final proof from being replayed against a different
Layer-0 statement.

Layer 1 uses:

- Tip5 MMCS and Fiat-Shamir transcript
- `log_blowup = 3`
- `num_queries = 20`
- `cap_height = 4`
- `pow = 0`

This gives `3 * 20 = 60` FRI query bits.

## Layer 2: Native BLAKE3 Final STARK

Layer 2 proves that the Layer-1 proof verified.

The Layer-2 verifier circuit still verifies a Tip5 Layer-1 proof. BLAKE3 is not
added as an in-circuit recursive gadget. BLAKE3 is used only by the native
outermost Layer-2 STARK commitment scheme and Fiat-Shamir transcript.

Layer 2 binds all Layer-1 statement-digest base limbs as final public values.
For the current D=2 route this is:

- `DIGEST_ELEMS = 5`
- `ext_degree = 2`
- `5 * 2 = 10` base-field public binding lanes

Layer 2 uses:

- native BLAKE3 MMCS and Fiat-Shamir transcript
- `log_blowup = 5`
- `num_queries = 12`
- `log_final_poly_len = 2`
- `max_log_arity = 3`
- `cap_height = 4`
- `pow = 0`

This gives `5 * 12 = 60` FRI query bits.

## Compact Certificate

The production certificate type is
`AiPowCompactBatchRecursiveCertificate`.

It contains:

1. `verifier_key_digest`
2. compact BLAKE3 Layer-2 proof body

It does not contain trusted verifier metadata, setup, prover data, or FRI
shape. Those are verifier-owned.

### Verifier-Key/Setup Digest

The verifier-key digest is a Tip5 digest over:

- domain separator: `ai-pow-compact-batch-blake3-v1`
- compact route parameters
- Layer-2 proof metadata
- expected FRI shape

At the miner/production config boundary this digest is encoded as 40 bytes:
five canonical Goldilocks limbs in little-endian order.

Verification rejects:

- wrong digest
- wrong digest length
- noncanonical Goldilocks limb encodings
- metadata/setup mismatch
- wrong FRI shape
- wrong public values

The digest is a setup selector and binding commitment. It is not a permission
for the prover to supply trusted setup. The verifier must derive or pin the
metadata and setup from trusted code/config.

### Compact Restoration

The compact proof omits verifier-deterministic material:

- preprocessed out-of-domain openings
- preprocessed FRI input batches
- redundant Merkle path material

The verifier restores those values from canonical setup and the expected FRI
shape before replaying the ordinary Plonky3 verifier.

For the BLAKE3 final layer, restoration uses:

- the native BLAKE3 MMCS
- bit-reversed LDE reconstruction
- verifier-owned preprocessed commitments
- statement-derived final public values

Accepting any of those as trusted miner-supplied data would be unsound.

## Miner Artifact

`ai-pow-miner` packages the compact certificate as canonical bytes inside the
existing `%ai-pow` noun envelope for Pearl-compatible submissions.

The Hoon-facing shape remains intentionally simple:

- opaque Rust-owned AI-PoW nonce
- compact recursive certificate bytes

Pearl details stay inside Rust. Hoon should not receive Pearl proof molds or
raw Layer-0 proof internals.

## Cache Policy

The compact prover cache is prover-side setup only. It may cache:

- guarded Layer-1 prover setup
- Layer-2 verifier-circuit targets
- Layer-2 AIR setup
- preprocessed prover data
- table-prover registration for the fixed proof shape

The cache is never a verifier authority and is never serialized as proof data.
Reuse is guarded by L1 circuit/setup shape and L2 metadata/setup checks. A stale
cache rejects and the bridge rebuilds setup.

## What Was Removed Or Demoted

- Native terminal compression is no longer an AI-PoW API or active fallback.
- Native terminal backend code, measurements, and tests were removed from
  `ai-pow-zk` and the vendored `p3-recursion` crate.
- The large recursive checkpoint remains only as a diagnostic/regression
  guardrail.
- Raw Layer-0 proofs and plain `MatmulProof` are not production block
  artifacts.

## Remaining Integration Work

The Rust proving, compact artifact packaging, and Rust compact artifact
verification path are in place. Remaining consensus integration work is:

- install the production source for the pinned verifier-key/setup digest
- wire the Hoon/Rust verifier boundary
- keep Hoon fail-closed until that verifier path is active

Those items do not change the selected proof shape.
