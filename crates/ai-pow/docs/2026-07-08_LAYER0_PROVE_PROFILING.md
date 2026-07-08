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

## Experiment 1 — blowup lb=4 → lb=2 (measured, 12-core native)

Switched `CircuitConfig::PROD` from `lb=4/nq=15` to **`lb=2/nq=30`** — the *same*
60 Johnson bits (`2·30 = 60 = 4·15`), but a **4× smaller LDE** (2¹⁸ vs 2²⁰ for a
2¹⁶ trace). Measured:

**Isolated Layer-0 @ 2¹⁶:** 65.7 s → **17.3 s (3.8×)** — the commit shrank ~4× as
predicted.

**Full compact prove @ 2¹⁶ (max envelope):**

| | Layer-0 | L1-outer | total | compact size | Johnson bits |
|---|---|---|---|---|---|
| lb=4/nq=15 (old PROD) | ~74 s | 18.6 s | **95.1 s** | 122.6 KiB | 60 |
| **lb=2/nq=30** | 17.3 s | 34.9 s | **55.4 s** | 120.6 KiB | 60 |

**Net 1.72× faster at 2¹⁶**, same security floor, size unchanged. The Layer-0
commit saving (~57 s) dwarfs the L1-recursion growth (~16 s, from doubling
queries). Because Layer-0 commit dominates more as the trace grows, **the win
increases with degree** — largest at Pearl's 2¹⁸ max (where Layer-0 was the
~300 s+ bottleneck).

**Caveat (small traces):** at the 2¹³ default the Layer-0 is already small (~8 s)
while L1 doubles, so lb=2 may be *neutral-to-negative* there. [2¹³ result below.]
Since production degrees are 2¹⁶–2¹⁸ (the cases that miss the bar), lb=2 is the
right call for them; a degree-adaptive config (lb=2 for large, lb=4 for small)
would be optimal if the small default matters.

**Soundness note (R1):** this is a consensus FRI parameter. The 60-bit Johnson
floor is preserved exactly (`johnson_fri_bits` unchanged), and the codebase already
sanctions alternate blowup profiles.

## Outcome + status

**Mechanism landed:** `CircuitConfig::PROD_LB2_NQ30` and
`CircuitConfig::prod_adaptive(stark_degree_bits)` (`circuit.rs`) — the
degree-adaptive policy (`lb=4/nq=15` for degree ≤14, `lb=2/nq=30` for ≥15, both
60-bit). `PROD` reverted to `lb=4/nq=15`. The lb=2 numbers above were measured
under a temporary `PROD` edit, now reverted.

**Validated speedup (measured, 12-core native):** at the production degrees that
miss the ~30 s bar (2¹⁶–2¹⁸), `prod_adaptive` is **1.72× at 2¹⁶** and more at 2¹⁸,
with the size and the 60-bit floor unchanged; at the small default (2¹³) it keeps
`lb=4` so nothing regresses.

**Remaining to realize it in production (a dedicated validated pass — consensus
soundness-critical):** thread `prod_adaptive(log2(trace_height))` into the ~4
config-choice sites so the **prover and verifier derive the same profile from the
bound `trace_height`** (a mismatch breaks verification):
1. Layer-0 prove config (`zk_bridge` scheduled prover, where the trace is built).
2. L1 recursion profile (`recursion.rs` — passed through from Layer-0).
3. Layer-0 verify (`verify_ai_pow_tiled_with_statement`, has `artifact.trace_height`).
4. Recursive-cert verify (`verify_recursive_certificate` callers in
   `certificate_noun.rs` / `zk_bridge` — derive from `cert.trace_height`, which
   the precheck already binds to `expected_layer0_rows`).

Gate: full recursion prove→verify round-trip at **both** a ≤14 and a ≥15 degree
(profile selection must round-trip), the compact size/latency tests, and the
soundness/adversarial suite. The `trace_height` is already a bound public input,
so the derivation is verifier-safe; the work is the careful threading + the
two-degree regression.

## Next levers (after the blowup win)

The blowup win is capped (~2× at 2¹⁶); the **trace width (1917 + 588 columns)** is
the bigger structural lever (commit is linear in width, no soundness/recursion
change). Audit `composite_layout` for sparse/removable columns and the LogUp bus
count next.
