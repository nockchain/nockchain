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
| `W` | fork-choice work credit | `+block-work-at(height, T)` |

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

### I4 — fork choice weights every block equally

From `+dual-puzzle-phase` on, every block contributes the same heaviness
whichever puzzle produced it (`+dual-puzzle-block-work`). Heaviness does not read
the pow artifact and does not scale with either puzzle's target.

**The boundary is the ZK re-pin / AI ASERT introduction, not admission.** Equal
weighting is justified by each puzzle's ASERT holding its own puzzle at its own
`ideal-block-time` — that is what makes the block-rate ratio the chainwork ratio.
The argument starts holding when the dual-puzzle regime does.

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
rule describes, so gating on admission would stop accumulating real difficulty
too early.

**Why not `1/target`.** ASERT pins each puzzle's target to that puzzle's own
capacity, so a `1/target` heaviness would make per-block weight track each
puzzle's capacity *relative to the other's*. The two are not comparable
quantities — one prices ZK proof attempts, the other matmul MAC-equivalents,
and the computations are heterogeneous and optimized separately — so that ratio
is arbitrary and drifts. Concretely: at a ratio `R`, one block of the heavier
puzzle outweighs `R` blocks of the lighter one, so at every height both puzzles
reached the lighter block loses, and a single late block can reorg `R` blocks of
history. Measured against real hardware, `R` was ~`2^37`.

Equal weighting makes a block of either puzzle worth a block of the other: no
puzzle's blocks are systematically orphaned, and no single block can displace
more than one block.

**Each puzzle's share of accumulated work is therefore the ratio of its block
rate**, which its own ASERT holds at its own `ideal-block-time`. The 250s/375s
pair splits fork-choice weight exactly as it splits block production, and
neither share depends on how either puzzle's work happens to be counted.

**What is given up, and what covers it.** Difficulty is enforced but no longer
accumulated: a block whose target is not the one its branch's ASERT computed is
invalid. The property this leans on is that every branch's ASERT drives that
branch to the same block *rate*, so a minority miner's private branch retargets
down to the same cadence but starts and stays behind on count — it can match the
honest chain's rate, never outpace it.

Blocks below the phase keep the unchanged `+compute-work` on their own target, so
nothing already on the chain changes weight. The constant above it is what a ZK
block at its own post-activation ASERT anchor contributed under the previous
rule, so accumulated work is continuous across the boundary.

**One definition.** `+block-work-at` (tx-engine) is the only place a block's work
is computed; candidate construction (`+new-candidate`) and validation
(`+block-compute-work`) both call it, so a candidate can never store an
accumulated-work that validation then rejects. `+new-candidate:v1` takes the work
rather than deriving it — only a caller holding the activation height can compute
it.

### I5 — the anchor targets a block interval, and only that

`anchor-target-atom.ai-asert = 2^193`. Under I4 the anchor carries no
fork-choice weight, so it is free to serve the one thing it can: the AI puzzle's
launch block interval.

An `%ai-pow` target prices one MAC-equivalent, so `2^256 / anchor` is the
expected MAC-equivalents per block and the cadence is that over the network's
real MAC rate. `2^193` is `2^63` MAC-equivalents — about 3.7e16 MAC/s at the 250s
ideal, about a hundred consumer GPUs at the 200-400 TeraMAC/s a 4090/5090 does
in Pearl pools.

Erring **hard** is the safe direction. Too hard costs a slow AI ramp that ASERT
heals at one doubling per half-life of *elapsed* time. Too easy mints blocks at
the wrong rate, and ASERT only heals that at `ideal/half-life` doublings per
*accepted* AI block — `250/43200`, one doubling per ~173 blocks. At the previous
`2^227` the anchor implied `2^29` MAC-equivalents per block, which one consumer
GPU clears in ~1.8 microseconds against a 250s target.

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
| I4 | Hoon: `test-equal-weight-starts-at-the-asert-phase-not-admission`, `test-post-activation-blocks-weigh-the-same`, `test-post-activation-weight-is-difficulty-independent`, `test-single-block-cannot-outweigh-a-run`, `test-dual-puzzle-mixed-accumulated-work`, `test-block-work-continuous-at-activation`, `test-pre-ai-heaviness-uses-zk-normalizer` |
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
```

Reading `A` as `2^256 / T = 2^29` — the shape factor omitted — overstates the
attempt count by `F` and understates the difficulty by the same factor.
