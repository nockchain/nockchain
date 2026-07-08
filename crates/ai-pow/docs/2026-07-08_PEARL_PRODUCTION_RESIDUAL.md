# Pearl-Compatible Merge-Mining — Production Residual (Dense + MoE)

**Date:** 2026-07-08
**Scope:** Everything remaining to make **both** the dense (V1/V2) and MoE
(grouped-GEMM) AI-PoW puzzles fully **Pearl-byte-compatible for merge-mining**
and **production-ready on Nockchain**, with the recursive certificate meeting the
production parameters documented in `crates/ai-pow-zk/docs/`.

This is a status + residual map, not a design doc. It is grounded in the current
code (function/file references are exact) and the production-parameter docs.

---

## 0. Production parameters — the acceptance bar

From `crates/ai-pow-zk/docs/2026-06-07_COMPACT_RECURSIVE_PRODUCTION_PIPELINE.md`,
`2026-06-07_OPEN_TEST_ISSUES.md`, and `crates/ai-pow/src/params.rs`:

| Parameter | Value | Source |
|---|---|---|
| **Production artifact** | the **compact** recursive certificate (`AiPowCompactBatchRecursiveCertificate`), **not** the Layer-0 proof and **not** the large batch-STARK checkpoint | pipeline doc |
| Artifact size | **≤ ~150 KB** (relaxed target); dense compact ≈ **122–124 KB** today | pipeline doc |
| Proving wall time | **~30 s** (release + `target-cpu=native`) | pipeline doc / open-issues |
| Proof-system security | **60 bits** (relaxed FRI) | pipeline doc |
| Hard trace bound | **`PEARL_TRACE_BOUND = 2²²`** rows | `params.rs:50` |
| Prove API | `prove_pearl_merge_compact_recursive_certificate[_with_prover_cache]` | pipeline doc |
| Verify API | `verify_compact_batch_recursive_certificate_with_context` via the jam/noun boundary | pipeline doc |

> The large checkpoint certificate (`prove_pearl_merge_recursive_certificate` →
> `AiPowRecursiveCertificate`) is explicitly a **regression/diagnostic** path and
> is "too large" for production. **Every MoE prove/verify validated so far uses
> that checkpoint path** — moving MoE onto the compact path is a first-class
> residual (see M2/M3).

---

## 1. What is complete (validated)

### Dense (Track A)
- V2-dense **byte-parity** with Pearl (commitment chain, `E`/`F` noise, 512-bit
  tile fold, keyed-BLAKE3 jackpot); MoE **fail-closed** at four guard layers;
  dense S0–S9 regression intact.
- Layer-0 + recursive certificate (checkpoint **and** compact) prove/verify; node
  precheck + verify at the Rust boundary
  (`certificate_noun.rs::verify_decoded_ai_pow_pearl_merge_artifact`).

### MoE (Track B) — circuit + soundness + selective opening
- **B1** routing canonicalization, **B2** routing-commitment splice `s_A`, **B3d**
  grouped tile / `outer_indices` gather — off-circuit, KAT'd against real Pearl
  vectors.
- **B5b** non-contiguous recursive opening, **B5c** grouped matmul, **B5d** Layer-0
  + checkpoint recursive prove/verify — the MoE grouped tile proves and the
  certificate verifies end-to-end.
- **Soundness verify**: routing-consistency binding
  (`verify_pearl_moe_routing_binding`, 10 adversarial tests), opened-schedule
  binding (`verify_pearl_moe_recursive_certificate` +
  `AiPowRecursiveCertificate::l0_program_matches`), and the §4.C.10
  malicious-miner adversarial for non-contiguous opening
  (`sec_4c10_noncontiguous_sweep_on_row_permuted_matrix_rejects`).
- **Selective disjoint-chunk opening** — MoE opens only the chunks its scattered
  routed-token rows touch (O(|rows|·log n)), not the O(max−min) covering range,
  keeping the Layer-0 trace inside `PEARL_TRACE_BOUND` at production scale
  (m=131 072 would have been 4.75M > 2²² rows with the covering range;
  selective is flat). **Byte-identical for contiguous tiles → dense path
  unchanged.** Validated as a unit: ai-pow-zk lib **405/0**, ai-pow **17
  binaries**; contiguous real prove byte-identical.

> **Key gap:** everything MoE above is validated on the **checkpoint** cert. The
> **compact** production path and the **node/consensus integration** are residual.

---

## 2. Dense residual — production hardening

- **D1. Compact size/latency re-validation** (post Plonky3 bump). Run the ignored
  production-size route in release/native and record bytes + wall time; confirm
  ≤150 KB and ~30 s:
  `cargo test -p ai-pow-miner --release --features node
  real_compact_pearl_merge_artifact_jam_size_for_selected_route -- --ignored --nocapture`
  (OPEN_TEST_ISSUES §1).
- **D2. Full compact verify round-trip.** Build a compact artifact, cue through
  the noun boundary, verify with verifier-owned setup, and **reject under a wrong
  verifier-key digest** (OPEN_TEST_ISSUES §2) — the most important public-API
  soundness regression.
- **D3. Prover-cache equivalence + stale-cache rejection**
  (`prove_pearl_merge_compact_recursive_certificate_with_prover_cache`,
  `into_prover_cache`) — warmed cache verifies; stale L1 metadata / changed FRI
  shape rejects (OPEN_TEST_ISSUES §3).
- **D4. Make the opened-rows binding an explicit, tested node invariant.**
  `verify_recursive_certificate` proves the statement for the certificate's *own*
  `l0_program`; soundness relies on the node precheck
  (`precheck_pearl_merge_certificate_metadata`) recomputing `found_idx` + the
  strip schedule + `trace_height` and binding them so a prover cannot embed a
  favorable-strip program. Add a direct adversarial test that a certificate
  proven over a different opened set is rejected at the node (the dense analogue
  of the MoE opened-schedule binding, which is already tested).

---

## 3. MoE residual — Track B "(B)" node integration

- **M1. MoE artifact shape + canonical noun encode/decode** in
  `certificate_noun.rs`: carry `PearlMoeParams` (expert_idx, routing_offsets,
  hash_routing, outer_indices) **plus** `routing_data`, with strict canonical
  encoding + DoS byte caps, mirroring the dense `PearlMergeAiPowArtifactShape`
  and its `precheck_..._jam` DoS boundary.
- **M2. High-level MoE prove path producing the COMPACT certificate.** Evaluate
  the MoE ticket (routing → splice `s_A` → grouped tile → jackpot), build the
  prover context with the MoE splice + `from_indices(outer_indices,
  expert-columns)`, and drive the **compact** pipeline
  (`prove_..._compact_...`) — not the checkpoint used in the current MoE tests.
  Selective opening flows through automatically (it is a Layer-0/trace change).
- **M3. MoE node verify branch.** Wire the MoE soundness verification into
  `verify_decoded_ai_pow_pearl_merge_artifact` for `e>0`: recompute `s_A` from the
  carried routing, run `verify_pearl_moe_routing_binding`, recompute + bind the
  MoE canonical program (`l0_program_matches` — already implemented in
  `verify_pearl_moe_recursive_certificate`), then verify the **compact**
  certificate (adapt from checkpoint `verify_recursive_certificate` to
  `verify_compact_batch_recursive_certificate_with_context`).
- **M4. Lift the `e>0` fail-closed guards** — `validate_pearl_merge_config_for_
  recursive_prover` (`pearl_compat.rs:1071`) and `parse_mining_config_trailer`.
  **Only after M1–M3 + the full adversarial suite pass on the compact path.**
  This is the mainnet-enabling step; land it **last**, staged, per R1.
- **M5. MoE compact size/latency validation** — a MoE tile's compact certificate
  must also meet ≤150 KB / ~30 s. The selective-opening trace is O(tile)
  (comparable to dense), so this should hold, but it must be **measured** on the
  compact route, not assumed.
- **M6. k≠1024 selective keying (correctness caveat).** Selective opening and the
  B5b sweep-lane formula (`a_indices[i] − ca0`) are validated for **k=1024** (all
  MoE / non-contiguous tests). Dense k≠1024 (e.g. Llama k=4096) uses the
  contiguous → byte-identical path and is unaffected. **If production MoE uses
  k≠1024 with scattered rows**, re-derive and re-validate the lane↔chunk keying:
  for k>1024 a row spans ⌈k/1024⌉ chunks, and the `noised_packed`
  producer key (`chunk_index − strip_c0`) vs the sweep consumer lane must be
  re-aligned (row-offset vs chunk-offset). Add k≠1024 scattered prove/verify
  coverage before relying on it.
- **M7. MoE adversarial on the compact path** — port the routing-binding,
  opened-schedule, and malicious-miner adversarial tests to the compact node
  boundary (they currently exercise the checkpoint path).

---

## 4. Shared consensus / merge-mining residual (dense + MoE)

- **S1. Hoon consensus wiring — the top production gate.** The Hoon `%ai-pow`
  surface is currently **fail-closed** and does **not** call the Rust verifier
  (`docs/ai-pow-integration/2026-06-01_PEARL_MERGE_MINING_COMPATIBILITY_SPEC.md`;
  OPEN_TEST_ISSUES). For real block acceptance, Hoon consensus must invoke the
  Rust verifier (dense **and** MoE) with the verifier-owned setup, at the
  DoS-safe jam boundary (`precheck_..._jam` caps bytes before cue). **Until this
  lands, no AI-PoW block — dense or MoE — is accepted by consensus.**
- **S2. Dual-submission merge-mining plumbing** ("in progress" per the compat
  spec): the Pearl-side submission and the Nockchain-side ticket must share the
  one attempt's commitments / tile-state / jackpot; the miner **must not** add a
  second Nockchain-only nonce into the attempt state (the forbidden shortcut).
- **S3. Milestone placeholders → production values**: `chain_id`,
  `target_epoch_or_height`, `extra_domain_data` (replay protection), operator
  timeout tuning, and the v1 reward pubkey-hash are currently milestone
  placeholders in the CLI / merge spec; finalize them for production.

---

## 5. Suggested landing order (each stage: KAT-first, full regression + adversarial gates, commit per validated stage)

1. **Dense compact hardening** (D1–D4) — closes the dense production path and pins
   the size/latency bar + the node opened-rows invariant.
2. **MoE compact prove/verify** (M1–M3, M5–M7) **behind the guards** — the MoE
   production path, fully adversarially validated on the compact route, while MoE
   stays fail-closed.
3. **Lift MoE guards** (M4) — mainnet-enable MoE, staged, only after (2) is green.
4. **Hoon consensus wiring** (S1–S2) — the block-acceptance gate for both puzzles.
5. **Production-parameter finalization** (S3).

---

## 6. One-line status

- **Dense**: byte-compatible and provable/verifiable at the Rust boundary; residual
  is production hardening (D1–D4) + consensus wiring (S1).
- **MoE**: circuit, soundness (routing + opened-schedule binding), and the
  production-scale selective opening are **done and validated on the checkpoint
  path**; residual is the compact/node integration (M1–M3), the mainnet guard-lift
  (M4), the compact size/latency + k≠1024 checks (M5–M6), and consensus wiring
  (S1). The MoE verify **logic** — including the soundness-critical opened-schedule
  binding — is implemented; (B) is wiring it through the compact artifact boundary.
