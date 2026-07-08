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

<!-- subsequent angles appended as evaluated -->
