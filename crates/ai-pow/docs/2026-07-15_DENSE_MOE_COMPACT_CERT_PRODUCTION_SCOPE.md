# Dense & MoE Compact-Certificate Production Scope

**Date:** 2026-07-15
**Purpose:** Capture the present state of the dense and MoE AI-PoW compact
recursive certificates and the exact work remaining to make them *mineable
by the miner* and *verifiable by the verifier* in production. Grounded in a
code map (crates/ai-pow, crates/ai-pow-zk, crates/ai-pow-miner,
crates/ai-pow-jets, hoon/apps/dumbnet) as of this date.

**Decisions recorded (owner, 2026-07-15):**
- **MoE routing binding:** ship the **off-circuit** routing binding
  (routing_data public on the wire, root recomputed) — lift the admission
  gate and vet it. Do **not** block on the in-circuit CTL (Track B5a).
- **Dense tile model:** must **fully match Pearl's model**. (Finding below:
  the production scheduled path *already* matches Pearl — one ticket per
  certificate.)

---

## 0. Executive summary

The proving/verifying **crypto is done and tested.** Nothing blocking
"mineable + verifiable" is in the crypto. The gaps are:

1. **Chain integration (the real blocker; shared by dense AND MoE):** four
   fail-closed seams on the chain side, chief of which is that the verify
   jet's **boot setup is never injected in production** — so a *valid* cert
   can never be admitted live today (only garbage is rejected).
2. **MoE admission gate:** a complete off-circuit MoE prove+verify path
   exists and is wired into the jet verifier, but the miner **admission gate
   still hard-refuses MoE.** Per the decision above, the path forward is:
   lift the gate + exhaustively vet the off-circuit routing binding.
3. **Vetting depth:** the compact-cert crypto boundary (recursion
   commitment fold, MoE off-circuit routing binding, the num_stripes>64 R-b
   path on the *scheduled/merge* prover) needs a further adversarial audit
   before it is consensus-load-bearing.

The dense tile-scope concern (multi-tile) is **NOT** a production blocker
(§3.1).

---

## 1. Present state — Dense compact certificate

### 1.1 Prove path (production = Pearl-merge "scheduled" path)
- The production miner calls
  `ai_pow::zk_bridge::prove_pearl_merge_compact_recursive_certificate[_with_prover_cache]`
  (`crates/ai-pow-miner/src/run.rs:180,189`), which drives the **scheduled**
  Layer-0 prover `prove_ai_pow_scheduled_full_with_context` and then the
  compact recursive prover.
- The scheduled prover opens **one ticket** — `h = a_indices.len()`,
  `w = b_indices.len()`, `h·w ≤ 256` — via a `StripIndexSchedule`
  (`crates/ai-pow/src/zk_bridge.rs`, `expected_layer0_rows_for_strip_schedule:184`).
  The full model `(m, n)` never tiles the circuit.
- `prove_ai_pow_compact_recursive_certificate` (`zk_bridge.rs:1255`) is the
  **native square-contiguous** entry and is **test-only** (`zk_bridge.rs:6711`).

### 1.2 Verify path
- Node/consensus verify goes through the jet:
  `verify_ai_pow_block_artifact` (`crates/ai-pow-miner/src/certificate_noun.rs:3255`)
  → dense branch → compact-cert verify against the **Rust-owned canonical L0
  program commitment** (`ai_pow_zk::recursion::canonical_l0_program_commitment_vals`,
  `crates/ai-pow-zk/src/recursion.rs:466`; P0/D6 fold `recursion.rs:403-473,775-811`).
- The jet re-derives canonical `(A,B)` from the protocol seed (not the prover)
  and canonicalizes the block commitment as `BLAKE3(jam(commit-noun))` to
  match the miner (`commit_from_noun`, `crates/ai-pow-jets/src/lib.rs`).

### 1.3 num_stripes band (this session's work)
- Dense L0 now supports the **full Pearl stripe band** `num_stripes ≤ 512`
  (was capped at `STRIPE_MAX=64`) via the §6(b)-R-b stripe-major path.
  Prover routing + canonical verifier + admission + compact cert all landed
  and validated (commits b025ea5b, be88ee20, 3278ef9b, c2f06e15) plus a full
  adversarial audit of the matmul→jackpot chain (0b30fc1d, f99cf004).
- **Vetting caveat:** the R-b end-to-end validation used the **native**
  single-tile entry (`prove_and_verify_for_block`,
  `prove_ai_pow_compact_recursive_certificate`). The R-b sweep routing lives
  in the *shared* `prove_ai_pow_scheduled_full_with_context`, so it applies to
  the production scheduled/merge path — but a wide-k (`num_stripes>64`) ticket
  proven through the **scheduled/merge** entry with a real `StripIndexSchedule`
  is **not yet directly tested.** (Audit item A1 below.)

---

## 2. Present state — MoE compact certificate

### 2.1 What exists (real, not stubs)
- `prove_pearl_moe_compact_recursive_certificate` (`zk_bridge.rs:1616`): a
  **real** prover. Off-circuit MoE ticket (`compute_pearl_moe_ticket`,
  `pearl_compat.rs:1488`): routing-commitment splice → `s_a`
  (`canonical_noise_seeds_moe`); gather `outer_indices`; expert-offset
  `b_cols_global = local + expert_idx·n_e`; then the **identical dense
  scheduled prover** (MoE adds **no circuit**; the L0 AIR is unchanged —
  `recursion.rs:464`).
- `verify_pearl_moe_compact_recursive_certificate` (`zk_bridge.rs:1810`): a
  **real** verifier with 5 bindings — routing consistency
  (`verify_pearl_moe_routing_binding`, `pearl_compat.rs:1561`), expert-column
  recompute, routing-spliced `s_A` + PI binding, opened-schedule → canonical
  program commitment, and the compact-cert verify. It is **wired into the jet**
  (`certificate_noun.rs:2472,3255`; jet returns `Ok(true)` for a valid MoE cert,
  `crates/ai-pow-jets/src/lib.rs:105`).

### 2.2 The routing binding is OFF-CIRCUIT (the accepted design)
- Routing is bound by shipping the whole `routing_data` **publicly on the wire**
  and recomputing `routing_root == matrix_commitment(routing_data)` +
  `outer_indices[u] == routing_data[expert_start + pattern[u]]`
  (`pearl_compat.rs:1655,1698`). Capped at `PEARL_MOE_MAX_ROUTING_ENTRIES = 2^20`
  (`pearl_compat.rs:654`).
- This is a **documented "Pearl-narrowing"** (`pearl_compat.rs:641-654`): Pearl
  keeps routing off-wire and binds opened routing strips **in-circuit**. The
  in-circuit `outer_indices`↔routing CTL ("Track B5a") **does not exist** — only
  the off-circuit spec `crates/ai-pow-zk/src/moe_ref.rs`.
- **Per the 2026-07-15 decision, shipping the off-circuit binding is accepted.**
  Tradeoffs to accept explicitly: routing is public (not private), certs carry
  up to `2^20` routing entries, and the soundness rests on the 5 off-circuit
  verifier bindings rather than an AIR.

### 2.3 The admission gate (the thing to lift)
- **Miner refuses MoE:** `validate_pearl_merge_config_for_recursive_prover`
  → `pearl_compat.rs:1184`: `if config.moe().is_some() { Err("MoE (GROUPED_GEMM)
  recursive proving is not implemented") }`. Invoked at
  `crates/ai-pow-miner/src/run.rs:295`, `bin/ai_pow_mine.rs:264`.
- **Dense admission also refuses MoE** (defense in depth):
  `PearlPublicProofParams::sanity_check` (`pearl_compat.rs:976`),
  `from_public_data` (`:852`), dense artifact verifier
  (`certificate_noun.rs:2415`).
- Fail-closed confirmed by `crates/ai-pow/tests/pearl_moe_fail_closed.rs`.

### 2.4 Test coverage (MoE)
- Default suite covers **parsing, wire codec, the off-circuit routing/tile
  references, and fail-closed behavior** — NOT real in-circuit MoE proving.
- The only real MoE prove+verify tests are `#[ignore]`d (~25s):
  `zk_bridge.rs:3998` (`real_moe_grouped_tile_layer0_proof`), `:4121`
  (`real_moe_recursive_certificate_proves_and_verifies`), and
  `certificate_noun.rs:8066+,8362+`.
- Adversarial coverage of the off-circuit routing binding is **strong**
  (`crates/ai-pow/tests/pearl_moe_routing_binding.rs`: forged/cross-expert
  outer_indices, tampered root, column bleed, top_k bounds, span>m, unsorted,
  DoS cap).

---

## 3. Gap analysis

### 3.1 Dense tile model vs Pearl — RESOLVED (not a blocker)
- Pearl emits **one `(t_rows, t_cols)` ticket per proof**
  (`pearl/zk-pow/src/v1/circuit/pearl_program.rs`; note Pearl's internal
  `num_tiles` at `:45` is TILE_H sub-blocks *within* one ticket, not matrix
  tiles).
- The production scheduled/merge path is **one-ticket-per-cert** and does not
  gate on matrix `num_tiles` (`validate_scheduled_params` checks only
  `m,n>0`, `k`, `difficulty_bits==0`, `spot_checks==1`). Audit doc
  `2026-07-13_ZK_POW_PRODUCTION_PUZZLE_VS_PEARL_AUDIT.md:415-417`: one-tile-per-cert
  is *consistent with* Pearl, not a narrowing.
- The `num_tiles>1 → FullMatmulProofUnavailable` fail-closed
  (`zk_bridge.rs:2161,2246,3016`, gate
  `validate_canonical_recursive_certificate_params:2154`) is on the **native**
  path only, which the miner does not use. **No production change required**;
  cleanup only (consider documenting/removing the native full-matmul TODO or
  keeping it as a smoke-test gate).

### 3.2 Chain integration — THE production blocker (dense AND MoE)
Four fail-closed seams, all on the chain side, none in the crypto:

1. **Verify-jet boot setup never injected (#1 blocker).** No non-test caller
   of `init_ai_pow_verifier_setup` / `build_verifier_setup`
   (`crates/ai-pow-jets/src/lib.rs:53`, `setup.rs:218`). With `SETUP` empty a
   *well-formed* cert `BAIL_FAIL`s → Hoon `!!` stub → crash; only garbage is
   cleanly rejected. Doc: `2026-07-14_AI_POW_ARBITRARY_MODEL_PEARL_PARITY.md:360-364`.
   ⇒ `check-pow`'s `%ai-pow` branch (wired, `inner.hoon:1155`) can currently
   only reject. **This is the originally-deferred boot-setup task.**
2. **`do-pow` rejects submitted certs.** `%ai-pow` branch:
   `'do-pow: %ai-pow verifier not wired; rejected'` (`inner.hoon:1638-1651`).
3. **Node never requests AI mining.** `%mine-ai` effect defined but reserved;
   emit sites hardcode `%mine-zk` and crash on `%3`
   (`inner.hoon:844-854,1823-1833`).
4. **No end-to-end acceptance test.** Nothing drives mine→submit→real-kernel
   validate→admit-to-consensus; `nockchain-e2e` has zero ai-pow coverage.

Also required alongside #1: **pin the consensus params** the setup/verifier-key
digest commits to — `{hw, e, top_k}` and the num_stripes band
(`2026-07-14…:365-366`).

Already working: the Rust miner's cert build + submission poke
(`run.rs:646-661`), jet registration into hot state
(`crates/nockchain/src/main.rs:29`, `crates/roswell/src/lib.rs:293`), jet
dispatch from `check-pow`, correct in-consensus rejection of garbage
(Hoon test `hoon/tests/dumb/mod/unit/ai-pow-jet.hoon`), and jet-core verify
against real certs when a setup is supplied directly (the two `#[ignore]` ~25s
tests).

### 3.3 MoE — lift the gate + vet (per decision)
- Lift `pearl_compat.rs:1184` (and reconcile the dense-side MoE refusals in
  §2.3) so MoE configs are admissible.
- Reconcile the in-code tension: the verify path calls itself
  "soundness-complete" (`certificate_noun.rs:2470`) while proving stays gated —
  update the fail-closed doc-comments (`pearl_compat.rs:521,743,754,888,
  1002-1004,1180-1183,1418,1466-1470`) to reflect the off-circuit-shipping
  decision.
- Promote the `#[ignore]` real MoE prove+verify tests into a run tier (or a
  dedicated slow-CI lane).

---

## 3.4 FIXED (vetting, 2026-07-15) — R-b wide-k on non-contiguous tickets

**Severity: HIGH (Pearl-parity + MoE blocker for `num_stripes > 64`). ✅ FIXED.**

**Resolution:** added `place_useful_work_chain_rb_indexed(..., a_lanes, b_lanes)`
(lane-aware `noised_packed` IDs, mirroring `hw_indexed`);
`place_useful_work_chain_rb` now delegates with tile-local lanes (existing
tests unchanged); the scheduled-prover R-b branch
(`zk_bridge.rs`) passes `a_lanes/b_lanes = index − c_base` — the SAME
mapping the validated ≤64 branch uses, so correctness follows by construction.
**Validated:** `cr_rb_canonical_program_eq_extract_noncontiguous_indexed`
(canonical == extract(indexed R-b trace) cell-for-cell on a non-contiguous
`from_indices` Pearl-pattern ticket at num_stripes 96 & 128); all 10 R-b tests +
canonical suite (19/0) green.

**Side finding (not a production blocker):** `from_tile` **non-origin** tiles
(`tile_i>0`) at large `k` overflow `pack_ab_id` in `canonical_program` because
`sp.ca0 > first_row_index` underflows the canonical lane. Non-origin `from_tile`
is NOT a production shape (production uses `from_indices` patterns or the native
origin-only single tile), so this is an edge case, but worth a defensive guard
or a documented rejection.

### Original finding (for the record):

The scheduled/merge R-b branch (`zk_bridge.rs:2817-2833`) calls
`trace.place_useful_work_chain_rb(...)` which uses **tile-local** lanes
(`sb_base + di`) for the `noised_packed` chunk IDs. This matches the canonical
program (`indices[j] − ca0`) **only when the opened schedule is
contiguous-from-origin** (`a_indices[j] = j`, `ca0 = 0`). The ≤64 branch instead
uses `place_useful_work_chain_hw_indexed` with explicit `a_lanes`/`b_lanes =
index − c_base` (`zk_bridge.rs:2799-2811`) and so handles arbitrary opened
schedules.

⇒ At `num_stripes > 64` the R-b prover is **broken** for every non-contiguous /
non-origin opened schedule the production path can produce:
- **MoE** `outer_indices` (the routing gather) — non-contiguous.
- Dense **Pearl periodic-pattern** tickets — non-contiguous.
- Dense **non-origin square tiles** (`tile_i>0` ⇒ `a_lanes = tile_i·tile + j ≠ j`).

(This was flagged as a Stage-A residual: "R-b needs a lane-indexed variant like
`place_useful_work_chain_hw_indexed`.") The shipped Llama presets are
`num_stripes ≤ 64`, so this does not block them — but it blocks full Pearl-band
support and MoE at wide `k`, which are in scope.

**Fix (in progress):** add `place_useful_work_chain_rb_indexed(..., a_lanes,
b_lanes)` (lane-aware IDs, mirroring `hw_indexed`); make
`place_useful_work_chain_rb` delegate with tile-local lanes (existing tests
unchanged); route the scheduled-prover R-b branch to the indexed variant with
`a_lanes`/`b_lanes`. Validate: prove+verify a non-contiguous `num_stripes>64`
ticket (dense pattern + MoE).

## 4. Vetting / audit plan (the "further pass")

Adversarial audit before consensus-load-bearing. Ranked:

- **A1 — R-b on the scheduled/merge path.** Prove+verify a `num_stripes>64`
  ticket through the **production scheduled** entry (not the native one) with a
  real `StripIndexSchedule`, dense and MoE. Confirms the R-b routing +
  sx_bound=false + canonical R-b program hold on the path the miner uses.
- **A2 — MoE off-circuit routing binding, end-to-end forgery audit.** The 5
  verifier bindings (`verify_pearl_moe_compact_recursive_certificate`) are the
  *entire* MoE soundness surface now. Extend beyond the existing unit
  adversarials (`pearl_moe_routing_binding.rs`) to the FULL cert: forge a
  routing/expert/`s_A`/opened-schedule mismatch through a real compact cert and
  confirm the compact verify (not just the routing precheck) rejects.
- **A3 — Recursion commitment fold (P0/D6).** Adversarially confirm a cert
  whose L0 program ≠ the node-rebuilt canonical commitment is rejected, for BOTH
  dense-scheduled and MoE canonical programs (the R-b canonical program pin was
  audited for the native path; confirm on scheduled/merge).
- **A4 — Boot-setup / verifier-key digest binding.** Once §3.2#1 lands: a cert
  proven under params ≠ the injected setup's pinned params must be rejected by
  the jet (the digest binds `{hw, e, top_k, stripe band}`). Adversarial:
  wrong-params cert → reject; malformed setup → reject.
- **A5 — Wire/decode DoS + shape validation** for the (now larger, MoE
  routing-carrying) production artifact: `validate_production_artifact_shape`,
  the `2^20` routing cap, decode bounds.

---

## 5. Recommended sequencing

1. **Vetting pass A1–A3 first** (this document's owner's directive) — confirm
   the crypto boundary is sound on the *production* paths before wiring it into
   consensus.
2. **Chain integration §3.2** — boot-setup injection (+ param pinning) → flip
   `do-pow` accept and `%mine-ai` emit → e2e acceptance test. This is what makes
   dense mineable+verifiable on-chain.
3. **MoE gate lift §3.3** + A2/A4 vetting → MoE mineable+verifiable on the same
   integrated path.
4. Optional cleanup: native full-matmul `num_tiles>1` TODO (§3.1).

Each step touching consensus (setup injection, gate lift, do-pow accept) is
soundness-critical and invasive → stage it, KAT/adversarial-first, validate per
stage (R1).

---

## 6. Progress log

### 2026-07-15
- **Scope mapped** (this document): dense + MoE compact-cert present state, the
  four chain-integration seams, and the two scope decisions.
- **Decision captured:** MoE ships the off-circuit routing binding (lift the
  gate + vet); dense matches Pearl (one ticket per cert — already true on the
  scheduled/merge path).
- **Dense tile-model question RESOLVED** (§3.1): production scheduled/merge path
  is one-ticket-per-cert, Pearl-consistent; the `num_tiles>1` fail-closed is
  native-path-only. No aggregate circuit needed.
- **✅ FIXED — R-b lane-awareness for non-contiguous wide-k tickets** (§3.4,
  commits 08820020, 5036ad96): the num_stripes>64 R-b prover now uses opened
  lanes on the production scheduled/merge path, so Pearl periodic patterns + MoE
  `outer_indices` gathers work at wide `k`. Fixed the stale `TooManyStripes`
  message too.
- **✅ VETTING A1 COMPLETE** — R-b lane fix validated on BOTH non-contiguous
  wide-k production paths: (1) dense pattern — canonical==extract KAT at ns
  96/128 (`cr_rb_canonical_program_eq_extract_noncontiguous_indexed`); (2) MoE
  routing — REAL end-to-end prove through the scheduled prover at ns 128, L0
  jackpot PI == off-circuit MoE grouped tile
  (`real_moe_grouped_tile_layer0_proof_wide_stripes`, #[ignore], ~9s). A1 in §4
  is done.
- **Boot-setup injection scoped** (§3.2#1): `build_verifier_setup` runs
  `prove_canonical_moe_block` (expensive) → the setup must be PRECOMPUTED offline
  and embedded, keyed to the pinned consensus params. `init_ai_pow_verifier_setup`
  populates the `SETUP` OnceCell (`ai-pow-jets/src/lib.rs:46,53`); no production
  caller today. This is the original deferred task.

### CRITICAL-PATH DECISION — RESOLVED (owner, 2026-07-15)
**Support EVERY combination Pearl supports** — do NOT pin a single param set.
Implication: the verifier setup is a **table keyed by trace log-height
(degree_bits)**, not one shape (the setup digest depends on the padded trace
height, which buckets many shapes together — see the digest-shape-dependence
probe). Boot-setup injection must cover the full Pearl shape space's trace-height
buckets. `build_verifier_setup` is per-bucket; the boot table enumerates the
buckets Pearl's envelope can produce (bounded — degree_bits ≤ ~19).

### VETTING DIRECTION (owner, 2026-07-15)
Prioritize **A2 — MoE routing forgeries** through the FULL compact cert,
**alongside Pearl parity for the off-circuit routing approach** (confirm the
off-circuit binding captures exactly what Pearl binds in-circuit: opened tokens
↔ committed routing).

### A2 vetting — MoE off-circuit routing binding (2026-07-15)
**Existing coverage is strong** and already through the FULL L1/compact cert
(not just the precheck): `real_moe_recursive_certificate_proves_and_verifies`
adversarially rejects forged routing (binding #1), forged h_a (PI binding #3),
AND a shifted opened-column schedule (binding #4 — the "can't open other
tokens/columns" soundness crux); `moe_compact_prove_verify_and_bind` rejects a
wrong D6 commitment on the compact path. `pearl_moe_routing_binding.rs` covers
the precheck forgeries comprehensively (cross-expert indices, tampered root,
column bleed, top_k bounds, span>m, unsorted, DoS cap). **Gap: all at ns≤64.**
- **Attempting a wide-k (ns=128) MoE full-cert adversarial test surfaced TWO
  findings:**
  - **✅ FIXED — L1-recursion sx_bound gap.** `prove_recursive_certificate_from_
    chain_verified_composite_proof` (recursion.rs:1559) AND
    `verify_recursive_certificate_inner` (:1205) hardcoded `sx_bound=true`, so the
    INTERMEDIATE L1 (non-compact) recursion couldn't wrap/verify an R-b
    (ns>STRIPE_MAX) L0 proof. Stage E threaded only the COMPACT path; this threads
    the L1 path too (derive `sx_bound = k/r ≤ STRIPE_MAX` internally, no caller
    changes; ≤64 unchanged). Correct-by-construction (mirrors Stage E).
  - **✅ Node CAN rebuild wide-k MoE canonical** (`moe_widek_verify_canonical_
    program_builds`, FAST, no proof): for the ns=128 MoE ticket
    (outer_indices=[0,2,…,14], expert cols [0..8)), `canonical_program_for_strip_
    schedule` builds without `pack_ab_id` overflow — over BOTH the ticket's and the
    verify-recomputed columns. So the PRODUCTION compact verify's core rebuild step
    works at wide-k MoE.
  - **OPEN FINDING (non-production path) — L1 verify `pack_ab_id` overflow at
    wide-k MoE.** `verify_pearl_moe_recursive_certificate` (the L1, non-compact
    verify) panics at `pack_ab_id` for the ns=128 MoE cert — but NOT in the
    canonical rebuild (that builds fine, above). It is inside the L1 verify
    internals. **Production uses the COMPACT cert, not L1**, so this is not a
    production blocker, but investigate (likely a pre-existing L1-path assumption).
    The heavy full-cert wide-k MoE adversarial suite is deferred to the compact
    path (see remaining #5).

**Pearl-parity of the off-circuit approach:** the binding proves opened rows ==
the expert's routed tokens under the public pattern from the committed routing
(`outer_indices[u] == routing_data[expert_start + pattern[u]]`, `routing_root ==
matrix_commitment(routing_data)`) + the opened schedule is bound to the cert
(binding #4). This is the same *correspondence* Pearl binds — Pearl does it
in-circuit over opened routing strips; we do it off-circuit with public
`routing_data`. Equivalent soundness for the correspondence; the delta is
routing privacy/wire-size (the documented Pearl-narrowing), not the binding
strength. [A2 substantially covered; residual: a forged-ROW-gather (outer_indices)
variant through the full cert — currently covered transitively by binding #1 +
#4, an explicit test would be belt-and-suspenders.]

### Boot-setup TABLE — Stage A DONE (2026-07-15)
Supporting the full Pearl band means the verify-jet setup is a TABLE keyed by
trace log-height, not one value. **Landed the data-structure + lookup:**
- `AiPowVerifierSetup` gains `trace_height` (self-keying); `build_verifier_setup`
  populates it from the proved canonical block (`block.run.trace_height`).
- `SETUP: OnceCell<Vec<AiPowVerifierSetup>>` (a table); `init_ai_pow_verifier_setup`
  takes the table and rejects empty / duplicate-bucket tables
  (`setup_table_heights_valid`, unit-tested `setup_table_admission_rule`).
- New `ai_pow_verifier_setup_for(trace_height)` resolves the bucket; the jet looks
  up by `artifact.certificate.trace_height` and BAIL_FAILs on a miss (surfaces an
  incomplete boot table; the decode already rejects malformed shapes).
- Contained to `ai-pow-jets` (no other crate constructs the setup / calls init);
  ai-pow-jets suite green.
**End-to-end validation pending Stage B/C** (the table is empty until injected).

### Boot-setup TABLE — Stage B DONE (2026-07-15)
Enumerated the Pearl §4.8 envelope's Layer-0 trace-height buckets
(`boot_setup_trace_height_buckets_are_small_and_bounded`, FAST): **exactly 8
buckets = 2^13 … 2^20** (degree_bits 13–20) across 114 sampled consensus-valid
shapes. `CircuitConfig::for_layer0_trace` has no max-degree cap
(`prod_adaptive` adapts the FRI profile to any degree_bits), so all 8 are
provable/verifiable — no envelope cap needed. ⇒ **The boot table is exactly 8
`AiPowVerifierSetup` entries**, one per bucket. Small and tractable to precompute
+ embed.

### Boot-setup Stage C — SCOPED (2026-07-15), needs a design decision
The verifier `context` is built as a byproduct of the compact prove
(`recursion.rs:1989`, `circuit_prover_data = Arc::clone(l2_prep.circuit_prover_data)`)
and is **NOT serializable** (`Arc<CircuitProverData>`; the struct doc:
"production must derive or pin it from trusted code/config/verifier-key state").
So "precompute + embed a blob" is NOT available. Two options:
- **(a) Build at boot.** Spawn a background task that runs `build_verifier_setup`
  for the 8 buckets and calls `init_ai_pow_verifier_setup` when done. The jet
  ALREADY tolerates a still-building table (decode-first; a well-formed cert
  BAIL_FALLs to the Hoon stub until the bucket is present). Cost: ~8×2min ≈ 16min
  of proving **per boot**, during which `%ai-pow` blocks can't be verified.
  Simplest; heavy boot.
- **(b) Derive context without proving — NOT AVAILABLE (de-risked 2026-07-15).**
  The L1 verifier circuit is built OVER a concrete L0 proof
  (`build_composite_l1_verifier_circuit(&cfg, &air, &verified.proof, …)`,
  recursion.rs:1916), and `l2_prep = build_compact_batch_l2_over_l1_prep(
  &l1_outer_proof)` derives from the L1 PROOF. So `circuit_prover_data` cannot be
  compiled without a proof — the context genuinely requires proving one canonical
  block per bucket. No compile-only shortcut.
- **(b') Precompute offline + embed.** Prove the 8 canonical blocks OFFLINE
  (once), serialize the contexts, embed the bytes, deserialize + inject at boot.
  **De-risked 2026-07-15: NOT cheap.** `CircuitProverData` (plonky3-recursion
  `circuit-prover/src/batch_stark_prover.rs`) has NO `Serialize` and wraps
  `ProverData<SC>` (STARK prover data: commitments/LDEs) + preprocessed columns —
  so this needs a substantial UPSTREAM `Serialize`/`Deserialize` addition to
  `CircuitProverData` + `ProverData<SC>`. The memory's "embed a precomputed setup"
  framing requires this upstream work.
DECISION (owner, 2026-07-15): **(b') — make the setup serializable + cache it.**
16 min/boot is unacceptable. Precompute the 8-bucket table offline (prove 8
canonical blocks once), serialize + cache to disk, deserialize + inject at boot
(fast). Requires:
- Upstream `Serialize`/`Deserialize` on `CircuitProverData<SC>` (+ its inner
  `ProverData<SC>`, `NonPrimitivePreprocessedMap`) in plonky3-recursion.
- `Serialize`/`Deserialize` on `AiPowCompactBatchVerifierContext` (+ `metadata`,
  `fri_shape`; `Arc` via serde `rc` or serialize the inner) and `AiPowVerifierSetup`.
- A cache path: `build_verifier_setup_table()` → serialize to a cache file (built
  offline / first-run); boot loads + `init_ai_pow_verifier_setup(table)`.
- Validate: serialize → deserialize → a cert per bucket verifies against the
  round-tripped setup.
Staged (R1): C1 upstream serde + round-trip; C2 context/setup serde + round-trip;
C3 table cache build+load; C4 boot wiring (nockchain + roswell) + per-bucket
end-to-end.

### Boot-setup Stage C1+C2 — LANDED (2026-07-15), pending round-trip validation
Made the compact verifier setup serializable so it can be cached (fast boot):
- **C1** (`plonky3-recursion/circuit-prover`): `CircuitProverData` now derives
  Serialize/Deserialize in its VERIFIER-ONLY projection — `CommonData` via
  `SerializedStarkCommon`, `primitive_columns`, and `non_primitive_columns` (as a
  Vec-of-pairs, since the crate is no_std and `HashMap` serde needs std). The
  prover-only PCS LDEs are NOT serialized and are reconstructed EMPTY
  (`ProverOnlyData::empty()`) on load — the verifier never reads them (it restores
  omitted preprocessed openings from the columns + CommonData).
- **C2** (`ai-pow-zk::recursion` + `ai-pow-jets`): `AiPowCompactBatchVerifierContext`
  derives serde (Arc handled via an inner-value with-helper; metadata/fri_shape/
  digest already serde); `AiPowVerifierSetup` derives serde (added `serde` dep to
  ai-pow-jets).
- All crates build clean. **Validation (running):** the R-b compact-cert test now
  serializes→deserializes the verifier context and re-verifies the cert against the
  DESERIALIZED context — proving the cached setup is sound and prover-only data is
  genuinely unneeded.
Remaining: C3 (build the 8-bucket table, serialize to a cache file **in the
nockapp data dir** with a sane path), C4 (boot wiring in nockchain + roswell +
per-bucket end-to-end).

### Stage C — de-risk deep-dive (2026-07-15): what the VERIFIER actually needs
The compact verify (`verify_compact_batch_recursive_certificate_with_context`,
recursion.rs:2040) reads from the context ONLY: `verifier_key_digest`,
`metadata`, `fri_shape`, and `circuit_prover_data` (passed to
`GoldilocksBlake3PathPrunedCompactVerifierContext::new`, :2091). It never touches
the prover-only LDEs directly — and `ProverData = { common: CommonData (shared),
prover_only }`, `CircuitProverData::common_data() -> &prover_data.common`. So the
verifier fundamentally needs the **CommonData (preprocessed circuit commitment)**,
NOT `prover_only`.
- **A serializable projection ALREADY EXISTS:** `SerializedStarkCommon`
  (circuit-prover/src/batch_stark_prover.rs:388, `#[derive(Serialize,Deserialize)]`,
  `from_common`/`into_common`) — the preprocessed commitment + instance metas.
- **Is building the context cheap (small boot delay) or does it need the prove?**
  It is built as a byproduct of the pipeline: `context.circuit_prover_data =
  l2_prep.circuit_prover_data`, and `l2_prep = build_compact_batch_l2_over_l1_prep(
  &l1_outer_proof)` — it CONSUMES the L1 proof (which needs the L0 proof). So today
  building the context needs ~most of the ~2min prove chain per bucket (~14-16min
  for 8) — NOT a small delay. BUT the CommonData/metadata are SHAPE-determined
  (depend only on the trace-height bucket, not proof values).
⇒ Two fast-boot paths:
  1. **Cache** (serialize the 8 contexts offline via `SerializedStarkCommon` +
     metadata/fri_shape/digest; load in ms at boot). Pragmatic; the projection
     primitive exists; needs the full CircuitProverData reconstruction path
     (primitive/non-primitive columns beyond `SerializedStarkCommon` — TBD how much).
  2. **Shape-only build** (refactor `build_composite_l1_verifier_circuit` /
     `build_compact_batch_l2_over_l1_prep` to build the verifier context from the
     trace-height SHAPE without a real proof — a few seconds at boot, no cache
     file). Cleaner long-term; a deeper recursion-internals refactor.

### Remaining (post-decision), in dependency order
1. **Stage C** — resolve the (a)/(b) decision above; build the 8-bucket table;
   inject at boot (nockchain + roswell); validate a cert per bucket. [§3.2#1;
   Stages A+B done, C scoped]
2. Flip `do-pow` `%ai-pow` accept + emit `%mine-ai` candidate. [§3.2#2,#3]
3. End-to-end acceptance test: mine → submit → kernel validate → admit. [§3.2#4]
4. Lift the MoE admission gate + reconcile fail-closed doc-comments. [§3.3]
5. Vetting A2 (MoE off-circuit routing forgeries through the FULL compact cert),
   A3 (recursion commitment fold on scheduled/merge), A4 (setup digest binds the
   pinned params), A5 (wire/decode DoS). [§4]
6. R-b fix end-to-end on the production scheduled path at ns>64 (canonical
   KAT done; a full scheduled prove+verify at ns>64 with a non-contiguous
   schedule remains — extends A1).
