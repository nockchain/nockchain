# AI-PoW: arbitrary-model support (Pearl parity) — options & roadmap

**Status:** **UPDATED 2026-07-14 after investigating (a) — recommendation flipped
to (a).** Both options remove the synthetic-matrix pin. This is a
soundness-critical consensus change — implement in validated stages (R1).

> ## UPDATE: (a) is tractable and is the recommended path (investigation result)
>
> The circuit **already binds the jackpot to the matmul over the committed A/B**.
> The pinned production AIR (`CompositeFullAirPinned`, `composite_full_air.rs`
> ~299–373, active for `num_stripes ≤ STRIPE_MAX` = the PROD case) closes the full
> chain `committed A/B → CUMSUM → SX_IN → SX_XR → FOLD_XSTEP → FoldChip →
> FOLD_STATE → JACKPOT_MSG → BLAKE3 → hash_jackpot` (constraints: `JACKPOT_MSG ==
> FOLD_STATE` on the last row; `Σ FOLD_STRIPE_SEL[s]·(FOLD_XSTEP − SX_XR[s]) = 0`;
> `SX_IN == nxt.CUMSUM_TILE`). `hash_jackpot` is already a **public input** (PI
> index 52..60). So:
> - **(a) is NOT a circuit change.** No new sub-AIR. The jackpot binding is done.
> - The off-circuit `compute_moe_tile` + synth re-derivation is **redundant** with
>   the proof for the jackpot; its only unique job is enforcing `H_A/H_B == synth`
>   — the pin we are removing.
> - **(a) is a verifier-side simplification:** verify the recursive cert, read the
>   proven `hash_jackpot` PI, check `≤ target`, accept the miner-committed
>   `H_A/H_B`; delete the synth recompute. **~0 artifact-size delta.**
> - **The one real dependency is D6/M2** — the recursive cert must be verified
>   against a **canonical verifier context** derived from the pinned program (not
>   the prover's supplied context). The current off-circuit recompute is precisely
>   the workaround for the not-yet-closed D6 binding. **D6 is needed regardless**
>   (compact-route soundness + valid-cert acceptance), so (a) rides on it and then
>   deletes the synth workaround — it does not add work beyond D6.
>
> Net: (a) has no circuit cost, ~0 size cost, and converges with the D6 work
> already on the critical path. (b) below is retained for the record.

**Date:** 2026-07-14
**Owner decision needed:** confirm (b) after Stage 0 measures the real size delta;
escalate to (a) if the delta is unacceptable (see §5.3, §7).

---

## 1. Goal

Make nockchain accept `%ai-pow` blocks mined over **arbitrary miner-chosen
matrices** `(A, B)` — the same as Pearl — rather than a single protocol-fixed
synthetic matrix set. This is the difference between "proof-of-useful-work over
whatever model the miner runs" (Pearl) and "proof-of-work over a fixed synthetic
GEMM" (nockchain today).

Non-goal: changing the mineable primitive (commitment chain, noise, difficulty
formula, keyed-BLAKE3 chunk-Merkle, periodic patterns) — those are already
byte-parity with Pearl and must stay so.

---

## 2. Why nockchain currently pins synthetic matrices

The consensus verifier (`ai-pow-miner::verify_ai_pow_block_artifact`, the entry
the `%ai-pow` jet calls) does, per block:

1. Re-derive `(A, B) = synth_matrices(AI_POW_PROD_SYNTH_SEED, params)`
   (`certificate_noun.rs` ~3170).
2. Recompute the work commitments from those matrices and **require the block's
   committed `hash_a`/`hash_b` to equal them** (`certificate_noun.rs:1235` via
   `derive_pearl_work_commitments` → `pearl_matrix_commitments`).
3. Recompute the opened **jackpot tile off-circuit** from those matrices and
   require `jackpot == hash_jackpot` and that it meets target — the
   *jackpot/difficulty gate the recursive certificate omits* (`compute_moe_tile`
   / `compute_tile`; comment in `verify_decoded_..._compact_artifact_...`).

Steps 2 and 3 both need the **full matrix entries**, and the only witness-free way
the verifier can get them is to re-derive `synth(seed, params)`. That — and only
that — is what forces every block to use the one synthetic matrix set. The
recursive certificate, the commitment binding, and the noise are all already
model-agnostic.

**The synth pin is NOT an anti-grinding measure** (an earlier claim, retracted).
Anti-grinding is the **noise matrix**, which is keyed to the commitment of the
matrices in use, so every attempt is a real noised matmul (see §3). nockchain
already computes it (`s_a`/`s_b`/`BlockNoise`). The synth pin only exists to give
the *off-circuit tile recompute* a matrix source.

---

## 3. What Pearl does (verified against the `pearl/` checkout)

- `ffi/mine.rs`: generates **random** `A` (m×k), `B` (k×n) per attempt
  (`rng.random_range(signal_min..=signal_max)`), derives the noise seeds via
  `compute_commitment_hash(job_key, A, B)`, and **adds noise before the matmul**.
  The noise — bound to the commitment of the very matrices being used — is the
  entire anti-grind. Grinding `(A,B)` re-randomizes the noise, so each attempt
  costs a full noised matmul. This is the useful work.
- `api/sanity_checks.rs`: constrains only **shape** (`r ∈ {2⁵..2¹⁰}`,
  `16r ≤ k ≤ 4r²`, `k%64==0`, `k≥1024`, `h·w ∈ [32,256]`, `m,n ≤ 2²⁴`, patterns).
  **Nothing constrains matrix values.** No non-degeneracy check is needed — the
  noise handles it.
- `api/proof.rs::PublicProofParams`: carries only `hash_a`/`hash_b` (blake3
  commitments keyed by `job_key = blake3(block_header ‖ mining_config)`); raw
  matrices are the witness.
- Two verification routes:
  - **plonky2 ZK proof** — proves the whole noised matmul + jackpot in-circuit;
    the verifier needs only the public commitments.
  - **plain-proof** (`ffi/plain_proof.rs`, `MatrixMerkleProof`) — carries the
    **opened `A`/`B` strips** for the jackpot tile, authenticated against
    `hash_a`/`hash_b`, and recomputes the tile off-circuit.

nockchain's compact recursive certificate is the analog of Pearl's ZK route but
**omits the jackpot gate**, so it falls back to a plain-proof-style off-circuit
tile recompute — without carrying the strips — which is exactly why it needs
synth. Both options below fix that.

---

## 4. What changes in either option (common)

1. **Drop** the `synth_matrices(...)` re-derivation in
   `verify_ai_pow_block_artifact`.
2. **Drop** the "block `hash_a`/`hash_b` must equal synth-recomputed commitments"
   check. Accept the miner's committed roots as the model fingerprint (matrices
   stay fully miner-chosen, Pearl base-PoW style).
3. Keep unchanged: the commitment chain, the noise derivation, the difficulty/
   target check, the recursive-certificate verify, and the opened-schedule
   binding (`l0_program_matches`).

The options differ only in **how the jackpot/difficulty check gets its data**.

---

## 5. Options

### 5.1 Option (a) — bind the jackpot in-circuit (commitment-only verify)

Extend the recursive certificate to **prove jackpot-meets-difficulty in-circuit**
(or at least expose the in-circuit `JackpotHash` as a bound public output). The
verifier then reads the 32-byte jackpot from the certificate's public inputs and
checks `jackpot ≤ target`. No matrices, no strips.

- **Verifier inputs:** committed `hash_a`/`hash_b` + jackpot (already public) + the
  recursive cert. Arbitrary matrices "just work."
- **Artifact size delta:** ≈ **0** (the jackpot is 32 bytes and likely already a
  public value).
- **Cost:** an in-circuit change to the canonical program / composite AIR — the
  item historically flagged as the big R1 wall. **BUT** the jackpot is already
  hashed in-circuit (`RowClass::JackpotHash` exists in
  `canonical.rs`), so the real work may be only *binding that hash as a public
  output and adding the `≤ target` comparison*, not a new sub-AIR. **This cost
  must be re-scoped before dismissing (a)** (see §7) — it may be far smaller than
  its reputation, and it preserves the whole point of the compact certificate.

### 5.2 Option (b) — carry authenticated opened strips (CHOSEN, pending §5.3)

Mirror Pearl's plain-proof: put the opened `A`/`B` strips for the jackpot tile in
the `%ai-pow` artifact, authenticate them against the committed `hash_a`/`hash_b`
(blake3 chunk-Merkle multi-proof), and recompute the jackpot tile from the
authenticated strips instead of from synth.

- **Verifier inputs:** committed roots + opened strips + Merkle branches + the
  recursive cert.
- **Artifact size delta:** **significant** — see §5.3.
- **Cost:** no circuit change. nockchain already has the strip-opening machinery
  (`ai-pow-miner::pearl_plain_proof.rs`, `MatrixMerkleProof` with `row_indices`);
  it just isn't wired into the *compact* artifact. Soundness-local: strips
  authenticate against the same roots the cert binds.
- **Caveat:** re-introduces raw matrix data into the on-chain artifact — partially
  undoing the compactness the recursive certificate was built to achieve.

### 5.3 Proof-size comparison — the deciding axis

The jackpot tile needs `h` rows of `A` and `w` cols of `B`, each of length `k`
(INT8), chunk-padded to 1024 B. Raw strip bytes ≈ `(h + w) · k` (plus Merkle
branches).

| config | h·w (tile) | k | raw strip bytes `(h+w)·k` | +Merkle (est.) | vs ~122–124 KB compact cert |
|---|---|---|---|---|---|
| `PROD` (Pearl-faithful) | 8·8 | 4096 | 16·4096 = **64 KiB** | ~10–20 KiB | **+~55–65 %** |
| max tile | 16·16 | 4096 | 32·4096 = **128 KiB** | ~15–30 KiB | **+~120 %** |
| small `k` | 8·8 | 1024 | 16·1024 = **16 KiB** | ~5 KiB | +~17 % |

Option (a): **+~32 bytes.**

> These are estimates. **Stage 0 must measure the real serialized delta for
> `PROD`** (opened row/col count follows the periodic pattern, and the Merkle
> multi-proof overhead depends on scatter). If measured (b) lands near the table,
> the size hit is large enough that **(a) should be strongly preferred** —
> especially since (b) works against the compact certificate's reason for
> existing.

---

## 6. Option (b) roadmap (staged, R1 — KAT-first, commit per validated stage)

**Stage 0 — de-risk + MEASURE (gate the whole decision).**
- Reuse/port the strip-opening + authentication (`pearl_plain_proof.rs`
  `build_matrix_proof`/`extract_strips`; Pearl `ffi/plain_proof.rs` as oracle).
- KAT: open the jackpot-tile strips for a real `PROD` block, authenticate against
  `hash_a`/`hash_b`, recompute the tile, assert `jackpot == hash_jackpot`.
- **Measure** the serialized strip+branch bytes for `PROD` and record it here.
- **Decision checkpoint:** if the delta is unacceptable, stop and pursue (a).

**Stage 1 — artifact carries the strips.**
- Extend the `%ai-pow` compact artifact noun (`AiPowCertificateShape` / the MoE
  artifact) with the opened `A`/`B` strips + Merkle branches. Encode/decode in
  `certificate_noun.rs`. Add hard bounds (max strip len, max branch, max opened
  count) for DoS — mirror the existing `*_MAX` limits.

**Stage 2 — verify sources the tile from strips (drop synth).**
- In `verify_ai_pow_block_artifact`: remove `synth_matrices(...)`; instead
  (i) authenticate the carried strips against the committed `hash_a`/`hash_b`
  (Merkle), (ii) recompute the noised jackpot tile from the strips
  (`compute_tile`/`compute_moe_tile` fed the opened strips, not full matrices),
  (iii) keep `jackpot == hash_jackpot` + `≤ target`.
- Remove the "committed roots must equal synth-recomputed" check (§4.2).
- **Soundness binding (critical):** the strips must be the *same rows/cols* the
  recursive cert's opened schedule proves — bind via the existing
  `l0_program_matches` / opened-schedule check, so a miner cannot open honest
  strips for the off-circuit jackpot while proving a different matmul.

**Stage 3 — miner emits strips.** `ai_pow_mine.rs` already has the matrices;
serialize the opened strips into the artifact. Allow a real (non-synth) matrix
source (arbitrary model), keeping synth available as a default/test source.

**Stage 4 — validation (all must pass, atomic soundness unit).**
- e2e: a block mined over **arbitrary (non-synth) matrices** is ACCEPTED through
  `heard-block` (jet → %.y). (This also closes the standing "valid-cert
  acceptance" residual, since the setup/verifier-key path is exercised.)
- Adversarial: forged strips (not under `hash_a`/`hash_b`) reject; strips
  inconsistent with the cert's opened schedule reject; wrong jackpot rejects;
  unmet difficulty rejects; degenerate matrices still verify (noise makes them
  valid work — parity with Pearl).
- Full `ai-pow-zk` + `ai-pow` + `ai-pow-miner` + dumb regression green.

---

## 7. Option (a) investigation — RESULT (2026-07-14): tractable, no circuit change

Findings (see the UPDATE box at the top for the summary):

1. The jackpot hash is computed in-circuit (`RowClass::JackpotHash` /
   `place_jackpot_hash_block`, `BLAKE3(JACKPOT_MSG, key=COMMITMENT_HASH)`).
2. It is a **bound public input** (`hash_jackpot`, PI 52..60) — and critically,
   `JACKPOT_MSG` is **constrained to the fold/matmul output** in the pinned
   production AIR (`composite_full_air.rs` ~299–373): `JACKPOT_MSG == FOLD_STATE`
   (last row) and `FOLD_XSTEP == SX_XR[stripe]` with `SX_IN == nxt.CUMSUM_TILE`,
   closing `committed A/B → … → hash_jackpot` for `num_stripes ≤ STRIPE_MAX` (the
   PROD path, `sx_bound=true`). So the jackpot is already proven over the committed
   matrices. **No new sub-AIR.**
3. The difficulty target stays a **node-side** check (`hash_jackpot ≤ target`),
   read from the proven PI — it is not an in-circuit input, so no folding
   interaction.
4. Added constraints/proof time vs today: **none** (circuit unchanged).

**The gating dependency is D6/M2, not the circuit.** To drop the off-circuit
`compute_moe_tile` + synth and trust the in-circuit `hash_jackpot`, the recursive
cert must be verified against a **canonical verifier context** derived from the
pinned program (not the prover's supplied `verifier_context`,
`recursion.rs:1865`). D6 has a documented wall (the compact cert omits the L0
`BatchProof` a naive context-builder wants), with two known paths: (i) build the
context witness-free from `canonical_program_for_strip_schedule` + shape, or
(ii) bind the program's preprocessed-commitment into the verifier-key digest. D6
is required regardless — for compact-route soundness and for valid-cert
acceptance — so (a) rides on it and then deletes the synth workaround.

### Revised recommendation

**Go (a).** Sequence: **close D6 (canonical verifier-context builder)** → then the
`%ai-pow` verify becomes: verify recursive cert against the canonical context,
read `hash_jackpot` PI, check `≤ target`, accept miner-committed `H_A/H_B`, delete
synth + `compute_moe_tile`. Arbitrary models, ~0 size delta, and it closes the
standing valid-cert-acceptance residual in the same stroke. Option (b) below is
retained only as a fallback if D6 proves intractable.

---

## 8. Soundness invariants (must hold in either option)

- **Commitment binding:** the block's `hash_a`/`hash_b` are what the cert and the
  jackpot are computed against (noise keyed to them). Matrices are miner-chosen;
  the noise is the anti-grind (§3).
- **Opened-schedule binding:** the rows/cols used for the jackpot (strips in (b);
  in-circuit opening in (a)) are the same the recursive cert proves
  (`l0_program_matches`). No favorable-strip substitution.
- **No new trust:** the verifier key stays a deterministic preprocessing of the
  pinned puzzle *shape* (transparent FRI-STARK; no ceremony). Arbitrary *matrices*
  do not change the shape, so the verifier key is unaffected.
- **Difficulty:** `jackpot ≤ nockchain-adjusted target` (ASERT), unchanged.

---

## 9. What still gets pinned (unchanged by this work)

Independent of (a)/(b): the puzzle **shape** envelope (`MatmulParams` + MoE
`{e, top_k}` within the §4.8 band incl. the nockchain narrowings `k/r ≤ 64`,
`h·w ≤ 256`), the merge-mining aux/domain constants
(`PEARL_NOCKCHAIN_AUX_COMMITMENT_TAG`, chain-id, extra-domain-data), and the
activation height. `AI_POW_PROD_SYNTH_SEED` is demoted from a **consensus pin** to
a **miner default** (test/bootstrap matrix source), and its `synth.rs` "must be
canonically pinned … otherwise a miner grinds" note should be corrected: the noise
is the anti-grind, and the seed is not a consensus requirement.

---

## Appendix — file/function index

- Verifier entry (jet core): `ai-pow-miner::certificate_noun.rs`
  `verify_ai_pow_block_artifact` (~3129); synth re-derivation (~3170).
- Commitment recompute + match: `pearl_merge_recursive_certificate_parts_from_ticket`
  (~1227, check at 1235); `ai-pow::pearl_compat.rs::derive_pearl_work_commitments`
  (2616), `verify_pearl_compatible_work` (2178), `verify_pearl_pattern_ticket`.
- Off-circuit tile / jackpot: `ai-pow::pearl_compat.rs::compute_moe_tile` (1419),
  `compute_tile` (via 2677).
- Existing strip-opening to reuse: `ai-pow-miner::pearl_plain_proof.rs`
  (`MatrixMerkleProof`, `row_indices`); Pearl oracle `pearl/zk-pow/src/ffi/plain_proof.rs`
  (`build_matrix_proof`, `extract_strips`).
- In-circuit jackpot (option a): `ai-pow-zk::canonical.rs` `RowClass::JackpotHash`.
- Noise (anti-grind, already present): `ai-pow::fiat_shamir.rs` (`noise_seed_a/b`),
  `BlockNoise`.

---

## Status — option (a) IMPLEMENTED, validated, and adversarially audited (2026-07-14)

Option (a) is done: `%ai-pow` (dense AND MoE) accepts **arbitrary miner-chosen
matrices** via the COMPACT certificate, with **no synthetic-matrix pin** anywhere on
the production verify path. The in-circuit jackpot binding (`JACKPOT_MSG == FOLD_STATE`
in the pinned `CompositeFullAirPinned`) already existed; (a) simply makes the node
**trust the proven `hash_jackpot`/tile** instead of recomputing a tile off-circuit
from a pinned synthetic model.

Commits: Stage 1 `d29c8020` (MoE), Stage 2+3 `be4eeaa2` (dense compact + synth removal),
audit `<this branch>` (forged raw-tile de-risk).

Change surface:
- `ai-pow::pearl_compat.rs::verify_pearl_compatible_work_committed` — matrix-free dense
  work precheck (committed `H_A`/`H_B`, public-pattern rows/cols, difficulty on the
  authenticated `hash_jackpot`, no tile recompute).
- `ai-pow-miner::certificate_noun.rs::precheck_ai_pow_pearl_merge_artifact_statement_committed`
  — dense node precheck; `precheck_pearl_merge_bound_public_inputs` gained a
  `check_jackpot` flag (FALSE on the compact node path — the raw tile PI is proof-bound
  in-circuit; `hash_jackpot` is always bound, tying difficulty to the proof).
- MoE compact verify drops the `a/b` matrix args and binds
  `digest(pis.hash_jackpot) == public_params.hash_jackpot`.
- `verify_ai_pow_block_artifact` (jet core) drops `synth_matrices(AI_POW_PROD_SYNTH_SEED)`
  and passes empty matrix slices; both compact dispatch paths ignore them.

Validation: e2e node-branch verify (dense 103s, MoE 27s) through the synth-free jet
core; de-risk forged `hash_jackpot` AND forged raw-tile `jackpot` both reject (25s);
ai-pow-jets real-proof KATs (3) accept a real block through the jet core + reject
garbage; fast node suite 137+9; ai-pow-zk green; roswell binary rebuilt clean.

Adversarial audit (2 passes, clean):
1. Difficulty gate — proven `hash_jackpot` == authenticated `hash_jackpot` ≤ target
   (no forge). 2. Matrices — matmul bound to committed `H_A`/`H_B` via `kappa`.
3. Favorable-strip — schedule from the public pattern, bound via canonical
   `l0_program_commitment` (+ MoE routing binding). 4. Verifier context — node-built
   canonical setup + `cert.digest == canonical digest`. 5. Dense-skipped raw tile —
   proof-bound (de-risk forged-tile reject). Code-level: `expected` pis built entirely
   from tile-INDEPENDENT commitments (`pearl_merge_recursive_public_inputs_from_work`);
   the only tile-dependent field (`pis.jackpot`) is default + skipped.

### Precise residual (NOT part of (a); pre-existing; blocks LIVE acceptance)

The compact verifier **setup is not injected** in any production binary — no callers
of `ai-pow-jets::init_ai_pow_verifier_setup` outside its definition + tests. With
`SETUP` empty and a well-formed compact artifact, the jet `BAIL_FAIL`s. This blocks
LIVE acceptance of ANY real `%ai-pow` compact cert (dense/MoE, synth/arbitrary alike)
— it is orthogonal to (a). Remaining steps:
1. Pin the canonical production params (`{hw, e, top_k}`) — a **consensus decision**,
   since the setup/verifier-key digest commits to them.
2. At node boot (jet-registration point), build the setup once via
   `ai-pow-jets::setup::build_verifier_setup(prod_params)` (proves one canonical block)
   and inject with `init_ai_pow_verifier_setup`. Decide boot-compute vs embed-precomputed
   (boot latency tradeoff).
3. Full-binary e2e: a real block verifies live (not just via the test harness that
   supplies the setup directly).
