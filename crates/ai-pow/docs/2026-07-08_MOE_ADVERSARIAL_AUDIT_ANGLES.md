# MoE upgrade + selective-opening + degree-adaptive — adversarial audit angles

**Date:** 2026-07-08
**Scope:** the entire MoE upgrade and every change since
`d5fc82f430852a26f6472103e6dc1102878ce33b` (45 commits). Goal: **break it.**
Prove-soundness (forgeability), byte-compatibility with Pearl (fork risk),
consensus difficulty, DoS, and correctness are all in scope.

This is a *map of attack angles*, ordered by estimated severity. Each angle names
the surface, the concrete attack hypothesis, why it might work, and how to try to
break it. A follow-on goal will drive each angle to a verdict (exploit PoC, or a
proof/test that it's safe).

## Threat model

- A malicious **prover/miner** who wants to (a) forge a valid recursive
  certificate for work they didn't do, (b) do *less* work than the difficulty
  implies (grinding), or (c) produce a proof our verifier accepts but that is
  **not** Pearl-equivalent (a merge-mining fork).
- A malicious **submitter** feeding crafted artifacts to a verifying node (DoS,
  panic, resource exhaustion) — mostly gated today because MoE is fail-closed at
  block acceptance, but selective opening + degree-adaptive config are on the
  **live dense path**.
- Assume the attacker can choose all prover-side inputs (routing, matrices,
  patterns, nonce, trace contents) subject only to what the AIR + the Rust
  precheck enforce.

---

## A. `noised_packed` LogUp key-space collision under scattered opening  ⚠ HIGH

**Surface:** `composite_trace.rs::noised_chunk_id` (key = `id_base + (lane·k +
col)/8`), `b_id_base = a_id_base + (h_tile·k)/8` (`composite_trace.rs:1835`,
`canonical.rs:723`, `zk_bridge.rs:2344`), and the B5b sweep lane
`lane = a_indices[i] − ca0` (the **covering-range position**).

**Hypothesis:** For a *contiguous* tile the A-side lane maxes at `h_tile−1`, so
A-side keys fill exactly `[a_id_base, b_id_base)` and never touch the B-side. For
a **scattered** opening (MoE `outer_indices`, non-contiguous dense), `lane =
a_indices[i] − ca0` can be ≫ `h_tile` (up to the covering range ≈ m), so A-side
keys **exceed `b_id_base` and overlap the B-side key space** — e.g. for
`[0,1,8,9,64,65,72,73]`, A-lane 8 and B-lane 0 both map to `a_id_base + h_tile·128`.
There is **no bound check** (`lane < h_tile`, `key < b_id_base`).

**Why it might break:** if the `noised_packed` LogUp fingerprint keys on the *id
alone* (not `(id, value)`), a prover can substitute a B-side noise term for an
A-side one (or vice versa), or make two distinct opened positions share a key and
cancel — forging the noised matmul inputs while the bus still balances.

**How to break it:** (1) confirm whether the fingerprint includes the noise value
or only the id (read the `noised_packed` interaction in `composite_lookups` /
`composite_full_air_with_lookups`). (2) If id-only: craft a scattered
`(a_indices, b_indices)` that forces an A/B key collision, then a prover that
swaps the colliding noise terms and check the AIR still verifies. (3) Even if
`(id,value)`: look for a *collision that cancels* (two producers, same id, values
that sum/fold to zero in the bus algebra). The honest round-trips pass because
honest values differ; the exploit needs crafted values.

---

## B. Selective-opening Merkle multi-proof soundness  ⚠ HIGH

**Surface:** `blake3_tree.rs` `open_strip_set` / `verify_strip_opening_set` /
`collect_siblings_set` / `fold_opening_set`; `canonical.rs::strip_blocks_set`;
`composite_trace.rs::place_matrix_strip_opening_set` / `fold_strip_set` /
`subtree_inside_set`; `indexed_strips_chunk_set`.

**Hypothesis:** the multi-proof authenticates a *disjoint* chunk set to `HASH_A/B`
via `sel_count` tree predicates + rank-indexed leaf bytes. Attacks: (1) a prover
supplies auth siblings for a *different* set that still folds to the committed
root (second-preimage — should be BLAKE3-hard, but verify the fold consumes
**every** node so a prover can't omit/duplicate a sibling); (2) the `sel_count`
predicate mis-classifies a boundary node (off-by-one in `partition_point`) so a
selected chunk is treated as a sibling (opened-but-unauthenticated) or vice
versa; (3) rank indexing (`sel.partition_point(<lo)`) desynchronises the in-circuit
leaf bytes from the off-circuit `open_strip_set` order for a non-monotone or
duplicate `sel`.

**How to break it:** fuzz `sel` (scattered, adjacent, endpoints, single, full,
and — crucially — **k≠1024** where a row spans multiple chunks) against a
reference full-tree root; assert `verify_strip_opening_set` ≠ root for any
tampered opened byte or omitted/dup sibling. Cross-check the in-circuit
`place_matrix_strip_opening_set` root vs `open_strip_set` for the *same* pathological
`sel`. Try a `sel` whose covering range triggers the §A collision simultaneously.

---

## C. `indexed_strips_chunk_set` / k≠1024 row-vs-chunk keying  ⚠ HIGH

**Surface:** `blake3_tree.rs::indexed_strips_chunk_set` (row → chunk expansion),
the B5b sweep lane (`a_indices[i] − ca0`, a **row** offset) vs the producer key
(`chunk_index − strip_c0`, a **chunk** offset), and `ca0 = chunks[0]`.

**Hypothesis:** everything is validated at **k=1024** (1 row = 1 chunk, row index
== chunk index). For **k>1024** a row spans `k/1024` chunks and `chunk_index ≠
a_indices[i]`; the sweep still uses the *row* offset `a_indices[i] − ca0` as the
`noised_chunk_id` lane, while the producer keys by *chunk* offset. The doc already
flags this as un-reverified. A mismatch means the sweep reads noise keyed to the
wrong position → either a spurious LogUp balance a prover can exploit, or dense
k=4096 (Llama) is silently mis-keyed.

**How to break it:** build a **scattered** opening at k=4096 (r=256, tile≥8) and
run the recursive prove/verify; if it fails, that's a correctness wall; if it
*passes*, hand-derive the producer/consumer keys and check they actually coincide
for the selected rows (not by luck). Then attempt a cross-row noise swap that the
k=1024 keying would catch but the k>1024 keying doesn't.

---

## D. MoE routing-consistency binding — grinding / difficulty  ⚠ HIGH

**Surface:** `pearl_compat.rs::verify_pearl_moe_routing_binding`;
`compute_pearl_moe_ticket`; the `s_A` splice (`fiat_shamir.rs::canonical_noise_seeds_moe`).

**Hypothesis:** `routing_data` and `routing_offsets` are **prover-supplied** and
only checked for structural validity (`routing_root == commit(routing_data)`,
offsets a non-decreasing partition, tokens `< m`, gather matches). Nothing forces
the routing to be a *real* model routing. So a prover freely chooses which tokens
each expert "routed," i.e. freely chooses the opened rows subject to a committed
routing. Questions: (1) does this widen the search space beyond what Pearl's
difficulty prices (grinding over routings/offsets/expert per nonce)? (2) can a
degenerate routing (all tokens to one expert, or duplicate tokens as in the KAT
`[0,1,3,3]`) open the *same* favorable row multiple times to cheapen a jackpot?
(3) is `routing_offsets`'s partition check enough — can a prover pick offsets that
make an expert's span cover cross-expert tokens the pattern then selects?

**How to break it:** enumerate the prover's free parameters (routing_data,
offsets, expert_idx, rows_pattern offset, nonce) and compare the effective
search space to the dense per-tile difficulty (`difficulty_target`). Show
(or refute) a grinding advantage. Check Pearl's own routing constraints (does
Pearl bind routing to a committed model inference we don't?) — if Pearl constrains
routing and we don't, that's both a fork **and** a difficulty gap.

---

## E. Pearl byte-compatibility of the MoE splice + grouped tile  ⚠ HIGH (fork)

**Surface:** `fiat_shamir.rs` (`moe_hash_routing`, `moe_hash_activations`,
`canonical_noise_seeds_moe`), `pearl_compat.rs::compute_moe_tile` /
`compute_pearl_moe_ticket`, `moe_ref.rs`.

**Hypothesis:** our jackpot must equal Pearl's for the *same* MoE job or a valid
Pearl block is rejected by us (or vice versa) → a merge-mining fork. Only
`moe_hash_activations` is KAT'd against a real Pearl vector; the full chain
(routing canonicalization → `routing_root`/`hash_offsets` byte layout →
`hash_routing` → `hash_activations` → `s_A` → grouped tile fold → jackpot) is
validated mostly by *self*-consistency (dense-equivalence), not against real Pearl
MoE vectors.

**How to break it:** pull real Pearl MoE test vectors (the `pearl/zk-pow` fixtures
/ `moe_test`) for a full job and diff **every** intermediate (`slot_indices`,
`routing_data` LE bytes, `routing_offsets` LE bytes, `routing_root`, `hash_offsets`,
`hash_routing`, `hash_activations`, `s_A`, the grouped tile state, the jackpot).
Any divergence is a fork. Especially check the counting-sort tie-breaking, the LE
padding of `routing_data`/`routing_offsets`, and the `outer_indices` gather order
(`routing_data[expert_start + inner]` vs Pearl's exact index math).

---

## F. Opened-schedule binding (`l0_program_matches`) completeness  ⚠ MED-HIGH

**Surface:** `recursion.rs::l0_program_matches` (compares `width` + `values` of
the preprocessed program), `zk_bridge.rs::verify_pearl_moe_recursive_certificate`
(recomputes the canonical program with `bp.tile_i = 0, tile_j = 0`, `s_A`, and
`trace_height` from `expected_layer0_rows`).

**Hypothesis:** the node binds the cert's opened rows by recomputing the canonical
program and requiring equality. Attacks: (1) is `bp.tile_i/tile_j = 0` actually
forced, or could a prover's program encode different `tile_i/j` yet still match
(is tile_i/j even in the preprocessed columns)? (2) is comparing `(width, values)`
of the preprocessed matrix a *complete* program identity — any selector/constant
the recursion trusts from the proof but not in the preprocessed matrix? (3) the
recomputed `trace_height` from `expected_layer0_rows_for_strip_schedule` must
equal the prover's; if a prover pads to a larger power-of-two, does the recompute
diverge (reject) or silently match a wrong config (§H)?

**How to break it:** construct two distinct valid Layer-0 programs with equal
`(width, values)` but different opened rows (if the encoding allows it). Try a cert
proven over rows X while `verify_pearl_moe_recursive_certificate` is called with
`outer_indices = Y` — confirm rejection. Verify `tile_i/j` non-zero cannot slip
through.

---

## G. Non-contiguous sweep ↔ opened-rows binding (beyond §4.C.10)  ⚠ MED

**Surface:** `canonical.rs` `RowClass::Sweep` (`a_indices[…] − ca0` lanes),
`composite_trace.rs::place_useful_work_chain_hw_indexed`, the `noised_packed`
producer↔consumer accounting for non-contiguous opening.

**Hypothesis:** `sec_4c10_noncontiguous...` proves a row-permuted matrix is
rejected. But that's one attack. Others: (1) sweep a tile over a *subset* or
*superset* of the opened rows (lane indices that don't cover `outer_indices`
exactly); (2) reuse one opened row's noise for two sweep rows (aliased lanes);
(3) a `sel` where the covering range opens extra chunks whose noise the sweep
*could* consume off-pattern. The B5b design says non-selected opened chunks are
0-row now (selective), so re-verify no covering-range remnants exist after the
selective-opening switch.

**How to break it:** adversarial sweep_override variants for non-contiguous
schedules (aliased lanes, subset/superset row sets); assert every variant rejects.

---

## H. Degree-adaptive FRI config — profile-selection attacks  ⚠ MED

**Surface:** `circuit.rs::for_layer0_trace` / `prod_adaptive` (crossover at degree
15), threaded through prove + verify keyed by `trace_height`.

**Hypothesis:** prover and verifier both derive the profile from `trace_height`.
Attacks: (1) `trace_height` not a power of two → `trailing_zeros`/
`next_power_of_two` mismatch between prover and verifier at the boundary; (2) a
prover proves a large trace at `lb=4` (fewer queries, smaller/faster) and claims a
degree-14 label — is `trace_height` bound tightly enough that the verifier's
`for_layer0_trace` must pick `lb=2`? (3) both profiles are 60-bit *Johnson*; is 60
the actual floor for a large trace, or does the CYCLE-SUM/LDR floor (referenced at
~22 bits, `circuit.rs`) interact with `lb=2/nq=30` differently than `lb=4/nq=15`?
(4) confirm the precheck binds `trace_height` (== `expected_layer0_rows`) before
`for_layer0_trace` consumes it — else a forged `trace_height` picks a weaker
config.

**How to break it:** verify `trace_height` binding order in the node precheck;
try a cert whose metadata `trace_height` disagrees with the recomputed one across
the 14/15 boundary; re-derive the true soundness bits of `lb=2/nq=30` at
degree 18 (not just the Johnson `2·30`).

---

## I. `verify_pearl_moe_recursive_certificate` component gaps  ⚠ MED

**Surface:** the node-facing MoE verify (routing binding + `s_A` recompute + PI
binding + opened-schedule binding + cert verify) — but it is **not** wired into
the production node path yet (MoE fail-closed).

**Hypothesis:** even assuming it's wired: (1) the expert-column derivation
(`expert_idx·n_e + cols_pattern`) — is `n_e = params.n / e` integer-exact and is
`e` bound? (2) `PearlMoeParams` (expert_idx, routing_offsets, hash_routing,
outer_indices) are function arguments — when carried in a real artifact they must
be bound to the proof; is there a path where they're trusted un-bound? (3) the
target/difficulty check is deferred to "a separate node concern" — confirm it
actually happens for MoE and uses the tile-scaled target.

**How to break it:** trace how `PearlMoeParams`/`routing_data` would be carried +
bound in the artifact; find any field the verify trusts without binding to `s_A`/
the proof.

---

## J. Fail-closed guard integrity  ⚠ MED

**Surface:** `pearl_compat.rs::validate_pearl_merge_config_for_recursive_prover`
(`e>0` reject), `parse_mining_config_trailer`, the four guard layers.

**Hypothesis:** MoE must stay fully rejected on the live path. Attacks: (1) an
`e>0` config that parses as `e==0` (trailer aliasing / reserved-byte confusion);
(2) a non-MoE config that nevertheless reaches the MoE tile/selective path; (3)
the selective-opening + non-contiguous code is now on the **dense** live path —
can a dense config trigger MoE-only code (e.g. a non-contiguous dense pattern that
exercises §A/§B)?

**How to break it:** fuzz `PearlMiningConfig` trailers around the MoE boundary;
confirm every guard rejects; confirm dense non-contiguous patterns are actually
reachable in production and are covered by §A–§C.

---

## K. DoS / resource exhaustion  ⚠ MED

**Surface:** `pearl_moe_routing.rs::build_routing_data` (m·top_k), the wire
formats (`PearlMoeParams` `MAX_OUTER_INDICES=128`, `PEARL_MOE_MAX_NUM_EXPERTS=1024`),
`indexed_strips_chunk_set` (allocates per row), `verify_pearl_moe_routing_binding`
(iterates routing_data).

**Hypothesis:** crafted sizes cause OOM/quadratic blowup before validation. e.g.
huge `m`·`top_k` routing_data; a `sel` whose covering range forces a giant
`strip_blocks` allocation; `routing_offsets` with `e` at the max.

**How to break it:** feed max/oversized wire + routing to the decode/verify path
(behind the DoS caps) and measure allocation/time; confirm the caps bound it
*before* the expensive work.

---

## L. Correctness of the selective-opening trace-size accounting  ⚠ LOW-MED

**Surface:** `strip_opening_rows_set`, `expected_layer0_rows_for_strip_schedule`
(now set-based), `schedule_layout` (na/nb from `strip_opening_rows_set`).

**Hypothesis:** the row budget must exactly equal `place_matrix_strip_opening_set`'s
placement or the trace/program disagree (CapMismatch) — or worse, a mismatch that
*doesn't* CapMismatch leaves unconstrained rows. Verify `8·strip_blocks_set(sel).len()
== placed rows` for pathological `sel` (already unit-tested for a few; widen).

**How to break it:** property-test `strip_opening_rows_set == place_matrix_strip_
opening_set` placed-row count over fuzzed `sel` and `num_chunks`, incl. k≠1024.

---

## Prioritization for the driving goal

1. **§A** (noised_packed key collision) — most concrete potential forgery; start here.
2. **§E** (Pearl byte-compat) — fork risk; needs real Pearl MoE vectors.
3. **§B, §C** (selective/multi-proof + k≠1024) — commitment-authentication linchpin.
4. **§D** (routing grinding / difficulty) — consensus soundness.
5. **§F, §G** (opened-rows binding completeness) — forgery.
6. **§H, §I, §J, §K, §L** — config, node-wiring, guards, DoS, accounting.

Each angle's verdict must be either a **working PoC** (prove a forged/cheaper cert
the verifier accepts, or a Pearl-divergence) or a **committed adversarial test +
argument** showing it's closed.
