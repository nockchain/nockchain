# MoE adversarial audit — verdicts

**Date:** 2026-07-08
Companion to `2026-07-08_MOE_ADVERSARIAL_AUDIT_ANGLES.md`. One section per angle
with the **verdict** (SAFE / EXPLOITABLE / MITIGATED) + the evidence (test, proof,
or PoC) and any mitigation landed.

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

<!-- subsequent angles appended as they are evaluated -->
