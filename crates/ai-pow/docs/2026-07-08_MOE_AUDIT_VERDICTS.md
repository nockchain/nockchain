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

<!-- subsequent angles appended as they are evaluated -->
