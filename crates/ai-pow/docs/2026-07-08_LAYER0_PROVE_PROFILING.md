# Layer-0 batch-STARK prove — profiling + speedup

**Date:** 2026-07-08
**Goal:** Profile and speed up the **Layer-0 batch-STARK prove**
(`composite_prove_pinned_logup` → `p3_batch_stark::prove_batch`), the component
that dominates recursive-certificate wall time at production scale.

## Why this is the primary work item

From `2026-07-08_D1_D2_DENSE_COMPACT_HARDENING.md`: the compact certificate size
is flat (~122 KiB), but the **wall time scales with the Layer-0 trace**. Pearl's
own `PearlCircuitParams.stark_degree_bits = 13..18` — production traces span
2¹³–2¹⁸ (Pearl mines Llama-3.3-70B, k = 8192/28672). Our measured wall times
(12-core, native, fully parallel):

| trace | Layer-0 prove | total compact | ~30 s bar |
|---|---|---|---|
| 2¹³ (miner default) | ~8 s | 28.8 s | ✓ |
| 2¹⁶ (max envelope) | ~74 s | 95.1 s | ✗ 3.2× |
| 2¹⁸ (Pearl max) | ~300 s+ (extrap.) | — | ✗ ~10× |

L1-outer (~18 s) and L2 (~2 s) are ~fixed; **the Layer-0 prove is the scaling
bottleneck.** Cutting it is what brings production degrees under the bar.

## Profiling methodology (Tracy)

p3-batch-stark 0.5.3 is instrumented with `tracing` `#[instrument]` / `info_span!`
(commit, `compute quotient`, FRI, open). The `tracing-tracy` bridge streams those
spans to a Tracy server the process opens on `127.0.0.1:8086`; `tracy-capture`
records and `tracy-csvexport` dumps per-zone timing.

**Isolation:** the harness `profile_layer0_prove_max_envelope`
(`zk_bridge.rs`, opt-in) proves a **single 2¹⁶-trace tile** (tile=16, k=4096,
r=64) with a `TracyLayer` subscriber and **no L1/L2 wrap**, so the zones are pure
Layer-0 (no L1 recursion mixed in).

```text
SC=<scratch>; tracy-capture -o $SC/l0.tracy -f -a 127.0.0.1 & sleep 2
RUSTFLAGS="-C target-cpu=native" RAYON_NUM_THREADS=12 cargo test -p ai-pow \
  --release --features zk profile_layer0_prove_max_envelope -- --ignored --nocapture --test-threads=1
wait; tracy-csvexport $SC/l0.tracy > $SC/zones.csv
```

CSV columns: `name, src_file, src_line, total_ns, total_perc, counts, mean_ns,
min_ns, max_ns, std_ns`. Sort by inclusive % (col 5). (Per the profiling skill:
inclusive double-counts recursion — cross-check leaf-ish zones by counts×mean;
and confirm the traced fraction ≈ wall time before trusting the ranking.)

## Results (isolated Layer-0, 2¹⁶, 12-core native, Tracy)

Harness `profile_layer0_prove_max_envelope`: trace_height = 65,536, **wall =
65.7 s**, traced fraction ≈ 94% (ranking is trustworthy).

**The Layer-0 prove is COMMIT-BOUND.** Top zones (inclusive %):

| zone | time | % | what |
|---|---|---|---|
| `first digest layer` | — | **83.6%** | Merkle **leaf hashing** (child of the builds below) |
| `build merkle tree [1917×2²⁰]` | 41.3 s | **63.1%** | commit the **main trace** (1917 cols × LDE 2²⁰) |
| `build merkle tree [588×2²⁰]` | 13.0 s | **19.8%** | commit the **LogUp permutation** trace (588 cols) |
| `coset_lde/dft` (LDE) | ~6 s | ~9% | low-degree extension |
| `compute quotient` | 1.66 s | 2.5% | quotient poly |
| `build merkle tree [20×2²⁰]` | 0.65 s | 1.0% | commit the quotient |
| `FRI prover` + `commit phase` | ~0.9 s | **0.7%** | FRI folding + queries |

So ~**84% of Layer-0 is the Merkle commitment** (leaf-hashing a ~2,525-column ×
2²⁰ matrix); **FRI is only 0.7%** and quotient ~2.5%.

### The two commit-cost multipliers

commit cost ≈ (trace width) × (**2^log_blowup** × trace rows) × hash/elem.

1. **`TOTAL_TRACE_WIDTH` = `MSG_PAIR_SEL_END` ≈ 1,917** columns (+ ~588 LogUp aux).
   The composite AIR is very wide.
2. **`log_blowup = 4` ⇒ 16× LDE** (`CircuitConfig::PROD`, `circuit.rs:133`). The
   committed matrix is 2²⁰ for a 2¹⁶ trace. This is a *deliberate* choice: PROD
   is `lb=4, nq=15` (60 Johnson bits) specifically to keep the **L1 verifier
   circuit small** (fewer FRI queries to verify in-circuit). Lowering the blowup
   speeds the Layer-0 commit but **grows the L1 recursion** — a coupled tradeoff,
   not a free win. Existing profiles: `PROD_LB2` (lb=2/nq=45, 4× LDE, fat proof),
   `PROD_LB4` (lb=4/nq=23), `PROD_LB5` (lb=5/nq=18, 32× LDE).

## Optimization levers (ranked)

1. **Blowup/query tradeoff (config, soundness-preserving, coupled to L1).** At the
   same 60-bit floor: `lb=2, nq=30` → **4× smaller LDE** (commit ~55 s → ~14 s),
   but `nq` 15→30 → ~2× the L1 verifier circuit. Net win *iff* the L1 growth is
   less than the Layer-0 saving. **Must measure the full Layer-0 + L1 compact
   prove, not just Layer-0.** First experiment.
2. **Trace width (AIR, soundness-neutral, invasive).** The 1,917 main + 588 aux
   columns are hashed every prove. Removing/packing sparse columns or cutting
   LogUp buses shrinks the commit *linearly* with no soundness or recursion
   change — the cleanest but most invasive win.
3. **Merkle leaf hash.** `first digest layer` is the hot leaf-compression. Confirm
   it's SIMD-saturated on native; a cheaper MMCS hash config (or larger Merkle
   cap) could cut constant factors.

**Non-levers (measured small):** FRI (0.7%), quotient (2.5%) — don't touch.
