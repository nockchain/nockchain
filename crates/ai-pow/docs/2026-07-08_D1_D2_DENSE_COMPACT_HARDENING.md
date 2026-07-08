# D1 + D2 — Dense compact certificate: size/latency + verify round-trip

**Date:** 2026-07-08
**Residual items:** D1 (compact size/latency re-validation), D2 (full compact
verify round-trip + wrong verifier-key digest rejection) from
`2026-07-08_PEARL_PRODUCTION_RESIDUAL.md`.
**Status:** in progress.

## Acceptance bar (from `crates/ai-pow-zk/docs/2026-06-07_COMPACT_RECURSIVE_PRODUCTION_PIPELINE.md`)

- Production artifact = the **compact** recursive certificate.
- Compact certificate bytes **≤ 150 000** (relaxed gate); jammed `%ai-pow`
  artifact **≤ 150 000**.
- Proving wall time **~30 s** (release + `-C target-cpu=native`).
- 60-bit proof-system security (relaxed FRI).

## Soundness criteria (D2)

The compact verifier binds a **verifier-key digest**. A node MUST reject a
certificate whose digest does not match the verifier-owned setup, and MUST reject
a non-canonical digest encoding. Without this, a compact proof can be replayed
against a different circuit/setup.

## Test coverage

`crates/ai-pow-miner/src/certificate_noun.rs::real_compact_pearl_merge_artifact_jam_size_for_selected_route`
(opt-in, release/native) already exercises the full path:

1. `prove_pearl_merge_compact_recursive_certificate` → compact cert (measures
   `prove_wall_ms` + the L1/L2 stage breakdown).
2. Build the `%ai-pow` noun artifact, `jam()` it, bounded-`decode` it.
3. Statement precheck: ticket + commitments + version + zk_params + found_idx +
   trace_height + public_inputs all equal the proven run.
4. Compact byte-node stays canonical postcard (encode == decode == re-encode).
5. `verify_decoded_..._compact_..._with_digest_bytes` with the pinned digest —
   accepts.
6. **Adversarial:** a wrong all-zero digest → `CompactVerifierKeyDigestMismatch`;
   a non-canonical (≥ Goldilocks modulus) digest →
   `CompactVerifierKeyDigestEncoding`.
7. Size gate: `compact_bytes.len() <= 150_000` and `jammed.len() <= 150_000`.

**Run:**
```sh
RUSTFLAGS="-C target-cpu=native" cargo test -p ai-pow-miner --release --features node \
  real_compact_pearl_merge_artifact_jam_size_for_selected_route -- --ignored --nocapture
```

## Caveat — parameter scale

The fixture uses `pearl_test_params` = `m=8, k=1024, n=8, noise_rank=64, tile=8`.
The compact L2 certificate size is essentially **tile-independent** (a fixed-shape
recursive SNARK), so the ≤150 KB size gate is representative. The **proving wall
time**, however, scales with the Layer-0 trace, which grows with the opened tile
(bounded by `PEARL_TRACE_BOUND = 2²²`). A production-scale measurement (a
larger Pearl tile, up to the trace bound) is required to confirm the ~30 s target
holds at scale — tracked as **D1a** below.

## Results (release + `-C target-cpu=native`, 2026-07-08, fixture `m=8,k=1024,n=8,tile=8`)

**PASS — meets the acceptance bar.**

| Metric | Value | Gate | Result |
|---|---|---|---|
| Compact certificate | **124,570 B (121.65 KiB)** | ≤ 150,000 | ✓ |
| Jammed `%ai-pow` artifact | **125,382 B (122.44 KiB)** | ≤ 150,000 | ✓ |
| Prove wall time | **28,320 ms (~28.3 s)** | ~30 s | ✓ |
| Wrong verifier-key digest | rejected (`CompactVerifierKeyDigestMismatch`) | must reject | ✓ |
| Non-canonical digest | rejected (`CompactVerifierKeyDigestEncoding`) | must reject | ✓ |

Stage breakdown: L1 build 184 ms, **L1 outer 17,744 ms** (dominant, ~fixed —
the recursive-verifier outer certificate), L2 prep 479 ms, L2 prove 1,847 ms,
L2 compact 1 ms, L2 compact-verify 17 ms. Layer-0 prove ≈ 8 s (remainder).

The compact certificate byte count (124,570) matches the pipeline doc exactly,
confirming the size is tile-independent (the L2 SNARK is fixed-shape).

**D1 (size) + D2 (verify round-trip + digest rejection) are met at the fixture
param.** The remaining question is the **wall time at production tile scale**
(D1a): L1-outer (~17.7 s) is roughly fixed, but the Layer-0 prove scales with the
opened tile's trace (bounded by `2²²`), so a larger Pearl tile pushes total wall
time up. Measured next.

## Follow-ups

- **D1a.** Add a production-scale compact size/latency measurement (largest Pearl
  tile within `PEARL_TRACE_BOUND`) to confirm the ~30 s wall-time target at scale,
  not just the small fixture.
- **D2a.** D2's digest-rejection is covered; keep it as the standing public-API
  soundness regression. Ensure it runs in whatever release CI gate is adopted.
