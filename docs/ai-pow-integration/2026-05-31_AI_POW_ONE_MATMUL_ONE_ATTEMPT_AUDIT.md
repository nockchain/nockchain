# AI-PoW One-Matmul-One-Attempt Soundness Audit

Date: 2026-05-31
Status: Critical audit finding, repair plan, and implementation tracking

> Supersession note, 2026-06-01: this audit's grinding invariant still applies,
> but any references below to `@uxncmn` are historical. The current canonical
> `%ai-pow` artifact carries `ai-pow-nonce=[len data]`, an opaque Rust-owned
> nonce envelope described in
> `2026-06-01_PEARL_MERGE_MINING_COMPATIBILITY_SPEC.md`.
>
> Supersession note, 2026-06-02: the one-nonce/one-verifier-selected-tile rule
> below applies to the native diagnostic path and the pre-Pearl repair plan,
> not to Pearl-compatible production consensus. The Pearl-compatible spec now
> adopts Pearl's work model: one committed noised matmul work instance, derived
> from `sigma || mu`, may expose many valid public tile tickets. The forbidden
> shortcut is a Nockchain-only nonce/hash loop or any proof that opens an
> unbound tile while claiming Pearl work, not Pearl's own tile-ticket reuse.
> Mentions below of `matmul_attempts_tried` as fresh full attempts should be
> read as native-path accounting, not Pearl-compatible ticket accounting.

## Executive Summary

The intended production invariant is:

> A miner must not be able to change a nonce and get a fresh proof-of-work trial
> unless that nonce also forces fresh AI matmul work.

Equivalently, the protocol should minimize reusable work between attempts.
Cache-friendly reuse of nonce-independent noised matrices, tile states, or final
matmul outputs is not a performance feature for this puzzle; it is the attack
class this audit is closing.

The pre-fix `ai-pow` and `ai-pow-miner` implementation did not satisfy that
invariant.

Before the fix, `BlockContext::build` computed `kappa`, `H_A`, `H_B`, `s_B`, `s_A`,
the noise factors, noised matrices, and every per-tile `M` state without the
nonce. Then `mine_inner` derives `pow_key_for_nonce(s_A, nonce)` and re-hashes
the cached `M` states. The production miner builds one `BlockContext` and loops
over extranonces. A malicious miner can therefore perform one expensive matmul
precomputation and then run many cheap nonce-derived BLAKE3 trials against the
same matmul outputs.

The pre-fix recursive ZKP path did not repair this. It proved the selected tile's
matmul and binds `HASH_JACKPOT = BLAKE3(M, key=pow_key_for_nonce(s_A, nonce))`,
but the noised matrices and matmul inputs still come from nonce-independent
`ctx.s_A` and `ctx.s_B`. The proof certifies "this cached matmul result was
hashed with this nonce-derived key", not "this nonce forced this matmul work".

This is a critical proof-of-work soundness issue, not just a misleading
comment.

Implementation status:

- `BlockContext::build` now takes `(block_commitment, nonce, A, B, params)` and
  derives `kappa` from `block_state(block_commitment, nonce) || params_tag`.
- `mine_block` and `ai-pow-miner` rebuild a fresh nonce-bound context for each
  nonce/extranonce.
- `mine_with_context_at_target` and ZK bridge prover entrypoints reject a
  context supplied with a different nonce.
- Plain and ZK verifiers derive `kappa`, `s_B`, and `s_A` from the same
  nonce-bound attempt state.
- The production miner and verifier now derive exactly one eligible jackpot
  tile from the nonce-bound attempt state, parameter tag, and nonce-bound
  `s_A` seed. `found_idx` is no longer miner-selected search space, and the
  eligible tile is not knowable from `(block, nonce, params)` alone.
- The production recursive-certificate statement precheck re-derives
  `JOB_KEY`, `COMMITMENT_HASH`, `HASH_A`, `HASH_B`, trace height, and jackpot
  target satisfaction from verifier-supplied block data before recursive
  certificate verification is allowed to trust the persisted metadata.
- The structured Pearl merge artifact decoder exposes bounded metadata and
  statement prechecks, so the Rust/Hoon boundary can reject replay, target,
  params, aux-inclusion, and public-input mismatches immediately after bounded
  noun decoding.
- The canonical certificate noun and Hoon `ai-pow-commitments` mold now carry
  only the proof-bound chunk commitments `[h-a-chunk h-b-chunk]`. Legacy
  row/column roots `h_a` / `h_b` remain Rust diagnostic/plain-proof fields and
  are not persisted in the block artifact.
- Hoon consensus remains fail-closed for `%ai-pow`: the kernel does not emit
  `%mine-ai`, does not persist `[%ai-pow nonce cert]`, and rejects typed AI
  certificates until recursive certificate verification is wired.
- Release regression tests now assert that changing the nonce changes `kappa`,
  `H_A`, `H_B`, chunk commitments, `s_A`, `s_B`, and matmul-derived tile
  states before final hashing.
- Miner accounting reports `matmul_attempts_tried`, not cheap hash or extranonce
  trials. Each increment is intended to mean a fully rebuilt nonce-bound
  commitment/noise/matmul attempt.
- Re-audit searches over active source, tests, benches, and README found no
  remaining code path or live documentation that presents final-hash-only nonce
  retrying as a valid production mode.

## Security Property

No `%ai-pow` block is currently consensus-admissible: the Hoon/kernel path is
fail-closed until recursive certificate verification is wired. Once that
verifier is enabled, every consensus-admissible AI-PoW block must satisfy:

1. The trusted block data includes a candidate block commitment, AI parameters,
   matrix commitments, a nonce, a target, and a claimed `found_idx`.
2. The verifier derives an attempt-specific transcript from that trusted data.
3. The transcript used to generate the noised matrices must include the nonce,
   directly or through an attempt seed derived from the nonce.
4. The verifier derives the single eligible jackpot tile from the nonce-bound
   attempt state after recomputing `s_A` from the matrix commitments. The
   submitted `found_idx` must match that tile.
5. The recursive proof must prove the matmul over those nonce-derived noised
   matrices for that verifier-derived `found_idx`.
6. The final jackpot hash must be derived from that same nonce-bound matmul
   result and checked against the target.

Changing the nonce must invalidate the old matmul work. Consensus can tolerate
reuse of raw matrix bytes, shape constants, and validation results only because
those are not attempt work; this reuse is not a protocol goal. It is not
acceptable to cache nonce-independent noised matrices, tile states, or `M`
values and treat many nonce hashes as many PoW attempts.

Engineering rule for this codebase: when in doubt, prefer more recomputation at
the nonce boundary over more reusable per-attempt state. The only safe cached
objects are those that are either independent of any attempted work (for
example parsed matrix bytes and shape validation) or already include the exact
nonce and block commitment they will be used with.

This matches the Pearl whitepaper's intended dependency chain: the noise seeds
are derived from commitments to `A`, `B`, mining configuration, and blockchain
state `sigma`, and the noisy matmul is the work whose trace is certified. For
Nockchain, the NCMN nonce is part of the blockchain attempt state. Therefore
changing the nonce must change the commitment key, matrix commitments, noise
seeds, noised matrices, matmul tile states, and jackpot preimages. Any design
that lets a miner keep the same noised matmul and merely resample the nonce hash
violates that Pearl-style lottery model.

## Pre-Fix Data Flow

### Plain Prover

Relevant files:

- `crates/ai-pow/src/prover.rs`
- `crates/ai-pow/src/fiat_shamir.rs`
- `crates/ai-pow/src/verifier.rs`
- `crates/ai-pow-miner/src/mining.rs`

Pre-fix derivation:

```text
tag     = params_tag(params)
kappa   = commitment_key(block_commitment, tag)
H_A     = MerkleRoot(row_leaf_hash(A_i, kappa))
H_B     = MerkleRoot(col_leaf_hash(B_j, kappa))
s_B     = noise_seed_b(kappa, H_B)
s_A     = noise_seed_a(s_B, H_A)
noise   = BlockNoise::expand(s_A, s_B, params)
M[i,j]  = compute_tile(A + E, B + F, i, j)

pow_key = pow_key_for_nonce(s_A, nonce)
leaf[i] = BLAKE3(M[i,j], key=pow_key)
```

The nonce only entered `block_state` for spot-check challenge derivation and
`pow_key_for_nonce` for final tile hashing. It does not enter `kappa`, `H_A`,
`H_B`, `s_B`, `s_A`, `BlockNoise`, `Matrices`, or `M`.

This was confirmed by:

- `BlockContext::build(block_commitment, a, b, params)` had no nonce argument.
- `BlockContext` stores `m_states: Vec<TileState>`.
- `mine_inner(ctx, block_commitment, nonce, target, opts)` hashes `ctx.m_states`
  with `pow_key_for_nonce(&ctx.s_a, nonce)`.
- `mine_block` built one `BlockContext` and looped over nonces.
- The removed legacy `ai-pow-miner::mining::run_inner` built one
  `BlockContext` and looped over `extranonce`.

### Plain Verifier

The pre-fix verifier mirrored the same bug-compatible statement:

```text
kappa   = commitment_key(block_commitment, tag)
s_B     = noise_seed_b(kappa, proof.H_B)
s_A     = noise_seed_a(s_B, proof.H_A)
pow_key = pow_key_for_nonce(s_A, nonce)
noise   = VerifierNoise::new(s_A, s_B, params)
M       = compute_tile_from_slices(A + E, B + F)
hash    = BLAKE3(M, key=pow_key)
```

Because this verifier explicitly accepted nonce-independent noise and
nonce-dependent final hashing, it accepted the cheap-retry strategy.

### Miner Loop

Pre-fix `crates/ai-pow-miner/src/mining.rs` made the exploit the production
behavior:

```text
ctx = BlockContext::build(job.puzzle_id, job.a, job.b, job.params)
loop extranonce:
  nonce = build_ncmn_nonce(...)
  result = mine_with_context_at_target(&ctx, job.puzzle_id, &nonce, target, ...)
```

The stats field `extranonces_tried` was therefore not a count of fresh matmul
attempts. It was a count of cheap final-hash retry attempts over one cached
matmul context.

### ZK And Recursive Certificate

Relevant files:

- `crates/ai-pow/src/zk_bridge.rs`
- `crates/ai-pow-zk/src/composite_trace.rs`
- `crates/ai-pow-zk/src/composite_public.rs`
- `crates/ai-pow-zk/src/recursion.rs`
- `crates/ai-pow-miner/src/certificate_noun.rs`

The pre-fix ZK bridge did this:

```text
pow_key = pow_key_for_nonce(ctx.s_A, nonce)
trace pins JOB_KEY = ctx.kappa
trace pins COMMITMENT_HASH = pow_key
noise = BlockNoise::expand(ctx.s_A, ctx.s_B, params)
M = in-circuit matmul over A + E(ctx.s_A), B + F(ctx.s_B)
HASH_JACKPOT = BLAKE3(M, key=pow_key)
```

This proved that the selected `M` was honestly computed from the committed
matrices and hashed under the nonce-derived jackpot key. It does not prove that
the nonce caused the matmul work, because `ctx.s_A`, `ctx.s_B`, and the noised
matrix values are the same for every nonce under the same block/matrix context.

The pre-fix recursive certificate only recursively verified this Layer-0
statement. It could not strengthen a statement that did not include
nonce-derived matmul work.

## Pearl Whitepaper Cross-Check

Source: `Pearl_Whitepaper.pdf`, Sections 4.2 through 4.6 and 7.

Pearl's intended derivation is straightforward:

```text
kappa = BLAKE3(sigma || mu)
H_A   = BLAKE3(flatten(A), key=kappa)       // Nockchain h_a_chunk / HASH_A
H_B   = BLAKE3(flatten(B^T), key=kappa)     // Nockchain h_b_chunk / HASH_B
s_B   = BLAKE3(kappa || H_B)
s_A   = BLAKE3(s_B || H_A)
E     = NoiseGeneration(key=s_A)
F     = NoiseGeneration(key=s_B)
M     = TiledMatMul(A + E, B + F)
hash  = BLAKE3(M, key=s_A)
```

The whitepaper says the commitment hash takes `A`, `B`, miner config `mu`, and
blockchain state `sigma`, and that the two noise seeds depend on all of those
inputs. Algorithm 2 then derives `kappa` from `sigma || mu`, derives `H_A` and
`H_B` as keyed matrix hashes under `kappa`, and derives `s_A`/`s_B` from
`kappa`, `H_A`, and `H_B`. Algorithm 1 generates the noise from those seeds
before the tiled matmul.

Pearl also removes the Bitcoin-style nonce field from the header and replaces
it with a variable-size block certificate. For Nockchain, which currently has
an explicit NCMN nonce/extranonce mechanism, the faithful translation is:

```text
sigma_attempt = encode(block_commitment, nonce)
kappa         = BLAKE3(sigma_attempt || params_tag)
```

In other words, if the nonce is the miner-controlled attempt variable, it must
be part of Pearl's `sigma` before the matrix commitments and noise seeds are
derived. In current Nockchain code, the canonical seed inputs are the
proof-bound chunk commitments `h_a_chunk` / `h_b_chunk`, while `h_a` / `h_b`
remain legacy row/column spot-opening roots. Changing the nonce then changes
`kappa`, all keyed matrix commitments, `s_A`, `s_B`, the low-rank noise
matrices, and the tile states `M`.

Pearl does not describe a construction where miners compute nonce-independent
noise once and then obtain many independent attempts by changing only a final
hash key. The pre-fix `pow_key_for_nonce(s_A, nonce)` path was a Nockchain
extension, and using it as the only nonce binding was exactly the cheap-retry
bug. The current implementation keeps the final key but derives `s_A` from the
nonce-bound attempt transcript first.

## Attack

Assume a miner has fixed `(block_commitment, params, A, B)`.

Pre-fix honest-looking but unsound strategy:

1. Build `BlockContext` once.
2. Compute all noised matrix tile states `M[i,j]` once.
3. For nonce `n = 0, 1, 2, ...`, derive `pow_key_for_nonce(s_A, n)`.
4. Hash the cached `M[i,j]` values under that key.
5. When one hash clears the target, generate the recursive certificate for that
   nonce and `found_idx`.

The pre-fix verifier accepted because it also derived the final hash key from
the nonce while leaving the matmul noise nonce-independent.

Impact:

- The miner receives many independent final-hash trials for one matmul.
- The cost per additional nonce is roughly BLAKE3 over cached tile states plus
  Merkle recomputation, not AI matmul.
- Difficulty no longer prices the intended useful work.
- The recursive ZKP does not prevent this; it proves the wrong statement for
  the desired work accounting.

## Scope Of Pre-Fix Violation

### Definite Violations

- `crates/ai-pow/src/prover.rs`: `BlockContext` was nonce-independent and stored
  reusable `m_states`.
- `crates/ai-pow/src/fiat_shamir.rs`: `pow_key_for_nonce` was the only current
  nonce binding for the jackpot hash path.
- `crates/ai-pow/src/verifier.rs`: verifier derived the same nonce-independent
  noise and accepted the same cheap-retry statement.
- `crates/ai-pow-miner/src/mining.rs`: production miner looped extranonces over
  one cached `BlockContext`.
- `crates/ai-pow/src/zk_bridge.rs`: ZK trace bound `COMMITMENT_HASH` to the
  nonce-derived final hash key, but used nonce-independent `ctx.s_A`/`ctx.s_B`
  for noised matmul.
- `crates/ai-pow-miner/src/bin/ai_pow_mine.rs`: production certificate builder
  correctly waits for the plain target hit before proving, but that target hit
  may have been found via a cheap nonce rehash over cached matmul output.

### Not A Fix

- Checking `verify_at_target` before recursive proving is necessary, but it
  verifies the current unsound statement.
- Making recursive proofs canonical is necessary, but the recursive proof only
  certifies the Layer-0 statement it is given.
- Serializing only the recursive certificate into Hoon is necessary, but it
  does not affect the mathematical statement.
- Binding `found_idx` into the recursive certificate is necessary, but it only
  selects which tile was proved; it does not make the tile computation
  nonce-dependent.

## Attempt Accounting Rule

The production accounting rule is stricter than a cache-friendly per-tile scan:

- one nonce-bound full matmul attempt gets exactly one eligible jackpot tile;
- the eligible tile is derived by the verifier from
  `block_state(block_commitment, nonce)`, `params_tag`, and `s_A`;
- because `s_A = H(noise_seed_b(kappa, h_b_chunk), h_a_chunk)`, the tile
  cannot be known until the nonce-bound, proof-bound matrix commitments are
  fixed;
- a miner must not scan all tile hashes from one full matmul and submit the
  best or first passing tile;
- a tile result `M[i,j]` must not be re-keyed or rehashed across many nonces;
- the submitted `found_idx` is only verifier-checkable metadata and must equal
  the derived jackpot tile.

This avoids two forms of work reuse:

- nonce grinding over one cached noised matmul, where only the final hash key
  changes;
- tile grinding over one cached full matmul, where one expensive attempt yields
  `params.num_tiles()` cheap jackpot trials.

The code now has explicit regression guards that `seek_best` is a no-op in
production mining and that the verifier rejects an in-range substituted
`found_idx` before accepting a Merkle path or target check. The target remains
checked against the eligible tile's digest, but the grid size does not create
additional miner-selected lottery tickets inside one nonce-bound attempt.

## Recommended Repair

Use the Pearl-faithful binding: the nonce is part of the attempt `sigma` before
the commitment hash, matrix commitments, noise seeds, and noised matmul are
computed.

Recommended production design:

```text
tag           = params_tag(params)
attempt_state = block_state(block_commitment, nonce)
kappa         = commitment_key(attempt_state, tag)
h_a           = MerkleRoot(row_leaf_hash(A_i, kappa))      // plain openings
h_b           = MerkleRoot(col_leaf_hash(B_j, kappa))      // plain openings
h_a_chunk     = matrix_commitment(A_row_major, kappa)      // ZK HASH_A
h_b_chunk     = matrix_commitment(B_col_major, kappa)      // ZK HASH_B
s_B           = noise_seed_b(kappa, h_b_chunk)
s_A           = noise_seed_a(s_B, h_a_chunk)
noise         = BlockNoise::expand(s_A, s_B, params)
M[i,j]        = compute_tile(A + E(s_A), B + F(s_B), i, j)
pow_key       = pow_key_for_nonce(s_A, nonce)
hash[i]       = BLAKE3(M[i,j], key=pow_key)
```

This intentionally minimizes work reuse between attempts. Cache-friendly
attempt derivations are a vulnerability for this protocol goal, not a desired
property: if a miner can carry keyed commitments, noised matrices, tile states,
jackpot inputs, Layer-0 witness data, or recursive proof inputs across nonces,
the implementation is too close to the current bug. The recommended fix is to
put the nonce in `sigma_attempt` and derive `kappa` from that. Raw matrix bytes
may be read repeatedly, but any consensus-significant value derived under an
attempt transcript must be rebuilt for the new attempt.

The production design is acceptable only if:

- `s_A` and `s_B` are verifier-derived from trusted block data and nonce.
- The ZK canonical program receives the nonce-bound seeds.
- The in-circuit noised matrix values are generated from the nonce-bound seeds.
- The final `HASH_JACKPOT` key is derived from the same nonce-bound transcript.
- The old API path that rehashes cached `M` states across nonce values is
  removed or made test-only and clearly non-consensus.

## Implemented Resolution

Current code follows the recommended production design above:

- `crates/ai-pow/src/prover.rs`: `BlockContext` is now a nonce-bound attempt
  context. It stores `block_commitment`, `nonce`, and `attempt_state`; `kappa`,
  commitments, seeds, noise, noised matrices, and `m_states` are all built from
  that attempt state.
- `crates/ai-pow/src/verifier.rs`: `verify_at_target` derives `kappa` from
  `block_state(block_commitment, nonce)` before deriving `s_B`, `s_A`, and
  verifier noise.
- `crates/ai-pow-miner/src/mining.rs`: the extranonce loop builds a fresh
  `BlockContext` for each NCMN nonce before checking the target.
- `crates/ai-pow-miner/src/lib.rs`: miner telemetry now reports
  `matmul_attempts_tried` and `matmul_attempt_rate_per_sec`, not a
  BLAKE3-style hash rate or cheap extranonce rate.
- `crates/ai-pow/src/zk_bridge.rs`: prover entrypoints reject a context used
  with a different nonce, verifier entrypoints derive public inputs from the
  nonce-bound attempt state, and
  `verify_ai_pow_full_matmul_production_statement` provides the persisted
  recursive-certificate full-matmul admission check. The lower-level
  `verify_ai_pow_selected_tile_statement` is crate-internal selected-tile
  binding plumbing.
- `crates/ai-pow-miner/src/certificate_noun.rs`: decoded Pearl merge-mined
  `%ai-pow` artifacts can be prechecked against verifier-trusted Nockchain block
  data, matrix inputs, target, and aux inclusion before recursive proof
  verification consumes the miner-controlled proof tree. Single-ticket
  statements derive canonical public inputs from the same chunk commitments
  bound by the ZK proof as `HASH_A` / `HASH_B`.
- `crates/ai-pow-miner/src/bin/ai_pow_mine.rs`: recursive certificate
  construction is reachable only from a winning Pearl-compatible ticket
  attempt. Target misses return no ticket artifact, and stale or mismatched
  recursive-run metadata is rejected before submission.
  It also rejects the current selected-tile recursive statement before
  spending ZK proving work when the configured proof would lack full-matrix
  binding.

Regression coverage now includes:

- different nonces re-key `kappa`, `H_A`, `H_B`, chunk commitments, `s_A`,
  `s_B`, and at least one tile state before final hashing;
- stale contexts fail if submitted with a different nonce;
- proofs fail under nonce substitution;
- `mine_block` is byte-equivalent to `mine` for the same nonce but rebuilds
  nonce-bound work for each nonce;
- ZK prover entrypoints reject nonce-substituted contexts before proving;
- the standalone miner's recursive certificate builder rejects bad targets,
  non-canonical, wrong-anchor, externally anchored NCMN nonces, and multi-tile
  selected-tile recursive statements before recursive proof generation;
- production recursive-certificate statement metadata rejects wrong nonce,
  wrong public inputs, and jackpots above target before recursive verification;
- the recursive certificate itself binds the Layer-0 public-input
  vector as outer STARK public values, so swapping verifier-derived statement
  inputs rejects at recursive certificate verification.
- real recursive certificate nouns roundtrip through structured proof-node
  serialization, jam/cue, bounded decode, reconstruction into
  `AiPowRecursiveCertificate`, canonical re-serialization, and recursive
  verification.
- decoded structured certificate nouns reject wrong nonce, zero target, and
  parameter mismatch at the statement precheck boundary.

Tile scanning is no longer a production optimization. The only jackpot tile
hash checked for a nonce-bound full matmul attempt is the verifier-derived
attempt tile, sampled after `s_A` is fixed. The target is still shape-weighted
per Pearl Section 4.5 (`r * t_m * t_n`), but grid size does not silently grant
extra cheap jackpot trials inside one cached full matmul.

## Latest Recursive-Certificate Re-Audit

The recursive certificate has two distinct verification layers:

1. Outer recursive STARK verification: prove that the Plonky3-recursion L1
   verifier circuit accepted its embedded Layer-0 composite proof.
2. Statement binding: prove that the embedded Layer-0 public inputs are exactly
   the verifier-derived AI-PoW statement for this block commitment, nonce,
   target, matrix commitments, params, and `found_idx`.

`crates/ai-pow-zk/src/recursion.rs` now exposes the recursive verifier API:

- `verify_recursive_certificate(cert, public_inputs)` accepts only the
  batch-STARK recursive checkpoint certificate and a verifier-derived
  `CompositePublicInputs`. The active production recursive proof candidate is
  the compact final-layer batch-STARK certificate; this older checkpoint
  verifier remains a hardened regression/fallback path.
- `verify_recursive_certificate_with_public_values(cert, public_values)` is the
  lower-level equivalent for callers that already hold the exact Layer-0 public
  input vector. It rejects empty statement vectors on the normal recursive path.
- `verify_recursive_certificate_outer` is deprecated outer-only diagnostic
  code for old unbound proof objects. Canonical bound certificates reject on
  that helper and direct callers to the full verifier.

The implemented binding is:

1. `build_composite_l1_verifier_circuit` records the Layer-0 AI-PoW public
   input vector passed to the L1 verifier circuit.
2. `prove_composite_l1_outer_cert` sets
   `TablePacking::with_public_binding_lanes(public_values.len())`.
3. The circuit-prover `PublicAir` exposes those leading lanes as STARK public
   values and constrains the first row to equal the supplied values.
4. `BatchStarkProof` serializes `public_binding_lanes` and validates that it
   agrees with `table_packing`.
5. `verify_recursive_certificate` recomputes the expected recursive table
   packing from the caller's public inputs, flattens each Goldilocks value into
   the D=2 outer field representation `[value, 0]`, and calls
   `verify_all_tables_with_public_values`.

This closes the metadata-swap gap at the Rust recursive verifier boundary:
reusing a valid recursive certificate with a different verifier-derived
Layer-0 public-input vector fails recursive certificate verification. Hoon
consensus still remains fail-closed for `%ai-pow` until the jet/wiring decodes
the structured noun, derives the trusted statement from block data, and calls
this full Rust verifier.

Do not accept: verifying only the outer certificate and trusting adjacent block
metadata. That permits metadata swapping or replay of a valid recursive
certificate for a different statement.

The minimal-reuse mining rule remains stronger than "do not cache final hashes".
Fresh attempts must rebuild every nonce-dependent work product:

- `kappa`;
- matrix commitments under `kappa`;
- `s_B` and `s_A`;
- low-rank noise;
- noised matrix strips;
- tile states and jackpot preimages;
- Layer-0 witness/proof inputs for the selected winning attempt.

Allowed reuse is limited to nonce-independent non-work inputs: immutable input
matrix bytes, shape constants, chain-pinned params, and read-only model
metadata. Even those caches should be treated as an engineering convenience
rather than a protocol objective. They must never contain transcript-derived
commitments, seeds, noised values, tile outputs, jackpot hashes, or proof
witnesses. Cache-friendly reuse between attempts is a consensus vulnerability,
not an optimization target.

## Concrete Implementation Plan

### Phase 0: Stop Treating Unbound Paths As Production-Sound

Status: implemented for current Hoon consensus; still required operationally
until the verifier jet is wired.

1. Keep the Hoon verifier path reject/deferred until the full recursive
   certificate verifier is callable from consensus.
2. Do not activate AI-PoW block acceptance on mainnet through any legacy
   nonce-independent matmul statement or outer-only recursive verifier.
3. Leave audit comments near `BlockContext`, `pow_key_for_nonce`,
   `attempt_tile_index`, and the miner loop so future readers do not mistake
   cache-friendly behavior for the desired invariant.

### Phase 1: Refactor Plain Attempt Context

Status: functionally implemented with `BlockContext` now representing one
nonce-bound attempt. A future cleanup may still split raw immutable inputs from
attempt-local state, but the soundness boundary no longer exposes nonce-free
`m_states`.

1. Split `BlockContext` into two concepts if the API is cleaned up further:
   - `BlockInputs`: immutable references to `A`, `B`, `params`, and at most
     trivial shape/range validation results. It must not contain keyed
     commitments, seeds, noised matrices, tile states, or jackpot inputs.
   - `AttemptContext`: `attempt_state`, `kappa`, nonce-bound `H_A`/`H_B`,
     nonce-bound seeds, noise, noised matrices, tile states, and openings for
     exactly one nonce.
2. Remove `m_states` from any context type that does not contain a nonce.
3. Replace:

```rust
BlockContext::build(block_commitment, a, b, params)
mine_with_context_at_target(&ctx, block_commitment, nonce, target, opts)
```

with:

```rust
BlockInputs::build(a, b, params)
AttemptContext::build(&inputs, block_commitment, nonce)
mine_attempt_at_target(&attempt, target, opts)
```

4. Done: `mine_block` builds a fresh nonce-bound context per nonce and its docs
   state that it recomputes nonce-derived matmul work per nonce.
5. Superseded: the legacy NCMN miner loop was removed. The remaining
   production miner loop is `pearl_mining::run`, where one counted attempt is
   one explicit Pearl-valid `(t_rows, t_cols)` ticket evaluation and recursive
   proof construction is performed only after a Nockchain target hit.

### Phase 2: Update Plain Verification

Status: implemented. The current code keeps `pow_key_for_nonce(s_A, nonce)` as
extra domain separation, but `s_A` is already nonce-bound before noise and
matmul.

1. The commitment helper consumes the full Pearl attempt state:

```rust
attempt_state(block_commitment, nonce) -> Vec<u8>
commitment_key_for_attempt(attempt_state, params_tag) -> [u8; 32]
pow_key_for_nonce(s_a, nonce) -> [u8; 32]
```

2. In `verify_at_target`, derive `attempt_state = block_state(block_commitment,
   nonce)`, then derive `kappa`, authenticate legacy `h_a` / `h_b` spot
   openings, and derive `s_B` / `s_A` from the proof's canonical
   `h_b_chunk` / `h_a_chunk` commitments under that attempt state.
3. Recompute opened tile values with `VerifierNoise::new(s_A, s_B, params)`.
4. Hash the recomputed tile state with `pow_key_for_nonce(s_A, nonce)`.
5. Reject old proofs by deriving verifier noise from the nonce-bound attempt
   state; legacy cheap-retry statements no longer verify under the new
   semantics.

### Phase 3: Update ZK Statement

Status: implemented for the Rust ZK bridge and recursive production
certificate; Hoon consensus still needs the verifier jet/wiring.

1. In `prove_ai_pow_tiled_full`, build the plain/ZK context from
   `(block_commitment, nonce)` and use nonce-bound `kappa`,
   `h_a_chunk` / `h_b_chunk`, `s_A`, and `s_B` for:
   - noise strips passed into `place_matrix_strip_opening`;
   - `BlockNoise::expand`;
   - `Matrices::build`;
   - `noise_ref::e_value` / `noise_ref::f_value` producer rows;
   - `BlockPublic { s_a, s_b }` used to build the canonical program.
2. Set `JOB_KEY = kappa`, where `kappa` is now nonce-bound because it is
   derived from `attempt_state || params_tag`.
3. Set `COMMITMENT_HASH` to the attempt jackpot key derived from the same
   nonce-bound transcript.
4. In `verify_ai_pow_block`, derive the same attempt seeds from trusted
   `(block_commitment, nonce, params, commitments)` and pass those into
   `verify_ai_pow_tiled_with_statement`.
5. Done: the recursive certificate path accepts the updated Layer-0 public
   input layout through `verify_recursive_certificate`.

### Phase 4: Update Hoon/Noun Statement

1. No new large proof artifact is required; the recursive proof remains the
   only canonical proof artifact.
2. The Rust noun boundary now exposes bounded Pearl merge artifact decoding and
   verification for the full persisted/wire artifact `[%ai-pow nonce cert]`;
   the Hoon/Rust verifier path must call this boundary with trusted Nockchain
   block data, matrix inputs, target, and aux-inclusion policy. The jam-byte
   entrypoint caps attacker input and preflights noun count, depth, and atom
   bytes before cueing.
3. The `%ai-pow` wire carries `[%ai-pow nonce cert]`, where `nonce` is an
   `@uxncmn` atom. This keeps the recursive certificate as the only proof
   artifact while still carrying the nonce commitment parameter needed to prove
   one NCMN nonce equals one fresh matmul attempt.
4. The Rust decoder/precheck rejects statement data that does not match trusted
   block data before recursive proof verification; if proof or params versions
   change again, add an explicit version check in the `ai-pow-certificate`
   decoder.
5. Do not optimize this protocol around cached matrix/noise work across
   nonces. Cache-friendly reuse between attempts is a soundness risk: a miner
   must not be able to keep one expensive matmul result and cheaply grind
   nonce-dependent hashes until the target hits.

### Phase 5: Regression Tests

Status: implemented for the main soundness paths; keep the remaining items as
future hardening/measurement work.

1. Plain attempt seed test:
   - Same `(block, A, B, params)`, different nonce.
   - Assert `kappa`, `H_A`, `H_B`, `s_A`, `s_B`, and at least one opened tile
     `M` differ before final hashing.

2. No cached `M` across nonces source/API guard:
   - No nonce-free context type may expose `m_states`.
   - `mine_block` must call the nonce-bound attempt constructor once per nonce
     or be removed.

3. Proof nonce substitution:
   - A proof generated for nonce A must fail with nonce B.
   - Implemented as a baseline regression.

4. Matmul nonce substitution:
   - Covered by stale-context rejection and verifier recomputation from the
     nonce-bound attempt state; add a dedicated adversarial seam if future API
     changes make mixed-context construction reachable again.

5. ZK public input test:
   - For two nonces, prove the same `found_idx`.
   - Assert `JOB_KEY`, `HASH_A`, `HASH_B`, `COMMITMENT_HASH`, `s_A`, `s_B`,
     noised rows, and `HASH_JACKPOT` differ.

6. Recursive certificate test:
   - Implemented: a recursive certificate rejects when verified with a different
     Layer-0 public-input vector.
   - Keep a follow-up integration test at the noun/jet boundary once Hoon calls
     the Rust verifier.

7. Miner accounting test:
   - Implemented: miner stats expose `matmul_attempts_tried` and
     `matmul_attempt_rate_per_sec`; every increment occurs after building a
     fresh nonce-bound `BlockContext` and running the target check.
   - Future hardening: instrument the attempt builder in tests and assert the
     constructor is invoked exactly once per extranonce tried.

8. Difficulty calibration test:
   - Done: `difficulty_target_is_per_tile_not_per_grid` pins that the Pearl
     target formula itself does not scale with `num_tiles`.
   - Done: `seek_best_does_not_scan_beyond_verifier_derived_attempt_tile` and
     the found-index substitution tests pin that production gets exactly one
     eligible jackpot tile per nonce-bound attempt.
   - Future measurement: sample many small test attempts and confirm the
     empirical success rate matches a single eligible tile digest per full
     matmul attempt.

### Phase 6: Benchmarks

Re-benchmark after the semantic fix:

1. Plain per-attempt latency with fresh nonce-derived matmul.
2. Recursive certificate generation latency after a successful attempt.
3. End-to-end miner rate reported as matmul attempts per second via
   `matmul_attempt_rate_per_sec`.
4. Any allowed non-work reuse:
   - input shape/range validation only;
   - immutable references to input matrix bytes.

Do not cache keyed commitments, `s_A`/`s_B`, low-rank noise, noised matrix
strips, tile states, jackpot preimages, or any other work whose reuse would
reduce the cost of a new nonce attempt.

Do not report BLAKE3-only nonce rate as mining rate. Do not optimize for cache
locality across attempts if the cache would hold anything below
`sigma_attempt` in the derivation tree.

## Acceptance Criteria

The fix is complete only when all of these are true:

1. There is no consensus path where changing only `nonce` changes only the
   final hash key while reusing old noised matmul output.
2. The verifier derives the same nonce-bound matmul seeds from trusted block
   data and rejects old cheap-retry statements.
3. The recursive certificate proves the nonce-bound matmul statement.
4. The production miner performs a fresh nonce-bound matmul attempt before each
   target check.
5. Tests explicitly cover old-proof rejection and nonce-substituted matmul
   rejection.
6. Documentation and comments no longer describe cheap nonce retries as a
   feature.
7. AI-PoW consensus acceptance remains disabled or reject-all until the above
   tests pass.

## Resolved Protocol Question: No Miner-Selected Tile Search

The repair above stops nonce grinding over one cached matmul result and also
settles the `found_idx` interpretation for the current protocol: one
nonce-bound full matmul attempt produces one verifier-derived jackpot tile.
The miner cannot turn a single full matmul into many lottery tickets by scanning
all tile digests. That cache-friendly behavior is a vulnerability because it
discounts wide matrix products by `params.num_tiles()`.

The certificate must prove the verifier-derived tile, and the verifier must
reject any other in-range `found_idx` even if its opening is internally
well-formed. If a future design wants multi-tile search, it must price that
search as separate consensus work or specify a different aggregate digest rule.
It must not reintroduce arbitrary `found_idx` selection over a cached full
matmul.

## Latest Re-Audit: Tile Preselection Boundary

The first derived-tile repair sampled the jackpot tile from
`block_state(block_commitment, nonce)` and `params_tag`. That removed
miner-selected tile submission, but it still revealed the eligible tile before
any nonce-bound matrix commitment or noise seed had been fixed. A custom miner
could therefore specialize all later work around the known tile.

The implementation now samples `attempt_tile_index` from
`block_state(block_commitment, nonce)`, `params_tag`, and `s_A`. Since `s_A`
depends on `kappa`, `H_B`, and `H_A`, changing either the nonce or the matrix
commitment transcript changes the eligible tile before the target check. The
production statement precheck rejects a certificate whose `found_idx` matches
the old commitments but not the submitted commitment transcript.

Residual design note: the current recursive certificate proves the selected
opened tile's in-circuit matmul and hash. It does not prove a full `comm_m`
Merkle tree over every tile state. If consensus requires cryptographic proof of
an entire full-matrix product per attempt, the remaining design work is to add a
full-matrix aggregate/commitment proof or keep AI-PoW block acceptance
fail-closed until that proof exists. The current Hoon path remains fail-closed,
so this residual issue is not consensus-admissible yet.

Code status after this re-audit: consensus-facing recursive-certificate noun
prechecks now call `verify_ai_pow_full_matmul_production_statement`. That API
first enforces the production parameter envelope, then rejects every multi-tile
statement with `FullMatmulProofUnavailable` until the recursive proof binds a
full-matrix aggregate. The selected-tile statement-binding helper is
crate-internal plumbing for the current Pearl-style recursive certificate,
benchmarks, and diagnostics; it is not an exported consensus admission rule.

Follow-up integration status: `prove_ai_pow_recursive_certificate`, the
production miner's certificate-builder entrypoint, now also fails closed for
multi-tile params after the cheap nonce/commitment/`found_idx` checks and
before any Layer-0 or recursive ZK proving work. That prevents the miner from
manufacturing a selected-tile recursive noun that the consensus boundary must
reject. The direct `ai-pow-zk` recursion helpers remain lower-level wrappers
over whatever Layer-0 statement the caller supplies; they are not the
full-matmul admission rule.

## Latest Re-Audit: Context/Nonce Boundary At ZK Trace Construction

The public prover and production recursive-certificate builder already rejected
a `BlockContext` supplied with a different nonce, but the deepest shared ZK
trace constructor still accepted both `ctx` and `nonce` and did not
independently check that they described the same attempt before building the
Layer-0 trace.

That lower-level helper is not a consensus entrypoint, but mixed-attempt
statements are exactly the class of bug this audit is trying to eliminate. A
caller that accidentally passed `ctx` from nonce A and `nonce` from nonce B
could build public inputs with `JOB_KEY`, noise, and matmul state from nonce A
but `COMMITMENT_HASH = pow_key_for_nonce(s_A, nonce B)`. On the production
co-location path, verifier-derived statement checks catch this mismatch. On
diagnostic non-production paths, the proof verifier can be intentionally less
strict, so the core constructor itself must fail before proof work begins.

Code status after this re-audit: `prove_ai_pow_tiled_full` now calls
`ensure_context_attempt(ctx, nonce)` immediately after parameter/context-shape
checks. The nonce-substitution regression covers this deepest shared path in
addition to the production `prove_and_verify_for_block` and recursive
certificate builder paths. The intended invariant is now local to the proof
constructor: no ZK trace is built from a context/nonce pair that is not a single
attempt.

## Latest Re-Audit: Target-Hit Boundary Before Recursive Proving

The production miner now constructs recursive certificates only from a winning
Pearl-compatible ticket attempt. The normal node-facing miner does not start
recursive proving unless `pearl_mining::run` has returned a ticket that clears
the Nockchain target. The public recursive certificate builders still need the
same cheap check at their own boundary so direct callers cannot ask them to
build the Layer-0 proof first and only learn after public-input derivation that
`HASH_JACKPOT > target`.

That is not a forged accepted block because the later statement check rejects,
but it is the wrong API shape for a production certificate constructor: missed
targets must fail before any ZK proving work. It also keeps the attempted work
unit unambiguous: a recursive certificate is only requested for an already-won
nonce-bound matmul attempt.

Code status after this re-audit: `prove_ai_pow_recursive_certificate` now calls
`ensure_found_tile_hits_target(ctx, nonce, target, found_idx)` after the
production parameter, context/nonce, and verifier-derived `found_idx` checks
and before `prove_ai_pow_tiled_full`. The helper hashes the already-computed
nonce-bound `ctx.m_states[found_idx]` with `pow_key_for_nonce(ctx.s_a, nonce)`
and rejects `FoundAboveTarget` before Layer-0 or recursive proving begins.
The crate-internal Layer-0 solved-block constructor and params-derived
`prove_and_verify_for_block` path use the same guard. Regression coverage
asserts the recursive builder rejects a missed target before ZKP.

## Latest Re-Audit: Public Attempt Context Mutability

`BlockContext` is part of the public Rust API because tests, diagnostics, and
the miner use it as the precomputed handle for one nonce-bound attempt. Before
this re-audit, every field on that handle was public, including `nonce`,
`attempt_state`, `s_A`, `s_B`, and `m_states`. That made it possible for
external callers to mutate the attempt identity or nonce-bound work fields
directly and then pass the incoherent context into lower-level helpers.

The production verifier paths do not trust a caller-mutated context, but public
mutable fields are still the wrong shape for a soundness-critical attempt
handle. They make it easy for downstream code to accidentally bypass the
constructor invariant that `kappa`, commitments, seeds, noise, and tile states
all came from the same `(block_commitment, nonce, params, A, B)` attempt.

Code status after this re-audit: `BlockContext` fields are now crate-private.
External users can build a context through `BlockContext::build` and inspect
read-only values through accessors such as `nonce()`, `s_a()`,
`h_a_chunk()`, and `tile_states()`, but they cannot rewrite nonce-bound work
fields. The nockchain wire regression asserts that `nonce`, `s_A`, and
`m_states` remain crate-private while read-only accessors remain available.

Follow-up API hardening: `BlockContext` is no longer re-exported from the
`ai-pow` crate root. It remains available only through the explicit
`ai_pow::prover::BlockContext` module path for diagnostics, tests, and miner
internals. This keeps the crate-root production-looking API centered on
`mine`/`mine_block`, both of which rebuild the nonce-bound attempt state, and
prevents the cached attempt handle from being advertised as a normal mining
primitive.

Follow-up harness hardening: the executable `f1_harness` had stale historical
wording and trace construction that set the ZK `COMMITMENT_HASH` slot to raw
`s_A`. Production code already binds this slot to
`pow_key_for_nonce(s_A, nonce)`. The harness now uses `ctx.pow_key()`, a
context-bound accessor that derives the jackpot key from the context's own
nonce, and the wire regression rejects the old `COMMITMENT_HASH = s_a` wording.
This keeps local profiling examples from reintroducing the nonce-independent ZK
jackpot-key bug as copy-paste guidance.

Follow-up diagnostic hardening: real-model compatibility tests and ZK bridge
internal tests no longer hash cached `TileState` values with raw `ctx.s_A`.
They use `ctx.pow_key()` so even non-consensus assertions exercise the same
nonce-bound jackpot key as production. The wire regression rejects
`keyed_hash(ctx.s_a())` / `keyed_hash(&ctx.s_a)` patterns in these diagnostic
surfaces.

Follow-up design-doc hardening: older `ai-pow-zk` design reports had live
guidance that still described `HASH_JACKPOT` or `COMMITMENT_HASH` as raw
`s_A`. Those reports now say Nockchain uses
`pow_key_for_nonce(s_A, nonce)`, while Pearl-only historical context remains
explicitly separated. The wire regression guards the live ZK design docs
against drifting back to raw-`s_A` jackpot-key wording.

## Latest Re-Audit: Decoded Recursive Certificate DoS Ordering

Historical note: the decoded Hoon-compatible certificate verifier had the right
ordering for recursive proof verification, but not for recursive proof
reconstruction. A block with stale nonce, wrong target, wrong anchor, or
multi-tile selected-tile metadata could therefore force extra
recursive-certificate deserialization/canonicalization work before being
rejected.

This is not a forged-proof acceptance bug, but it is the wrong consensus
boundary for a hostile wire artifact. The verifier should reject all cheap
metadata failures before doing any work proportional to the proof tree whenever
the already-decoded shape carries enough metadata to do so.

Code status after this re-audit: the Pearl merge artifact verifier now runs
metadata-only Pearl/Nockchain statement prechecks before
`ai_pow_recursive_certificate_from_node`. Regression coverage constructs an
intentionally invalid proof-node and confirms replay or wrong statement
metadata returns the cheap precheck error before proof-node reconstruction is
attempted. The legacy NCMN verifier entrypoints have since been removed.

## Latest Re-Audit: Legacy Plain-Proof Envelope Naming

`MatmulProof` still exposes historical helpers named `encode_consensus` and
`decode_consensus_for_params`. Those helpers serialize the plain Pearl opening
proof, not Nockchain's canonical production AI-PoW block artifact. The names
are legacy from the earlier selected-tile/Layer-0 transition and are easy to
misread now that production submission is `[%ai-pow nonce cert]` with a
structured recursive certificate noun.

This is an API soundness hazard rather than a verifier acceptance bug: a
downstream caller that sees "consensus" on `MatmulProof` could accidentally
persist or transmit the wrong proof object, bypassing the recursive certificate
path and reopening the selected-tile/plain-proof confusion this audit is
closing.

Code status after this re-audit: the `MatmulProof` module now documents itself
as the legacy plain-proof wire format, states that it is not the canonical
production block/wire proof, and marks the versioned plain-proof envelope
helpers deprecated with a note pointing callers to the structured recursive
certificate noun. The nockchain wire regression asserts those warnings remain
present.

## Latest Re-Audit: Jammed Artifact DoS Ordering

Historical NCMN note: the byte-oriented NCMN verifier previously routed through
a generic `%ai-pow` jam decoder, which fully decoded the structured proof-node
tail before the verifier checked trusted block data or re-derived cheap
statement metadata. That verifier has now been removed from `ai-pow-miner`; the
generic decoder is crate-internal, and the active Pearl merge verifier keeps
the same intended ordering through the public Pearl-specific artifact APIs.

This is bounded by `CertificateNounLimits`, so it is not an unbounded parser
bomb, but it is still the wrong consensus ordering: cheap rejection using
trusted block data should happen before semantic traversal of the
miner-controlled recursive certificate tree whenever the top-level noun shape
already exposes the required metadata.

Code status after this re-audit: Pearl merge artifact verification performs the
byte-length cap, jam preflight, cue, canonical-jam check, artifact tag check,
metadata decode, and Pearl/Nockchain statement precheck before decoding the
proof-node tail or reconstructing the
recursive certificate. Regression coverage builds jammed `%ai-pow` artifacts
with an intentionally invalid proof node and confirms wrong candidate anchors
and missed targets return the cheap precheck errors before proof-node decode.

## Latest Re-Audit: Plain Verifier API Visibility

The plain `MatmulProof` object, plain mining helpers, and plain verifier remain
useful as internal/diagnostic target-hit plumbing before recursive certificate
generation, but they are not canonical block proof APIs. Crate-root re-exports
make the plain proof path look like normal production surface area and increase
the risk that a downstream caller admits a legacy selected-tile/plain proof
instead of the structured recursive certificate noun.

Code status after this re-audit: `ai-pow` no longer re-exports plain proof
objects, plain mining helpers, or plain verifier functions from the crate root.
Callers that intentionally need diagnostic/plain pre-ZKP functionality must use
explicit `ai_pow::proof::...`, `ai_pow::prover::...`, or
`ai_pow::verifier::...` module paths. The production miner uses explicit module
paths before recursive certificate generation, and the crate docs state that
plain verifiers are diagnostics/prechecks, not canonical block-acceptance APIs.

## Latest Re-Audit: Layer-0 ZK Side-Effect Admission

The crate-internal `prove_and_verify_for_block` path is not a persisted block
artifact, but it was still reachable as a `mine()` side-effect when `ai-pow` was
compiled with the `zk` feature. For multi-tile production parameters, that
side-effect proved one verifier-derived selected tile. That is useful test
coverage for the Layer-0 circuit, but it is not a proof of one full-matmul work
unit and should not run behind a production-looking boundary.

Code status after this re-audit: production-envelope
`prove_and_verify_for_block` now performs the selected-tile/full-matmul
admission guard and returns `FullMatmulProofUnavailable` for
`params.num_tiles() > 1` before selected-tile ZK proving. The `mine()` ZK
side-effect only runs on the single-tile smoke shape. That path is retained as
Layer-0 circuit coverage, not as a multi-tile block certificate. The structural
`prove_and_verify_for_block_inner(..., require_prod_envelope = false)` path
remains available for local AIR/chip regression tests that intentionally
exercise arbitrary selected tiles.

## Latest Re-Audit: ai-pow-zk Top-Level Documentation

The `ai-pow-zk` library docs had been partially updated for recursive
certificates, but the README still described the crate as wrapping a multi-MB
plain proof and constructing traces from a verified plain proof. That wording
was historical and conflicted with the current canonical block artifact:
`[%ai-pow nonce cert]` carrying the recursive certificate noun, with
`MatmulProof` only serving as miner diagnostic / pre-ZKP target-hit evidence.

Code status after this re-audit: the `ai-pow-zk` README and crate-level docs
now describe the recursive AI-PoW certificate as the production-facing artifact,
state that raw Layer-0 proofs and plain `MatmulProof` values are not persisted
block proofs, and repeat the fail-closed rule for selected-tile multi-tile
statements that lack full-matrix binding. The Nockchain wire regression now
checks the `ai-pow-zk` top-level docs for those claims and rejects the old
plain-proof-wrapping phrases.

The same top-level docs now surface the minimal-reuse security invariant:
changing the explicit attempt input must force fresh transcript-derived
commitments, noise, noised matrix strips, tile states, jackpot preimages, and
proof witness data. Cache-friendly attempt reuse is a vulnerability, not a
desired trait: any reuse of those values across attempts is treated as
consensus attack surface, not as a production optimization target.

## Latest Re-Audit: Miner Submission Preflight

The node-connecting `ai-pow-mine` integration previously allowed production
mining to start with multi-tile parameters and only discovered the current
selected-tile/full-matmul recursive proof gap after a plain matmul solution was
found. That behavior was fail-closed for consensus submission, but it still
made the operator-facing miner look usable for a configuration that cannot
produce the canonical block certificate.

Code status after this re-audit: `ai_pow::zk_bridge` now exposes
`validate_canonical_recursive_certificate_params`, the single gate for whether
the current recursive certificate artifact is full-matmul-admissible for a
parameter set. The node miner calls this gate during startup preflight and
refuses to connect or enable mining when no recursive certificate builder is
configured, or when the current selected-tile recursive statement is used with
multi-tile parameters. The legacy NCMN miner path has since been removed; the
remaining tests cover required Pearl submission config, parameter/config
mismatches, unsupported recursive params and patterns, stale recursive-run
metadata, target misses that produce no `%ai-pow` poke, and successful
Nockchain-side Pearl-compatible submission. The `ai-pow-mine` CLI now uses
Pearl-compatible submission with single-tile production-envelope smoke parameters
(`m=8, k=512, n=8, r=32, tile=8, sigma=1`) for local Layer-0 development; the
Pearl header fields are still explicit because they define the shared attempt
transcript. Multi-tile canonical block submission remains fail-closed until the
recursive certificate binds a full-matrix aggregate.

Follow-up code status: the prover-only `ai-pow-mine-prover` smoke CLI and the
legacy NCMN miner module have been removed. They served only diagnostic
plain-proof smoke coverage and were not the canonical production submission
API.

## Latest Re-Audit: Seed-Root Binding

The earlier recursive statement metadata carried `h_a` and `h_b` and derived
`s_B`, `s_A`, noise, the verifier-derived tile index, and the final jackpot key
from those row/column opening roots. The Layer-0 proof, however, binds the
chunk-Merkle commitments `h_a_chunk` and `h_b_chunk` as `HASH_A` / `HASH_B`.
Without an in-proof relation between those two commitment families, `h_a` /
`h_b` were an extra prover-chosen seed surface.

Code status after this re-audit: the canonical seed chain now derives from the
proof-bound chunk commitments through
`canonical_noise_seeds_from_matrix_commitments(kappa, h_a_chunk, h_b_chunk)`.
`BlockContext`, the legacy plain verifier, selected-tile statement prechecks,
and noun certificate fixtures all use this helper. `h_a` / `h_b` remain legacy
spot-opening roots and are no longer seed inputs. Native selected-tile
recursive statements can pass the Rust production precheck only for single-tile
params; native multi-tile params still fail first with
`FullMatmulProofUnavailable` until that native path binds a full-matrix
aggregate. Pearl-compatible production consensus is now governed by the
2026-06-01 Pearl merge spec and uses explicit Pearl tile-ticket semantics.

## Latest Re-Audit: Hoon Commitment Mold Source of Truth

After moving canonical seed derivation to proof-bound chunk commitments, the
Rust noun codec serialized only `[h_a_chunk h_b_chunk]`, but the Hoon
`ai-pow-commitments` mold still listed the legacy row/column roots before the
chunk commitments. That was not yet an accepting consensus path because the
Hoon verifier remains fail-closed, but it was a dangerous boundary drift:
future Hoon-side code could accidentally treat `h_a` / `h_b` as persisted
commitment parameters even though they are not bound by the recursive proof's
`HASH_A` / `HASH_B` public inputs.

Code status after this re-audit: `hoon/common/tx-engine-1.hoon` now defines
`ai-pow-commitments` as exactly two fields, `h-a-chunk` and `h-b-chunk`. The
Nockchain wire regression checks for that two-field mold and rejects the old
`h-a=ai-blake` / `h-b=ai-blake` entries. The canonical persisted/wire
commitment surface is now the same in Rust, the noun encoder, the specification
document, and the Hoon type mold.

## Latest Re-Audit: Jammed Artifact Verifier Source of Truth

The jammed Pearl merge `%ai-pow` verifier enforces the intended DoS-resistant
ordering: byte cap, jam preflight, cue, canonical jam check, metadata-only
Pearl/Nockchain statement precheck, proof-node reconstruction, and recursive
verification. A small cleanup remained: after prechecking the metadata-only
decode, the final recursive verifier used the public-inputs field from a second
full certificate decode of the same noun. Honest artifacts decode identically,
but the production boundary should not carry two apparent public-input sources.

Code status after this re-audit: the Pearl merge verifier verifies the
reconstructed recursive certificate against the public-input vector that passed
the cheap verifier-derived statement precheck. The full certificate decode is
still used to reconstruct the proof
tree, but it no longer supplies a separate public-input source for recursive
verification. The Nockchain wire regression guards this verifier ordering.

This is intentionally fail-closed. Pearl's reference miner computes every
output tile before returning the final matrix and records an opened block when a
tile hash hits the target, but the proof artifact only opens the winning tile.
Honest-miner full scans are not a soundness argument against a custom miner. A
verifier that receives only the recursive selected-tile proof cannot tell
whether all other tile states were computed, so multi-tile AI-PoW block
acceptance must remain disabled until one of the following is implemented:

1. Recursive proof includes and verifies a commitment/aggregate over every tile
   state and derives the jackpot from that aggregate.
2. Recursive proof recursively folds all tile traces or an equivalent
   full-matrix computation certificate into the production certificate.
3. The protocol explicitly redefines the work unit as one selected tile and
   prices difficulty accordingly. This is not the current "one matmul = one
   attempt" target.
