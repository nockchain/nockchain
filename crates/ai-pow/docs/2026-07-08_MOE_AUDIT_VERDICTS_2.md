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

<!-- subsequent angles appended as evaluated -->
