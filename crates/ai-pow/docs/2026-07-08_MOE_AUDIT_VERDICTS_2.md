# MoE adversarial audit ROUND 2 — verdicts

**Date:** 2026-07-08
Companion to `2026-07-08_MOE_ADVERSARIAL_AUDIT_ANGLES_2.md` (angles N1–N15). One
section per angle with the **verdict** (SAFE / ISSUE→MITIGATED / EXPLOITABLE) +
evidence and any mitigation landed.

---

## N1 Column-within-expert bleed — **ISSUE FOUND → MITIGATED**

**Confirmed real.** Pearl draws the intra-expert column pattern from the
**per-expert** dimension `n_e` and stacks blocks (`proof_utils.rs:278`:
`first_expert_col = n_e·expert_idx`). Our code validated `cols_pattern` against the
**total** `n` and computed `b_cols_global = inner_cols + expert_idx·n_e`, bounded
only by the downstream `validate_strip_indices` global check `< n`
(`canonical.rs:346`). Nothing enforced `inner_cols < n_e`, so a `cols_pattern`/
`t_cols` reaching `local ≥ n_e` made the opened columns **bleed into a neighbouring
expert's weight block** while still passing `< n` — a divergence from Pearl (fork)
and a column-grinding lever (open a favourable expert's columns under a different
`expert_idx`). The MoE verify path is fail-closed today, so this was a latent
boundary, but it is a real soundness gap in the MoE statement verifier.

**MITIGATED:** added `pearl_compat::moe_expert_b_cols_global(mining_config, e, n,
expert_idx, t_cols, max_pattern_len)` — it enforces `n % e == 0` and the missing
`local < n_e` clamp, then offsets by `expert_idx·n_e`. `verify_pearl_moe_recursive_
certificate` now derives `b_cols_global` through it. New errors
`MoeColumnOutsideExpert` / `MoeColumnDimIndivisible`. Tests:
- `moe_columns_stay_within_expert_block_accept` — expert 0/1 with `local < n_e`
  yield the correct offset global columns.
- `moe_column_bleed_via_t_cols_rejected` + `moe_column_bleed_via_wide_pattern_rejected`
  — `local ≥ n_e` (via offset or pattern) rejected with `MoeColumnOutsideExpert`.
- `moe_column_dim_indivisible_rejected`.
Honest recursive round-trip re-verified (`local < n_e`, unaffected).

**Related:** N12 (pattern is validated against total `n`, not `n_e` — the root
cause) and N8 (`n` vs `n_e` semantics) — the clamp closes the exploit regardless of
the pattern-validation dimension; N8/N12 remain worth reconciling for full
Pearl-parity of the *pattern validation* itself.

---

## N2 MAT_UNPACK range-check divergence from Pearl — **ISSUE FOUND → MITIGATED**

**Confirmed real.** Pearl range-checks the PLAIN matrix operand `MAT_UNPACK` to
int7 `[-64,64]` via IRANGE7P1 (`pearl_stark.rs:141`, verbatim `// Signal is in
[-64, 64]`, chained `MAT_UNPACK_RANGE.chain(NOISE_UNPACK_RANGE)`). Our composite AIR
routed `MAT_UNPACK` to IRANGE8 `[-128,127]` instead, admitting plain bytes in
`[65,127]∪[-128,-65]` that Pearl rejects — an **accept-set divergence** on the
**dense live path**: a proof we accept that Pearl's constraint set rejects, and a
weakened useful-work domain. (Not a direct forgery/grind — the jackpot is a hash,
so its success probability is byte-range-independent — but a genuine merge-mining
fork surface.)

**MITIGATED (3-site AIR change + freq, matching Pearl exactly):**
- `irange7p1()` now range-checks `MAT_UNPACK` alongside `NOISE_UNPACK` (Pearl's
  IRANGE7P1 = MAT ⧺ NOISE).
- `irange8()` no longer queries `MAT_UNPACK` (Pearl's IRANGE8 = A_NOISED ⧺
  B_NOISED, the genuinely-i8 noised operands).
- `populate_lookup_freq` moves the `MAT_UNPACK` histogram from IRANGE8 to
  IRANGE7P1.

**Validation (soundness-critical, staged per R1):**
- New `prop_mat_unpack_out_of_int7_rejects` (proptest) — a *consistent* staging
  row whose `MAT_UNPACK` i8 view ∈ [65,127]∪[-128,-65] is now REJECTED (it
  VERIFIED before the fix). `prop_urange8_valid_query_verifies` updated to the
  reachable UINT8 range `{0..64}∪{192..255}` (u8 view of int7).
- The composite unit tests used full-i8 synthetic *plain* data (13 generators in
  `composite_proof.rs`); masked to int7 `[-64,64]` (Pearl's domain — the real
  prove paths already use `synth_matrices` ∈ int7, so they were unaffected). No
  full-i8 plain generators exist in the integration tests or recursion.
- **Regression (12-core native):** ai-pow-zk lib **408/0** (composite prove/verify
  + range LogUp), recursion round-trips **2/0** (`noncontiguous` + `real_moe`
  proving real composite traces with the tightened range). The honest plain matrix
  is int7 in all real paths, so tightening only rejects out-of-domain values.

---

## N3 Noise-value seed-binding pin-dependency — **SAFE**

**Finding:** the binding is live. (1) The production prove path uses **only** the
pinned variant (`composite_prove_pinned_logup`); no unpinned `composite_prove`
exists in `zk_bridge`/`recursion`. (2) The noise pin: `NOISE_PACKED_PREP` is in
`PROGRAM_COLS` (`composite_full_air.rs`), pinned per row to the canonical program's
`e_value/f_value(s_A/s_B)`, and IRANGE7P1's `[-64,64]` width makes the base-129
packing bijective → the pinned noise is unique. The verifier independently rebuilds
the canonical program and requires equality (§F `l0_program_matches`). (3) The
matmul-side noised operands are bound by the `noised_packed` `(id,value)` RAM LogUp
(§A), not the (reverted, redundant) operand pin; the shipping sweep **does** place
the matmul rows — the `real_moe`/`noncontiguous` recursion round-trips prove real
sweeps end-to-end. **SAFE.**

## N4 Matmul chip under-constraint (delegated exclusivity + A/B binding) — **SAFE**

**Finding:** the delegations are enforced upstream. (1) **Selector exclusivity:**
`CONTROL_PREP` pins all 21 selectors + `MAT_ID` and is in `PROGRAM_COLS`
(`composite_full_air.rs:101-123`); the ControlChip enforces `CONTROL_PREP ==
pack(selectors, mat_id)` bijectively, so the program pin fixes the selectors to the
canonical one-hot assignment — both `IS_RESET_CUMSUM` and `IS_UPDATE_CUMSUM` = 1 is
impossible (a prover cannot forge `CONTROL_PREP`). (2) **A/B-input binding:** the
`noised_packed` `(id,value)` LogUp (§A, round-1) ties the dot's operands to the
committed store; the §6(b)/§4.D keystone chain (Agent-confirmed: committed A/B →
CUMSUM → SX → FOLD → JACKPOT) closes dense faithfulness, validated by the `sec_4c*`
suite. The last-row cumsum is pinned by the §4.D keystone (`when_last_row`
JACKPOT_MSG == FOLD_STATE). **SAFE.** (Residual noted in round 1: the "full
step-transcript binding" at `composite_full_air.rs:315-319` remains an explicitly
scoped open item, but the keystone chain already binds the fold to the accumulator.)

---

## N6 Unbound PIs (cumsum/jackpot/hash_jackpot) + strip_schedule — **SAFE**

**Finding:** the composite AIR binds all three "unbound" PIs to the trace
(`composite_full_air.rs:522-536` + selector-gated binding at 551+): `pi_cumsum ==
CUMSUM_TILE` (the matmul accumulator), `pi_jackpot == JACKPOT_MSG`, `pi_hash_jackpot
== CV_OUT`. So the 4-of-7 re-derivation in `verify_pearl_moe_recursive_certificate`
is correct by design: the 4 chain-derived PIs (COMMITMENT_HASH/HASH_A/HASH_B/
JOB_KEY) are re-checked in Rust because the AIR cannot rebuild them; cumsum/jackpot/
hash_jackpot are bound to the tile computation **by the proof** (the recursion
verifies the composite AIR), so a prover cannot forge them. `strip_schedule` is
bound via the recomputed `l0_program` (§F / N1). **SAFE.** Residual: the
jackpot-vs-target difficulty check for MoE is an external node concern (wiring, =
dense; tracked under §I/N14/N7).

---

## N9 Certificate-noun decode heap-amplification DoS — **ISSUE FOUND → MITIGATED**

**Confirmed real.** `DecodeState` capped node *count* (`max_total_nodes` = 1M) and
per-node atom size (`max_atom_bytes` = 1 MiB) but had **no cumulative-byte budget**.
Each `%bytes`/`u64s`/`i64s`/`ext2s` node allocates `vec![0u8; declared]` sized by the
noun's length field, so `1M × 1 MiB ≈ 1 TiB` of transient allocation was reachable
from a ≤4 MiB jam (a back-referenced `%bytes` tag costs a few jam bytes yet
allocates `declared`). Reachable unauthenticated via the public non-precheck
decoders (`decode_ai_pow_certificate_noun` etc.); the `verify_*_jam` consensus
entrypoints gate it behind the statement precheck.

**MITIGATED:** added `CertificateNounLimits::max_total_atom_bytes` (default 64 MiB
— ~16× the jam cap, generous for any real proof tree, bounds the DoS from ~1 TiB to
64 MiB) + `DecodeState::charge_atom_bytes`, charged at all four `expect_declared_
bytes` sites in `decode_proof_node`. Test `decode_rejects_cumulative_atom_bytes_
over_budget` (3 small atoms summing past a tight 5 KiB budget → rejected on the SUM;
the same tree decodes under the default 64 MiB). 51 certificate_noun decode tests
green (honest decode unaffected).

---

## N5 Multi-commitment coinbase / Pearl-PoW work-reuse — **CONFIRMED → consensus-policy residual**

**Confirmed:** `contains_subslice` (`pearl_compat.rs`) is an unrestricted
`windows().any()` substring match; nothing enforces a single Nockchain-aux tag per
coinbase (coinbase-only, `PEARL_AUX_INCLUSION_MAX_MERKLE_BRANCH = 0`). A miner who
knows two `nock_block_commitment`s before solving Pearl can embed both aux tags in
one coinbase, so one Pearl PoW satisfies `verify_pearl_aux_inclusion` for **both** —
binding one unit of work to two Nockchain candidate blocks (competing same-height
forks). The single-commitment binding itself is sound given a unique verifier-
supplied `candidate_nock_block_commitment`; the gap is non-uniqueness of what one
PoW may attest.

**Residual (not unilaterally mitigated — needs a consensus decision):** whether one
Pearl PoW may bind multiple Nockchain commitments is a **merge-mining policy**
question. The mitigation is ready: enforce **exactly one** occurrence of the
`"NOCKCHAIN-AI-POW-AUX"` tag prefix in the coinbase and require it to equal the
expected commitment (reject multi-tag coinbases). This is a consensus-soundness
change on a not-yet-wired path; flagged for decision rather than changed blind, per
R1 (don't rush an invasive consensus change without the policy intent). If work-
reuse across forks is unwanted (the default expectation), land the single-tag rule.

## N7 Compact L2 verify path unwired; digest doesn't stand alone — **SOUND but UNWIRED (residual)**

`verify_compact_batch_recursive_certificate_with_context` is self-consistent
(digest = cert vs context checksum; real pinning = verifier-owned
`context.circuit_prover_data`), but no consensus caller pins the digest bytes or
calls `l0_program_matches` — unlike the full L1 path (`zk_bridge.rs:1667`). MoE +
compact are fail-closed today. **Residual (= §I):** when wired, the compact verifier
must (a) build the context from verifier-pinned constants (never the prover's
returned `verifier_context()`, else the digest checks are vacuous), (b) call
`l0_program_matches`, (c) derive PI/profile from chain data. Not a live defect.

## N8 `params.n` vs `n_e` semantic mismatch — **SAFE (exploit closed by N1); doc reconciliation residual**

The code is internally consistent: `params.n` is the **total** (`n_e = params.n/e`,
`zk_bridge.rs:1616`; tests use `n = n_e·e`). The spec docs use the opposite
convention (`params.n == n_e`). N1's `local < n_e` clamp makes the column derivation
exploit-safe regardless of the naming. **Residual:** reconcile the spec docs to the
code's total-`n` convention and confirm the wire maps Pearl's per-expert `n` to our
total (Pearl `total_b_cols = n_e·e`, so Pearl's public `n` = `n_e`; the artifact
builder must set `params.n = n_e·e`). Verify at wiring time; no live exploit.

## N10 Grouped-GEMM dense/expert-agnostic — **SAFE (rests on N1 + §F)**

The MoE↔dense correspondence is `l0_program_matches(canonical_program_for_strip_
schedule(from_indices(outer_indices, b_cols_global), s_A, …))`. §F established
`l0_program_matches` is a complete `RowMajorMatrix` identity; N1 closed the column
clamp; the schedule encodes the exact opened rows/cols + noise, so distinct openings
yield distinct programs. **SAFE**, contingent on N1's clamp (landed).

## N11 Recursion transcript / profile binding — **SAFE**

Agent-confirmed: public values are observed before `alpha`/`zeta` (no challenge-
grinding gap); PoW bits threaded from `profile`. The full production path derives
`profile`/`zk_params` from chain data (`zk_bridge.rs:1644,1677`). **Residual (note):**
any *other* caller of `verify_recursive_certificate` must likewise derive them from
the statement (the compact path, N7, must too when wired).

## N12 Periodic-pattern reaches past n_e — **MITIGATED by N1**

Root cause: `cols_pattern` is validated against the total `n` (not `n_e`), so a
pattern/`t_cols` can index past `n_e`. N1's explicit `local < n_e` clamp in
`moe_expert_b_cols_global` closes the resulting bleed (tests
`moe_column_bleed_via_wide_pattern_rejected` / `..._via_t_cols_rejected`). Full
Pearl-parity of the *pattern validation dimension* (n_e vs n) is the N8 reconciliation.

## N13 Tile-selection grinding — **SAFE**

`attempt_tile_index = blake3_derive_key(CTX, params_tag ‖ s_A) % num_tiles`
(`fiat_shamir.rs:208`) is bound to `s_A` (routing/nonce/commitment-derived). A
different tile requires a different `s_A`, i.e. a fresh attempt (full tile
computation) — no independent tile grinding. Pinned by
`attempt_tile_index_is_deterministic_bounded_and_attempt_bound`. **SAFE.**

## N14 Difficulty-factor arithmetic — **SAFE (bound); MoE difficulty check is a wiring residual**

`difficulty_adjustment_factor = h·w·dot` (`checked_mul`) with `h/w/dot` the
schedule-bound tile dims (`|outer_indices|`, `|b_cols_global|`, `k`), so a prover
cannot inflate the factor without a proportionally larger real tile. The target
math (`pearl_nbits_to_target_le` + `pearl_adjust_target_for_config`) is the same for
dense and MoE. **Residual (= §I):** the jackpot-vs-target check for MoE is deferred
to the node (not wired); confirm it runs with the tile-scaled target when enabled.

## N15 Noise expansion byte-compat — **SAFE**

Agent-verified byte-identical to Pearl `pearl_noise.rs` (keyed BLAKE3 counter mode,
`&0x3F−32` uniform, `first^(1+mul_hi(r−1,rnd))` permutation, base-129/256 packing),
with a real-Pearl KAT + cross-crate equivalence tests. Note N2/N3 (the range +
pin around the noise) turned up the MAT_UNPACK divergence, now fixed.

---

## Round-2 summary

| angle | verdict | outcome |
|---|---|---|
| **N1** column-within-expert bleed | **ISSUE → MITIGATED** | `local < n_e` clamp + errors + 4 tests; recursion re-verified |
| **N2** MAT_UNPACK range divergence | **ISSUE → MITIGATED** | routed to int7 [-64,64] (Pearl parity); AIR+freq; 408/0 + recursion 2/0 |
| N3 noise pin dependency | SAFE | pinned-only prod; NOISE_PACKED_PREP pinned; LogUp binds operands |
| N4 matmul under-constraint | SAFE | CONTROL_PREP pins selectors; keystone chain closes fold |
| N5 multi-commitment work-reuse | **CONFIRMED — residual** | consensus policy; single-tag mitigation ready |
| N6 unbound cumsum/jackpot | SAFE | composite AIR binds all three PIs to the trace |
| N7 compact verify unwired | UNWIRED (residual) | fail-closed; wiring guard specified |
| N8 n vs n_e | SAFE (N1 closes it) | doc/wire reconciliation residual |
| **N9** decode heap-amplification DoS | **ISSUE → MITIGATED** | `max_total_atom_bytes` budget + charge + test |
| N10 dense/expert-agnostic | SAFE | rests on N1 + §F |
| N11 transcript/profile | SAFE | observe-order correct; full path derives profile |
| N12 pattern past n_e | MITIGATED by N1 | clamp closes the bleed |
| N13 tile grinding | SAFE | tile index bound to s_A |
| N14 difficulty arithmetic | SAFE (bound) | MoE difficulty-check wiring residual |
| N15 noise byte-compat | SAFE | byte-identical + KAT |

**3 issues found and fixed: N1 (column bleed, fork+grind), N2 (MAT_UNPACK range,
fork), N9 (decode DoS). 1 confirmed finding awaiting a consensus decision: N5
(work-reuse). Everything else safe (several contingent on N1/N2, now landed) or a
documented wiring residual (all gated by MoE/compact being fail-closed).**

