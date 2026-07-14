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
  onto the compact format**. The shared prerequisite **D6** (compact
  opened-schedule binding) is now **✅ DONE + validated** (program-commitment
  digest fold, §4 P0) — so the compact path binds the opened schedule for both
  dense and MoE.
- **Order:** ~~fix D6~~ **(done)** → ~~MoE compact prove (**M2**)~~ **(done)** →
  ~~MoE artifact codec + decode dispatch (**M1**)~~ **(done, encoder remains)** →
  ~~MoE compact verify logic + **the node verify branch** (**M3**)~~ **(DONE +
  validated end-to-end)** → ~~size/latency + k≠1024 + adversarial (**M5–M7**)~~
  **(done)** → **lift the last admission guard (M4 admission half)**. Actual
  mainnet acceptance additionally waits on the Hoon↔Rust consensus wiring
  (**D4/S1**), shared with dense.
- **Coupling finding (2026-07-13):** the M4 *envelope* guard (`sanity_check`) AND
  a second parse guard (`from_public_data`) both fail-close MoE and are
  **prerequisites** for the M3 node branch (the node can't reconstruct a MoE
  ticket's work while they reject it). Split + landed as `sanity_check_allowing_moe`
  + `from_public_data_allowing_moe` (dense paths byte-identical, not wired to
  admission). The *admission* guard stays last.
- **The MoE jackpot ≤ target binding is now BUILT + validated** (§4 M3):
  `verify_pearl_moe_compatible_work` recomputes the opened tile via
  `compute_moe_tile` and binds difficulty — the gate the recursive verify omits.
  Validated proof-free (cross-check vs `compute_pearl_moe_ticket`) AND through a
  real end-to-end node proof with a statement-derived kappa.
- **Rust-side MoE compact PARITY (with dense) is ACHIEVED + validated end-to-end.**
  MoE now does everything dense does on the compact path: prove
  (`prove_pearl_moe_compact_recursive_certificate`), node verify
  (`verify_decoded_ai_pow_pearl_merge_compact_moe_artifact`), artifact codec
  (encode+decode), and the jam-boundary verify entry — all exercised by a single
  real-proof test (mine-inputs → prove → build noun → jam → decode → node verify →
  jam-boundary verify) with adversarial rejects. The soundness-critical additions
  (jackpot/difficulty binding, P0/D6 opened-schedule fold) are validated.
- **What remains is SHARED with dense, not a MoE-parity gap** — two integration
  workstreams, both staged last per R1:
  1. **Miner enablement:** lift the stale MoE fail-close in
     `validate_pearl_merge_config_for_recursive_prover` **and** wire the MoE
     search/prove path into the mining loop (`run.rs`) — the dense miner loop
     dispatches dense proving; a MoE loop (routing/expert selection + the MoE
     prove entrypoint) is the miner-side counterpart. A miner workstream, not the
     puzzle's prove/verify parity.
  2. **D4/S1 consensus wiring:** the Rust jet target
     (`verify_ai_pow_block_artifact_jam`, which re-derives canonical A/B from the
     protocol seed — **no model distribution**) is **built + validated**. Remaining:
     a `params→(compact_context,digest)` verifier-setup builder + the Hoon jet +
     kernel-jam rebuild — **shared with dense**, all derivable (no missing infra).
     No `%ai-pow` block, dense or MoE, is accepted until the jet lands.
- **Nothing is mis-accepted today:** MoE is fail-closed at the admission guard,
  the new MoE envelope/parse/verify are not wired into admission, and no `%ai-pow`
  path is wired into consensus (Hoon rejects `%ai-pow`). This is a *build-forward*
  plan, not a live-vulnerability fix.

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

### P0 — D6 compact opened-schedule binding — *prerequisite, shared with dense* — ✅ **IMPLEMENTED + VALIDATED 2026-07-13 (program-commitment digest fold)**

> **DONE (2026-07-13).** The compact opened-schedule binding is implemented and
> validated end-to-end with real proving. The L0 program's preprocessed
> commitment is now folded into the L1 **statement digest** (the value the L2
> proves), and the node derives the **canonical** commitment witness-free from the
> opened schedule (never the prover) and folds the same value into its expected
> digest — so a certificate proven over a different program fails the digest check.
>
> - **Circuit** (`build_composite_l1_verifier_circuit`, `recursion.rs`): the Tip5
>   statement-digest sponge absorbs `air_public_targets[0] ‖
>   to_observation_targets(common_data.preprocessed.commitment)`; the expected
>   `statement_public_values` folds the matching value flatten (`get_values`).
> - **Verify** (`verify_compact_batch_recursive_certificate_with_context`): takes
>   the canonical commitment and folds it via
>   `compact_batch_l1_public_values_for_statement`.
> - **Node** (`certificate_noun.rs`): `verify_decoded_ai_pow_pearl_merge_compact_artifact_*`
>   rebuilds the canonical program from the precheck's opened schedule
>   (`work.ticket` → `canonical_program_for_strip_schedule`) and derives the
>   commitment via `canonical_l0_program_commitment_vals` (witness-free
>   `logup_common_for`) — never from the prover's context.
> - **Reachability primitive:** `CommonDataTargets::preprocessed_commitment()`
>   (plonky3-recursion). Flatten pair `to_observation_targets` (target) ↔
>   `get_values` (value) match by construction.
>
> **Validation (real proving):** (a) the compact round-trip
> (`compact_batch_recursive_certificate_round_trip_for_test_pearl`) verifies with
> the fold **and** a wrong-commitment cert is **rejected** (D6 adversarial),
> 21.99 s; (b) the full node round-trip
> (`real_compact_pearl_merge_max_envelope_size_and_latency`, production scale
> m=n=512/k=4096/r=64) verifies via the node's canonically-derived commitment,
> 47.68 s, cert 122.68 KiB (≤ 150 KB — the fold adds negligible size, confirming
> "not gigantic"). **MoE-aware for free** — a MoE canonical program is just a
> different `Program`.
>
> **Below: the original design (now implemented). Kept for the rationale.**

> **READ THIS FIRST — it corrects an earlier wrong conclusion.** A previous pass
> concluded P0 was "blocked at a concrete API wall" because *rebuilding the
> verifier context's `circuit_prover_data`* needs a real L0 `BatchProof` the
> compact cert omits (and `BatchStarkProof` has no shape-only constructor). **That
> framing was wrong about the fix.** You do **not** need to rebuild
> `circuit_prover_data`, and you do **not** need a shape-proof synthesizer or to
> re-prove L0. The sound fix binds one small, deterministic value.

**The actual situation.** The Layer-0 (composite) STARK is *already* program-pinned:
the verifier rebuilds the canonical program from the trusted per-block context and
derives its preprocessed commitment **witness-free** via
`ProverData::from_airs_and_degrees` — see `composite_proof.rs:343-348` and
`logup_common_for` (`composite_proof.rs:405-421`, `pub(crate)` precisely "so the
recursion integration can obtain the CommonData the recursive verifier needs").
The gap is only that the **compact recursion does not carry that pin**: in
`build_composite_l1_verifier_circuit` the L0 `common_data` (which holds the L0
program's preprocessed commitment) is packed as a **prover-supplied witness**
(`recursion.rs:742` `pack_values(..., proof, common_data)`), and the L1 statement
digest hashes only the L0 public values (`recursion.rs:713-737`,
`air_public_targets[0]`), **not** the commitment. So the L2 proves "the L0 proof
verified against *some* program whose commitment the prover chose."

**The fix (concrete, code-grounded, NOT gigantic):** fold the L0 program
commitment into the **statement digest** (the value the L2 actually proves — not
`verifier_key_digest`, which is only a metadata check and is *not* proof-bound),
and have the node derive the canonical commitment witness-free and fold the same
value into its expected digest. The commitment is tiny (`CompositeComm =
MerkleCapTargets<Val, DIGEST_ELEMS>`, `recursion.rs:557` — a few field elements),
deterministic from the public opened schedule, and **verifier-derived, not
transmitted** — so proof size is unchanged and nothing new rides the wire.

Exact edits:
1. **Reachability primitive (done):** `CommonDataTargets::preprocessed_commitment()`
   accessor added in `plonky3-recursion/recursion/src/types/proof.rs` (the target
   was `pub(crate)`; `CompositeComm: Into<Vec<ExprId>>` — verified it flattens).
2. **Circuit** (`build_composite_l1_verifier_circuit`, `recursion.rs:713-740`):
   extend the Tip5 statement-digest sponge to absorb
   `air_public_targets[0] ‖ flatten(common_data.preprocessed.commitment)`, and set
   `statement_public_values = statement_public_digest(public_values ‖
   commitment_vals)` (the value flatten of `common_data`'s commitment — the arg is
   the *value* `&CommonData`, so both sides are available).
3. **Node** (`statement_public_digest` / `compact_batch_l1_public_values_for_statement`,
   `recursion.rs:390-412`): absorb `60 PIs ‖ canonical_commitment_vals`, where
   `canonical_commitment_vals = flatten(logup_common_for(cfg, canonical_program,
   sx_bound).preprocessed.commitment)` — witness-free, using the program the node
   already rebuilds (`canonical_program_for_strip_schedule`).
4. **Correctness crux:** the value-flatten (node, `MerkleCap<Val, DIGEST_ELEMS>`)
   and the target-flatten (circuit, `MerkleCapTargets` `Into<Vec<ExprId>>`) MUST
   use the same digest order. Add a shared flatten helper and a KAT that a target
   built from a value flattens identically.
5. **MoE-aware for free:** the MoE canonical program is just a different
   `Program`; `logup_common_for` handles it. No MoE-specific work in P0 itself.

**Validation (real-proving gated — the reason this is one atomic unit):**
(a) the existing compact round-trip (`compact_batch_recursive_certificate_round_trip_for_test_pearl`,
`#[ignore]`) must still prove+verify; (b) **adversarial:** a cert proven over one
opened schedule verified against the canonical context for a *different* same-shape
schedule must **reject** (the D6/M7 test). Land circuit + node + both tests as one
commit; per R1 do not commit any subset that leaves the linchpin half-bound.

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
> **Decode plumbing landed (validated) 2026-07-13:** `PearlMergeAiPowArtifactShape`
> now carries `moe: Option<PearlMergeMoeArtifact>`, and
> `decode_ai_pow_pearl_merge_artifact_noun` **dispatches on the nonce tag** — an
> `AIM1` nonce decodes via `decode_pearl_merge_ai_pow_nonce_moe` into
> `Some(moe)`; a dense `AIP1` nonce is byte-identical and yields `None` (the only
> construction site touched is the decoder; dense path unchanged). This carries
> the MoE tail to the node so the `e>0` verify branch (M3) can reach the validated
> `verify_pearl_moe_compact_recursive_certificate`. Full miner suite green.
>
> **Remaining (still M1):** thread the MoE fields through the **artifact builders**
> (`build_ai_pow_pearl_merge_artifact_noun_from_ticket_*`) — encode side — so a
> full MoE artifact noun round-trips. Only needed to construct a MoE artifact
> *noun* end-to-end (the M3 verify branch can be validated directly on a
> `PearlMergeAiPowArtifactShape` without the noun encoder). MoE stays fail-closed
> at every admission gate throughout.

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

### M2 — MoE prove path that emits the *compact* cert — ✅ **DONE + VALIDATED 2026-07-13**
Evaluate the MoE ticket (routing → splice `s_A` → grouped tile → jackpot), build
the prover context with the MoE splice + `from_indices(outer_indices,
expert-columns)`, and drive the compact prover. Selective opening flows through
automatically (Layer-0/trace property).

> **DONE — the compact prover is program-generic.** The MoE Layer-0 (already built
> by `prove_ai_pow_scheduled_full_with_context` over the MoE strip schedule)
> wraps as a `ChainVerifiedCompositeProof` and drives
> `prove_compact_batch_from_verified_l0` directly — the same path as dense. P0's
> program-commitment digest fold is MoE-aware for free. Validated by
> `real_moe_compact_recursive_certificate_proves_and_verifies` (`zk_bridge.rs`,
> real proving, **26.38 s**): a MoE compact cert **proves + verifies** with its
> canonical program commitment, a **wrong commitment is rejected** (D6 binding for
> the MoE program = part of M7), and the cert is **125,237 bytes (≤ 150 KB)** —
> which also validates **M5** (size/latency) for MoE at this scale. MoE stays
> fail-closed at the node admission gates; this is the prove+recursion path.

### M3 — MoE node verify branch on compact — ✅ **DONE + VALIDATED END-TO-END 2026-07-13 (real MoE compact proof through the node boundary)**
Wire MoE soundness verification into
`verify_decoded_ai_pow_pearl_merge_compact_artifact` for `e > 0`.

> **DONE + VALIDATED — the full node MoE compact verify branch.**
> `verify_decoded_ai_pow_pearl_merge_compact_moe_artifact_with_context_and_limits`
> (`certificate_noun.rs`) runs: aux binding (dense-identical) → MoE-tolerant
> `public_data` parse + `verify_pearl_moe_compatible_work` (M4 envelope + routing
> binding + **jackpot/difficulty binding** via `compute_moe_tile`) → `MatmulParams`
> reconstructed from the **authenticated** statement dims (certificate `zk_params`
> required to agree) → `verify_pearl_moe_compact_recursive_certificate` (routing/PI/
> schedule binding + compact proof with the P0/D6 fold). The dense compact verify
> now rejects a MoE artifact explicitly.
>
> **Validated by a real end-to-end proof** (`real_moe_compact_pearl_merge_artifact_verifies_through_node_branch`,
> `certificate_noun.rs`, real proving 24.5 s, 124,206-byte cert): a MoE artifact
> with a **statement-derived `kappa`** (the node re-derives it from block header +
> config — the full chain the M2 kappa-fixed test does not exercise) proves,
> assembles into a `PearlMergeAiPowArtifactShape`, and verifies through the node
> branch. Adversarial: **forged routing**, **unmet difficulty**, and **routing a
> MoE artifact through the dense verify** all reject. Enabled by a new public MoE
> compact prove entrypoint `zk_bridge::prove_pearl_moe_compact_recursive_certificate`
> (M2 builder) + `PearlPublicProofParams::from_public_data_allowing_moe` (a second
> fail-closed guard — `from_public_data` — surfaced + MoE-tolerantly handled; dense
> parse byte-identical).

> **DONE + VALIDATED — the MoE compact verify LOGIC:**
> `verify_pearl_moe_compact_recursive_certificate` (`zk_bridge.rs`) is the compact
> counterpart of `verify_pearl_moe_recursive_certificate`: identical routing
> binding + expert-column recompute + routing-spliced `s_A`/PI binding, with the
> opened-schedule binding done via the **P0/D6 program-commitment digest fold**
> (`canonical_l0_program_commitment_vals`) instead of `l0_program_matches`.
> Validated by `real_moe_compact_recursive_certificate_proves_and_verifies` and
> `..._k_neq_1024` (real proving, k=1024 and k=4096): proves+verifies, node
> independently derives the same program commitment, wrong-commitment rejected,
> forged-routing rejected.

> **⚠️ Precise residual — the node verify BRANCH + the jackpot/target binding.**
> Two pieces remain to wire `verify_decoded_ai_pow_pearl_merge_compact_artifact_with_context_and_limits`
> for `artifact.moe.is_some()`:
>
> 1. **A MoE work precheck** (analogue of the dense `verify_pearl_compatible_work`,
>    which the dense branch runs via `precheck_ai_pow_pearl_merge_artifact_statement_with_context`).
>    It must NOT reuse the dense precheck — that goes through `sanity_check`, which
>    fail-closes MoE. It uses the new **`sanity_check_allowing_moe`** (M4, landed)
>    for the envelope, then: verify aux inclusion + `nock_block_commitment` +
>    `aux_commitment` (consensus binding, unchanged from dense — the
>    `verify_pearl_merge_mining_public_data` prefix before `sanity_check`);
>    recompute `kappa`/`h_a`/`h_b` via `derive_pearl_work_commitments(sigma, mu, a, b)`.
>
> 2. **The jackpot/target binding — soundness-critical, and the reason this can't
>    be rushed.** ⚠️ *Neither* `verify_pearl_moe_recursive_certificate` *nor*
>    `verify_pearl_moe_compact_recursive_certificate` checks
>    `hash_jackpot ≤ target`. For BOTH dense and MoE, the recursive verify binds
>    only matmul correctness + opened schedule + seeds (`s_A`/`h_A`/`h_B`/`kappa`
>    PIs); the **difficulty check is the node precheck's job** (dense does it in
>    `verify_pearl_compatible_work`: `verify_pearl_pattern_ticket` recomputes the
>    opened tile → `jackpot_hash`, asserts `== public_params.hash_jackpot`, then
>    `hash_le_target`). The MoE branch must do the same via the MoE tile:
>    recompute the opened tile with **`compute_moe_tile`** from the schedule the
>    verify already derives — `outer_indices` (public, in `artifact.moe`),
>    `b_cols_global` (from `moe_expert_b_cols_global` on the public column pattern),
>    and the recomputed `s_A`/`s_B` — take its `jackpot_hash`, assert
>    `== public_params.hash_jackpot`, then `hash_le_target(jackpot, nockchain_adjusted_target)`.
>    Soundness of this binding rests on the PI chain: `h_A = commit(a)` and
>    `h_B = commit(b)` (PIs) pin the node's matrices to the proven ones, and the
>    program-commitment fold pins the opened rows/cols — so the node's tile
>    recompute *is* the proven tile. A bug here would admit a valid MoE proof that
>    does **not** meet difficulty (or reject valid ones): a consensus break that
>    compiles clean and passes every non-proving unit test. Per R1 it must land
>    with its validation, not before it.
>
> **What its validation requires (the concrete wall this session hit):** a full
> **MoE merge-mining statement fixture** — a `block_header` + MoE `public_data`
> (mining_config with `e>0`, `m`/`n`/`t_rows`/`t_cols`) + `aux` + `aux_inclusion`
> + a `nockchain_target` the recomputed MoE jackpot actually meets — paired with a
> real MoE compact certificate (from M2's `prove_compact_batch_from_verified_l0`
> path, ~26 s). No such MoE statement fixture exists yet (the dense fixtures are
> dense-only, and the M2 test validates `verify_pearl_moe_compact_recursive_certificate`
> at the kappa/h_a/h_b level, below the statement layer). Building that fixture so
> the recomputed jackpot meets the target — and the aux commitment binds — is the
> next validated stage. The branch is directly testable on a
> `PearlMergeAiPowArtifactShape` (no noun encoder needed).

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

### M4 — lift the `e>0` fail-closed guards — split into the envelope guard (done, a prerequisite) and the admission guards (LAST)
> **Re-scoped 2026-07-13 (a coupling finding).** The node MoE verify branch (M3)
> must run an envelope check that *accepts* a MoE config before it can reach
> `derive_pearl_work_commitments` + the validated compact verify. But the
> `sanity_check` MoE guard (was `pearl_compat.rs:929`) fail-closes at the first
> line. So the **envelope guard is a hard prerequisite for M3, not a follow-on**.
>
> **Envelope guard — DONE + VALIDATED 2026-07-13.** Split the shared dense
> dimension/pattern envelope into a private `envelope_check_dims`; the dense
> `sanity_check` is **byte-identical** (still fail-closes MoE) and a new
> **`sanity_check_allowing_moe`** runs the identical envelope but validates the
> MoE config bounds (`e ≤ 1024`, `0 < top_k < e`) instead of rejecting. It is
> **not wired to admission** — reachable only from the (still-to-land) M3 branch,
> which additionally requires the full recursive certificate — so no MoE ticket
> can be admitted on the envelope alone. 6 new fast tests
> (`pearl_moe_fail_closed.rs`); dense regression green (81/81 across
> compat+routing+wire). The D3 `h·w ≤ 256` per-tile cap is inside the shared
> envelope, so it now applies to MoE too.
>
> **Parse guard — DONE + VALIDATED 2026-07-13 (a second fail-close, surfaced by M3).**
> `PearlPublicProofParams::from_public_data` ALSO fail-closes MoE (bytes 20..22
> discriminant), independently of `sanity_check` — the node MoE branch hit it after
> the envelope was lifted. Split the shared 164-byte core parse into
> `from_public_data_core`; added `from_public_data_allowing_moe` (MoE-tolerant, used
> by the node MoE branch). Our MoE `public_data` is the SAME 164-byte core with the
> MoE trailer in the mining-config (routing lives in the artifact nonce), so this is
> a pure parse-gate lift; dense `from_public_data` is byte-identical.
>
> **Admission guard — still LAST (mainnet-enabling), and now the *only* MoE
> fail-close left.** `validate_pearl_merge_config_for_recursive_prover`
> (`pearl_compat.rs:~1184`, `UnsupportedRecursivePearlParams("MoE … not
> implemented")`) is the miner/block-acceptance config guard (called from
> `ai-pow-miner/run.rs:295` + `ai_pow_mine.rs:262`). **Its comment is now stale** —
> the recursive certificate *does* bind the routing commitment + grouped matmul
> (M2/M3, validated). Lifting it lets the miner prove+submit MoE. The node VERIFY
> path does NOT go through it (M3 is already complete), so this guard is purely the
> admission/mining policy. The FP8 guard (`params.rs:488`) is a model-quantization
> concern (Llama FFN group split), orthogonal to the MoE compact puzzle. These stay
> closed until D4/S1 (Hoon↔Rust consensus wiring, shared with dense) is ready;
> lifting them is the final staged step, per R1.

### D4/S1 — Hoon↔Rust consensus wiring — 🟡 **Rust jet target BUILT + validated; remaining is the verifier-setup builder + the Hoon jet (shared with dense)**

> **⚠️ Correction (2026-07-13): an earlier revision of this section wrongly called
> D4 a "model-distribution wall." That was wrong and is retracted.** The matrices
> are **not** external model weights: the production miner synths them
> deterministically from a **constant seed** (`ai_pow_mine.rs`:
> `synth_matrices(DEFAULT_SYNTH_SEED="ai-pow-prod-v1", params)`), so a consensus
> node **re-derives the identical `A`/`B`** from the seed + the block's `params`.
> No model distribution is needed. The `run.rs` "local-state inputs" framing +
> `synth.rs`'s stale "real miners supply their own A/B" comment misled the first
> pass; the production default is synth, and canonical-synth `A`/`B` is exactly
> what AI-PoW soundness requires (else a miner grinds favorable matrices).

Block-level acceptance of any `%ai-pow` block (dense **or** MoE) is gated in
`inner.hoon`:
- `check-pow` (`~line 1146`): `?: ?=([%ai-pow *] u.pow) %.n` — block validation
  fail-closes `%ai-pow`.
- `do-pow` (`~line 1631–1644`): the `%ai-pow` arm has the activation gate but ends
  `'%ai-pow verifier not wired; rejected'`.
Both are **sound today** — a stub returning `%.y` would let a forged block pass, so
the fail-close is correct until the real verifier is wired.

**DONE this session — the Rust jet target.**
`verify_ai_pow_block_artifact_jam` (`certificate_noun.rs`) is the single
self-contained entrypoint the Hoon jet calls: it decodes the jammed artifact,
reconstructs `m`/`k`/`n` from the authenticated statement, **re-derives the
canonical `A`/`B`** from `AI_POW_PROD_SYNTH_SEED` (now a shared protocol constant
in `ai_pow::synth`, no external input), dispatches on the `AIM1`/`AIP1` tag, and
runs the node verify branch. Validated end-to-end (real MoE proof): a block
verifies through it **without the matrices being supplied** (it re-derives them);
unmet difficulty rejects.

**Precise residual (exact, actionable) — all derivable, none "missing infra":**
1. **Verifier-setup builder** (`compact_context` + `verifier_key_digest`) — ✅
   **soundness VALIDATED 2026-07-13.** These are produced as a by-product of proving
   (`recursion.rs:1942`), and the test
   `moe_compact_verifier_setup_is_proof_independent` proves they are
   **proof-INDEPENDENT**: two blocks of identical shape but different `kappa` yield
   the SAME `verifier_key_digest`, and block A verifies against block B's context.
   So a node builds the setup **once at startup** (prove one canonical block; the
   prove entrypoints already return `(context, digest)`) and reuses it for every
   same-shape block. **Shared with dense** (puzzle-variant-agnostic; the per-block
   schedule is bound separately via the P0 commitment fold). What remains is only
   the thin startup wrapper + choosing the canonical production shape.
2. **The Hoon→Rust jet** wrapping `verify_ai_pow_block_artifact_jam` — **the
   remaining consensus linchpin**, and (investigated 2026-07-13) a **multi-crate
   VM/consensus project**, not a one-file wire. Concrete architecture:
   - **A new jet crate is required.** `zkvm-jetpack` (where the STARK jets live,
     `hot.rs` → `Vec<HotEntry>`) depends **only on `nockvm`** — it cannot reach
     `ai-pow-miner`'s verify logic. So the AI-PoW verify jet needs a **new crate**
     (e.g. `ai-pow-jets`) depending on `ai-pow-miner` + `nockvm`, exposing
     `produce_ai_pow_hot_state() -> Vec<HotEntry>` (no circular dep — `ai-pow-zk`
     does not use `zkvm-jetpack`).
   - **Wire it into the kernel hot state.** `crates/nockchain/src/lib.rs` boots the
     dumbnet kernel with `hot_state: &[HotEntry]`; the new jet set must be appended
     there.
   - **The jet function** decodes the noun args (jammed artifact + block
     commitment + target), obtains the **cached setup** (#1, built once — a jet is
     stateless, so the context lives in Rust-side global/once state), calls
     `verify_ai_pow_block_artifact_jam`, returns a boolean noun.
   - **Jet-consistency.** A `~/`-hinted Hoon arm (`++ai-pow-verify`) must formally
     match the Rust jet's result, or the jet is unsound. `check-pow`/`do-pow`
     (`inner.hoon`) call it, replacing the two fail-closes (+ activation gate +
     target/commitment plumbing from consensus state).
   - **Rebuild + validate.** Rebuild the kernel jam → binary (per
     `hoon-jam-builds`), then a `roswell` integration test that a real `%ai-pow`
     block is accepted and a forged one rejected — KAT-first on the jet function.
   **Shared with dense** — dense `%ai-pow` verify has the identical unwired-jet gap;
   this is whole-feature consensus integration, not a MoE-parity task. Per R1 it is
   soundness-critical invasive work to land in careful validated stages, not a
   session-tail rush (a mis-matched jet or mis-cached setup = forgeable blocks).

   > **⚠️ Decisive architecture fork (investigated 2026-07-13) — the jet is NOT a
   > drop-in.** nockchain's existing consensus verify (`check-pow` → `verify:nv`,
   > `/common/nock-verifier`) is a **Hoon** STARK verifier (the STARK engine is
   > ~8,800 lines of Hoon in `ztd/*.hoon`) with `zkvm-jetpack` jetting only the
   > *primitives* (field ops, NTT, Tip5) — **semantically transparent** (Hoon is the
   > spec; jets just accelerate). AI-PoW's compact **recursive**-STARK verify is a
   > large Rust-only system (recursion + FRI + batch STARK). To follow the same
   > consensus pattern it needs EITHER **(a)** a Hoon recursive-STARK verifier
   > (re-implementing `ai-pow-zk`'s verifier in Hoon — comparable in scale to the
   > existing multi-thousand-line Hoon STARK engine; a major project), OR **(b)** a
   > **jet-required** consensus arm with no Hoon equivalent — which breaks the
   > transparency model every current nockchain verifier uses and needs a
   > consensus-framework policy decision (are jet-required arms allowed in block
   > acceptance?).
   >
   > **DECIDED (user, 2026-07-13): Branch (b)** — a full Rust verify jet with a
   > stubbed Hoon arm, *provided the data representation stays somewhat transparent
   > to Hoon* (the artifact sample is the structured `ai-pow-artifact` noun; only
   > the opaque nonce + cert body are byte-atoms; the result is a loobean).

   **Branch (b) Stage 1 — DONE + VALIDATED 2026-07-13 (the Rust verify jet).** New
   crate `ai-pow-jets`:
   - `ai_pow_verify_jet(context, subject)` — sample `[artifact commit target]`,
     result loobean. Re-derives canonical `(A,B)` from the protocol seed
     (non-grindable), dispatches on the tag, verifies.
   - `ai_pow_verify_with_setup` — the load-bearing core, unit-testable: slot axes +
     atom extraction + `decode_ai_pow_pearl_merge_artifact_noun` +
     `verify_ai_pow_block_artifact`. Malformed sample → `JetErr` (legit Hoon
     fallback); invalid block → `Ok(false)`, never a jet failure.
   - proof-independent setup boot-injected via `init_ai_pow_verifier_setup`.
   - `produce_ai_pow_hot_state()` — the `HotEntry` set.
   Validated by a real-proof KAT (~24 s): a real MoE `%ai-pow` block verifies
   through the jet core via the transparent noun sample; wrong commitment + unmet
   difficulty reject. `verify_ai_pow_block_artifact_jam` refactored to expose the
   shape-based `verify_ai_pow_block_artifact` (no re-jam).

   **Branch (b) Stage 2 — boot setup builder DONE + validated 2026-07-13.**
   `ai_pow_jets::setup::build_verifier_setup(params, hw, e, top_k)` proves one
   canonical block and returns the `(context, digest)` for
   `init_ai_pow_verifier_setup`; `prove_canonical_moe_block` is the shared canonical
   block. Sound because the setup is proof-independent (validated). The jet-core KAT
   now runs through it (real proof ~24 s).

   **Branch (b) Stage 2 — remaining: the Hoon consensus-kernel change — one
   ALL-OR-NOTHING validated unit (investigated to the implementation boundary
   2026-07-13; all Rust it calls is built + validated).**

   > **⚠️ Coupling + stub-safety finding — why this cannot be partially landed.**
   > The jet-required stub must be `!!` (crash), NOT `%.n`: if the stub were `%.n`,
   > a node lacking the jet would *reject* `%ai-pow` while a jetted node *accepts*
   > it — a **consensus split**. With `!!`, a node lacking the jet **crashes**
   > (fail-safe: it halts rather than forking). But that means rebuilding the kernel
   > jam with the `check-pow` wiring **without** also injecting the boot setup makes
   > the jet `BAIL_FAIL` (setup uninit) → fall through to `!!` → **consensus crashes
   > on every `%ai-pow` block** — strictly worse than today's `%.n`. Therefore the
   > Hoon arm + jet registration + boot setup-inject + jam rebuild must land
   > together, validated by the integration test. No safe partial landing exists;
   > per R1 this is committed only once the integration test is green.

   Exact wiring (verified against `inner.hoon` + `page:t`; **attempted 2026-07-13,
   hoonc-compiled, hit the representation wall below**):
   - `check-pow` (`inner.hoon ~1146`): replace `?: ?=([%ai-pow *] u.pow) %.n` with
     a call to `ai-pow-verify` on `u.pow` (`[%ai-pow ai-pow-artifact]`, the
     transparent artifact noun) + the page's commitment + target.
   > **⚠️ Soundness-critical representation binding (the concrete wall the attempt
   > surfaced).** The naive `[commit=@ target=@]` sample does NOT type-check: the
   > page's `target` is a **structured `bignum:bn`** (`ztd/three.hoon`), and
   > `block-commitment:page:t` is a **tip5 hash**, not a bare atom. The Hoon arm
   > must convert BOTH to the exact **32-byte little-endian atoms** the Rust verify
   > expects (`candidate_nock_block_commitment` = `aux.nock_block_commitment`;
   > `nockchain_target` = the 256-bit target LE). A wrong conversion (endianness,
   > limb order, hash serialization) mis-verifies — accepts a wrong block or rejects
   > a valid one. This binding must be pinned + validated by the integration test;
   > it is exactly the soundness-critical detail R1 says not to rush. The jet's
   > `atom_to_32` assumes a ≤32-byte LE atom, so the conversion lives in the Hoon
   > arm (or the jet gains a `bignum`/hash decoder — the Hoon-side conversion keeps
   > the jet interface simple + transparent).
   - The stubbed `++ai-pow-verify` arm (`~/ %ai-pow-verify`, body `!!`) as a sibling
     of `check-pow`; **set the jet path** in `produce_ai_pow_hot_state` to that
     arm's `~%` parent chain (validate at runtime — a mis-chained hint prints).
   - Register `produce_ai_pow_hot_state` in `nockchain/src/main.rs` (extend
     `produce_prover_hot_state()`); at boot call
     `ai_pow_jets::setup::build_verifier_setup(..)` → `init_ai_pow_verifier_setup`.
     **Blocker: the boot setup needs the FINALIZED production puzzle shape**
     (`params`/`hw`/`e`/`top_k`) — not pinned yet (pre-activation, height 95k). The
     boot builder is written + validated; it just needs the production constants.
   - Rebuild the jam: `make assets/dumb.jam` (`hoonc … hoon/apps/dumbnet/outer.hoon hoon`),
     then the `nockchain` binary.
   - Integration test: boot the kernel (jet + setup injected), submit a real
     `%ai-pow` block → accepted; a forged one → rejected.
   Shared with dense (the jet dispatches on the tag; both variants covered). This is
   a coupled multi-slow-step (jam rebuild + ~25 s boot prove + block fixture)
   consensus change — the maximal correct subset (all Rust) is landed + validated;
   this unit is the precise residual, per R1.

Items 1–2 are **shared with dense** (the block, the setup, and the jet are
puzzle-variant-agnostic except the tag dispatch, which is done). The
**Rust side of the boundary — canonical `A`/`B` derivation AND the proof-independent
setup — is complete + validated for both dense and MoE**;
this residual is entirely the consensus-integration + model/trusted-setup
infrastructure.

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
| **D6 / P0** compact opened-schedule binding | ✅ **IMPLEMENTED + VALIDATED** — program-commitment **digest fold** (§4 P0). Circuit folds the L0 program commitment into the L1 statement digest; node derives the canonical commitment witness-free from the opened schedule and binds it. Validated with real proving: honest round-trip verifies + wrong-commitment rejects (21.99 s); full node round-trip at production scale (47.68 s, 122.68 KiB). D6 gap closed. |
| **M1** MoE artifact noun | ✅ **DONE** — codec + DoS cap (16 tests), decode dispatch (`AIM1`→`moe: Some`), **and the encoder** (`build_ai_pow_pearl_merge_moe_artifact_noun_from_node`). Full encode→jam→decode→verify round-trip validated in the end-to-end real-proof test; dense `AIP1` byte-identical |
| **Jam boundary** (consensus dispatch point) | ✅ **DONE** — `verify_ai_pow_pearl_merge_compact_moe_artifact_jam_with[_digest_bytes]_and_context`: cue+decode dispatches on the `AIM1` tag → node MoE branch. Validated from jammed bytes end-to-end; the dense jam entry rejects a MoE artifact |
| **M2** MoE compact prove | ✅ **done + validated** — MoE Layer-0 wraps + drives `prove_compact_batch_from_verified_l0` (program-generic); real proving 26.38 s, 125,237-byte cert |
| **M3** MoE compact node verify | ✅ **DONE + VALIDATED END-TO-END** — `verify_decoded_ai_pow_pearl_merge_compact_moe_artifact` runs aux binding + MoE envelope/parse + routing binding + **jackpot/difficulty binding** (`compute_moe_tile`, the difficulty gate the recursive verify omits) + `verify_pearl_moe_compact_recursive_certificate`. Real end-to-end proof through the node boundary with a **statement-derived kappa** (24.5s, 124 KB); forged-routing + unmet-difficulty + dense-verify-on-MoE all reject. |
| **M5** MoE compact size/latency | ✅ measured — 124–125 KB / ~24–26s at m=64–128,k=1024; production-scale MoE still to measure |
| **M6** k≠1024 keying | ✅ **validated** — MoE compact prove+verify at k=4096 (row spans 4 chunks); node-commitment binds; adversarial rejects (real proving 45.47s) |
| **M7** adversarial on compact | ✅ wrong-commitment + **forged-routing** + unmet-difficulty rejects validated on the compact verify logic AND the end-to-end node branch |
| **M4** envelope + parse guards | ✅ **done + validated** — `sanity_check_allowing_moe` + `from_public_data_allowing_moe` (both prerequisites for M3; dense paths byte-identical; 6+ tests, regression green). **Admission guard** (`validate_pearl_merge_config_for_recursive_prover`, comment now stale) 🔒 still LAST — the only MoE fail-close left |
| **D4/S1** Hoon↔Rust consensus wiring | 🟡 **Branch (b) chosen; Stage 1 DONE + validated** — new `ai-pow-jets` crate: the Rust verify jet (`ai_pow_verify_jet`) with a transparent `[artifact commit target]` sample, canonical-A/B re-derivation, tag dispatch, proof-independent boot-injected setup. Real-proof KAT green (block accepts; wrong commitment + unmet difficulty reject). **Stage 2 remaining:** boot setup builder + stubbed Hoon `++ai-pow-verify` arm/hint + jet-path fix + `check-pow`/`do-pow` wiring + kernel-jam rebuild + roswell integration test. Hoon fail-closes correct until Stage 2 lands |

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
