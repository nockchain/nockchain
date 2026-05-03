+++
version = "0.1.14"
status = "draft"
consensus_critical = true

activation_height = 65500
published = "2026-04-27"
activation_target = ""

authors = ["@nockchain-core"]
reviewers = ["@nockchain-core"]

supersedes = "0.1.13"
superseded_by = ""
+++

# Aletheia

ASERT per-block difficulty adjustment, block-time reduction from 600s to
150s, and a unified emissions curve with a smooth-decay 9.5-year body and
a 64-NOCK floor sustained for ~68 years to a 2³² hard cap, with every
post-activation reward split 80/20 between the miner and a time-locked
protocol fund.

## Summary

Aletheia bundles three consensus changes onto a single activation block at
height 65,500:

1. **ASERT difficulty adjustment.** Replaces nockchain's Bitcoin-style
   2,016-block epoch retarget with aserti3-2d (Absolutely Scheduled
   Exponentially Rising Targets), the per-block difficulty algorithm
   formalized by Jonathan Toomim and shipped on Bitcoin Cash in 2020.

2. **Block-time reduction from 600s to 150s.** The ideal inter-block
   interval drops by 4×, with ASERT's half-life scaled proportionally
   to preserve its block-count-based stability properties.

3. **Unified emissions curve.** Replaces the 16-halving schedule with a
   single 14-row table that runs from genesis through a 2³² hard cap.
   Eons 0–2 (the historical bootstrapping phase) are preserved as-is
   but eon 2 is truncated from block 78,895 to block 65,500. Post-
   activation, reward decays through nine smaller steps (alternating
   25% / 33% drops, no halvings) over 9.5 years, then settles at a
   64-NOCK floor for ~68 years until the cap is met exactly. Every
   post-activation reward is split 80% miner / 20% protocol fund;
   both outputs are coinbase outputs subject to the existing standard
   coinbase timelock — no separate unlock schedule is layered on the
   fund's share.

The block-time change *is* the immediate halving the new emissions
curve calls for at activation: 16,380 NOCK per 600s pre-activation
(1,638 NOCK/min) becomes 2,048 NOCK per 150s post-activation
(819.2 NOCK/min) — exactly half the per-time issuance rate. Bundling
these into one consensus event avoids two separate hard-fork
coordinations.

## Motivation

### Epoch retarget is sampling-sensitive

The current algorithm recomputes difficulty only at epoch boundaries
(every 2,016 blocks, ~14 days at 600s cadence) by comparing the actual
epoch duration against the target epoch duration and clamping the ratio
to a 4×/¼× bucket. Within an epoch the target is fixed. This has two
well-known failure modes:

- **Hashrate swings cause long off-target epochs.** A 50% hashrate drop
  midway through an epoch leaves the network running at half speed for
  up to a week before the next retarget can respond.
- **Sampling attacks.** Because the algorithm samples a single
  `(epoch-start, epoch-end)` pair, miners can influence the next-epoch
  target by timestamp placement around those two boundary blocks,
  independent of real-time difficulty behavior in the interior. This
  class of attack is documented against Bitcoin-style retarget since
  2014 and is what motivated BCH's move to aserti3-2d.

### ASERT responds every block and is anchor-based

aserti3-2d recomputes the target at every block from a fixed anchor
`(height, target, timestamp)` captured at fork activation. The response
to drift is a smooth exponential rather than a clamped ratio, and the
computation depends only on the current block's parent chain (not on a
per-epoch "window boundary"), which eliminates the boundary-sampling
attack surface entirely.

### Block time reduction

This upgrade also drops the ideal inter-block interval from 600s to
150s. The block-time change is bundled with ASERT activation because:

- ASERT parameters (`asert-anchor-target-atom`,
  `asert-ideal-block-time`, `asert-half-life`) directly encode the new
  cadence. Introducing the new DAA on the old cadence and then moving
  the cadence later would require two separate anchor pinnings and two
  separate consensus events.
- ASERT's half-life is measured in real-time seconds but its stability
  properties are measured in block counts. BCH's 172,800s half-life
  corresponds to 288 blocks at 600s. Preserving the 288-block stability
  window at the new 150s cadence gives a 43,200s (12h) half-life.
- The anchor target (`2^291`) is chosen to approximate a ~4× target
  increase over the pre-activation mainnet target, so expected blocks
  per second at constant hashrate rise by ~4× (600s → 150s). The
  closest power of two to 4× the pre-activation mainnet target
  (~2^291.38) yields ~3.2× faster blocks (~187s expected) under
  unchanged hashrate — slightly conservative vs the ideal 150s target.
  ASERT converges to the ideal 150s from that starting point.

### Why bundle emissions with the difficulty / block-time change

The original schedule front-loads ~99% of emissions into the chain's
first thirty years, leaving no meaningful long-run mining subsidy and
forcing the network onto fee revenue earlier than the application
ecosystem can sustain. A revised schedule has to (a) cut per-time
emissions to extend the chain's revenue tail, and (b) avoid the
familiar 50%-revenue-cliff dynamics that traditional halvings impose
on miners.

The block-time drop from 600s to 150s already cuts per-time emissions
by 75% on its own (4× more blocks at the same per-block reward would
quadruple issuance, so a 4× block-time drop demands a per-block reward
that is at least 1/4 the prior value to keep issuance flat — and we go
further, dropping to ~1/8 the prior per-block reward, for a clean 1/2
per-time cut). Doing the per-block reward change at the same height as
the block-time change means the activation experience is a single
clean per-time halving, not two staggered economic events.

Beyond the activation halving, the new schedule's smooth-decay curve
(alternating 25% / 33% drops over 9 era boundaries) and 64-NOCK floor
require their own table-lookup `block_reward(height)`. That table is
a strict superset of the activation-height behaviour, so it slots in
cleanly at the same `asert-phase` boundary and reuses no logic from
the legacy halving schedule beyond the eon-0/1/2 head.

The 80/20 miner/fund split falls out of the same activation:
pre-activation the chain has no fund and a single-output coinbase.
Post-activation it pays a second output to a well-known fund hash,
subject to the same standard coinbase timelock as the miner's share.
Bundling means the consensus rule "a v1 coinbase has either one
output (pre-activation) or two (post-activation)" can be stated
against a single height boundary.

## Technical Specification

### Blockchain-Constants

Five new fields are added to `blockchain-constants` in
`open/hoon/common/tx-engine-1.hoon`:

```hoon
asert-phase=65.500                      :: activation height
asert-anchor-height=65.499              :: asert-phase - 1
asert-anchor-target-atom=^~((bex 291))  :: 2^291
asert-ideal-block-time=150              :: seconds, new block time
asert-half-life=^~((mul 12 ^~((mul 60 60))))  :: 43,200s = 12h
```

`rbits` is **not** a `blockchain-constants` field. The polynomial
coefficients in `lib/asert.hoon` are tied to `rbits=16` and cannot
vary without replacing the polynomial, which is itself a hard fork.
`rbits` is therefore a compile-time constant (`++  asert-rbits  16`)
rather than a per-network parameter.

The emissions changes do **not** add any new `blockchain-constants`
fields. The schedule's era boundaries (`asert-phase + 105.000` for
the end of eon 3, `+ 210.000` per subsequent eon, tail-end at
`16.144.876`), the floor (64 NOCK), the 80/20 split (`1/5` to fund),
are all hardcoded as bare integer literals in the relevant
`++ schedule` and `++ new:coinbase` arms.
The single piece of consensus-known data the emissions layer needs —
the fund recipient hash — is a hardcoded `++ fund-address` arm in
`open/hoon/common/tx-engine-1.hoon`. Setting the real fund-multisig
hash before mainnet activation is therefore a code change, not a
constants-overlay change. (The implementation ships with a
placeholder `*hash` zero value for review and testing.)

The reversion-to-100%-miner trigger described elsewhere is *not*
part of this upgrade. It is a **future** protocol upgrade that will
ship the new fully useful PoW puzzle and a corresponding
consensus-layer mechanism for the split to revert. Until that
ships, every post-activation block pays the 80/20 split
unconditionally.

The anchor's median-of-11 timestamp is **not** a constant — see
"Consensus State: Anchor Timestamp" below.

`blocks-per-epoch` is not changed. Legacy `epoch-counter` and
`epoch-start` accounting continue to be written and validated for
pre-activation blocks; post-activation blocks continue writing them
(inert but preserved for header compatibility) but they no longer
gate target computation.

### ASERT Algorithm

Added as a new pure-math library at
`open/hoon/apps/dumbnet/lib/asert.hoon`. The core arm is
`+compute-target`:

```
time-diff   = current-min-timestamp  - anchor-min-timestamp         (signed)
height-diff = current-height         - anchor-height                (>= 1)
exponent    = ((time-diff - ideal * (height-diff - 1)) * radix) / half-life
                                                                    (signed)
shifts      = exponent >> rbits                                     (arithmetic)
frac        = exponent - (shifts << rbits)                          (in [0, radix))
factor      = radix + (  195_766_423_245_049 * frac
                       +         971_821_376 * frac^2
                       +               5_127 * frac^3
                       +               2^47 ) >> 48
next-target = (anchor-target * factor) << shifts         (or >> -shifts)
next-target = next-target >> rbits
clamp to [1, max-target]
```

`current-min-timestamp` is the *parent* block's median-of-11 and
`current-height` is the *child* block's height (the block whose target we
are computing). `anchor-min-timestamp` is the anchor block's *own*
median-of-11 — this is PDF Eq. (2) §1.3 Option 2 ("nearly identical but
distinct" from BCH's anchor-parent convention). Under a perfect schedule,
`current-min-timestamp - anchor-min-timestamp` spans `height-diff - 1`
ideal intervals (parent is `height-diff - 1` blocks past the anchor), so
the `(height-diff - 1)` term leaves a zero exponent at schedule.

Where `rbits = 16`, `radix = 2^16`. The 3rd-order polynomial
approximates `2^x` on `[0, 1)` with max error under 0.13%. Polynomial
coefficients are tied to `rbits`; changing `rbits` would itself be a
hard fork. The `+ 2^47` term is round-to-nearest on the fixed-point
divide by `2^48` and is required for bit-for-bit parity with BCH's
canonical `aserti3-2d` polynomial.

Sign handling: `compute-target` branches early on the sign of
`current-min-timestamp - anchor-min-timestamp` and keeps all
intermediate arithmetic on unsigned atoms, tracking the exponent sign
in a `?` and inverting shift direction at the end. This avoids `+si`
ceremony and mirrors how `compute-target-raw` keeps math in plain
atoms today.

Shift magnitude is capped at `max(max-target-bits) + rbits + 2` for
positive shifts and at the bit-length of the intermediate for negative
shifts, so the noun representation of intermediates stays bounded even
under pathological exponents.

A thin bignum wrapper `+compute-target-bn` converts at the boundary
so callers on `bignum` don't reach into atom math.

### Timestamp Source: Median-of-11

ASERT reads timestamps from the existing `min-timestamps` median-of-11
map, not raw `pag.timestamp`. Specifically, `current-min-timestamp` is
`min-timestamps[pag.parent]` — the parent's median-of-11, already
computed, stored, and validated at the point we need it. This
sidesteps the "miner sets a bogus timestamp on the next block" attack
surface that raw-timestamp ASERT implementations are vulnerable to.

*Consequence*: BCH's published aserti3-2d test vectors (which use
raw timestamps) are **not** directly reusable. The polynomial piece
itself is timestamp-independent and is cross-checked against hand-
computed values; end-to-end correctness is verified against a
reference Hoon implementation in the test file.

### Anchor timestamp resolution

The anchor's median-of-11 timestamp is not known at compile time — it
is only well-defined once block `asert-anchor-height` has been
processed and its median-of-11 computed. Phase 1 resolves it at
target-compute time by walking `.blocks` from the requested
`parent-digest` back to its ancestor at `asert-anchor-height` and
reading that ancestor's entry from `.min-timestamps`. No consensus-
state field is introduced; `consensus-state` remains at version 6.

Fork-correctness falls out for free: every caller walks its own
ancestry, so competing forks never read or write a shared scalar and
cannot diverge each other's target computation. Phase 2 (post-65500)
replaces the walk with a hardcoded protocol constant and retires the
helper.

#### `+find-anchor-min-ts`

In `open/hoon/apps/dumbnet/lib/consensus.hoon`:

```hoon
++  find-anchor-min-ts
  |=  bid=block-id:t
  ^-  @
  =/  anchor-height=@  asert-anchor-height.blockchain-constants
  =/  cur=page:t  (to-page:local-page:t (~(got z-by blocks.c) bid))
  |-
  ?:  =(~(height get:page:t cur) anchor-height)
    (~(got z-by min-timestamps.c) ~(digest get:page:t cur))
  $(cur (to-page:local-page:t (~(got z-by blocks.c) ~(parent get:page:t cur))))
```

Callers (`+compute-target-asert` in `consensus.hoon` and the miner
candidate-target path in `miner.hoon`) must guarantee the input
digest's block is in `.blocks` with height ≥ `asert-anchor-height`.
This invariant holds at every ASERT call site because ASERT activates
at `asert-phase = asert-anchor-height + 1` and all such callers pass
a parent whose block is already in `.blocks`.

### Consensus Wrapper

`+compute-target-asert` in
`open/hoon/apps/dumbnet/lib/consensus.hoon` (new arm):

```hoon
++  compute-target-asert
  |=  [child-height=@ parent-digest=block-id:t]
  ^-  bignum:bignum:t
  =/  parent-min-ts=@  (~(got z-by min-timestamps.c) parent-digest)
  =/  anchor-min-ts=@  (find-anchor-min-ts parent-digest)
  %-  chunk:bignum:t
  %-  compute-target:asert
  :*  asert-anchor-target-atom.blockchain-constants
      anchor-min-ts
      asert-anchor-height.blockchain-constants
      parent-min-ts
      child-height
      asert-ideal-block-time.blockchain-constants
      asert-half-life.blockchain-constants
      max-target-atom:t
  ==
```

### Activation Gating

Two call sites in `consensus.hoon` are gated on
`(gte height asert-phase)`:

1. **Target write in `+accept-page`** — if post-activation,
   `targets[pag.digest] := compute-target-asert(child-height, parent)`.
   Else retain existing epoch/genesis/carry-forward logic.
2. **Target check in `+validate-page-without-txs`** — if
   post-activation, compare `pag.target` against
   `compute-target-asert(child-height, parent)`. Else retain the
   existing `pag.target == targets[parent]` lookup.

Mirrors the existing activation-height patterns
(`height-to-proof-version`, `v1-phase`, `bythos-phase`).

### Miner

`open/hoon/apps/dumbnet/lib/miner.hoon`'s `heard-new-block` path
derives `anchor-min-ts` at candidate-target time by calling
`+find-anchor-min-ts` from `consensus.hoon` against heaviest-block.

### Emissions Schedule

`open/hoon/common/schedule.hoon`'s `++ schedule` is replaced with a
height-dispatched function that covers all heights from genesis through
the 2³² hard cap in a single 14-row table. The function is pure and
constant-time after a small chain of inequality checks.

| Eon  | Block range              | Blocks      | Reward (NOCK) | Block time | Duration | Emission (NOCK) | Drop  | Cum. % |
|------|--------------------------|------------:|--------------:|------------|----------|----------------:|-------|-------:|
| 0    | 1 – 13,150               |     13,150  |        65,536 | 10 min     | ~3.0 mo  |     861,798,400 | —     | 20.06% |
| 1    | 13,151 – 39,448          |     26,298  |        32,768 | 10 min     | ~6.0 mo  |     861,732,864 | −50%  | 40.12% |
| 2    | 39,449 – 65,500          |     26,052  |        16,384 | 10 min     | ~5.9 mo  |     426,835,968 | −50%  | 50.07% |
| 3    | 65,501 – 170,500         |    105,000  |         2,048 | 2.5 min    | 6 mo     |     215,040,000 | −50%* | 55.08% |
| 4    | 170,501 – 380,500        |    210,000  |         1,536 | 2.5 min    | 1 yr     |     322,560,000 | −25%  | 62.58% |
| 5    | 380,501 – 590,500        |    210,000  |         1,024 | 2.5 min    | 1 yr     |     215,040,000 | −33%  | 67.59% |
| 6    | 590,501 – 800,500        |    210,000  |           768 | 2.5 min    | 1 yr     |     161,280,000 | −25%  | 71.35% |
| 7    | 800,501 – 1,010,500      |    210,000  |           512 | 2.5 min    | 1 yr     |     107,520,000 | −33%  | 73.85% |
| 8    | 1,010,501 – 1,220,500    |    210,000  |           384 | 2.5 min    | 1 yr     |      80,640,000 | −25%  | 75.73% |
| 9    | 1,220,501 – 1,430,500    |    210,000  |           256 | 2.5 min    | 1 yr     |      53,760,000 | −33%  | 76.98% |
| 10   | 1,430,501 – 1,640,500    |    210,000  |           192 | 2.5 min    | 1 yr     |      40,320,000 | −25%  | 77.92% |
| 11   | 1,640,501 – 1,850,500    |    210,000  |           128 | 2.5 min    | 1 yr     |      26,880,000 | −33%  | 78.54% |
| 12   | 1,850,501 – 2,060,500    |    210,000  |            96 | 2.5 min    | 1 yr     |      20,160,000 | −25%  | 79.01% |
| Tail | 2,060,501 – 16,144,876   | 14,084,376  |            64 | 2.5 min    | ~67 yrs  |     901,400,064 | −33%  | 100.00%|

\* The −50% drop at the eon 2 → 3 boundary is **per unit time**, not
per block. The per-block reward falls 87.5% (16,384 → 2,048) but the
block time also drops 4× (600s → 150s), so NOCK-per-minute exactly
halves: 1,638.4 NOCK/min → 819.2 NOCK/min.

The cumulative emission at block 16,144,876 is exactly
`4,294,967,296 = 2³²` NOCK; subsequent blocks emit zero. At 64
NOCK/block, the tail block count is exactly `901,400,064 ÷ 64 =
14,084,376` and the cap is hit exactly with **no per-block dust
carve-out** required.

#### Why this shape

- **Immediate halving on a per-time basis.** The per-time issuance rate
  exactly halves at activation, satisfying the canonical "halving"
  contract without imposing a 50% per-block revenue drop on miners.
  The per-block reward drops by 87.5% but is paired with a 4× increase
  in blocks per minute, so a miner running fixed hardware sees their
  NOCK-per-minute revenue cut by exactly 50% — the same rate-of-change
  a Bitcoin-style halving produces, but with the rest of the per-block
  schedule decoupled from this one shock.

- **Smooth decay, no halvings, max −33% step.** After the activation
  era, reward decays through a deliberately smooth sequence:
  `2048 → 1536 → 1024 → 768 → 512 → 384 → 256 → 192 → 128 → 96 → 64`.
  Each transition is either a 25% drop (a 3/4 reduction) or a 33% drop
  (a 2/3 reduction); the drops alternate predictably. No transition is
  a full halving. A miner whose hardware becomes unprofitable at a
  given era boundary was already close to marginal profitability —
  roughly the bottom 25% or 33% of the fleet by efficiency, not the
  bottom 50%.

- **Multiples of 32.** Every reward is a multiple of 32, which keeps
  the arithmetic exact in any fixed-point system and ensures the
  20% fund share (= reward / 5) is also exactly representable
  (since both `2^16 = atoms-per-nock` and the reward sequence are
  divisible by 5 after multiplication; the proportional-allocation
  arithmetic in `++ new:coinbase-split` is exact for any reward that
  is a clean multiple of 5, which all post-
  activation rewards are).

- **A 64-NOCK floor for ~68 years.** The tail spans approximately
  two to three human generations. Setting a fixed floor (rather than
  decaying to an asymptotic trickle as the original schedule did)
  preserves a meaningful real per-block subsidy through the tail
  phase and pins exact emission of `2³²` NOCK at a known terminal
  block.

#### Cap accounting

Cumulative supply through end-of-decay (block 2,060,500) totals
`3,393,567,232` NOCK:

- Eons 0–2 (powers of two, the actual on-chain history):
  `861,798,400 + 861,732,864 + 426,835,968 = 2,150,367,232 NOCK`.
- Eon 3: `105,000 × 2,048 = 215,040,000 NOCK`.
- Eons 4–12 (smooth decay): `1,028,160,000 NOCK`.

Remaining budget to the `2³² = 4,294,967,296` cap:
`901,400,064 NOCK`. At 64 NOCK/block this is exactly `14,084,376`
tail blocks, so the tail spans `2,060,501..=16,144,876`, every tail
block emits 64 NOCK, and the cap is hit exactly with no boundary
case for "dust" handling. `++ schedule` returns 0 for any height
past 16,144,876.

#### Reference pseudocode

```
const DECAY_REWARDS: [u64; 9] =
    [1536, 1024, 768, 512, 384, 256, 192, 128, 96];

fn block_reward(height: u64) -> u64 {
    if height == 0                      { return 0; }
    if height >  16_144_876             { return 0; }
    if height <= 13_150                 { return 65_536; }
    if height <= 39_448                 { return 32_768; }
    if height <= 65_500                 { return 16_384; }
    if height <= 170_500                { return 2_048; }
    if height <= 2_060_500              {
        let era_idx = (height - 170_501) / 210_000;
        return DECAY_REWARDS[era_idx as usize];
    }
    64  // tail
}
```

The hoon implementation (in `open/hoon/common/schedule.hoon`) follows
this branch order. Returned values are in units of NOCK; the schedule
arm multiplies by `atoms-per-nock = 2^16` internally to produce on-
chain atom-denominated coin amounts.

### Reward Distribution

Pre-activation, every coinbase pays one output: 100% to the block
miner, with the existing relative timelock (`coinbase-timelock-min =
100`). Post-activation, every coinbase pays two outputs — 80% to the
miner and 20% to the consensus-known protocol fund address — with
both outputs subject to the same standard coinbase timelock. There
is no separate unlock schedule layered on the fund's share: it is a
normal coinbase output, spendable as soon as the standard relative
lock matures.

#### 80/20 split

Post-activation, the miner builds the v1 `coinbase-split` via a
dedicated arm `++ new-with-fund-share:coinbase-split` (in
`tx-engine-1.hoon`) which takes the block's `emission`, `fees`, and
the miner-side `shares` map separately:

```
fund-coins  = (div emission 5)                  :: subsidy only
miner-pool  = (sub emission fund-coins) + fees  :: 80% + all fees
miner-split = (++new miner-pool shares)         :: legacy proportional arm
output      = miner-split + { (fund-address, fund-coins) }
```

`shares` is the standard miner-side recipient map (1 or 2 PKHs
under the existing `max-coinbase-split = 2` cap), and is shared
with the pre-activation builder. Partner mode keeps working
post-activation: a 2-PKH `shares` produces a 3-output coinbase
(2 miner + fund), which the relaxed `++ based:coinbase-split:v1`
admits up to `max-coinbase-split + 1 = 3` entries.

The fund value is the integer floor `(div emission 5)`. Any per-
block atom remainder accrues to the miner pool (the higher-share
side), and within the miner pool the legacy `++new` arm assigns
the residual to whichever miner key sorts first in z-map order.
This rounding is exact at the NOCK level for every post-activation
reward (the schedule table values are all multiples of 5).

**Fees never enter the fund slot.** Computing the fund from
`(emission + fees)` would produce a coinbase that consensus rejects
as `%improper-fund-split`, since `+check-fund-split` keys on
`emission` alone — see "Review fixes" below.

The 80/20 split applies uniformly to every reward value from the
2,048 NOCK activation reward through the 64-NOCK tail floor. The
underlying emission curve is unchanged by the distribution layer:
total NOCK issued per block, per era, and across the full schedule
is identical to the values in the schedule table above. Only the
set of recipients differs.

The split is gated by a single phase boundary:

- `height < asert-phase` — single-output coinbase, 100% miner. (Pre-
  activation behaviour, unchanged.)
- `height ≥ asert-phase` — two-output coinbase, 80/20.

A future protocol upgrade is expected to flip the chain back to a
100%-miner coinbase once the new fully useful PoW puzzle ships on
the upgraded Nock ZKVM. That reversion is **not** part of this
upgrade; it will be specified and audited in a separate consensus
event when the puzzle is ready.

#### Effect on sell-flow

Under the standard assumption of ~100% miner sell-through to cover
electricity and hardware costs, post-activation sell-flow from
miners is `80%` of emissions rather than `100%`. The remaining `20%`
flows to the consensus-known fund address and is not automatically
sold; the fund's disposition is governed by a multisig, and its on-
chain behaviour is observable — any output moving from the fund to a
known exchange address can be tracked in real time, allowing
sophisticated participants to price in the fund's actual behaviour
rather than worst-case assumptions about it.

Combined with the per-time halving from the block-time change, the
effective post-activation miner sell-flow is `40%` of the pre-
activation baseline (`50%` from the per-time halving × `80%` from
the split-to-fund), with another `10%` of the pre-activation rate
flowing to the fund.

#### Validation

`open/hoon/apps/dumbnet/lib/consensus.hoon`'s coinbase enforcement adds
one new check, `+check-fund-split`, that runs when
`height ≥ asert-phase`:

- The coinbase is v1.
- The fund-address slot exists in the `coinbase-split` (a hardcoded
  `++ fund-address` arm in `tx-engine-1.hoon`).
- That slot's coin value equals `(div emission 5)` (the integer
  floor of 20% in atoms).

The miner-side allocation is implicitly verified: the existing
total-split-equals-(emission+fees) check at the top of
`+validate-page-with-txs` already pins the sum, and `+based:
coinbase-split` caps total entries at `max-coinbase-split + 1 = 3`,
so the miner side equals `(emission - fund-coins) + fees` partitioned
across one or two miner outputs. Both coinbase outputs use the same
standard coinbase timelock, enforced by the existing
`+based:coinbase-split` check and the per-output `timelock-intent`
field on each nnote.

#### Review fixes

Two issues caught in review of the initial implementation, both
addressed in this branch:

- **Fee handling.** The earlier `++ new-with-fund-share` took a
  single `assets` parameter that callers populated with
  `emission + fees`, so the fund slot was computed as
  `(div (emission + fees) 5)` — but consensus expects
  `(div emission 5)`. Any post-activation block with non-zero fees
  was self-rejected by the same miner that built it. The builder
  now takes `emission` and `fees` separately, routing fees entirely
  to the miner pool.
- **Multi-miner crash.** The earlier builder asserted exactly one
  `shares` key and discarded share weights, which crashed candidate
  construction whenever a closed-miner partner-mode configured two
  payout PKHs and the chain tip reached `asert-phase`. The builder
  now distributes the miner pool across `shares` via the same
  proportional arm as `++ new`, supporting 1- or 2-recipient
  miner-side configurations. `+check-fund-split` and
  `+based:coinbase-split:v1` were relaxed to admit the resulting
  3-entry coinbases (2 miners + fund) while still pinning the
  fund slot to `(div emission 5)`.

## Activation

- **Height**: 65,500 (`asert-phase`). All three changes — ASERT
  difficulty, 150s block time, new emissions schedule with 80/20
  miner/fund split — activate atomically at this single height.
- **Anchor block height**: 65,499 (`asert-phase - 1`).
- **Eon-2 truncation**: under the unified schedule, eon 2 ends at
  block 65,500 instead of its original 78,895. The 13,395 blocks of
  emission that would have been issued under the old curve at 16,380
  NOCK each (~219.4M NOCK) are absorbed into the new post-activation
  budget rather than minted at the legacy rate.
- **Coordination**: all nodes must upgrade before block 65,500. The
  anchor's median-of-11 is derived at target-compute time from
  `.blocks` and `.min-timestamps`, so there is no runtime capture
  step for operators to coordinate.
- **Fund address**: a real fund-multisig hash must be committed to
  `blockchain-constants.fund-address` before mainnet rollout. The
  reference implementation ships with a placeholder zero hash that
  must be replaced before activation; activating with the placeholder
  would direct fund-share outputs to an unspendable address.

## Migration

### Requirements

- Software version: 0.1.14+
- All nodes must upgrade before `asert-phase = 65,500`.

### Configuration

No mandatory configuration changes.

### Data Migration

No kernel-state version bump for this upgrade. The ASERT schema
extension to `blockchain-constants:v1` is already covered by the
existing `++ state-6-to-7` arm in `open/hoon/apps/dumbnet/inner.hoon`
(Aletheia's pre-existing kernel-state-7 bump). The emissions changes
are pure-logic edits to the `++ schedule` and `++ new:coinbase`
arms; the noun layout of `blockchain-constants` is unchanged.

The anchor's median-of-11 is still derived at target-compute time
from existing `.blocks` and `.min-timestamps` indices. Phase 2
(post-65500) will add an `asert-anchor-min-timestamp` constant,
which is read-only config and does not touch consensus state.

### Steps

1. Stop the node.
2. Update to version 0.1.14 or later before block 65,500.
3. Restart the node. State auto-upgrades through v7 on load.

### Rollback

Rollback to a pre-Aletheia binary is safe only before `asert-phase`.
After activation, downgrading will reject valid blocks (their targets
are computed by ASERT, their coinbases pay two outputs, and the
post-activation rewards are not on the legacy halving schedule).

## Backward Compatibility

### Breaking Changes

This is a **consensus-critical** upgrade. After activation:

- Nodes running pre-0.1.14 software will reject blocks with ASERT
  targets, and will compute expected targets from the old epoch
  algorithm.
- The `blockchain-constants` noun structure gains 6 ASERT fields,
  which causes decoding failures on pre-0.1.14 software.
- Pre-0.1.14 nodes will reject post-activation coinbases that pay two
  outputs (their validators expect the legacy single-output coinbase).
- The legacy `++ schedule:emission` arm produces the same NOCK/block
  values as the new schedule for blocks 1..=65,500, so a pre-0.1.14
  node that somehow sees a post-activation block will additionally
  reject it on emission validation (`emission-and-fees != total-split`).

### Network Partition Risk

Any node that does not upgrade before block 65,500 will:
- fork onto an incompatible chain,
- reject valid post-activation blocks, and
- have mined blocks rejected by upgraded nodes.

**All node operators must upgrade before block 65,500.**

### Transaction Compatibility

This upgrade does not change transaction formats. Transactions created
with pre-Aletheia software remain structurally valid.

## Security Considerations

- **Timestamp manipulation resistance.** ASERT reads median-of-11
  timestamps (via the existing `min-timestamps` map), not raw
  `pag.timestamp`. A single miner setting a bogus timestamp on one
  block cannot move the target more than the median-of-11 absorbs.
- **No sampling attack surface.** Unlike epoch retarget, ASERT
  recomputes at every block from a fixed anchor. There is no "next
  retarget boundary" a miner can influence by placing timestamps.
- **Anchor derivation is fork-correct.** The anchor's median-of-11
  is derived per-call by `+find-anchor-min-ts` walking `.blocks`
  from the caller's own `parent-digest` back to the ancestor at
  `asert-anchor-height`. No shared mutable scalar exists, so
  competing pre-activation forks at `asert-anchor-height` with
  different median-of-11 values produce divergent *ancestries*,
  not divergent *state*, and their post-activation descendants
  compute independently. Phase 2 (post-65500) replaces the walk
  with a hardcoded protocol constant paired with a checkpoint at
  anchor-height, at which point only one block at that height is
  admissible anywhere on the network.
- **Shift-magnitude bound.** Intermediate noun size is explicitly
  capped by clamping shift magnitudes before `lsh`/`rsh`, so
  pathological inputs cannot produce a state explosion.
- **No new cryptographic primitives.** The polynomial is a pure
  arithmetic approximation and introduces no new trust assumptions.
- **Hard cap is exact.** Cumulative emission summed across all
  blocks `1..=16,144,876` equals exactly `2^32` NOCK. Every tail
  block emits the same 64 NOCK, with no boundary carve-outs; a
  unit test pins the cumulative total to `2^32 × atoms-per-nock`,
  and `++ schedule` returns 0 for any height past 16,144,876.
  There is no asymptotic-tail trickle and no path to overemission.
- **80/20 split cannot be diverted.** `++ check-fund-split` in
  `consensus.hoon` rejects any post-activation coinbase whose split
  does not have exactly two entries with one entry's key equal to
  the consensus-known `fund-address` and that entry's coin value
  equal to `(div emission 5)`. A miner who attempts to redirect the
  fund's 20% to a different address produces a block that fails this
  check.

## Operational Impact

- **Block time drops from 600s to 150s.** Expect ~4× more blocks per
  unit time after activation. Mempool churn, block propagation
  requirements, and fee estimation tooling should be validated
  against the new cadence.
- **Per-block difficulty is lower by ~3.2–4× at activation.** Anchor
  target is `2^291`, chosen slightly conservative vs the ideal 150s
  target. ASERT converges to the ideal 150s from there under
  unchanged hashrate.
- **Per-block reward drops to 2,048 NOCK.** Per-time NOCK issuance
  exactly halves at activation (1,638 NOCK/min → 819.2 NOCK/min). A
  miner running fixed hardware sees their NOCK-per-minute revenue cut
  by 50%; the per-block reward cut is larger (87.5%) only because the
  block time also drops.
- **Reorg depth expectations shift.** Confirmation counts measured in
  blocks should be reinterpreted in wall-clock terms. Six blocks is
  now ~15 minutes of work, not ~1 hour.
- **Half-life is 12 hours.** A block taking `half-life` seconds
  longer than scheduled halves the difficulty (doubles the target);
  a block `half-life` seconds earlier than scheduled doubles the
  difficulty.
- **Effective-revenue cadence is annual after the activation era.**
  After the 6-month activation era at 2,048 NOCK, every era boundary
  is exactly one calendar year apart and produces either a 25% or a
  33% revenue drop (alternating). Mining-fleet planning horizons
  should be aligned to this annual cadence.
- **Coinbase outputs have changed.** Pre-activation coinbases pay one
  output (100% miner). Post-activation coinbases pay two: an 80%
  miner output and a 20% fund output. Both use the same standard
  coinbase timelock; there is no separate per-era unlock schedule.
  Wallets, block explorers, and accounting tooling should expect
  two coinbase outputs per block from height 65,500 onward.
- **Monitoring.** Operators should watch for:
  - first post-activation block at height 65,500 matching expected
    ASERT target,
  - target drift tracking ASERT convergence toward 150s over the
    first few half-lives,
  - block 65,501 emitting `2,048` NOCK total (eon 3 starts),
  - block 65,501's coinbase split producing exactly two outputs,
    with the fund's atom value `= 2,048 × 2^16 / 5` (integer
    floor; the `coinbase-split` proportional-allocation arm sends
    the per-block atom remainder to the higher-share recipient,
    so the miner output is the residual `total − fund`).

## Testing and Validation

### Unit tests — `closed/hoon/tests/dumb/mod/unit/asert.hoon`

Identity and monotonicity:
- `test-asert-anchor-identity`: at `blocks-since-anchor=1` with
  `current-min-timestamp = anchor-min-timestamp`, the computed target
  equals `anchor-target` exactly.
- `test-asert-on-schedule-approx-identity`: N blocks on schedule keeps
  the target at `anchor-target` (exponent = 0, factor = radix).
- `test-asert-monotonic-timestamp`: holding other inputs constant, a
  larger `current-min-timestamp` yields a strictly larger target.
- `test-asert-monotonic-height`: holding other inputs constant, a
  larger `current-height` yields a strictly smaller target.

Polynomial approximation (`+poly-factor`):
- `test-asert-poly-factor-zero`: polynomial equals `radix` exactly at
  `frac = 0`.
- `test-asert-poly-factor-monotonic`: polynomial is non-decreasing
  across `frac` samples spanning `[0, radix)`.
- `test-asert-poly-factor-near-one`: at `frac = radix - 1` the
  polynomial is within 200 of `2 * radix` (well inside the 0.13%
  bound).
- `test-asert-poly-factor-reversible`: PDF §1.2 reversibility —
  `poly-factor(f) * poly-factor(radix - f)` is within 0.33% of
  `2 * radix^2` for a range of `f` values.
- `test-asert-poly-factor-rejects-oversized-frac`: `+poly-factor`
  guard fails loudly at `frac = radix` and `frac = radix + 1`.

Exponent decomposition and branches (`+compute-exponent`,
`+decompose-exponent`):
- `test-asert-decompose-exponent-corners`: direct pins for the
  `exp = 0, radix-1, -1, -radix, -(radix+1)` corner inputs.
- `test-asert-compute-exponent-negative-time`: canary for the
  negative-time-diff branch (parent median below anchor median).

Halflife and clamping:
- `test-asert-halflife-doubles`: a block `half-life` seconds late
  approximately doubles the target (within 0.01%).
- `test-asert-halflife-halves`: a block `half-life` seconds early
  approximately halves the target.
- `test-asert-clamps-max-target`: extreme positive exponent saturates
  at `max-target-atom`.
- `test-asert-clamps-min`: extreme negative exponent saturates at 1
  (never 0).
- `test-asert-rejects-zero-anchor-target`: `+compute-target` guard
  fails loudly on a misconfigured `anchor-target = 0`.

Cross-checks against independent implementations:
- `test-asert-ref-cross-check`: `+compute-target` agreement with a
  separate reference implementation in the test file, across a grid
  of `(blocks-since, drift, anchor-target)` inputs including
  negative-time-diff cases.
- `test-asert-bn-wrapper`: `+compute-target-bn` matches
  `+compute-target` after bignum ↔ atom conversion.
- `test-asert-bch-nbits-expansion`, `test-asert-bch-nbits-roundtrip`:
  codec pins for the BCH compact-`nBits` encoding used by the run
  vectors.
- `test-asert-bch-run01` … `test-asert-bch-run12`: all 12 published
  BCH aserti3-2d QA vectors (143 iter rows total) reproduced via
  `+compute-target:asert` under BCH mainnet parameters.

### Integration tests — `closed/hoon/tests/dumb/mod/integration/asert-activation.hoon`

- `test-asert-wrapper-matches-library`: the consensus wrapper
  `+compute-target-asert` matches a direct call to
  `compute-target:asert` with the same inputs at the first
  post-anchor height. The test resolves `anchor-min-ts` via
  `+find-anchor-min-ts`, exercising the parent-walk against the
  trivial one-step case where `parent == anchor`.
- `test-asert-wrapper-past-anchor`: same, one block further past
  the anchor, exercising the case where `parent-min-ts` diverges
  from `anchor-min-ts`. Drives a two-step `+find-anchor-min-ts`
  walk so the fallthrough branch runs end-to-end.

### Emissions — `closed/hoon/tests/dumb/mod/unit/emissions.hoon`

Schedule pin tests:
- `test-eon-2-truncation`: `++ schedule` at height 65,500 returns
  the eon-2 reward (16,384 NOCK in atoms); at 65,501 returns the
  eon-3 reward (2,048 NOCK in atoms).
- `test-decay-table`: pin every era boundary — heights 170,501
  (1,536 NOCK), 380,501 (1,024), 590,501 (768), 800,501 (512),
  1,010,501 (384), 1,220,501 (256), 1,430,501 (192), 1,640,501
  (128), 1,850,501 (96), 2,060,501 (64).
- `test-tail-floor`: a sample of tail-phase heights all return 64
  NOCK (e.g. 2,060,501, 5,000,000, 10,000,000, 16,144,876).
- `test-tail-final-block`: schedule(16,144,876) returns 64 NOCK
  (last non-zero block); schedule(16,144,877) returns 0.
- `test-post-cap`: schedule(16,144,877) returns 0; schedule(20,000,000)
  returns 0.
- `test-supply-totals-to-cap`: cumulative `++ schedule` over
  `1..=16,144,876` equals exactly `(bex 32) * atoms-per-nock` =
  `2^32 NOCK` worth of atoms. This is the load-bearing cap-accounting
  invariant.

### Coinbase split — builder unit tests

`closed/hoon/tests/dumb/mod/unit/coinbase-split.hoon` covers
`++ new-with-fund-share` directly:

- `test-split-pre-activation`: at `height < asert-phase`, miner
  passes `shares = {(miner-hash, 1)}`; the resulting v1 coinbase-
  split has one entry, full emission to the miner.
- `test-split-post-activation`: zero-fee post-activation builder
  produces fund = `(div emission 5)` and miner-coins the residual.
- `test-split-post-activation-rounding`: pins per-block atom-
  remainder behaviour at emission = 134,217,728 atoms.
- `test-split-post-activation-with-fees`: pins fee handling — fund
  stays at `(div emission 5)` regardless of fees; the miner pool
  is `(emission - fund) + fees`. **Regression test for the bug
  where the builder used `(emission + fees) / 5` for the fund slot.**
- `test-split-post-activation-two-miners` /
  `…-two-miners-with-fees`: pin partner-mode multi-miner support
  (two miner PKHs + fund), at zero fees and with fees.
- `test-split-rejects-fund-in-shares`: builder crashes if `shares`
  aliases the fund-address, preventing the honest miner from
  paying the fund slot twice by mistake.
- `test-split-fund-address-is-zero-hash`: pins the placeholder so
  shipping with the zero hash on mainnet is obvious.

### Coinbase split — consensus rejection tests

`closed/hoon/tests/dumb/mod/integration/fund-split.hoon` drives
`validate-page-with-txs` directly with hand-crafted post-activation
coinbases at `bc-fund-split` (v1-phase = asert-phase = 5):

- `test-fund-split-accepts-honest-block`: building 5 blocks reaches
  the first post-activation block; the candidate produced by
  `++ new-candidate` validates.
- `test-fund-split-accepts-two-miners-plus-fund`: 3-entry coinbase
  with 2 miners + fund-address at the canonical share is accepted
  (pins multi-miner support).
- `test-fund-split-rejects-no-fund-slot`: 2-entry coinbase summing
  to emission with no fund-address slot — `%improper-fund-split`.
- `test-fund-split-rejects-three-entries-no-fund-slot`: 3-entry
  coinbase summing to emission with no fund-address slot —
  `%improper-fund-split`. (The legacy `=(2 ~(wyt z-by +.cb))` pin
  has been relaxed to enable multi-miner partner mode; the rule
  now binds on the fund slot, not the entry count.)
- `test-fund-split-rejects-wrong-fund-share`: 2-entry coinbase with
  the fund-address slot present but with a non-canonical amount
  (`(div emission 5) + 1`).
- `test-fund-split-rejects-wrong-fund-address`: 2-entry coinbase
  totalling emission with the canonical fund amount paid to a
  non-fund-address hash.

### Kernel-state upgrade

No new kernel-state upgrade arm is required for the emissions
changes. Aletheia's pre-existing `state-6-to-7` upgrade
(commit `3aecbbf1d`) already covers the only schema extension to
`blockchain-constants:v1` shipping in this version, namely the six
ASERT fields. The emissions logic is layered on top of the existing
constants without changing the noun layout, so a saved v6 state on a
node loading 0.1.14 software auto-upgrades to v7 the same way it
would have under 0.1.13.

### Anchor-pinning sanity

Once mainnet block 65,500 is produced, run a fixture chain from
genesis through activation and verify:
- the derived `anchor-min-ts` when computing the target for block
  65,500 equals the observed mainnet median-of-11 at height **65,499**
  (the anchor block), not 65,500 — `+find-anchor-min-ts` walks from
  the parent (block 65,499) and stops immediately because
  `parent-height == asert-anchor-height`,
- first ASERT target at height 65,500 matches an offline
  reference-implementation computation against real anchor inputs,
- no off-by-one in anchor-height vs anchor-min-ts vs parent-min-ts
  at heights 65,499, 65,500 and 65,501.

## Phase 2 — post-anchor cutover

Once block 65,500 is mined on the canonical chain, a follow-up
release replaces the runtime parent-walk with hardcoded values.
This removes `+find-anchor-min-ts` from the hot path and makes any
attempt to mine a competing block at 65,499 (anchor) or 65,500 (activation) inadmissible
network-wide via a consensus checkpoint.

### Values to bake in

- `asert-anchor-digest`: the digest of the observed mainnet block
  at height **65,499** — the ASERT anchor block (to be filled in
  once that block is final). Added to `checkpointed-digests` at
  height 65,499 so `+validate-page-without-txs` rejects any
  competing block at the anchor.
- `asert-anchor-min-timestamp` (constant): the median-of-11
  timestamp at the canonical block **65,499** (equal to
  `min-timestamps[asert-anchor-digest]`). This is exactly what
  Phase 1's `+find-anchor-min-ts` returns — pinning it here
  preserves bit-for-bit continuity across the cutover. Added as a
  `blockchain-constants` field.
- `asert-activation-digest` (checkpoint only): the digest of the
  observed mainnet block at height **65,500** — the first ASERT
  block. Added to `checkpointed-digests` at height 65,500
  separately from the anchor, so no competing first-ASERT block is
  admissible post-cutover. This value is not used in target
  computation.

### Code changes at cutover

- `open/hoon/common/tx-engine-1.hoon`: add
  `asert-anchor-min-timestamp=@` to `blockchain-constants` and
  set its value in the realnet constants.
- `open/hoon/apps/dumbnet/lib/consensus.hoon`:
  - `+compute-target-asert` reads the new constant directly; drop
    the `+find-anchor-min-ts` call.
  - Delete `+find-anchor-min-ts`.
- `open/hoon/apps/dumbnet/lib/miner.hoon`: read the new constant
  directly; drop the `+find-anchor-min-ts` call and the `dcon`
  import if no longer needed.
- Add `[65.499 asert-anchor-digest]` and
  `[65.500 asert-activation-digest]` to the realnet
  `checkpointed-digests` map.

### Tests at cutover

- `test-asert-wrapper-matches-library` and
  `test-asert-wrapper-past-anchor` still apply: they resolve
  `anchor-min-ts` against the same mechanism the production code
  uses, so they switch from "walks .blocks" to "reads constant"
  automatically.
- Add tests that competing blocks at height 65,499 (anchor) and
  65,500 (first ASERT block) are each rejected by their respective
  checkpoint entries.

## Reference Implementation

- ASERT branch: `la/asert`
- Emissions branch: `la/emissions` (built atop `la/asert`)
- Key commits (ASERT):
  - `fe017f962` — ASERT initial implementation
  - `8026c2f4b` — aserti3-2d anchor and timing params
  - `e4cd48cb0` — capture anchor min-timestamp into consensus state
  - `3aecbbf1d` — kernel-state v7 bump for ASERT constants schema

Primary files (ASERT):
- `open/hoon/apps/dumbnet/lib/asert.hoon` (new)
- `open/hoon/apps/dumbnet/lib/consensus.hoon` (modified: wrapper +
  `+find-anchor-min-ts` helper + activation gating)
- `open/hoon/apps/dumbnet/lib/miner.hoon` (modified: derive anchor
  via `+find-anchor-min-ts`)
- `open/hoon/common/tx-engine-1.hoon` (modified: blockchain-constants
  fields)
- `closed/hoon/tests/dumb/mod/unit/asert.hoon` (new)
- `closed/hoon/tests/dumb/mod/integration/asert-activation.hoon` (new)

Primary files (emissions):
- `open/hoon/common/schedule.hoon` (rewritten: 14-row unified table,
  exact-cap accounting via 47-block tail extension)
- `open/hoon/common/tx-engine-1.hoon` (modified: `++ fund-address`
  arm)
- `open/hoon/apps/dumbnet/lib/miner.hoon` (modified: 80/20 share
  construction post-activation)
- `open/hoon/apps/dumbnet/lib/consensus.hoon` (modified:
  `+check-fund-split`)
- `closed/hoon/tests/dumb/mod/unit/emissions.hoon` (new)
- `closed/hoon/tests/dumb/mod/unit/coinbase-split.hoon` (new)
