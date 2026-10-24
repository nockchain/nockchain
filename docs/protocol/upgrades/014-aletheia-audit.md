# 014-aletheia — ASERT audit vs. da-asert.pdf

Audit of the Aletheia (aserti3-2d) implementation and spec on branch
`la/asert` against:

- Mark Lundeberg, *DA-ASERT v.2* (`da-asert.pdf`, July 31 2020).
- Jonathan Toomim's aserti3-2d as shipped on Bitcoin Cash in 2020
  (`bitcoincashorg/bitcoincash.org/spec/2020-11-15-asert.md`,
  `upgradespecs.bitcoincashnode.org/2020-11-15-asert/`, BCHN QA vectors).

Scope: the changelog (`014-aletheia.md`), the library
(`open/hoon/apps/dumbnet/lib/asert.hoon`), its consensus wrapper
(`open/hoon/apps/dumbnet/lib/consensus.hoon`), and the unit / integration
tests (`closed/hoon/tests/dumb/mod/unit/asert.hoon`,
`closed/hoon/tests/dumb/mod/integration/asert-activation.hoon`).

This is a paper-audit only. No behaviour was executed; no fix is proposed.
Each flag is for reviewer triage.

## Summary

| # | Item                                                          | Kind        | Severity |
| - | ------------------------------------------------------------- | ----------- | -------- |
| 1 | Changelog formula has `(height-diff+1)`, code doesn't  *(resolved)* | Discrepancy | HIGH     |
| 2 | Anchor timestamp convention ≠ BCH's (parent-of-anchor)  *(resolved)* | Discrepancy | MEDIUM   |
| 3 | Polynomial missing `+2^47` rounding term  *(resolved)*        | Discrepancy | LOW      |
| 4 | `(need ...)` crashes accept/validate on minority-fork blocks  *(resolved — Phase 1)* | Bug         | HIGH     |
| 5 | Set-once anchor capture is not reorg-safe at anchor-height  *(resolved — Phase 1)* | Bug         | MEDIUM   |
| 6 | `current-min-timestamp` semantics inconsistent lib vs wrapper  *(resolved)* | Bug         | MEDIUM   |
| 7 | `+poly-factor` does not validate `frac < radix`  *(resolved)* | Bug         | LOW      |
| 8 | `anchor-target == 0` is silently rewritten to 1  *(resolved)* | Bug         | LOW      |
| 9 | No production-semantic activation target test  *(resolved)*    | Test gap    | HIGH     |
| 10 | No BCH-parity test for polynomial + shift stages  *(resolved)* | Test gap  | MEDIUM   |
| 11 | No reorg-safety tests for anchor capture  *(resolved)*        | Test gap    | HIGH     |
| 12 | Memorylessness / reversibility (PDF §1.2) untested  *(resolved — polynomial; end-to-end redundant with on-schedule identity)* | Test gap    | MEDIUM   |
| 13 | Wrapper-level negative-`time_diff` path untested  *(resolved — library seam)* | Test gap    | MEDIUM   |
| 14 | `+decompose-exponent` corner cases not in isolation  *(resolved)* | Test gap    | LOW      |
| 15 | Test names in changelog ≠ test names in code  *(resolved)* | Docs drift  | LOW      |

---

## Discrepancies

### 1 — Changelog formula ≠ implementation  *(HIGH; consensus-critical — RESOLVED)*

Changelog `014-aletheia.md:115`:

```
exponent = ((time-diff - ideal * (height-diff + 1)) * radix) / half-life
```

Implementation `asert.hoon:47` (`+compute-exponent`):

```
=/  ideal-total  (mul ideal blocks-since-anchor)   :: no +1
```

`blocks-since-anchor = current-height - anchor-height`
(`asert.hoon:97`), so the code computes

```
exponent = (time-diff - ideal * height-diff) * radix / half-life
```

— differing from the changelog by exactly one `ideal / half-life` term.
With mainnet parameters (`ideal=150`, `half-life=43 200`) that is a
`~0.24 %` constant target bias versus what the spec text describes. The
changelog is marked `consensus_critical = true`; spec and code must be in
lockstep for anyone re-implementing from the doc. One of them is wrong.

**Resolution:** fix applied as the nockchain-convention variant (item 2
Option b). `+compute-exponent` now uses `ideal * (blocks-since-anchor - 1)`
at `asert.hoon:55`, matching PDF Eq. (2) under §1.3 Option 2 (anchor's
own median-of-11). Changelog formula at `014-aletheia.md:115` updated
to `ideal * (height-diff - 1)` with the convention spelled out in the
adjacent paragraph. See item 2 resolution for the convention rationale.

### 2 — Anchor timestamp convention differs from BCH aserti3-2d  *(MEDIUM — RESOLVED)*

BCH aserti3-2d (confirmed against `upgradespecs.bitcoincashnode.org` and
the official spec at
`bitcoincashorg/bitcoincash.org/spec/2020-11-15-asert.md`) defines:

```
time_diff   = parent.time   - anchor_parent.time      :: ANCHOR'S PARENT
height_diff = parent.height - anchor.height
exponent    = (time_diff - ideal*(height_diff+1)) / half_life
```

The `+1` in BCH's formula exactly compensates for `time_diff` spanning
(anchor_parent → parent), which is `height_diff + 1` ideal intervals.

Nockchain captures the **anchor block's own** median-of-11, not its
parent's:

```
consensus.hoon:580
=/  anchor-min-ts=@
  (~(got z-by min-timestamps.c) ~(digest get:page:t cur))
c(asert-anchor-min-timestamp `anchor-min-ts)
```

Under that convention, to match PDF Eq. (2) the subtracted term should
be `ideal * (height_diff - 1)` (where `height_diff = child_height -
anchor_height`), not `ideal * height_diff` (implementation) or
`ideal * (height_diff + 1)` (changelog).

So **neither** the changelog nor the implementation is right for the
anchor-ts convention the code actually uses. The impl is off by
`-ideal/half_life`; the spec text is off by `-2·ideal/half_life`. Practical
impact at mainnet: a systematic `-0.24 %` (impl) or `-0.48 %` (spec) target
bias that is silently absorbed into the chosen `2^291` anchor target.
Worth either (a) switching to capturing the anchor's parent min-ts and
adopting the BCH formula verbatim, or (b) keeping the current convention
but correcting the `+1` accounting and documenting the choice.

**Resolution:** took option (b). Nockchain's pre-activation DAA is
Bitcoin-style epoch retarget, not relative ASERT, so the BCH rationale
for anchor-parent-ts (continuity with Eq. (1) at activation per PDF
§1.3) does not apply. Keeping anchor's own median-of-11 as the reference
is sanctioned by PDF §1.3 Option 2 and avoids walking one extra parent
during capture. The formula is corrected at `asert.hoon:55` and the
precondition tightened at `asert.hoon:104` to `current-height >
anchor-height`. Unit-test helpers (`+on-schedule`, `+drift`,
`+ref-compute-target`) now feed production semantics, and
`+bch-vector-check` shifts BCH's t_{M-1} → t_M by `+bch-ideal` at the
helper boundary so the 12 BCH QA vectors verify verbatim under Option 2.
Commit `de56b4167`; all 30 asert arms pass.

### 3 — Polynomial missing `+2^47` rounding term  *(LOW; BCH-divergence — RESOLVED)*

Canonical BCH `aserti32d.cpp`:

```c
factor = 65536 + ((
    195766423245049ull*e + 971821376ull*e*e + 5127ull*e*e*e +
    (1ull<<47)
) >> 48);
```

`asert.hoon:32-33`:

```hoon
=/  num  :(add (mul 195.766.423.245.049 frac) (mul 971.821.376 f2) (mul 5.127 f3))
(add asert-radix (rsh [0 48] num))
```

`rsh [0 48]` is floor, not round-to-nearest. Missing
`(add num (bex 47))` inside the shift. At most 1 LSB of `factor`
difference per evaluation, but it breaks parity with BCH's published
vectors at the polynomial stage. The changelog already caveats that BCH
vectors aren't reusable *because of median-of-11*; this is the other
reason they aren't reusable, which should be noted or fixed.

**Resolution:** fix applied at `asert.hoon:36` —
`(rsh [0 48] (add num (bex 47)))` now matches BCH canonical
round-to-nearest. BCH-vector parity in the polynomial + shift pipeline
was validated; see item 10.

---

## Potential bugs

### 4 — `(need asert-anchor-min-timestamp.c)` can crash accept/validate  *(HIGH — RESOLVED, Phase 1)*

`consensus.hoon:215`:

```hoon
=/  anchor-min-ts=@  (need asert-anchor-min-timestamp.c)
```

`+capture-asert-anchor-if-needed` (`consensus.hoon:566-582`) only runs
from `+update-heaviest` and only writes when the **heaviest** chain first
reaches `asert-anchor-height`. The comment on `consensus.hoon:213-214`
asserts the invariant:

> by the time any caller asks for an aserti3-2d target
> (child-height >= asert-phase), heaviest has already advanced past the
> anchor, so (need ...) succeeds.

But `+compute-target-asert` is called from:

1. `+accept-page` target write, `consensus.hoon:323` — for **every**
   accepted post-activation block, heaviest or not.
2. `+validate-page-without-txs`, `consensus.hoon:407` — for every
   validated post-activation block.

Failure scenario: fork A is heaviest at height < `asert-anchor-height`.
Fork B has been received with blocks through `asert-phase` but is not
heaviest (e.g., it came in while the node was still catching up, or its
cumulative work is less than A's). When B's block at `asert-phase`
hits accept/validate, `(need ...)` on the still-empty unit panics. This
is a remote-triggerable crash.

Mitigation candidates: (a) fire capture on *any* accepted block at
`asert-anchor-height` keyed by its digest, and store per-digest so reorgs
select the right one; (b) inside `+compute-target-asert`, if the unit is
`~`, walk from `parent-digest` back to `anchor-height` and read
`min-timestamps` there — no consensus-state write, just a local
computation; (c) if capture hasn't happened, reject the block with
`%page-target-invalid` (or a fresh reason) rather than panicking.

**Resolution (Phase 1):** adopted option (b), and removed the scalar
cache entirely. `+compute-target-asert` now derives `anchor-min-ts`
on every call by invoking a new `+find-anchor-min-ts` helper in
`consensus.hoon` that walks `parent-digest` back through `.blocks`
to the ancestor at `asert-anchor-height` and reads its median-of-11
from `.min-timestamps`. No consensus-state field is introduced —
the state-version-7 bump and the `+capture-asert-anchor-if-needed`
arm were reverted. `miner.hoon` now calls the same helper against
heaviest-block. Because there is no shared mutable cache, the path
is fork-correct by construction (every caller walks its own
ancestry). Tests in
`closed/hoon/tests/dumb/mod/integration/asert-activation.hoon`
exercise the 1-step and 2-step walk cases end-to-end. Phase 2
(below) replaces the walk with a hardcoded constant + checkpoint at
anchor-height, at which point `+find-anchor-min-ts` is deleted.

### 5 — Set-once anchor capture is not reorg-safe at anchor-height  *(MEDIUM — RESOLVED, Phase 1)*

`consensus.hoon:565-568`:

```hoon
::  subsequent calls are silent no-ops: set-once, even across reorgs
::  through anchor-height.
++  capture-asert-anchor-if-needed
  ^-  consensus-state:dk
  ?^  asert-anchor-min-timestamp.c  c  :: already set - silent no-op
```

The changelog (`014-aletheia.md:336-340`) defends this:

> Pre-activation blocks (including the anchor) are shared history across
> any post-activation reorg, so the captured `asert-anchor-min-timestamp`
> remains valid under reorg.

That claim is a convention, not an invariant. Pre-activation chains can
fork, and two forks can both have blocks at `anchor-height` with
different median-of-11 timestamps (different ancestors populate the
median window differently). If two nodes capture from different
initially-heaviest forks, then later converge on the same post-activation
heaviest chain, their captured `asert-anchor-min-timestamp` values stay
divergent — producing different ASERT targets for the same child and a
consensus split.

The risk is small on a quiet chain but non-zero around activation and is
a fundamental property of the set-once design. Options: capture keyed by
digest rather than as a scalar; require the anchor-height block to be
buried by N confirmations before capture; update capture when a reorg
replaces the block at `anchor-height`.

**Resolution (Phase 1):** fully resolved by removing the scalar
entirely. `+compute-target-asert` and the miner candidate-target path
now derive `anchor-min-ts` per-call via `+find-anchor-min-ts`, which
walks `.blocks` from the caller's own `parent-digest` back to the
ancestor at `asert-anchor-height`. There is no shared mutable state,
so two nodes that observed different pre-activation forks at
anchor-height cannot diverge each other's target computation — each
target compute resolves against the specific fork the block being
validated lives on, which is the correct behaviour. Phase 2
(post-67000) will additionally checkpoint the anchor-height digest,
making the block at that height unique network-wide, and replace
the walk with a `blockchain-constants` value; see "Phase 2 —
post-anchor cutover" in `014-aletheia.md`.

### 6 — `current-min-timestamp` semantics inconsistent library vs wrapper  *(MEDIUM — RESOLVED)*

The wrapper passes **parent's** min-ts together with **child's** height
(`consensus.hoon:215-227`):

```hoon
=/  parent-min-ts=@  (~(got z-by min-timestamps.c) parent-digest)
...
%-  compute-target:asert
:*  asert-anchor-target-atom.blockchain-constants
    anchor-min-ts
    asert-anchor-height.blockchain-constants
    parent-min-ts         :: passed as current-min-timestamp
    child-height          :: passed as current-height
    ...
==
```

The unit tests (`closed/hoon/tests/dumb/mod/unit/asert.hoon:26-38`) feed
them as a same-block pair:

```hoon
++  on-schedule
  |=  [blocks-since=@ anchor=@]
  %-  compute-target:asert
  :*  anchor
      anchor-ts
      anchor-h
      (add anchor-ts (mul blocks-since ideal))   :: child's timestamp
      (add anchor-h blocks-since)                :: child's height
      ...
```

On the unit-test semantics, perfect schedule produces `exponent = 0` and
`target = anchor_target` — `test-asert-on-schedule-approx-identity` passes.
On the production semantics (parent-ts, child-height), perfect schedule
produces `exponent = -ideal/half_life` and
`target ≈ 0.9976 * anchor_target`. The tests cannot catch this because
they never instantiate the wrapper-style argument pairing.

This is the root cause that makes Discrepancies 1 and 2 invisible to CI.

**Resolution:** subsumed by the fix for items 1 and 2. The library now
documents the anchor-own-ts / parent-ts / child-height convention
explicitly on `+compute-exponent` and `+compute-target`
(`asert.hoon:38-46`, `asert.hoon:83-92`), matching the wrapper. Unit-test
helpers were rewritten to feed production semantics: `+on-schedule` and
`+drift` now produce parent timestamps via `(mul (dec blocks-since)
ideal)`, `+ref-compute-target` uses the same `(dec bsa)` factor with a
tightened `lte` precondition, and `test-asert-anchor-identity` pins the
activation case (child at `anchor+1`, parent = anchor) to
`anchor-target` exactly. Commit `de56b4167`.

### 7 — `+poly-factor` does not validate `frac < radix`  *(LOW — RESOLVED)*

`asert.hoon:27-33`. `+decompose-exponent` always produces
`frac ∈ [0, radix)` today (`asert.hoon:64-73`), so no in-tree caller can
misuse it. But `+poly-factor` is publicly exported by the `asert` core,
is called from tests, and will be callable from any future
re-implementation of the exponent pipeline. A defensive
`?>  (lth frac asert-radix)` fails loudly on misuse vs silently returning
a nonsense factor.

**Resolution:** guard added at the head of `+poly-factor` in
`open/hoon/apps/dumbnet/lib/asert.hoon` (just after the `^-  @`
line). Docstring already stated the `[0, radix)` precondition; the guard
now enforces it. New unit test
`test-asert-poly-factor-rejects-oversized-frac` in
`closed/hoon/tests/dumb/mod/unit/asert.hoon` pins crash behaviour at
`frac = radix` and `frac = radix + 1` via `expect-fail`.

### 8 — `anchor-target == 0` silently rewrites to 1  *(LOW — RESOLVED)*

`asert.hoon:108`: `unshifted = anchor-target * factor`. If
`anchor-target = 0`, `unshifted = 0`, `(met 0 unshifted) = 0`, any
negative-shift path returns 0 which is then clamped to 1
(`asert.hoon:123`). Similarly, any positive-shift path shifts zero →
zero → 1. A misconfigured `blockchain-constants` with
`asert-anchor-target-atom = 0` would turn the DA into "every post-activation
block targets hash = 1" — effectively a frozen chain — silently. An
early `?>  !=(0 anchor-target)` would surface this on first call.

**Resolution:** guard added at the head of `+compute-target` in
`open/hoon/apps/dumbnet/lib/asert.hoon`, immediately after the existing
`current-height > anchor-height` precondition (`?<  =(0 anchor-target)`).
A misconfigured `asert-anchor-target-atom = 0` now crashes on the first
post-activation target compute rather than silently freezing the chain
at target = 1. New unit test `test-asert-rejects-zero-anchor-target` in
`closed/hoon/tests/dumb/mod/unit/asert.hoon` pins the crash via
`expect-fail`.

---

## Test gaps

### 9 — Production-semantic activation target test  *(HIGH — RESOLVED)*

The integration file `asert-activation.hoon` only compares the wrapper
against a *direct library call with the same arguments* — any mis-mapping
between wrapper and library (Discrepancy 1 and/or 2) matches itself.

Proposed test: at activation (`child-height = anchor-height + 1`, parent
= anchor, so `parent-min-ts = anchor-min-ts`), assert the target equals
the value prescribed by Eq. (2) under the nockchain anchor convention
— either `anchor-target` exactly (if the `+1` / anchor-parent question
is resolved to match PDF Eq. (2)), or
`anchor-target * 2^(-ideal/half_life)` (if the current code is deemed
correct). Either way, the test pins a number that is *derived
externally*, not by self-consistency.

**Resolution:** `test-asert-wrapper-activation-identity` in
`closed/hoon/tests/dumb/mod/integration/asert-activation.hoon` builds
the chain up to the anchor and calls `+compute-target-asert` at
`child-height = anchor-height + 1`. Under the anchor-own-ts
convention (item 2 Option b) the exponent at activation is exactly 0,
so the emitted target must equal `asert-anchor-target-atom.bc`.  The
expected value is the bc constant itself — derived externally from
PDF Eq. (2), not by self-consistency with a library call on the same
inputs. If any wrapper-to-library mis-mapping were introduced, this
test would emit a different number at activation and fail.

### 10 — BCH-parity test for polynomial + shift stages  *(MEDIUM — RESOLVED)*

The changelog (`014-aletheia.md:158-160`) justifies dropping BCH vectors
because "BCH's published aserti3-2d test vectors (which use raw
timestamps) are not directly reusable". That argument only covers the
time-diff stage. The polynomial, shift decomposition, and saturating
shift arithmetic are timestamp-agnostic and can be cross-checked against
BCH's vectors by feeding the BCH exponent directly into the downstream
pipeline and comparing `factor` and `next_target` shifts. Such a test
would catch Discrepancy 3 and any future drift in `+poly-factor`.

**Resolution:** all 12 BCH aserti3-2d QA runs (runs 01–12, 143
iter vectors total) are embedded as unit tests in
`closed/hoon/tests/dumb/mod/unit/asert.hoon`. The suite reuses
`+compute-target:asert` directly with `(ideal=600, half-life=172.800,
max-target=0xffff<<208)` — the BCH mainnet parameters — and asserts
per-iter `nBits` parity via `+bch-vector-check`. Two helper sanity
tests (`test-asert-bch-nbits-expansion`,
`test-asert-bch-nbits-roundtrip`) pin the compact-nBits codec that the
check relies on. All 30 asert arms pass under `roswell test-dumb`
(187 OK, 0 FAIL, post-`+2^47` fix).

### 11 — Reorg-safety of anchor capture  *(HIGH — RESOLVED, under Phase 1 walk)*

`test-asert-anchor-captured-at-activation` builds one chain and checks
the happy path. Missing:

- Two forks both reach `asert-anchor-height` with **different**
  min-of-11 values. Which wins? Is the outcome independent of the
  order in which blocks were received?
- Pre-activation reorg that replaces the block at `anchor-height` on
  the heaviest chain. Expected behaviour needs to be specified
  (currently: no-op, see bug 5) and asserted.
- Post-activation reorg through `anchor-height`. Captured value must
  not change (currently true by no-op) and should be asserted so a
  future refactor can't break it.

**Resolution:** under Phase 1 there is no capture — `+find-anchor-min-ts`
walks from the caller-supplied parent-digest per-call, so the three
sub-items collapse to a single property: the walk must trace the
caller's own ancestry and be indifferent to the heaviest pointer.
`test-asert-find-anchor-walk-fork-safety` in
`closed/hoon/tests/dumb/mod/integration/asert-activation.hoon` pins
this property end-to-end:

- Builds chain A to height 5 as heaviest.
- Builds a sibling chain B = G → A1 → B2 → B3 → B4, where B2 uses a
  bumped timestamp so B4's median-of-11 diverges from A4's by
  construction.
- Accepts B2..B4 via `+accept-page` but never promotes them via
  `+update-heaviest`, so heaviest-block stays on A5.
- Asserts that `+find-anchor-min-ts(a5-digest)` returns A4's median,
  `+find-anchor-min-ts(b4-digest)` returns B4's own (different)
  median, and that the heaviest pointer never moved off chain A.

This covers the "two forks with different min-of-11" sub-item
verbatim, the "pre-activation reorg replacing anchor-height" case by
showing two distinct anchor-height blocks coexist in `.blocks` and
each gets its own correct walk result, and the "post-activation reorg
through anchor-height" case by showing heaviest-movement is irrelevant
to the walk. The older `test-asert-wrapper-past-anchor` additionally
pins that the walk works on a descendant two steps past the anchor.

### 12 — Memorylessness / reversibility (PDF §1.2)  *(MEDIUM — RESOLVED)*

The PDF puts reversibility at the centre of ASERT's design: a drift of
`+x` followed by `-x` returns to the exact pre-perturbation target
(modulo approximation error). Two concrete unit tests would pin this:

- **Polynomial reversibility**: pick `y ∈ (0,1)` and check that
  `poly-factor(y*radix) * poly-factor((1-y)*radix)` is within tolerance
  of `2 * radix^2` (since `2^y · 2^{1-y} = 2`).
- **End-to-end reversibility**: chain of (on-schedule → fast by Δ → slow
  by Δ → on-schedule) blocks returns to `anchor-target` within
  polynomial tolerance.

**Resolution:** the polynomial-level reversibility check is now
pinned by `test-asert-poly-factor-reversible` in
`closed/hoon/tests/dumb/mod/unit/asert.hoon` — it asserts
`poly-factor(f) * poly-factor(radix - f)` lands within 0.33% of
`2 * radix^2` across `f ∈ {1, radix/4, radix/2, 3*radix/4, radix-1}`.
End-to-end chain reversibility is mathematically equivalent to the
on-schedule identity under ASERT's stateless design — the target at
any post-anchor block depends only on `parent_ts - anchor_ts` and
`child.height - anchor.height`, so a chain whose net drift is zero
lands at `anchor-target` by the same code path exercised by
`test-asert-on-schedule-approx-identity`. No additional chain-level
test adds coverage that the polynomial test and the on-schedule test
do not already cover.

### 13 — Wrapper-level negative-`time_diff` path  *(MEDIUM — RESOLVED, library seam)*

`+compute-exponent` has a third branch for negative `time_diff`
(`asert.hoon:54-55`). The cross-check test covers it in-library but no
wrapper-or-integration test drives `parent-min-ts < anchor-min-ts`.
Median-of-11 can step backwards across a reorg involving
pre-activation blocks (different ancestors → different medians), so the
branch is reachable in production.

**Resolution:** covered at the library seam rather than the wrapper
seam. `test-asert-compute-exponent-negative-time` in
`closed/hoon/tests/dumb/mod/unit/asert.hoon` directly pins that the
third branch emits `sign = %.n` for `time-diff-sign = %.n`; the
existing `test-asert-ref-cross-check` and `test-asert-halflife-halves`
already exercise the branch end-to-end through `+compute-target`; and
the wrapper cross-checks (`test-asert-wrapper-matches-library`,
`test-asert-wrapper-past-anchor`) prove the wrapper forwards
`parent-min-ts` and `anchor-min-ts` to the library unchanged — so any
library-seam coverage of negative time-diff transfers to the wrapper.
Synthesising a median-of-11 regression on a real chain is deferred to
item 11 where fork helpers are introduced; the library-seam test is
load-bearing for the branch itself.

### 14 — `+decompose-exponent` corner cases  *(LOW — RESOLVED)*

Currently only exercised end-to-end via `+ref-compute-target`. Direct
unit tests would shorten the debugging chain when something further
downstream changes:

- `exp = 0` → `(%.y, 0, 0)`
- `exp = radix - 1` positive → `(%.y, 0, radix-1)`
- `exp = -1` → `(%.n, 1, radix-1)` (round-up path)
- `exp = -radix` exact → `(%.n, 1, 0)`
- `exp = -radix - 1` → `(%.n, 2, radix-1)`

**Resolution:** `test-asert-decompose-exponent-corners` in
`closed/hoon/tests/dumb/mod/unit/asert.hoon` pins all five cases
verbatim via a single vase-level `expect-eq` on a list of inputs and
expected outputs. If any future rounding refactor perturbs the
boundary behaviour, this test surfaces it before anything downstream.

### 15 — Test names in changelog ≠ in code  *(LOW; docs drift — RESOLVED)*

Changelog (`014-aletheia.md:374-392`) lists:

- `test-asert-anchor-returns-identity`
- `test-asert-poly-factor-known-values`
- `test-asert-halflife-scaling`
- `test-asert-reference-impl`

Code (`closed/hoon/tests/dumb/mod/unit/asert.hoon`) defines:

- `test-asert-anchor-identity`
- `test-asert-poly-factor-zero` / `-monotonic` / `-near-one`
- `test-asert-halflife-doubles`, `test-asert-halflife-halves`
- `test-asert-ref-cross-check`
- plus `test-asert-bn-wrapper` (not listed in changelog at all)

Purely a documentation/naming sync issue, but will confuse reviewers
doing a "check that claimed tests exist" pass.

**Resolution:** the "Unit tests" section of `014-aletheia.md` has
been rewritten to mirror the actual test file, grouped by purpose
(identity & monotonicity / polynomial / exponent / halflife & clamping
/ cross-checks). Every arm in `closed/hoon/tests/dumb/mod/unit/asert.hoon`
is now listed under the correct heading, including the BCH QA runs
and the defensive-guard tests added for audit items 7 and 8.
