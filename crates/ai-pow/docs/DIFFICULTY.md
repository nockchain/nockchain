# AI-PoW difficulty: representations and invariants

Normative. Every producer and consumer of an `%ai-pow` acceptance decision —
the consensus verifier, the recursive-certificate jet, the canonical CPU miner,
the Pearl-gateway miner, the ASERT constants, and the fork-choice work formula —
must agree with what is written here.

## The four quantities

| Symbol | Name | Where it lives |
|---|---|---|
| `T` | consensus target | `page.target` for an `%ai-pow` block; emitted by `+compute-target-ai-asert` |
| `F` | shape work factor | `h · w · dot_product_length`, derived from the statement's `PearlMiningConfig` |
| `Θ` | effective jackpot threshold | `T · F`; never stored, always derived |
| `W` | fork-choice work credit | `+block-work-at(height, puzzle, T)` |

`h` and `w` are the opened tile's row and column counts (`rows_pattern.size()`,
`cols_pattern.size()`); `dot_product_length` is Pearl's `k − (k mod r)`.

**`T` prices one MAC-equivalent of matmul work, not one attempt.** This is the
single fact that makes the rest coherent, and the single fact that is easy to
get wrong: `T` looks like a Bitcoin-style per-hash target and is not one.

## Invariants

### I1 — one accept predicate

An attempt wins iff `jackpot ≤ Θ`. There is exactly one implementation,
`ai_pow::difficulty::attempt_wins`, and exactly one implementation of the
scaling it depends on, `effective_jackpot_threshold`.

A producer that compares `jackpot ≤ T` instead is not merely conservative: it
discards every win in `(T, Θ]`, so it spends `F` times more work per block than
consensus asks for. At the canonical shape `F = 2^16`; at the envelope maximum
`F = 2^24`. A difficulty parameter tuned against such a producer's measured
block rate lands `F` times too easy.

`F` is derived from `PearlMiningConfig::shape_work_factor()` — the config object
the statement carries and the verifier re-parses — never from a parallel copy of
`(h, w, k, r)` held alongside it.

### I2 — the unit of work is shape-invariant

Expected MAC-equivalents to find a block is `2^256 / T`, whatever tile shape the
miner chose: it needs `2^256 / Θ` attempts and each costs `F`, and
`(2^256/Θ) · F = 2^256/T`.

This is what makes `W` a meaningful fork-choice weight, and it is why the
shape factor belongs in the threshold rather than in the work credit. Without
it, a miner would minimise `F` and the puzzle would stop being a matmul.

### I3 — the target domain is the minable domain

`Θ` is computed in 256 bits, fail-closed. A target whose `Θ` does not fit is not
an easy target — it is an **unminable** one: every block carrying it is
rejected, and because the AI ASERT advances only when an AI block is *accepted*,
such a target never retargets back down. The puzzle would be permanently dead,
not temporarily slow.

Consensus therefore never emits a target above

```
AI_POW_MAX_CONSENSUS_TARGET = floor((2^256 − 1) / F_max) = 2^232 − 1
```

with `F_max = 2^24`. Consensus enforces it on **every** path, not just the ASERT
one: `+compute-target-ai-asert` caps its own output, but a block below the ASERT
phase inherits the epoch target, which is uncapped and on a fresh chain is the
genesis target. `+validate-page-without-txs` rejects an `%ai-pow` block whose
target exceeds the cap (`%ai-pow-target-outside-minable-domain`) rather than
letting it surface as an opaque pow failure.

Four constants encode this and must move together:

- `ai_pow::difficulty::AI_POW_MAX_CONSENSUS_TARGET`
- Hoon `+max-ai-target-atom` (`hoon/common/tx-engine-0.hoon`), the ceiling
  passed to `+compute-target:asert` by `+compute-target-ai-asert`
- `AI_ASERT_MAX_BEX` (`crates/nockchain/src/config.rs`), the fakenet
  `--fakenet-ai-asert-anchor-target-bex` bound
- the `%ai-pow` target gate in `+validate-page-without-txs`

### I4 — fork choice prices expected work per puzzle, at a hardware exchange rate

From `+dual-puzzle-phase` on, a block's heaviness is the expected work at its
own target for the puzzle named by its pow artifact, priced in
ZKPoW-attempt-equivalents (`+block-work-at`):

- `%dumb-zkpow`: `2^320 / (T+1)` attempts — the unchanged pre-activation
  formula, so every ZK block already on the chain keeps its weight.
- `%ai-pow`: `2^256 / (T+1)` MAC-equivalents (I2), converted at
  `+mac-equivalents-per-zk-attempt`.

Heaviness therefore scales inversely with target for both puzzles: a branch
whose ASERT lets its target drift to a ceiling earns proportionally less
fork-choice credit per block and cannot win by count.

**The boundary is the ZK re-pin / AI ASERT introduction, not admission.**
Per-puzzle pricing is justified by each puzzle's ASERT holding its own puzzle
at its own `ideal-block-time` — that is what makes per-target expected work the
chainwork ratio. The argument starts holding when the dual-puzzle regime does.

`+dual-puzzle-phase` is **one constant**, `phase.ai-asert`, not a derived value.
The ZK re-pin (`zk-asert-post-ai`) and the introduction of the AI puzzle's own
ASERT are the same event — there is no coherent chain state where one has
happened and the other has not — so `phase.zk-asert-post-ai == phase.ai-asert`,
asserted at kernel load.

`phase.zk-asert` is the ORIGINAL Aletheia pin, made before the dual puzzle
existed. It precedes this boundary and is not part of it (`phase.zk-asert <=
dual-puzzle-phase`, also asserted).

`ai-pow-activation-height` is when AI blocks become *admissible* — the same
height on mainnet, but a separate question. A fakenet may admit AI below the
re-pin, and until the re-pin neither puzzle is retargeting under the regime this
rule describes, so a pre-phase block accumulates the ZK formula on its own
target whatever puzzle produced it.

#### Deriving the exchange rate

The rate was measured on one reference consumer GPU (RTX 5090 class, 2026-07)
by co-benchmarking the ZK prover against the Pearl mining kernel on identical
hardware: measured MAC-equivalent throughput divided by measured attempt
throughput prices one ZKPoW attempt at

```
mac-equivalents-per-zk-attempt = 25,750,000,000
```

The prover-side throughput figure is deliberately not published: it prices
exactly how fast the ZK puzzle can be attacked. The AI-side figure is public
(Pearl pools report ~309 TH/s for this GPU class — see the unit warning
below), so anyone with reference hardware can reproduce the rate without this
document disclosing it.

**Unit warning: Pearl's "H" is ambiguous — check the magnitude.** Pearl's own
difficulty scaling (`MiningJob.adjust_target`: win probability per check is
`T · (h·w·dot) / 2^256`) prices one tile-level PoW check at `h·w·dot`
MAC-equivalents, i.e. exactly `F`, so a pool displaying native Pearl units
reads `MAC-rate / F`. In practice pools pre-normalize: the observed figure for
one 5090 is **309 TH/s**, which read as tile-checks would imply 2 × 10¹⁹
MAC/s — ~48× the card's theoretical dense int8 peak (~419 TMAC/s) and
therefore impossible. That pool's "H/s" is already MAC-scale and enters this
derivation unmultiplied. A pool displaying native units would instead read
~4.71 MH/s for the same card; the 65,536× spread makes the two conventions
easy to distinguish empirically.

This constant is consensus-critical: every node must use the same value. It
need not be exact; it must be stable and roughly track the real hardware ratio.

**Cross-check at the launch anchors.** The ZK anchor
(`floor(375 · 2^291 / 214)`) prices 306,374,333 attempts per block; the AI
anchor (`2^192`) prices `2^64` MAC-equivalents per block. At their 214 s ZK
and 500 s AI ideals, both lanes produce about 1.43 × 10⁶
attempt-equivalents of heaviness per second — within 0.1% of each other — and
both calibrate to roughly a hundred reference-class consumer GPUs. At launch
calibration neither puzzle orphans the other, and per-block weights differ
only by the 500 s / 214 s cadence ratio (≈ 2.336).

**Why not unnormalized `1/target`.** ASERT pins each puzzle's target to that
puzzle's own capacity, so a raw `1/target` heaviness would make per-block
weight track each puzzle's capacity *relative to the other's*. The two are not
comparable quantities without a measured rate — one prices ZK proof attempts,
the other matmul MAC-equivalents, and the computations are heterogeneous and
optimized separately — so that ratio is arbitrary and drifts. Concretely: at a
ratio `R`, one block of the heavier puzzle outweighs `R` blocks of the lighter
one, so at every height both puzzles reached the lighter block loses, and a
single late block can reorg `R` blocks of history. Measured against real
hardware, `R` was ~`2^37`. The exchange rate above is exactly that ratio,
benchmarked and frozen as a constant instead of left to emerge.

**Why not equal weight.** A flat per-block weight makes a capped block worth a
hard block. A branch forked at the activation parent banks elapsed wall-clock
time against a zero branch-local ASERT count; six legal timestamps flip the
median-time-past window, the branch's targets clamp at their ceilings, and the
branch wins fork choice by raw block count. Reproduced end-to-end against the
live kernel; see `2026-07-29_TIME_BANKED_FORK_EXPLOIT.md`. Difficulty must be
priced into heaviness, not only enforced at admission.

**What exchange-rate drift does.** The constant freezes the 2026-07 hardware
ratio; reality drifts as provers and inference hardware improve.

- *Within one puzzle, drift is harmless.* Same-puzzle heaviness comparisons
  share the constant, so it cancels: weight is proportional to expected work
  for any value of the rate. The property that kills the time-bank exploit —
  a discounted target earns proportionally less credit — does not depend on
  the rate's accuracy at all.
- *Across puzzles, drift is a fairness skew equal to the drift factor.* If the
  true ratio rises above the constant (AI underpriced), AI blocks earn less
  heaviness per unit of real work than ZK blocks; miners migrate off the AI
  lane, its ASERT eases to hold cadence, and per-block AI weight falls
  further. If the ratio falls (AI overpriced), the mirror image starves the
  ZK lane, and an attacker concentrated on the overpriced lane converts
  hardware time into heaviness up to the drift factor faster than honest
  miners on the other lane.
- *The failure mode is lane imbalance, never a difficulty discount.* No value
  of the rate lets any branch earn more heaviness than the expected work at
  its blocks' targets. GPU generations are years apart, so the ratio moves
  slowly; recalibrate the constant at an upgrade when the hardware benchmark
  does.

**One definition.** `+block-work-at` (tx-engine) is the only place a block's work
is computed; candidate construction (`+new-candidate`, `+build-ai-candidate`)
and validation (`+block-compute-work`) both call it, so a candidate can never
store an accumulated-work that validation then rejects. `+new-candidate:v1`
takes the work rather than deriving it — only a caller holding the activation
height and the puzzle can compute it.

### I5 — the anchor targets a block interval and prices launch weight

`anchor-target-atom.ai-asert = 2^192`. Under I4 the anchor prices both the AI
puzzle's launch block interval and its launch fork-choice weight:
`2^256 / anchor` is both the expected MAC-equivalents per block and the
anchor block's heaviness, convertible to attempt-equivalents at the I4 rate.

An `%ai-pow` target prices one MAC-equivalent, so `2^256 / anchor` is the
expected MAC-equivalents per block and the cadence is that over the network's
real MAC rate. `2^192` is `2^64` MAC-equivalents — about 3.7e16 MAC/s at the
500s ideal, about a hundred consumer GPUs at the 200-400 TeraMAC/s a
4090/5090 does in Pearl pools.

Erring **hard** is the safe direction. Too hard costs a slow AI ramp that ASERT
heals at one doubling per half-life of *elapsed* time. Too easy mints blocks at
the wrong rate, and ASERT only heals that at `ideal/half-life` doublings per
*accepted* AI block — `500/43200`, one doubling per ~86 blocks. At the previous
`2^227` the anchor implied `2^29` MAC-equivalents per block, which one consumer
GPU clears in ~1.8 microseconds against a 500s target.

**Phase ordering.** Asserted at kernel load:
`phase.zk-asert-post-ai == phase.ai-asert` (simultaneous re-pin),
`phase.zk-asert <= dual-puzzle-phase` (the Aletheia pin precedes it), and
`v1-phase <= dual-puzzle-phase` — only the v1 candidate builder is told that
height, so a v0 page at or above it would store an accumulated-work validation
rejects.

## Where each invariant is pinned

| Invariant | Test |
|---|---|
| I1 | `ai-pow-miner`: `canonical_grind_threshold_matches_the_consensus_verifier` |
| I2 | `ai-pow`: `difficulty::tests::expected_work_is_shape_invariant` |
| I3 | `nockchain`: `ai_pow_valid_block_is_admitted` (real block admitted through the kernel); `ai-pow`: `difficulty::tests::max_consensus_target_never_overflows`, `..._is_the_tight_bound`; `ai-pow-miner`: `canonical_grind_threshold_covers_the_whole_consensus_target_domain`; Hoon: `test-max-ai-target-atom-keeps-every-shape-representable`, `test-max-ai-target-atom-is-the-tight-bound`; `nockchain`: `validate_rejects_ai_asert_bex_above_the_minable_domain` |
| I4 | Hoon: `test-puzzle-pricing-starts-at-the-asert-phase-not-admission`, `test-post-activation-work-is-puzzle-priced`, `test-post-activation-weight-tracks-target`, `test-single-block-cannot-outweigh-a-run`, `test-dual-puzzle-mixed-accumulated-work`, `test-zk-work-continuous-at-activation`, `test-anchor-work-is-exchange-rate-priced`, `test-pre-ai-heaviness-uses-zk-normalizer`, `test-time-banked-fork-loses-by-work` |
| I5 | Hoon: `test-ai-anchor-sets-the-launch-block-interval`, `test-mainnet-ai-anchor-is-inside-the-minable-domain`; `nockchain-types`: `ai_anchor_sets_the_launch_block_interval` |

## Worked example — the canonical shape

`m=64, k=1024, n=64, r=64, tile=8`, so `h = w = 8` and
`dot_product_length = 1024`:

```
F = 8 · 8 · 1024 = 65,536 = 2^16
```

At an anchor `T = 2^227`:

```
Θ  = 2^227 · 2^16 = 2^243
A  = 2^256 / Θ    = 2^13  = 8,192 attempts per block
W  = 2^256 / T    = 2^29           MAC-equivalents per block
W  = 2^29 / 25.75e9 ≈ 20.8       ZKPoW-attempt-equivalents of fork-choice credit
```

Reading `A` as `2^256 / T = 2^29` — the shape factor omitted — overstates the
attempt count by `F` and understates the difficulty by the same factor.
