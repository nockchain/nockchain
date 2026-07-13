# MoE → Compact Production Parity — Plan, Dependencies & D6

**Date:** 2026-07-13
**Purpose:** A single self-contained roadmap for bringing the MoE (GROUPED_GEMM)
AI-PoW puzzle to **compact production parameters**, including the shared
soundness prerequisite (D6) uncovered while auditing the compact path. Companion
to the discrepancy audit
(`2026-07-13_ZK_POW_PRODUCTION_PUZZLE_VS_PEARL_AUDIT.md`) and the production
residual (`2026-07-08_PEARL_PRODUCTION_RESIDUAL.md`, items M1–M7 / S1); this doc
folds their MoE-relevant threads together with the D6 investigation so the whole
track lives in one place.

---

## 0. TL;DR

- The **hard part is already done — but only on the diagnostic-L1 format.** The
  MoE circuit (grouped GEMM, routing splice into `s_A`, selective disjoint-chunk
  opening), the routing-consistency binding, and the opened-schedule binding all
  prove/verify end-to-end against the large diagnostic-L1 certificate. None of it
  runs on the **compact** certificate that production actually ships.
- Getting to compact production is therefore mostly **integration + re-validation
  onto the compact format**, plus **one shared prerequisite: D6** — the compact
  path has no verifier-side opened-schedule binding, and the whole point of MoE
  (prover opened *exactly the routed tokens*) depends on that binding.
- **Order:** fix **D6** (MoE-aware) → land MoE compact prove/verify (**M1–M3**) →
  size/latency + k≠1024 + adversarial (**M5–M7**) → **lift the fail-closed guards
  last (M4)**. Actual mainnet acceptance additionally waits on the Hoon↔Rust
  consensus wiring (**D4/S1**), which is shared with dense.
- **Nothing is mis-accepted today:** MoE is fail-closed at four independent
  guards, and the compact node-verify path is not wired into consensus (Hoon
  rejects `%ai-pow`). This is a *build-forward* plan, not a live-vulnerability
  fix.

---

## 1. Terminology (read this first)

These are **proof-artifact formats**, not work-in-progress states:

- **diagnostic L1 certificate** — this doc's readable name for the code's
  **`checkpoint`** certificate (`AiPowRecursiveCertificate`). The recursion stops
  at the intermediate L1 outer proof and is emitted as a *large* artifact that
  **embeds the L0 proof and the L0 program**. It is a diagnostic/regression
  format ("too large for the wire", `recursion.rs:1995`; hidden from rustdoc). It
  is **not** production and **not** saved/partial progress. Grep the code for
  `checkpoint`.
- **compact certificate** — the **production** artifact
  (`AiPowCompactBatchRecursiveCertificate`): the recursion continues one more
  layer (L2) and is pruned to a small body (≈ 122–124 KB). This is what consensus
  would verify.

The **format axis** (diagnostic-L1 vs compact) is **orthogonal** to the **puzzle
axis** (dense vs MoE):

| | diagnostic L1 (diagnostic) | compact (production) |
|---|---|---|
| **Dense** | validated | validated ← production runs here |
| **MoE** | validated | **not implemented** (this doc) |

MoE's circuit + soundness logic is validated only in the top-right-of-dense cell;
this plan moves it into the bottom-right (compact) cell.

---

## 2. Current state — what exists vs. what's missing

### 2.1 Done and validated (on the diagnostic-L1 format)

- **MoE circuit / off-circuit reference:** routing canonicalization, routing
  commitment splice `s_A` via `hash_activations`
  (`fiat_shamir::moe_hash_activations`, KAT-matched to Pearl `proof_utils.rs:33`),
  grouped tile / `outer_indices` gather, and **selective disjoint-chunk opening**
  that keeps the Layer-0 trace inside `PEARL_TRACE_BOUND = 2²²` at production
  scale (byte-identical to dense for contiguous tiles).
- **Routing-consistency binding:** `verify_pearl_moe_routing_binding`
  (`pearl_compat.rs:1460`) — the opened A-rows (`outer_indices`) provably match
  the committed routing; ~10 adversarial tests, plus the expert-column clamp
  (`local < n_e`, prevents bleeding into a neighbour expert's weights).
  > **Acceptance-parity fixes (2026-07-13, found by re-auditing vs Pearl
  > `sanity_checks.rs`):** the binding was missing two Pearl checks — it now
  > rejects **`top_k == 0`** (Pearl `:69`) and **non-strictly-increasing
  > `outer_indices`** (Pearl `:132`, sorted, no duplicates). Both are configs
  > Pearl's own verifier rejects; accepting them would have been a merge-mining
  > divergence (a ticket valid for us but not for Pearl). New errors
  > `MoeTopKZero` / `MoeOuterIndicesNotSortedUnique`; +2 adversarial tests.
  > (`n·e ≤ 2²⁴` is not a gap: our `n` is the total column count, so the dense
  > `n ≤ 2²⁴` bound already covers Pearl's per-expert `n·e ≤ 2²⁴`.)
- **Opened-schedule binding:** `verify_pearl_moe_recursive_certificate`
  (`zk_bridge.rs:1577`) binds the opened set via
  `from_indices(outer_indices, expert-columns)` + `l0_program_matches`
  (`recursion.rs:141`) — but this is the **diagnostic-L1** verifier.

### 2.2 Missing for compact production

- **No MoE branch anywhere in the compact pipeline.** `recursion.rs` (the compact
  L1→L2 prover/verifier) has **zero** MoE / routing / expert awareness;
  `prove_pearl_merge_compact_recursive_certificate` (`zk_bridge.rs:1692`) is
  **dense-only**.
- **No compact MoE verify.** `verify_pearl_moe_recursive_certificate` targets the
  diagnostic-L1 cert; there is no `e>0` branch in
  `verify_decoded_ai_pow_pearl_merge_compact_artifact`.
- **The compact opened-schedule binding itself does not exist yet (D6).** See §3.
- **MoE is fail-closed** at four guards (intact): `pearl_compat.rs:850` (public
  data), `:929` (`sanity_check`), `:1087` (recursive-prover admission), plus the
  FP8 layer guard (`params.rs:488`).

---

## 3. The shared prerequisite — D6 (compact opened-schedule binding)

**Finding (from the audit's Part G, condensed):** the compact certificate carries
**no `l0_program`**, so the diagnostic-L1 binding `l0_program_matches` cannot run
on it. The compact `verifier_key_digest` is **shape-only** (route params + L2
proof metadata + FRI shape, `recursion.rs:357`) — it does **not** bind which
rows/columns were opened. The entire opened-schedule / constraint-selector
binding lives in the verifier-owned `context.circuit_prover_data`, which encodes
the specific L0 program (`recursion.rs:1790, 1868`). **But no verifier-side
builder derives that context from the canonical program** — the only constructor
is prover-side (`recursion.rs:1865`), and every verify path (all `#[ignore]`d
tests) uses `run.verifier_context()`, the prover's own context. Wiring the node to
a prover-supplied or reused-generic context would let a malicious miner open a
favorable strip and still verify.

**Why this gates MoE specifically:** the soundness guarantee of MoE is that the
prover opened *exactly the routed tokens* (`outer_indices`), not a favorable set.
That is an opened-schedule binding — precisely what D6 is missing on compact. So:

- D6 must be fixed for the compact path (shared with dense), **and**
- the D6 verifier-side context builder must be **MoE-aware**: able to build the
  canonical context from the **MoE** canonical program (the scattered
  `from_indices(outer_indices, expert-columns)` schedule + the routing splice),
  not only the dense square/strided program.

**Feasibility + wall:** the circuit prep (`circuit_prover_data`) is
**witness-independent** — a function of (program, proof *shape*), confirmed by the
prover cache being reused across proofs. So a verifier-side builder is possible in
principle, and `common_data` derives from the canonical program
(`logup_common_for(&cfg, &program, true)`). **The wall:**
`build_composite_l1_verifier_circuit` still needs a shape-correct L0 `BatchProof`,
which the compact cert deliberately omits. Closing it needs either **(i)** a
shape-only `BatchProof` synthesizer (from params / `trace_height`), or **(ii)** a
redesign that binds the canonical program's preprocessed commitment into
`verifier_key_digest` / the statement digest.

**Status:** characterized, not fixed. A `# Soundness` contract was added at the
node compact-verify entry (`certificate_noun.rs`,
`verify_decoded_ai_pow_pearl_merge_compact_artifact_with_context_and_limits`)
stating the context MUST be canonically derived; block acceptance must not reach
that path until the builder lands.

---

## 4. Work breakdown to reach MoE compact production parity

Dependency-ordered. Each stage: KAT/de-risk first, full regression + adversarial
gates, commit per validated stage (per R1). "M#" cross-reference the residual.

### P0 — D6 verifier-side canonical context builder (MoE-aware) — *prerequisite, shared with dense* — 🚧 **attempted 2026-07-13; blocked at a concrete API wall**
Build `AiPowCompactBatchVerifierContext` (or at least the pinned
`circuit_prover_data` + expected `verifier_key_digest`) from the **canonical
program** re-derived from the public opened schedule
(`canonical_program_for_strip_schedule` + noise pins + the 60 PIs), via
`logup_common_for` → `build_composite_l1_verifier_circuit` →
`build_compact_batch_l1_prep` → `build_compact_batch_l2_over_l1_prep`. **Design it
MoE-aware from the start** so the dense and MoE canonical programs share one
builder. Node rewiring: the compact verify path builds its own context; never
accepts a prover-supplied one.

> **Concrete wall (found by attempting the code path, 2026-07-13):** the context's
> `circuit_prover_data` cannot be built from the canonical program alone.
> - `build_composite_l1_verifier_circuit` (`recursion.rs:632`) takes a
>   `BatchProof<AiPowStarkConfig>` — the **L0 proof**, which the compact cert
>   deliberately omits. It uses the proof only for *shape* (allocated as circuit
>   inputs), but `BatchStarkProof` wraps a real FRI `BatchProof<SC>`
>   (`batch_stark_prover.rs:532`) and there is **no shape-only / dummy
>   constructor** — you cannot synthesize a stand-in.
> - The path to `circuit_prover_data` also runs through
>   `prove_compact_batch_l1_with_prep` → `run_composite_l1_verifier_traces`
>   (`recursion.rs:1021,1027`), which consume the **actual L0 proof values** to
>   produce the L1 outer proof that `build_compact_batch_l2_over_l1_prep`
>   (`recursion.rs:1520`) needs. So even though the *final* `circuit_prover_data`
>   is witness-independent (confirmed: `build_compact_batch_l1_prep` keys on
>   `circuit_shape`, `recursion.rs:1012`), the *route* to it requires real proofs.
>
> **⇒ Three sound options, each a large invasive change requiring real-proving +
> adversarial validation (out of a single session; half-landing forbidden by R1):**
> 1. **Shape-proof synthesizer** — add a p3-recursion API to construct a
>    dimensionally-valid `BatchProof`/`BatchStarkProof` scaffold from
>    (params, trace_height, FRI shape) so the verifier builds the context from
>    shapes, keeping verify succinct. *New, soundness-adjacent p3 capability.*
> 2. **Program-commitment binding** — bind the canonical L0 program's
>    preprocessed commitment into the L2 statement/`verifier_key_digest` and have
>    the node recompute + compare it cheaply. *Invasive p3-recursion circuit
>    change (L1 verifier must expose the L0 program commitment as a constrained
>    public value).*
>    > **Root cause + exact seam (found by reading the circuit, 2026-07-13):** in
>    > `build_composite_l1_verifier_circuit` the L0 `common_data` — which carries
>    > the L0 program's **preprocessed commitment** — is packed as
>    > **prover-supplied values** and allocated as circuit *inputs*
>    > (`recursion.rs:686` allocate, `:742` `pack_values(..., proof, common_data)`).
>    > So the L1 circuit proves "the L0 proof is valid for the program whose
>    > commitment the prover supplied" — the commitment is never a *public* value,
>    > which is exactly why the program is unbound. **Fix seam:** promote that
>    > preprocessed-commitment target to a public input alongside
>    > `statement_digest_targets` (`recursion.rs:729-737`), thread it through
>    > `statement_public_digest` / the L2 statement, and have the node recompute
>    > the canonical program's MMCS preprocessed commitment and pass it in the
>    > expected public values. **Non-localized:** changing the L1 public-input
>    > count ripples into L2 packing (`public_binding_lanes`), the FRI shape, and
>    > `verifier_key_digest`; validation requires real proving (does the modified
>    > circuit still prove/verify, and does a wrong-program commitment reject?).
> 3. **Node re-proves L0** — the node has `A`/`B`, so it can reconstruct the
>    canonical trace, re-prove L0, and run the existing builder. *Sound but
>    NON-succinct (re-proving defeats the compact cert's purpose) — acceptable
>    only as a correctness-first stopgap, not production.*
>
> R1.1 self-test satisfied: the load-bearing code was actually traced/attempted
> and the wall is concrete (a missing constructor / a required circuit change),
> not a size objection. The next session should pick option (1) or (2) and land
> it with the cross-schedule adversarial test (M7) as one validated unit.

### M1 — MoE artifact noun shape + canonical encode/decode — 🟡 **codec landed 2026-07-13; artifact wiring remains**
Extend the compact `%ai-pow` artifact to carry `PearlMoeParams` (`expert_idx`,
`routing_offsets`, `hash_routing`, `outer_indices`) **plus** `routing_data`, with
strict canonical encoding + DoS byte caps, mirroring the dense
`PearlMergeAiPowArtifactShape` and its `precheck_..._jam` boundary. (Pearl's own
wire cap is `PublicDataMaxSizeV2 = 4807`.)

> **Landed (validated) 2026-07-13 — the opaque-nonce codec + DoS cap:**
> `encode_pearl_merge_ai_pow_nonce_moe` / `decode_pearl_merge_ai_pow_nonce_moe`
> (`certificate_noun.rs`) under a new `AIM1` tag. The dense `AIP1` path is
> **byte-identical and untouched** (the MoE encoder reuses the dense framing
> verbatim, retags the magic, and appends the Pearl MoE tail
> `expert_idx ‖ routing_offsets ‖ hash_routing ‖ outer_count ‖ outer_indices`
> plus a DoS-capped `routing_data` block). Every read is length-checked and every
> count capped before allocation. New cap `PEARL_MOE_MAX_ROUTING_ENTRIES = 1<<20`.
> **16 new tests** (round-trip incl. max-experts/max-outer, dense-framing-verbatim,
> dense↔MoE tag disjointness, every cap, `e`-in-trailer cap, per-length truncation
> fuzz, trailing-byte rejection, faithful-carry); full miner suite green
> (137 passed, 6 ignored).
>
> **Remaining (still M1):** thread the MoE fields through the **artifact builders**
> (`build_ai_pow_pearl_merge_artifact_noun_from_ticket_*`) and the **decode +
> statement-precheck** path (`decode_ai_pow_pearl_merge_artifact_*`,
> `precheck_ai_pow_pearl_merge_artifact_statement_*`) so a full MoE artifact
> round-trips and feeds `verify_pearl_moe_routing_binding`. That wiring is only
> exercisable end-to-end once M2 (MoE compact prove) produces a MoE ticket, so it
> lands with M2/M3. MoE stays fail-closed at every verify gate throughout.

> **⚠️ New documented Pearl-narrowing (routing_data DoS cap):** our native routing
> binding carries the full `routing_data` (`m·top_k` u32s) publicly and recomputes
> `routing_root == matrix_commitment(routing_data)` — Pearl instead keeps routing
> **off-wire** and binds opened routing strips **in-circuit**, allowing `m·top_k`
> up to 2³². The `PEARL_MOE_MAX_ROUTING_ENTRIES = 1<<20` cap (≈ 4 MiB, matching the
> jam DoS budget) bounds our accepted MoE space to `m·top_k ≤ 2²⁰` — fine for the
> known scale (m=131072, top_k=2 ⇒ 262144), but a **narrowing of Pearl's MoE space
> and a wire-size divergence**. Closing it means moving the routing binding
> in-circuit (Pearl's approach). This is a D2-class discrepancy, MoE-specific.
> The cap constant lives in `pearl_compat` and is enforced at **both** layers: the
> artifact nonce codec (before decode allocation) and
> `verify_pearl_moe_routing_binding` (before the O(m·top_k) token loop + routing
> hash), with boundary tests on each. Defense-in-depth: the binding does not rely
> on the codec having already capped the input.

### M2 — MoE prove path that emits the *compact* cert
There is no `prove_pearl_moe_compact` today. Evaluate the MoE ticket
(routing → splice `s_A` → grouped tile → jackpot), build the prover context with
the MoE splice + `from_indices(outer_indices, expert-columns)`, and drive
`prove_pearl_merge_compact_recursive_certificate` (not the diagnostic-L1 prover).
Selective opening flows through automatically (it is a Layer-0/trace property).

### M3 — MoE node verify branch on compact
Wire MoE soundness verification into
`verify_decoded_ai_pow_pearl_merge_compact_artifact` for `e > 0`: recompute `s_A`
from the carried routing, run `verify_pearl_moe_routing_binding`, recompute + bind
the MoE canonical program **through the P0 context** (the compact analogue of
`l0_program_matches`), then verify the compact cert. This is porting
`verify_pearl_moe_recursive_certificate`'s logic from diagnostic-L1 to the compact
boundary.

### M5 — compact size/latency validation for a MoE tile
The selective-opening trace is O(tile), comparable to dense, so ≤ 150 KB / ~30 s
*should* hold — but **measure** it on the compact route; do not assume.

### M6 — k≠1024 selective-keying re-validation
Selective opening + the sweep-lane formula are validated for **k = 1024**. If
production MoE uses k≠1024 with scattered rows, re-derive and re-validate the
lane↔chunk keying (for k>1024 a row spans ⌈k/1024⌉ chunks; the `noised_packed`
producer key vs the sweep consumer lane must be re-aligned) and add coverage
before relying on it.

### M7 — adversarial suite on the compact path
Port the routing-binding, opened-schedule, and malicious-miner adversarial tests
from the diagnostic-L1 boundary to the compact node boundary — including the D6
**cross-schedule rejection** test (prove two same-shape schedules; a cert verified
against the *other* schedule's canonical context must reject), now for the MoE
program.

### M4 — lift the four `e>0` fail-closed guards — **LAST**
`pearl_compat.rs:850, 929, 1087` + the FP8 guard (`params.rs:488`). Only after
P0 + M1–M3 + M5–M7 and the full adversarial suite are green on compact. This is
the mainnet-enabling step; land it staged, per R1.

> Note: the D3 fix (Pearl's `h·w ≤ 256` per-tile cap, now enforced in
> `sanity_check`) already applies to MoE — it fires once the guards lift, since
> `sanity_check` currently fail-closes on MoE *before* reaching the shape checks.

---

## 5. Acceptance bar — "compact production parameters"

From the compact-recursive production pipeline:

| Parameter | Target |
|---|---|
| Production artifact | the **compact** recursive certificate (not diagnostic-L1, not raw Layer-0) |
| Artifact size | ≤ ~150 KB (dense today ≈ 122–124 KB); MoE must be measured (M5) |
| Proving wall time | ~30 s (release + `target-cpu=native`) |
| Proof-system security | 60 bits (relaxed FRI) |
| Hard trace bound | `PEARL_TRACE_BOUND = 2²²` rows (selective opening keeps MoE inside it) |
| Prove API | `prove_pearl_merge_compact_recursive_certificate[_with_prover_cache]` (needs a MoE branch — M2) |
| Verify API | `verify_compact_batch_recursive_certificate_with_context` via the jam/noun boundary (needs the P0 canonical context + M3 MoE branch) |

---

## 6. Ordering, gates, and how the pieces relate

```
                 ┌─────────────────────────────────────────────┐
   D6 (P0) ─────▶│ MoE-aware verifier-side canonical context   │  shared with dense
   prerequisite  │ builder + node rewiring (compact binding)    │
                 └───────────────┬─────────────────────────────┘
                                 │
              ┌──────────────────┼───────────────────┐
              ▼                  ▼                   ▼
        M1 artifact        M2 MoE compact       M3 MoE compact
        noun shape          prove                node verify
              └──────────────────┼───────────────────┘
                                 ▼
                 M5 size/latency · M6 k≠1024 · M7 adversarial
                                 ▼
                 M4 lift fail-closed guards  ── mainnet-enable MoE
                                 ▼
                 D4 / S1  Hoon↔Rust consensus wiring  (shared gate: no
                          %ai-pow block — dense or MoE — is accepted until
                          this lands; verifier is currently fail-closed)
```

- **D6 is upstream of MoE production**, not parallel — the MoE opened-schedule
  binding *is* a D6-shaped binding, which is why solving D6 for dense is most of
  the mechanism MoE reuses. Recommended global order across the whole track:
  D6 → MoE (this plan) → D4.
- **D4 (consensus wiring) is separate and shared:** even a fully-parity MoE
  compact prove/verify produces no accepted block until Hoon consensus calls the
  Rust verifier at the DoS-safe jam boundary (`inner.hoon:1631-1644` currently
  rejects `%ai-pow`). Sequence D4 after D6 is closed.

---

## 7. Status snapshot

| Item | State |
|---|---|
| MoE circuit + selective opening (diagnostic-L1) | ✅ done, validated |
| MoE routing-consistency binding | ✅ done (`verify_pearl_moe_routing_binding`, ~10 adversarial) |
| MoE opened-schedule binding (diagnostic-L1) | ✅ done (`l0_program_matches`) |
| **D6 / P0** compact opened-schedule binding | 🚧 attempted in code → concrete API wall (no shape-proof constructor; context needs real L0/L1 proofs). 3 options scoped (§4 P0). **Not fixed** — the invasive p3-recursion change is out of one session and half-landing is forbidden (R1). |
| **M1** MoE artifact noun | 🟡 opaque-nonce codec + DoS cap landed & tested (16 tests); builder/verify wiring remains (lands with M2/M3) |
| **M2** MoE compact prove | ❌ not started (compact pipeline dense-only) |
| **M3** MoE compact node verify | ❌ not started |
| **M5** MoE compact size/latency | ❌ not measured |
| **M6** k≠1024 keying | ⚠️ validated only for k=1024 |
| **M7** adversarial on compact | ❌ not ported |
| **M4** lift fail-closed guards | 🔒 gated on all above |
| **D4/S1** Hoon↔Rust consensus wiring | ❌ fail-closed (shared with dense) |

---

## 8. Reference index (file:line)

- **Compact pipeline (dense-only):** `crates/ai-pow-zk/src/recursion.rs`
  (prove/verify, context `:1865`, digest `:357`, verify `:1910`);
  `crates/ai-pow/src/zk_bridge.rs:1692` (`prove_pearl_merge_compact_...`).
- **Compact node verify + precheck:**
  `crates/ai-pow-miner/src/certificate_noun.rs:1867-1938` (compact),
  `:1646-2144` (precheck), `# Soundness` contract on `:1903`.
- **MoE (diagnostic-L1) logic:** `verify_pearl_moe_recursive_certificate`
  `zk_bridge.rs:1577`; routing binding `pearl_compat.rs:1460`; splice
  `fiat_shamir.rs:128`; `l0_program_matches` `recursion.rs:141`;
  `canonical_program_for_strip_schedule` `zk_bridge.rs:1662, 2117`.
- **Fail-closed guards:** `pearl_compat.rs:850, 929, 1087`; `params.rs:488`.
- **Pearl reference:** MoE fork height `pearl/node/chaincfg/params.go:319`; V2
  wire cap `pearl/node/wire/certificate_v2.go:17`; MoE splice
  `pearl/zk-pow/src/api/proof_utils.rs:33`.
- **Consensus gate:** `hoon/apps/dumbnet/inner.hoon:1631-1644` (fail-closed).
- **Companion docs:** `2026-07-13_ZK_POW_PRODUCTION_PUZZLE_VS_PEARL_AUDIT.md`
  (full discrepancy audit incl. D6 Part G); `2026-07-08_PEARL_PRODUCTION_RESIDUAL.md`
  (M1–M7 / S1 origin).
