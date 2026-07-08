# MoE upgrade — adversarial audit angles, ROUND 2 (fresh surfaces)

**Date:** 2026-07-08
**Scope:** the entire MoE upgrade + all changes since
`d5fc82f430852a26f6472103e6dc1102878ce33b`. Goal: **break it.**
**Explicitly disjoint from round 1** (`2026-07-08_MOE_ADVERSARIAL_AUDIT_ANGLES.md`
/ `..._VERDICTS.md`, angles §A–§L), which covered selective-opening mechanics, the
commitment splice, routing *structure*, the degree-adaptive config, and
guards/DoS/accounting. This round targets the surfaces round 1 never examined: the
**tile arithmetic, the in-circuit constraint completeness, the noise expansion, the
recursion/transcript internals, and the decode / merge-mining binding paths.**

This map was produced by four parallel deep-reads of the code (tile arithmetic,
noise+MMA, recursion+digest, decode+plaintext+aux) plus direct grounding of the
pattern/tile-selection/difficulty paths. **Several angles already have a concrete
candidate break identified** — those are the ones to drive first.

## Threat model (unchanged from round 1)

Malicious prover/miner who wants to forge a valid certificate, do less work than
difficulty implies, produce a proof we accept but Pearl rejects (merge fork), reuse
one PoW across blocks, or DoS a verifying node. Attacker controls all prover-side
inputs subject only to the AIR + Rust checks.

**Structural fact that frames everything below (Agent 1):** the STARK proves a
**dense, expert-agnostic tile** — `compute_moe_tile` gathers rows/cols and calls the
*same dense kernel* as the non-MoE path (`matmul.rs:344-396`); there is **no MoE
sub-AIR**. *All* expert structure (which rows/cols belong to expert `x`) is enforced
**off-circuit** in `verify_pearl_moe_recursive_certificate` + `verify_pearl_moe_
routing_binding` + the `l0_program_matches` schedule rebuild. So every MoE-specific
soundness property rests on Rust checks outside the circuit — that is the attack
surface.

---

## N1. Column-within-expert bleed (row/col clamp asymmetry)  ⚠ HIGH — candidate break

**Surface:** `zk_bridge.rs:1606-1624` (expert-column derivation), `canonical.rs:346`
(`validate_strip_indices`: `if last >= dimension` where `dimension = params.n`
**total**), vs the row side `pearl_compat.rs:1547-1556` (`if pos >= expert_end →
MoeOuterIndexOutsideExpert`).

**The asymmetry:** opened **rows** are clamped to the expert's routed span
(`pos < expert_end`), but opened **columns** are only clamped to the *total* `n`,
**never** to `n_e = n/e` or to `[expert_idx·n_e, (expert_idx+1)·n_e)`. The global
column is `expert_idx·n_e + local` (`zk_bridge.rs:1616-1620`).

**Hypothesis:** if `cols_pattern` / `t_cols` can yield a local column `≥ n_e`, the
global column bleeds into a **neighbouring expert's weight block** while still
passing `< n`. A prover could open columns from a *different* expert than
`expert_idx` claims — proving work over the wrong weights, or grinding which
columns are favorable. This is the strongest forgery candidate in this round.

**How to break it:** trace `t_cols` / `cols_pattern.period` provenance (are they
independently clamped to `n_e`?). Construct a MoE statement where `cols_pattern`
selects a `local ≥ n_e`, run it through `verify_pearl_moe_recursive_certificate`,
and check whether the bled columns are rejected. If not, build a tile that opens
expert B's columns under expert A's `expert_idx`. (Connects to N12.)

---

## N2. `MAT_UNPACK` range-check divergence from Pearl  ⚠ HIGH — candidate break (fork/soundness)

**Surface:** `composite_full_air_with_lookups.rs:318-325` (port sends `MAT_UNPACK`
to **IRANGE8 `[-128,127]`**) vs Pearl `pearl/zk-pow/src/v1/circuit/pearl_stark.rs:
141-146` (`MAT_UNPACK` → **IRANGE7P1 `[-64,64]`**, `// Signal is in [-64, 64]`).
Bus defs: `composite_lookups.rs:93-99`.

**Hypothesis:** the plain useful-work operand of the int7 MMA is range-checked to
i8 `[-128,127]` in our circuit but `[-64,64]` in Pearl. Our circuit **admits plain
bytes in `[65,127]∪[-128,-65]` that Pearl's circuit rejects**. In the honest path
this is masked by the off-circuit `[-64,64]` validation (`pearl_compat.rs:170-175`),
but *the ZK proof alone does not enforce Pearl's input domain*. Because the jackpot
is hashed over exactly these bytes (i8u8 → UINT8_DATA → BLAKE3 → HASH_A), this is an
in-circuit-constraint divergence that **desynchronizes accept-sets between us and
Pearl** — a proof we accept that Pearl would reject (or a differently-valued jackpot
if Pearl's out-of-range handling differs).

**How to break it:** build a proof with a plain matrix byte in `[65,127]`, prove it
through the pinned composite (bypassing the off-circuit check), and confirm our
verifier accepts while Pearl's constraint set rejects. Also check whether the
wider range lets a prover reach a jackpot Pearl can't. Decide the fix direction
(tighten to `[-64,64]` to match Pearl) — this is soundness-critical (R1).

---

## N3. Noise-value seed-binding is program-pin-dependent  ⚠ HIGH — soundness linchpin

**Surface:** `chips/input.rs:113-132` (only `NOISED_PACKED = MAT + NOISE`, no
seed constraint), `composite_full_air.rs:281-295` (the *program pin*
`main[PROGRAM_COLS[k]] == preprocessed[k]`), `canonical.rs:836-874`
(`coloc_leaf_noise_pins → e_value/f_value`), and the reverted-pin note
`composite_full_air.rs:105-114`.

**Hypothesis:** there is **no per-row AIR constraint** forcing the noise to equal
the seed-derived `e_value/f_value`. It rests **entirely** on (i) always using the
*pinned* AIR variant and (ii) the verifier independently rebuilding the canonical
program from `ZkParams`+`BlockPublic`. The **unpinned** `CompositeFullAir` leaves
`NOISE_UNPACK` a free witness (only range-capped). Worse, pinning of the
matmul-side `A_NOISED_UNPACK`/`B_NOISED_UNPACK` was **deliberately reverted** as
"vacuous in the shipping path ... places no matmul rows until §4.A"
(`composite_full_air.rs:105-114`).

**Hypotheses to break:** (1) is there any production path (mining, recursive-inner,
compact) that proves with the **unpinned** variant, or with a program not rebuilt
by the verifier? (2) does the shipping `zk_bridge` sweep actually place the
leaf/matmul rows the noised-operand RAM-lookup binding depends on, or is the
matmul-input binding vacuous (so a prover feeds arbitrary noised operands into the
dot)? (3) the `[-64,64]` width of IRANGE7P1 is *load-bearing* for noise uniqueness
(base-129 bijection) — N2's IRANGE8 slip does not touch NOISE_UNPACK, but confirm.

**How to break it:** find a prove/verify entrypoint that skips the pin or the
program rebuild; feed a noise value ≠ seed-derived and check acceptance. Confirm
whether the "vacuous in shipping" matmul rows are actually placed in the live
`prove_and_verify` / MoE path.

---

## N4. Matmul chip under-constraint (delegated exclusivity + A/B-input binding)  ⚠ HIGH — forgery

**Surface:** `chips/matmul/chip.rs:26-29,146-176` (dot + cumsum), `chips/matmul/
compute.rs:80-85` ("doesn't enforce exclusivity ... at the cost of accepting
arbitrary `cumsum_new`"), `composite_full_air.rs:392` (ControlChip/CONTROL_PREP),
`composite_full_air.rs:459-469` (M-S1 pack-link + `BUS_MATMUL_INPUT` LogUp).

**Hypothesis:** the matmul chip does **not** self-enforce (a) selector exclusivity
(both `IS_RESET_CUMSUM` and `IS_UPDATE_CUMSUM` = 1 degenerates the row formula to
`dot + cumsum_old`), nor (b) that its `A_UNPACK`/`B_UNPACK` cells equal the
committed matrices — both are delegated to the ControlChip and the M-S1 LogUp. Also
the cross-row cumsum is `when_transition()`-gated, so the **last row's CUMSUM is not
forward-constrained** (chip.rs:161-176).

**How to break it:** verify the ControlChip actually forces selector exclusivity in
the composite (not just the matmul chip in isolation); construct a trace with both
selectors set and see if the AIR accepts. Verify the `BUS_MATMUL_INPUT` link is live
(the dot's operands are provably the committed-store bytes) — if a gap, feed unbound
A/B into the dot. Check the last-row cumsum is pinned elsewhere (the §4.D keystone).

---

## N5. Multi-commitment coinbase / Pearl-PoW work-reuse  ⚠ HIGH — merge-mining

**Surface:** `pearl_compat.rs:2573-2578` (`contains_subslice` = unrestricted
`windows().any()`), `pearl_compat.rs:1710-1750` (`verify_pearl_aux_inclusion`),
`PEARL_AUX_INCLUSION_MAX_MERKLE_BRANCH = 0` (coinbase-only).

**Hypothesis:** the aux commitment is found by an **unrestricted substring search**
in the coinbase, with **no single-commitment-per-chain rule**. A miner who knows two
target `nock_block_commitment`s *before* solving Pearl can embed both
`"NOCKCHAIN-AI-POW-AUX"‖commitment_A` and `‖commitment_B` in one coinbase. A single
Pearl PoW then satisfies `verify_pearl_aux_inclusion` for **both** — the same work
binds two different Nockchain candidate blocks (e.g. competing same-height forks).

**How to break it:** construct a coinbase with two aux tags; confirm both aux
statements verify against the one Pearl solution. Then assess consensus impact: does
Nockchain treat the Pearl PoW as per-block non-reusable? If so, this is a real
work-reuse / nothing-at-stake amplifier. (The direct commitment→block binding is
sound given a *unique* verifier-supplied `candidate_nock_block_commitment`; the gap
is non-uniqueness of what one PoW can attest.)

---

## N6. Unbound public inputs at the node boundary (cumsum / jackpot / hash_jackpot) + unchecked strip_schedule  ⚠ HIGH — forgery/binding

**Surface:** `zk_bridge.rs:1637-1640` (`expect_pi_eq` binds only 4 of 7 PI groups:
COMMITMENT_HASH, HASH_A, HASH_B, JOB_KEY), `certificate_noun.rs:2099-2134`
(`precheck_pearl_merge_bound_public_inputs` pins hash_a/b, job_key, commitment_hash,
jackpot, hash_jackpot — but **not** `cumsum`, decoded `:2470-2480`), and
`certificate_noun.rs` (the embedded `strip_schedule` is never equality-checked
against the ticket-derived `StripIndexSchedule` at verify time — bound only
indirectly via `trace_height`).

**Hypothesis:** `verify_pearl_moe_recursive_certificate` leaves CUMSUM, JACKPOT, and
HASH_JACKPOT to be "constrained only by the ZK proof / an upstream difficulty
check". If (a) the recursion circuit does **not** fully bind `cumsum` to the folded
tile, or (b) the upstream jackpot-vs-target check is missing/deferred for the MoE
path (round-1 §I flagged the difficulty check as "a separate node concern"), a
prover could present a cert with a prover-chosen jackpot. Separately, if the
embedded `strip_schedule` isn't equality-checked, a mismatch between the proven
schedule and the ticket schedule might not be caught except through `trace_height`
(a coarse binding).

**How to break it:** confirm the recursion circuit binds `cumsum`/`jackpot` to
the tile fold (trace the §4.D keystone → JACKPOT_MSG). Confirm the jackpot-vs-target
difficulty check runs for MoE with the tile-scaled target (see N14). Construct a
cert with a tampered `cumsum`/embedded schedule and check rejection.

---

## N7. Compact L2 verify path unwired; digest doesn't stand alone  ⚠ MED-HIGH

**Surface:** `recursion.rs:1910-1971` (`verify_compact_batch_recursive_certificate_
with_context`), `recursion.rs:357-388` (digest construction), `recursion.rs:182-201`
(context is "verifier-owned by contract"), vs the full path's binding at
`zk_bridge.rs:1667` (`l0_program_matches`).

**Hypothesis:** the compact L2 path has **no consensus caller** that (a) pins the
digest bytes against a verifier-fixed value, (b) calls `l0_program_matches`, or (c)
derives PI from chain data — unlike the full L1 path. The digest only pins the
circuit via `metadata.stark_common`; the *real* pinning is the verifier-owned
`context.circuit_prover_data`. If any future wiring builds the context from the
**prover-returned** `verifier_context()`, the digest checks are **vacuous** and the
compact route's soundness (asserted only in comments) collapses.

**How to break it:** when the compact path is wired, verify the context is built
from verifier-pinned constants (not the prover's returned value) and that
`l0_program_matches` + PI binding are executed. Today: confirm the compact path is
genuinely unreachable in consensus (fail-closed), and write the guard that a future
compact verifier must include.

---

## N8. `params.n` vs `n_e` semantic mismatch (spec ↔ code)  ⚠ MED-HIGH — fork/divergence

**Surface:** `zk_bridge.rs:1616` (`n_e = params.n / e`, i.e. `params.n` = **total**),
vs the spec docs `2026-07-07_PEARL_MOE_TRACK_B_SPEC.md:48-50` and `..._SPARSE_
UPGRADE_PATH.md:97` (`weight_col_offset = expert_idx·n`, i.e. `params.n` = **n_e**).
Tests agree with the code (`pearl_moe_tile.rs:106,179` use `n = n_e·e`).

**Hypothesis:** the code and the spec disagree on what public `n` means. If any
producer/artifact populates `MatmulParams.n` per the *doc* convention (= n_e), then
`n_e = params.n/e` becomes `n_e/e` and every column offset collapses — a silent
divergence from Pearl (fork) or an internal inconsistency. Also relates to whether
Pearl's public param is `n_e` or the total (byte-compat of the params envelope).

**How to break it:** trace which `n` the real artifact / Pearl public params carry
(is it `n_e` or `n_e·e`?), and confirm producers and the verifier agree. Diff
against Pearl's `total_b_cols = n·e` (`proof_utils.rs:277-278` uses `n·expert_idx`,
suggesting Pearl's `n` = n_e). If Pearl's `n` is `n_e` but our code treats it as
total, that's a concrete fork.

---

## N9. Certificate-noun decode heap-amplification DoS  ⚠ MED

**Surface:** `certificate_noun.rs:2390-2417` (`DecodeState` tracks node *count*, no
cumulative-byte budget), `:2927-2955` (`expect_declared_bytes` → `vec![0u8;
declared]` sized by the noun's length field, ≤1 MiB/node), `:2502-2624`
(`decode_proof_node`), and the public non-precheck decoders `:1156`, `:1475`,
`:1553`.

**Hypothesis:** each node allocates up to ~1 MiB from a length field even if the
payload is one byte; the node *count* cap (1M) × 1 MiB and a 4 MiB jam (repeated
`%bytes` tags back-referenced cheaply) yields tens-to-hundreds of GiB transient
allocation. Reachable **unauthenticated** via the public `decode_*` functions that
take no context and skip the statement precheck; the `verify_*_jam_*` entrypoints
gate it behind the precheck.

**How to break it:** craft a ≤4 MiB jam that expands to a huge allocation through
`decode_ai_pow_certificate_noun` / `decode_ai_pow_pearl_merge_artifact_jam`; measure
peak RSS. Fix direction: add a cumulative-byte budget to `DecodeState`. (Compact
certs also fully build the tree before the single-`Bytes`-node shape rejection.)

---

## N10. Grouped-GEMM is dense + expert-agnostic (meta-angle: everything rides on off-circuit checks)  ⚠ MED — completeness

**Surface:** `matmul.rs:344-396` (dense kernel), `moe_ref.rs` (commitment splice
only, **no tile arithmetic**), `zk_bridge.rs:1642-1671` (off-circuit schedule
rebuild + `l0_program_matches`).

**Hypothesis:** because there is no MoE sub-AIR, the entire MoE↔dense correspondence
is `l0_program_matches(canonical_program_for_strip_schedule(from_indices(
outer_indices, b_cols_global), s_A/s_B/κ, trace_height))`. Round-1 §F argued
`l0_program_matches` is a *complete* identity, but this round asks the deeper
question: does `canonical_program_for_strip_schedule` **faithfully and injectively**
encode (outer_indices, b_cols_global, s_A) — including the N1 column bound? If the
schedule rebuild omits the col-within-expert clamp (N1) or any expert structure, the
dense STARK cannot catch it.

**How to break it:** audit `canonical_program_for_strip_schedule` /
`StripIndexSchedule::from_indices` for what they *do not* constrain; look for two
distinct (outer_indices, b_cols_global) that map to the same program, or a program
that encodes bled columns (N1).

---

## N11. Recursion transcript / profile binding  ⚠ MED — verify (no gap found yet)

**Surface:** `plonky3-recursion/recursion/src/verifier/batch_stark.rs:997-1096`
(observe order: public values `:1001-1003` before `alpha` `:1074` / `zeta` `:1096`),
`recursion.rs:669-679` (PoW bits threaded from `profile`), `recursion.rs:1146`
(`cert.l0_program` trusted, bound externally).

**Hypothesis:** the transcript order is correct (public values observed before the
challenges — no grinding gap found). The residual risk is that `profile`/`zk_params`
are **caller-supplied** to `verify_recursive_certificate` rather than
statement-derived; the full-path bridge derives them correctly
(`zk_bridge.rs:1644,1677`), but any *other* caller (or the compact path, N7) that
passes a mismatched `profile` desyncs the challenger / weakens PoW.

**How to break it:** enumerate all callers of `verify_recursive_certificate`; verify
each derives `profile`/`zk_params` from the chain statement, not the cert. Confirm a
`profile.pow_bits` mismatch is actually rejected (the comment warns hardcoding 0
skips the observe+sample). Low priority — likely safe — but cheap to pin.

---

## N12. Periodic-pattern soundness (cols_pattern reaching past n_e)  ⚠ MED — connects to N1

**Surface:** `pearl_compat.rs:469-481` (`indices_with_offset_bounded` — only
`checked_add`s the offset, **no per-expert clamp**), the pattern validators
`pearl_compat.rs:126-140` (period ≤ 2²⁴, must divide dimension, stride multiple of
prior period, canonical trailing).

**Hypothesis:** the pattern validation ensures a well-formed periodic pattern and
that its period *divides the matrix dimension* — but "the dimension" for the column
pattern: is it `n` (total) or `n_e` (per-expert)? If validated against total `n`, a
pattern can select `local ≥ n_e` (feeding N1). If the pattern is the *attack vector*
for N1, this is where to clamp.

**How to break it:** determine which dimension `cols_pattern` is validated against
(`params.n` total or `n_e`), and whether `t_cols` can exceed `n_e`. If total, craft
a pattern that indexes into a neighbouring expert (N1 exploit).

---

## N13. Tile-selection grinding (`attempt_tile_index`)  ⚠ MED — difficulty

**Surface:** `fiat_shamir.rs:208` (`attempt_tile_index(attempt, tag, s_a,
num_tiles)`), test `fiat_shamir.rs:579-587` (deterministic, attempt-bound).

**Hypothesis:** the tile a miner must solve is derived from `(attempt, tag, s_a,
num_tiles)`. If a prover can cheaply vary an input (e.g. the attempt/tag) to select
a favorable tile *without* redoing proportional work, that's a grinding lever atop
the routing freedom (round-1 §D). Bound to `s_a` (which is routing/nonce-bound), so
likely each selection costs a full attempt — but confirm the tile index can't be
grinded independently of the work.

**How to break it:** check what `tag` is and whether it's free; verify that changing
the tile selection forces a fresh tile computation (no reuse). Compare the selection
to Pearl's (byte-compat).

---

## N14. Difficulty-factor arithmetic (`h·w·dot` target scaling)  ⚠ MED — difficulty / fork

**Surface:** `pearl_compat.rs:969-977` (`difficulty_adjustment_factor = h·w·dot`,
`checked_mul`), `:1041-1055` (`pearl_adjust_target_for_config`), `:1018-1040`
(`pearl_nbits_to_target_le`), `:996-1015` (`check_*_jackpot_target`).

**Hypothesis:** the jackpot target is scaled by the tile "work" `h·w·dot`. Two
questions: (1) is `h·w·dot` byte-compatibly derived vs Pearl (mantissa/exponent
shift in `pearl_nbits_to_target_le`, the multiply, the `<<`/`>>` in the target
adjust)? A divergence = miners who satisfy our target but not Pearl's (fork). (2)
are `h`, `w`, `dot` bound to the **actual** opened tile (so a prover can't claim a
large `h·w·dot` → easier target while computing a smaller/degenerate tile)? For MoE,
`h = |outer_indices|`, `w = |b_cols_global|`, `dot = k` — confirm these come from the
bound schedule, not prover free choice.

**How to break it:** diff `difficulty_adjustment_factor` + `pearl_adjust_target_for_
config` against Pearl's target math for identical inputs; check `h/w/dot` are the
schedule-bound tile dims. Try inflating the claimed factor vs the real tile.

---

## N15. Noise expansion byte-compat — **already verified SAFE** (record, low priority)

Agent 2 verified the `s_A/s_B → E/F` expansion is **byte-identical** to Pearl
`pearl_noise.rs` (keyed BLAKE3 counter mode, `&0x3F−32` uniform, `first^(1+mul_hi(
r−1,rnd))` permutation, base-129/256 packing), with a real-Pearl KAT and cross-crate
equivalence tests. Listed for completeness; no open attack unless N2/N3 (the
range-check + pin dependencies around the noise) turn up something.

---

## Prioritization for the driving goal

1. **N1** (column-within-expert bleed) — concrete forgery candidate; start here, with **N12** (pattern clamp) as its enabler and **N10** (schedule injectivity) as its backstop.
2. **N2** (MAT_UNPACK range divergence) — concrete accept-set/fork divergence; soundness-critical.
3. **N3 + N4** (noise pin dependency + matmul under-constraint) — the in-circuit soundness linchpins; verify the pinned/bound path is actually live.
4. **N5** (multi-commitment work-reuse) + **N6** (unbound cumsum/jackpot + schedule) — merge-mining / forgery bindings.
5. **N8** (n vs n_e) — fork/divergence; cheap to resolve, high impact.
6. **N7** (compact wiring), **N9** (decode DoS), **N14** (difficulty math), **N13** (tile grinding), **N11** (transcript profile) — MED.
7. **N15** — already SAFE.

Each angle's verdict must be a **working PoC** (a forged/cheaper/fork-divergent
proof our path accepts, or a DoS) or a **committed adversarial test + argument**
that it's closed. Note MoE recursive proving is **fail-closed** today
(`pearl_compat.rs:1066-1079`), so several of these are latent boundaries — but N1,
N2, N3, N4, N9 touch the **dense live path** or the byte-compat surface and matter
now.
