# D1 + D2 — Dense compact certificate: size/latency + verify round-trip

**Date:** 2026-07-08
**Residual items:** D1 (compact size/latency re-validation), D2 (full compact
verify round-trip + wrong verifier-key digest rejection) from
`2026-07-08_PEARL_PRODUCTION_RESIDUAL.md`.
**Status:** in progress.

## Measurement methodology (required for an accurate read)

Wall-time reads are only meaningful **fully parallelized on native codegen**,
matching how a production miner runs:

- **Native codegen:** `RUSTFLAGS="-C target-cpu=native"` + `--release`.
- **Full parallelization:** `RAYON_NUM_THREADS=<cores>` (the batch-STARK prover
  parallelizes via rayon; it defaults to `available_parallelism()` but we set it
  explicitly), and `--test-threads=1` so the single heavy test owns all cores.
- **Reference machine:** this box = **12 logical cores** (`hw.logicalcpu`). Wall
  times below are the 12-core read; a production miner with more cores is faster,
  fewer is slower. The **certificate byte size is machine-independent**.

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

## D1a — production-scale reads (12 cores, native, `RAYON_NUM_THREADS=12`)

Test `real_compact_pearl_merge_prod_scale_m_size_and_latency`.

**Production default tile with a REAL strip-opening merkle proof** (m=n=512,
tile=8, k=1024, r=64 — the miner default tile over a 512-row model matrix, opening
8 of 512 chunks):

| Metric | Value | Gate | Result |
|---|---|---|---|
| Compact certificate | **124,484 B (121.57 KiB)** | ≤ 150,000 | ✓ |
| Prove wall time | **28,784 ms (~28.8 s)** | ~30 s | ✓ |
| `available_parallelism` | 12 | — | (fully parallel) |
| **trace_height** | **8,192** | ≤ 2²² | ✓ |

Stage breakdown matches the fixture (L1-outer 17,727 ms). **Decisive finding:**
the trace height is **8192 (the `MIN_STARK_LEN` floor) for both m=8 and m=512** —
the strip-opening auth siblings are 0-row, so a deeper merkle tree does **not**
grow the Layer-0 trace. The production default (tile=8, k=1024) is therefore
**dominated by the ~fixed L1-outer recursion (~17.7 s)** and comfortably meets
both bars regardless of matrix size `m`.

### Envelope headroom — MEASURED (the critical finding)

Test `real_compact_pearl_merge_max_envelope_size_and_latency`, max in-circuit tile
(tile=16 = `PEARL_HW_MAX`, k=4096, r=64, num_stripes=64=`STRIPE_MAX`):

| Metric | Value | Gate | Result |
|---|---|---|---|
| Compact certificate | **125,579 B (122.64 KiB)** | ≤ 150,000 | ✓ |
| Prove wall time | **95,100 ms (~95.1 s)** | ~30 s | ✗ **3.2× over** |
| trace_height | **65,536** (8× default) | ≤ 2²² | ✓ |
| L1-outer | 18,558 ms (~fixed) | — | — |

**Full scaling picture (12 cores, native, fully parallel):**

| Config | trace | wall time | ~30 s bar | compact size |
|---|---|---|---|---|
| default tile=8, k=1024 (m=8 fixture) | 8,192 | 28.3 s | ✓ | 121.65 KiB |
| default tile=8, k=1024 (m=512, real merkle) | 8,192 | 28.8 s | ✓ | 121.57 KiB |
| **max-envelope tile=16, k=4096** | **65,536** | **95.1 s** | ✗ | 122.64 KiB |

The **compact byte size is flat (~122 KiB) across the whole envelope** (the L2
SNARK is fixed-shape). The **wall time scales with the Layer-0 trace**: at 8× the
trace the Layer-0 prove grows ~9× (L1-outer stays ~fixed at ~18 s), pushing the
total to ~95 s. So the **~30 s latency bar holds only for the production default
(tile=8, k=1024)** and is exceeded ~3× at the top of the admitted envelope.

## Conclusion

- **D1 (size)** — MET: compact ≈ 121.6 KiB, jammed ≈ 122.4 KiB, both machine-
  independent, well under 150 KB.
- **D1 (latency)** — MET for the production **default** (tile=8, k=1024): ~28.8 s
  on 12 cores/native, with a real merkle proof; dominated by fixed recursion.
- **D2 (verify round-trip + digest rejection)** — MET (wrong + non-canonical
  digest both reject).
- **Latency action item (D1b):** the ~30 s bar is a **config policy** decision,
  not a fixed property. It holds at the miner default (tile=8, k=1024) and is
  exceeded ~3× at the top of the envelope (tile=16, k=4096 → ~95 s). **The
  merge-mining acceptance policy must either (a) pin the admitted merge config to
  tile=8/k=1024 so ~30 s is guaranteed, (b) explicitly widen the latency target
  for larger configs, or (c) optimize the Layer-0 prove for larger traces.** Since
  the Pearl mining config is *verifier-supplied* (chain-pinned via `job_key`), the
  clean answer is (a): constrain the accepted `PearlMiningConfig` envelope at the
  consensus boundary. This must be decided before MoE (whose tiles are also
  bounded by the same `PEARL_HW_MAX`/`STRIPE_MAX`).

## Follow-ups

- **D2a.** D2's digest-rejection is the standing public-API soundness regression;
  ensure it runs in the release CI gate.
- **D1b.** Decide the config-envelope policy (above) and encode it as a consensus
  admission check; add a release latency gate for whatever tiles/k are admitted.
