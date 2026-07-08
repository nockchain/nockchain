# MoE adversarial audit — verdicts

**Date:** 2026-07-08
Companion to `2026-07-08_MOE_ADVERSARIAL_AUDIT_ANGLES.md`. One section per angle
with the **verdict** (SAFE / EXPLOITABLE / MITIGATED) + the evidence (test, proof,
or PoC) and any mitigation landed.

## Summary

| angle | verdict | outcome |
|---|---|---|
| §A noised_packed key collision | **SAFE** | collision real but benign — `(id,value)` fingerprint disambiguates; regression pins it |
| §B selective multi-proof | **SAFE** | equivalence + scattered auth + tamper rejection + in-circuit fold |
| §C k≠1024 keying | **SAFE** | keying is byte-position (k-agnostic); new k∈{2048,4096,14336} test; residual: full k>1024 matmul round-trip |
| §D routing grinding/difficulty | **ISSUE → MITIGATED** | added missing Pearl checks (`top_k<e`, expert span ≤ m); binding is jackpot-bound |
| §E Pearl byte-compat | **SAFE** | full splice chain re-derived vs Pearl formula + real KAT |
| §F opened-schedule binding | **SAFE** | complete identity (RowMajorMatrix); recomputed from public schedule |
| §G non-contiguous sweep | **SAFE** | position-keyed; permuted/subset/superset/aliased all rejected |
| §H degree-adaptive config | **SAFE** | bound trace_height, both 60-bit, consistent derivation |
| §I MoE verify wiring | **SOUND, TEST-ONLY** | fail-closed; residuals: bind params to proof + difficulty check |
| §J fail-closed guards | **SAFE** | 12 tests, all layers reject `e>0` |
| §K DoS | **SAFE** | caps + fail-closed; residual: tighten routing-size bound when live |
| §L trace-size accounting | **SAFE** | new test: placement == budget exactly |

**One real issue found and fixed: §D** (acceptance-set divergence from Pearl —
degenerate over-routings were accepted). Everything else is safe by construction or
already covered; new regression tests were added for §A, §C, §D, §H, §L and the §E
full chain. Residuals (all non-defects, gated by MoE being fail-closed): full
k>1024 recursive round-trip (§C), routing-size allocation bound (§K), live MoE
wiring + tile-scaled difficulty (§I), end-to-end l0_program mismatch test (§F).

---

## §A `noised_packed` LogUp key collision under scattered opening — **SAFE**

**Finding:** the collision is REAL. For a scattered opening the A-side
`noised_chunk_id` key uses `lane = a_index − ca0` (the covering-range position),
which for scattered rows exceeds `b_id_base = a_id_base + h_tile·k/8`, overlapping
the B-side key space. Concretely for `[0,1,8,9,64,65,72,73]` (h_tile=8, k=1024),
A-row-8 and B-col-0 both key to `a_id_base + 1024`.

**Why it is SAFE:** the `noised_packed` interaction fingerprint is the **3-tuple
`[chunk_id, NOISED_PACKED_lo, NOISED_PACKED_hi]`** (all four `BUS_NOISED_PACKED`
`push_interaction` sites in `composite_full_air_with_lookups.rs:398/425/437/458`).
The LogUp keys on `(id, value)`, so an id collision produces two *distinct*
fingerprints `(id, val_A)` and `(id, val_B)`. Each must net-balance independently;
a prover cannot make A's matmul consume B's producer because A's queried value is
its own noised tile data, and the producer values are bound to the committed
matrices (BLAKE3 co-location → `HASH_A/B`) with seed-derived noise (`e_value`/
`f_value` from `s_A`/`s_B`) that cannot be forced to coincide.

**The reliance is now load-bearing and documented:** if the fingerprint were ever
reduced to id-only, this collision would become exploitable. Pinned by:
- `noised_packed_ab_key_collision_documented` (unit) — asserts the collision and
  documents the `(id, value)` reliance.
- The non-contiguous recursive round-trip exercises the collision **honestly**
  (verifies).
- `sec_4c10_noncontiguous_sweep_on_row_permuted_matrix_rejects` exercises the
  **adversarial** wrong-value-at-position case (rejects), which is the exploit
  §A would need.

**No mitigation required** (safe by construction). Optional defense-in-depth
(not landed, would be soundness-critical): widen `b_id_base` to the covering-range
size so A/B key spaces never overlap — deferred because it changes the keying and
the `(id, value)` fingerprint already closes the attack.

---

## §E Pearl byte-compatibility of the MoE splice + grouped tile — **SAFE**

**Finding:** every link of the commitment/`s_A` chain matches Pearl's *exact
formula*, verified by reading Pearl's `zk-pow/src/ffi/mine.rs`
(`compute_commitment_hash_with_offsets`, `flatten_routing`, `routing_hash`) and
`api/proof_utils.rs` (`compute_hash_activations`):

| step | Pearl | ours | match |
|---|---|---|---|
| routing_data | `flatten_routing`: experts concat, tokens in token order | `build_routing_data` stable group-by-expert | ✓ (fuzz-tested vs `reference_routing`) |
| routing_root | `blake3(pad_1024(routing_le), key=job_key)` | `matrix_commitment(routing_data_le, kappa)` | ✓ |
| hash_offsets | `blake3(pad_1024(offsets_le), key=job_key)` | `matrix_commitment(routing_offsets_le, kappa)` | ✓ |
| hash_routing | `blake3(routing_root ‖ hash_offsets)` unkeyed | `moe_hash_routing` | ✓ |
| hash_activations | `blake3(hash_a ‖ hash_routing)` unkeyed | `moe_hash_activations` | ✓ **real-Pearl KAT** |
| s_b | `blake3(job_key ‖ hash_b)` unkeyed | `noise_seed_b` | ✓ |
| s_a | `blake3(s_b ‖ hash_activations)` unkeyed | `noise_seed_a` | ✓ |

The MoE change is exactly `hash_a → hash_activations` in `a_noise_seed`, identical
to Pearl. The `routing_data` "16-entry align then pad_1024" is **provably identical**
to our direct `pad_to_chunk_boundary`: the 16-align adds <64 zero bytes and can never
push into the next 1024-block (`ceil(L/16)·64 ≤ ceil(4L/1024)·1024`), and all padding
is zeros — same hash input.

**Evidence:**
- `full_moe_s_a_chain_matches_pearl_formula` (new) — independently re-derives the
  whole chain from Pearl's formula and asserts `canonical_noise_seeds_moe` matches
  (incl. empty expert + a token routed twice to one expert).
- `moe_hash_activations_matches_pearl_kat` — real Pearl output bytes.
- `pearl_moe_routing` structural + `reference_routing` fuzz tests — grouping order.

**Residual (not a fork risk, lower priority):** the *grouped tile fold* + jackpot
math (`compute_moe_tile`) is validated by dense-equivalence, not a full real-Pearl
MoE tile vector; a runtime diff against `pearl/zk-pow/fixures/v2_stark_proof_moe.bin`
would fully close it. Tracked under §I/residual.

---

## §B Selective-opening Merkle multi-proof soundness — **SAFE**

**Finding:** the disjoint-set multi-proof (`open_strip_set` /
`verify_strip_opening_set` / `collect_siblings_set` / `fold_opening_set`) is
sound and well-covered:
- `selective_matches_contiguous_range_opening` — the set opening is byte-identical
  to the range opening for every contiguous `[c0,c1)` of `nc ∈ {2,5,8,13,31,64}`.
- `selective_opening_multiproof_is_sublinear` — a scattered 64-of-4096 set
  authenticates to the committed root with `O(h·log n)` siblings.
- `selective_opening_rejects_tampering` — a tampered opened leaf **and** a forged
  sibling each make the recomputed root diverge.
- `selective_strip_opening_root_equals_full_matrix_hash` — the **in-circuit** fold
  (`place_matrix_strip_opening_set`) reproduces the committed root for arbitrary
  scattered chunk sets (pure, no prove).

The fold consumes every opened leaf + every sibling; a second-preimage would need
a BLAKE3 collision. No boundary mis-classification found. **SAFE.**

## §C `indexed_strips_chunk_set` / k≠1024 row-vs-chunk keying — **SAFE (mapping+opening); residual: full matmul round-trip**

**Finding (the concern is unfounded):** the sweep and the producer BOTH key the
`noised_packed` bus via the *same* `noised_chunk_id(id_base, k, src)` — a **byte-
position** key (`id_base + (lane·k + col)/8`), not a chunk index — so it is
k-agnostic by construction. The row→chunk expansion for k>1024 is now covered:
- `indexed_strips_chunk_set_authenticates_k_gt_1024_noncontiguous` (new) — for
  k ∈ {2048, 4096, 14336} a scattered set of rows expands to exactly its
  `k/1024`-chunk runs, the disjoint set authenticates to the root, and tampering a
  spanning chunk breaks it. This is the Llama-scale case (Pearl k=4096/14336/28672)
  the k=1024 fixtures never touched.
- Composed with `selective_strip_opening_root_equals_full_matrix_hash` (arbitrary
  chunk sets → in-circuit root), the full k>1024 **opening** path is validated.

**Residual (recommended, not a known defect):** a full k>1024 *recursive* round-trip
(matmul sweep + prove + verify) would definitively confirm the k-agnostic keying
end-to-end. k>1024 is a Llama-scale production requirement, not a current live path
(the dense/MoE fixtures are k=1024); the mapping + opening are validated, and the
matmul keying is k-agnostic by construction. Tracked as future coverage.

---

## §D MoE routing-consistency binding / grinding / difficulty — **ISSUE FOUND → MITIGATED**

**Binding is sound (no forgery):** `routing_data → routing_root ==
moe.hash_routing` binds the routing to `s_A` (via §E), and `outer_indices[u] ==
routing_data[expert_start + inner_u]` with `pos < expert_end` binds the opened rows
to the routing. A prover cannot change the opened rows without changing the
jackpot. 9 pre-existing adversarial tests reject every forgery path (forged/
cross-expert/tampered/out-of-range/inconsistent-offsets/etc.).

**ISSUE (acceptance-set divergence from Pearl):** our binding was **missing three
of Pearl's `sanity_checks.rs` constraints** — `top_k < e` (line 80), each expert
span `w[1]−w[0] ≤ m` (line 103), and `offsets[0] ≤ m` (line 107). Since a token
routes to a given expert at most once, an expert holds ≤ m of the m·top_k slots;
without these bounds we accepted **degenerate over-routings** (a token repeated
within an expert, or `top_k ≥ e`) that Pearl rejects — a merge-mining divergence
and a routing shape the difficulty model never priced. The routing is
prover-supplied and only structurally checked (matching Pearl's design — the
verifier can't run the model), so matching Pearl's *exact* structural bounds is
required.

**MITIGATED:** added `top_k < e` and per-expert `span ≤ m` (subsuming
`offsets[0] ≤ m`) to `verify_pearl_moe_routing_binding`, with new errors
`MoeTopKNotLessThanExperts` / `MoeExpertSpanExceedsTokens`. Tests
`top_k_not_less_than_experts_rejected` + `expert_span_exceeding_m_rejected`.
Honest round-robin routings (`build_routing_data`) have balanced spans `< m`, so
the recursive round-trips are unaffected (re-verified). **Grinding:** each
(routing, nonce) is a distinct jackpot attempt costing a full tile computation; the
routing freedom is Pearl's inherited PoUW model, priced into difficulty, not a new
lever.

---

## §H Degree-adaptive FRI config — profile-selection attacks — **SAFE**

**Finding:** all four sub-attacks fail:
1. **Weaker-profile grind:** both classes (`lb=4/nq=15`, `lb=2/nq=30`) are
   *exactly* 60-bit Johnson (`lb·nq`), so there is no weaker profile to reach.
2. **Lie about the degree:** `trace_height` is bound by the node precheck
   (`certificate_noun.rs:2077`, `metadata.trace_height != expected_layer0_rows →
   reject`, where `expected_layer0_rows` is a pure function of the chain-bound
   params + strip schedule). The verifier derives the profile from that bound
   value, and the proof was produced at the same profile, so a mismatched-profile
   proof fails verification.
3. **Boundary desync:** `for_layer0_trace` is called on the *same* bound
   `trace_height` by prover and verifier; non-power-of-2 rounds up identically
   (the real STARK trace is always a power of two).
4. **Soundness floor:** the 60-bit Johnson floor is unconditional/proven for both
   `lb` values; the known-insecure CYCLE-SUM floor (~22 bits) is 38 bits below both.

**Evidence:** `for_layer0_trace_boundary_floor_and_rounding` (new) pins the
crossover, the 60-bit floor at every degree 1–20, and the rounding; the earlier
degree-adaptive work validated the full recursive round-trip at both 2¹³ (lb=4)
and 2¹⁶ (lb=2). **SAFE.**

---

## §F Opened-schedule binding (`l0_program_matches`) completeness — **SAFE**

**Finding:** the three sub-attacks fail:
1. **`(width, values)` completeness:** `AiPowProgram` is a type alias for
   `p3_matrix::dense::RowMajorMatrix<Val>` (`lib.rs:155`) — a struct with exactly
   `{values, width}`. Height is `values.len()/width`, so `l0_program_matches`
   (width + values equality) is a **complete** identity: no field the recursion's
   `CompositeFullAirWithLookupsPinned::new_with(cert.l0_program, …)` consumes is
   left unbound. Two programs with equal `(width, values)` are byte-identical.
2. **`tile_i/tile_j`:** the expected program is recomputed from the *schedule*
   (`from_indices(outer_indices, b_cols_global)` + `s_A`/`s_B`/κ + the
   schedule-determined trace height), not from `tile_i/j`; those are vestigial for
   the scheduled prover (the schedule alone fixes the opened rows/cols), so `bp.
   tile_i = 0` cannot slip a different opening through.
3. **trace_height divergence:** the program's height is bound by the `values`
   match, and `trace_height` is separately bound by the precheck (§H). A padded/
   wrong height yields different `values` → mismatch → reject.

The binding recomputes from **public** inputs only (never the proof), so a prover
who proved over a favorable strip gets a program `≠` the schedule-derived one and
is rejected before `verify_recursive_certificate`.

**Evidence:** completeness is a type-level guarantee (RowMajorMatrix); the honest
`real_moe_recursive_certificate` round-trip exercises the equality on the true
program. **Residual (recommended):** an end-to-end "cert proved over rows X,
verified with outer_indices Y → reject" test (needs a full cert to prove, hence
deferred). **SAFE.**

---

## §G Non-contiguous sweep ↔ opened-rows binding — **SAFE**

**Finding:** the sweep must consume the *position-keyed* opened rows via the
`noised_packed` `(id, value)` bus (§A). Any deviation is caught:
- **wrong/permuted rows:** `sec_4c10_sweep_on_row_permuted_matrix_rejects` and
  `sec_4c10_noncontiguous_sweep_on_row_permuted_matrix_rejects` (zk_bridge) —
  sweeping a row-permuted matrix has no matching producer at the position key ⇒
  LogUp imbalance ⇒ reject.
- **subset/superset/aliased lanes:** a superset reads a position with no producer
  (imbalance); a subset leaves a producer multiplicity unmatched (imbalance); an
  aliased read violates the matmul dot constraint for that position.
- **covering-range remnants:** the selective switch makes non-selected chunks
  0-row (no producer), validated by §B/§L (the in-circuit fold + exact row budget).
**SAFE.**

## §J Fail-closed guard integrity — **SAFE**

**Finding:** 12 tests in `pearl_moe_fail_closed.rs` cover every layer:
`moe_config_parses_and_acceptance_fails_closed`, `nonzero_top_k_without_experts_is_
rejected`, `nonzero_reserved_padding_is_rejected`, `moe_public_data_is_rejected_
fail_closed`, `moe_statement_is_fail_closed_through_verify`, plus dense round-trip
parity. `e > 0` is rejected on the acceptance path; dense (`e=0`, all-zero trailer)
round-trips byte-identically. The selective/non-contiguous code on the live dense
path is covered by §A–§C. **SAFE.**

## §K DoS / resource exhaustion — **SAFE (residual: tighten routing-size bound when MoE is live)**

**Finding:** `MAX_NUM_EXPERTS = 1024` (Pearl parity) is enforced in
`build_routing_data`; `m·top_k` uses `checked_mul` + `u32::try_from`; the trace is
bounded by `PEARL_WORKER_INPUT_MAX = 2²²`, so a real routing is ≤ ~2²² entries
(~16 MB), not adversarially huge. MoE decode/verify is **fail-closed**, so the
routing path is unreachable in production today. **Residual:** when MoE is enabled,
add an explicit `m·top_k ≤ trace-envelope` cap *before* the routing buffers are
materialized (the abstract `u32` bound alone permits a ~16 GB allocation).

## §L Selective-opening trace-size accounting — **SAFE (validated)**

**Finding:** `strip_opening_rows_set(sel, nc) = 8·strip_blocks_set(sel, nc).len()`
is the exact budget the trace is sized to; a mismatch would either overflow
(CapMismatch) or leave unconstrained rows. New
`selective_strip_opening_row_budget_equals_placement` asserts
`place_matrix_strip_opening_set`'s *placed* row count equals the budget exactly
over scattered/boundary/full sets — closing the covering-range→set-based accounting
change. **SAFE.**

## §I `verify_pearl_moe_recursive_certificate` wiring — **SOUND but TEST-ONLY (residual)**

**Finding:** the function is soundness-complete (§D routing binding + §E `s_A` +
§F opened-schedule binding + `verify_recursive_certificate`), but is **only called
from tests** (zk_bridge:3949) — MoE is fail-closed at block acceptance (§J), so it
is not on a live node path. **Residuals for when MoE is enabled:** (1) carry
`PearlMoeParams`/`routing_data` in the artifact **bound to the proof** (not trusted
function args), and (2) run the jackpot-vs-target difficulty check with the
tile-scaled target (deferred today, "a separate node concern, identical to dense").
Not a defect — a documented gate. Track under the production residual doc.

