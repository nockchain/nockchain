::  tests/dumb/mod/unit/dual-puzzle.hoon
::
::    Dual-puzzle (ZK-PoW %2 + AI-PoW %3) consensus mechanism tests.
::
::    Focus: fork choice must not favour either puzzle. Once both are live every
::    block contributes the SAME heaviness whichever puzzle produced it, so a
::    block of one is worth a block of the other, no single block can reorg more
::    than one block of history, and each puzzle's share of accumulated work is
::    the ratio of its block rate — which its own ASERT holds at its own
::    ideal-block-time.
::
/=  helpers  /tests/dumb/helpers
/=  dcon     /apps/dumbnet/lib/consensus
/=  asert    /apps/dumbnet/lib/asert
/=  txe      /common/tx-engine
/=  *        /apps/dumbnet/lib/types
/=  *        /common/zeke
/=  *        /common/h-zoon
/=  *        /common/test
|%
++  t  ~(. txe bc-ai-pow-provable:helpers)
++  hd  ~(. helpers bc-dual-puzzle:helpers)
++  hc  ~(. helpers bc-ai-anchor-test:helpers)
++  hp  ~(. helpers bc-dual-post:helpers)
++  ht  ~(. helpers bc-tandem:helpers)
::
::  Every post-activation block weighs the same, whichever puzzle produced it.
::  This is what stops one puzzle's blocks being systematically orphaned by the
::  other's at heights both reached, and what bounds a single block to displacing
::  at most one block of history.
++  test-post-activation-blocks-weigh-the-same
  ^-  tang
  =/  pt  ~(. txe bc-dual-post:helpers)
  ::  Tips at height 2, the first height at or above +dual-puzzle-asert-phase.
  =/  zk-built  (build-typed-chain:hp ~[%zk %zk])
  =/  ai-built  (build-typed-chain:hp ~[%zk %ai])
  =/  zk-w
    %-  merge:bignum
    (~(block-compute-work dcon con.zk-built der.zk-built bc-dual-post:helpers) tip.zk-built)
  =/  ai-w
    %-  merge:bignum
    (~(block-compute-work dcon con.ai-built der.ai-built bc-dual-post:helpers) tip.ai-built)
  %+  weld
    (expect-eq !>(zk-w) !>(ai-w))
  ::  ...and it is the shared constant, not a coincidence of equal targets
  (expect-eq !>(zk-w) !>((merge:bignum dual-puzzle-block-work:page:pt)))
::
::  ...and the weight does not move with difficulty. Two AI blocks whose ASERT
::  targets differ contribute the same. A heaviness that scaled as 1/target would
::  make per-block weight track each puzzle's capacity relative to the other, so
::  one block of the heavier puzzle could displace as many blocks of the lighter
::  one as that ratio -- the reorg churn this rule exists to prevent.
++  test-post-activation-weight-is-difficulty-independent
  ^-  tang
  =/  one  (build-typed-chain:hp ~[%zk %ai])
  =/  two  (build-typed-chain:hp ~[%zk %ai %ai])
  =/  w1
    %-  merge:bignum
    (~(block-compute-work dcon con.one der.one bc-dual-post:helpers) tip.one)
  =/  w2
    %-  merge:bignum
    (~(block-compute-work dcon con.two der.two bc-dual-post:helpers) tip.two)
  %+  expect-eq  !>(w1)  !>(w2)
::
::  Equal weighting starts at +dual-puzzle-phase and NO EARLIER. That is the
::  height of the ZK re-pin / AI ASERT introduction, NOT `ai-pow-activation-
::  height`: admission can be configured below the re-pin, and until the re-pin
::  neither puzzle is retargeting under the regime the rule describes, so a block
::  must still accumulate its own difficulty.
::
::  Here admission is height 1 but the phases are height 2, so the height-1 block
::  keeps +compute-work on its own target while the height-2 block goes flat.
++  test-equal-weight-starts-at-the-asert-phase-not-admission
  ^-  tang
  =/  pt  ~(. txe bc-dual-post:helpers)
  =/  built  (build-typed-chain:hp ~[%zk %zk])
  =/  h1=page:t  (to-page:local-page:t (~(got h-by blocks.con.built) ~(parent get:page:t tip.built)))
  =/  flat=@  (merge:bignum dual-puzzle-block-work:page:pt)
  =/  w1=@
    %-  merge:bignum
    (~(block-compute-work dcon con.built der.built bc-dual-post:helpers) h1)
  =/  w2=@
    %-  merge:bignum
    (~(block-compute-work dcon con.built der.built bc-dual-post:helpers) tip.built)
  ;:  weld
    ::  admission is below the phase, so the two heights straddle the boundary
    (expect-eq !>(1) !>(ai-pow-activation-height:bc-dual-post:helpers))
    (expect-eq !>(2) !>(dual-puzzle-phase:page:pt))
    ::  height 1: own difficulty, NOT the flat weight
    (expect-eq !>(%.y) !>(!=(w1 flat)))
    (expect-eq !>(w1) !>((merge:bignum (compute-work:page:pt ~(target get:page:t h1)))))
    ::  height 2: flat
    (expect-eq !>(w2) !>(flat))
  ==
::
::  Accumulated work is continuous across the activation boundary: a
::  post-activation block contributes what a ZK block at its own post-activation
::  ASERT anchor contributed under the previous rule.
++  test-block-work-continuous-at-activation
  ^-  tang
  =/  mt  ~(. txe *blockchain-constants:txe)
  =/  mainnet  *blockchain-constants:txe
  =/  actual=@  (merge:bignum dual-puzzle-block-work:page:mt)
  =/  expected=@
    %-  merge:bignum
    (compute-work:page:mt (chunk:bignum anchor-target-atom.zk-asert-post-ai.mainnet))
  %+  expect-eq  !>(expected)  !>(actual)
::
::  The AI ASERT anchor sets the puzzle's LAUNCH BLOCK INTERVAL and nothing else
::  -- it carries no fork-choice weight under the equal-weight rule. An %ai-pow
::  target prices one MAC-equivalent, so 2^256/anchor is the expected
::  MAC-equivalents per block; bex 193 is 2^63 of them, about a hundred consumer
::  GPUs at the 250s ideal.
++  test-ai-anchor-sets-the-launch-block-interval
  ^-  tang
  =/  mt  ~(. txe *blockchain-constants:txe)
  =/  mainnet  *blockchain-constants:txe
  =/  anchor  anchor-target-atom.ai-asert.mainnet
  %+  weld
    (expect-eq !>(63) !>((sub 256 (dec (met 0 anchor)))))
  (expect-eq !>(%.y) !>((lte anchor max-ai-target-atom:mt)))
::
::  Largest shape work factor the Pearl envelope admits: h*w <= 256 times
::  dot-product-length <= (bex 16). An %ai-pow target is scaled by this factor
::  before the jackpot is compared against it.
++  max-shape-work-factor  ^~((bex 24))
::
::  Every target the AI ASERT may emit must stay MINABLE: the verifier compares
::  the 256-bit jackpot against target * shape-work-factor, computed in 256 bits
::  and fail-closed. A target whose scaled threshold does not fit is rejected for
::  every shape, and because the AI ASERT only advances when an AI block is
::  ACCEPTED, such a target never retargets back down -- the puzzle would be
::  permanently dead rather than merely easy.
::
::  Stated as the property, not the literal, so it still holds if the ceiling or
::  the envelope moves. Mirrors ai_pow::difficulty's
::  max_consensus_target_never_overflows.
++  test-max-ai-target-atom-keeps-every-shape-representable
  ^-  tang
  %+  expect-eq  !>(%.y)
  !>  (lth (mul max-ai-target-atom:t max-shape-work-factor) ^~((bex 256)))
::
::  ...and the ceiling is TIGHT: one above it does not fit, so the constant is
::  not silently conservative in a way that would hide the real domain.
++  test-max-ai-target-atom-is-the-tight-bound
  ^-  tang
  %+  expect-eq  !>(%.y)
  !>  (gte (mul +(max-ai-target-atom:t) max-shape-work-factor) ^~((bex 256)))
::
::  The mainnet AI anchor must itself be minable -- an anchor above
::  +max-ai-target-atom is rejected for shape-scaling overflow on every block, and
::  the AI ASERT never advances to escape it.
++  test-mainnet-ai-anchor-is-inside-the-minable-domain
  ^-  tang
  =/  mt  ~(. txe *blockchain-constants:txe)
  =/  mainnet  *blockchain-constants:txe
  %+  expect-eq  !>(%.y)
  !>((lte anchor-target-atom.ai-asert.mainnet max-ai-target-atom:mt))
::
::  At AI activation the ZK puzzle re-anchors on a HARDCODED target equal to the
::  one it launched with at the Aletheia ASERT phase, discarding the difficulty
::  the chain accumulated across the intervening blocks. The regime's ideal block
::  time changes at the same height, so some reset is intended; the size of the
::  step is whatever the chain's real ZK target has drifted to by then, and ASERT
::  can only walk it back at ideal-block-time/half-life doublings per ZK block.
::
::  Pinned so the reset is a deliberate constant rather than an emergent one.
++  test-zk-post-ai-re-anchors-at-the-aletheia-launch-target
  ^-  tang
  =/  mainnet  *blockchain-constants:txe
  %+  expect-eq
    !>(anchor-target-atom.zk-asert.mainnet)
  !>(anchor-target-atom.zk-asert-post-ai.mainnet)
::
::  AI ASERT can never emit a target outside its minable domain, even
::  when a configured anchor or a long delay would otherwise saturate at the
::  320-bit ZK ceiling.
++  test-ai-asert-target-capped-to-jackpot-domain
  ^-  tang
  =/  target
    %-  compute-target:asert
    :*  (bex 300)
        0
        0
        0
        1
        300
        600
        max-ai-target-atom:t
    ==
  %+  expect-eq  !>(max-ai-target-atom:t)  !>(target)
::
::  Cross-puzzle accumulated-work over a MIXED chain: because every
::  post-activation block weighs the same, two chains of the same length
::  accumulate the same total whatever order the puzzles produced them in. No
::  interleaving of ZK and AI is heavier than another of equal length.
++  test-dual-puzzle-mixed-accumulated-work
  ^-  tang
  =/  zk-first  (build-typed-chain:hp ~[%zk %zk %ai])
  =/  ai-first  (build-typed-chain:hp ~[%zk %ai %zk])
  %+  expect-eq
    !>((merge:bignum ~(accumulated-work get:page:t tip.zk-first)))
  !>((merge:bignum ~(accumulated-work get:page:t tip.ai-first)))
::
::  A single block of either puzzle can never outweigh a longer run of the other.
::  Under a 1/target heaviness the heavier puzzle's block could displace as many
::  of the lighter puzzle's blocks as their capacity ratio; here one AI block
::  loses to two ZK blocks, and one ZK block loses to two AI blocks.
++  test-single-block-cannot-outweigh-a-run
  ^-  tang
  =/  one-ai  (build-typed-chain:hp ~[%zk %ai])
  =/  one-zk  (build-typed-chain:hp ~[%zk %zk])
  =/  two-zk  (build-typed-chain:hp ~[%zk %zk %zk])
  =/  two-ai  (build-typed-chain:hp ~[%zk %ai %ai])
  =/  w  |=(pag=page:t (merge:bignum ~(accumulated-work get:page:t pag)))
  %+  weld
    (expect-eq !>(%.y) !>((lth (w tip.one-ai) (w tip.two-zk))))
  (expect-eq !>(%.y) !>((lth (w tip.one-zk) (w tip.two-ai))))
::
::  Per-block work is a real quantity, not the clamped floor of 1.
++  test-dual-puzzle-anchor-work-nonzero
  ^-  tang
  =/  mt  ~(. txe *blockchain-constants:txe)
  %+  expect-eq  !>(%.y)  !>((gth (merge:bignum dual-puzzle-block-work:page:mt) 1))
::
::  Branch-local state counts each puzzle independently on a mixed chain.
++  test-ai-subchain-count
  ^-  tang
  =/  built  (build-typed-chain:hd ~[%zk %ai %zk %zk %ai])
  =/  tip-bid  ~(digest get:page:t tip.built)
  =/  state  (~(got h-by puzzle-asert-states.der.built) tip-bid)
  %+  expect-eq  !>([2 3])  !>([ai-count.state zk-count.state])
::
::  RETARGETING — AI difficulty tracks the AI subchain, not global height.
::  Two chains share the same AI subchain (one AI block on genesis); chain B
::  interleaves a ZK block. The next AI block's ASERT target must be IDENTICAL
::  (same AI ancestor, same AI-subchain distance). Under the old global-height
::  math the extra ZK block would change the target — so equality here is exactly
::  the per-puzzle-cadence property the design requires.
++  test-ai-asert-ignores-interleaved-zk
  ^-  tang
  =/  a  (build-typed-chain:hd ~[%ai])
  =/  b  (build-typed-chain:hd ~[%ai %zk])
  =/  target-a
    (~(compute-target-ai-asert dcon con.a der.a bc-dual-puzzle:helpers) 2 ~(digest get:page:t tip.a))
  =/  target-b
    (~(compute-target-ai-asert dcon con.b der.b bc-dual-puzzle:helpers) 3 ~(digest get:page:t tip.b))
  %+  expect-eq  !>((merge:bignum target-a))  !>((merge:bignum target-b))
::
::  Symmetric ZK check: interleaving an AI block does not advance the ZK
::  subchain count or replace its lineage head.
++  test-zk-asert-ignores-interleaved-ai
  ^-  tang
  =/  a  (build-typed-chain:ht ~[%zk])
  =/  b  (build-typed-chain:ht ~[%zk %ai])
  =/  target-a
    (~(compute-target-zk-asert dcon con.a der.a bc-tandem:helpers) 2 ~(digest get:page:t tip.a))
  =/  target-b
    (~(compute-target-zk-asert dcon con.b der.b bc-tandem:helpers) 3 ~(digest get:page:t tip.b))
  %+  expect-eq  !>((merge:bignum target-a))  !>((merge:bignum target-b))
::
::  PRODUCTION — +build-ai-candidate re-targets the ZK candidate to exactly the
::  AI ASERT target and the AI-normalized accumulated-work that validation
::  recomputes (+block-compute-work). This is the block the miner solves against;
::  if either field were off, +heard-block would reject the mined block as
::  %page-target-invalid / %page-heaviness-invalid.
++  test-build-ai-candidate-retargets
  ^-  tang
  ::  bc-dual-post: post-asert at the candidate height, so +build-ai-candidate
  ::  actually re-targets (pre-asert it returns the ZK candidate unchanged).
  =/  built  (build-typed-chain:hp ~[%ai %zk])
  =/  con  con.built
  =/  zk-cand=page:t  (make-empty-page:hp tip.built)
  ::  shares only need to be a valid single-miner split — this test pins the AI
  ::  candidate's target and accumulated-work, which are independent of the
  ::  coinbase +build-ai-candidate rebuilds from them.
  =/  shares=shares:t
    (~(put z-by *(z-map hash:t @)) (hash:schnorr-pubkey:t default-a-pt-1:helpers) 1)
  =/  ai-cand=page:t
    (~(build-ai-candidate dcon con der.built bc-dual-post:helpers) zk-cand shares)
  =/  expected-target
    (~(compute-target-ai-asert dcon con der.built bc-dual-post:helpers) ~(height get:page:t zk-cand) ~(parent get:page:t zk-cand))
  =/  parent-work  (merge:bignum ~(accumulated-work get:page:t tip.built))
  =/  expected-work  (add parent-work (merge:bignum dual-puzzle-block-work:page:t))
  %+  expect-eq
    !>([(merge:bignum expected-target) expected-work])
  !>  :-  (merge:bignum ~(target get:page:t ai-cand))
      (merge:bignum ~(accumulated-work get:page:t ai-cand))
::
::  The first AI block on a branch becomes that branch's AI ASERT anchor
::  immediately; no later global-height crossing is involved.
++  test-ai-anchor-populates
  ^-  tang
  =/  built  (build-typed-chain:hc ~[%zk %ai])
  =/  state  (~(got h-by puzzle-asert-states.der.built) ~(digest get:page:t tip.built))
  %+  expect-eq  !>(%.y)
  !>(?=(^ ai-anchor.state))
::
::  A ZK block above the configured AI anchor height must not populate the AI
::  anchor. The AI ASERT starts from the first AI block, not from whichever
::  puzzle first crosses a global height.
++  test-ai-anchor-ignores-zk-crossing
  ^-  tang
  =/  built  (build-typed-chain:hc ~[%zk %zk %zk])
  =/  state  (~(got h-by puzzle-asert-states.der.built) ~(digest get:page:t tip.built))
  %+  expect-eq  !>(%.n)
  !>(?=(^ ai-anchor.state))
::
::  A puzzle lineage remains available after an arbitrarily long run of the
::  other puzzle. A fixed global-hop cap would make AI target selection fall
::  back to a ZK parent and let the ZK rate influence AI difficulty.
++  test-ai-lineage-survives-long-zk-gap
  ^-  tang
  =/  zks=(list ?(%zk %ai))  (reap 45 %zk)
  =/  built  (build-typed-chain:hd (weld ~[%ai] zks))
  =/  state  (~(got h-by puzzle-asert-states.der.built) ~(digest get:page:t tip.built))
  %+  expect-eq  !>([1 %.y])
  !>([ai-count.state ?=(^ ai-head.state)])
::
::  A post-activation parent must have a branch-local lineage entry. Silently
::  synthesizing zero counts would make a restarted or corrupted node derive a
::  different target from peers that retained the entry.
++  test-missing-branch-state-fails-closed
  ^-  tang
  =/  built  (build-typed-chain:hd ~[%ai])
  =/  tip-bid  ~(digest get:page:t tip.built)
  =/  broken=derived-state
    der.built(puzzle-asert-states (~(del h-by puzzle-asert-states.der.built) tip-bid))
  %+  expect-fail
    |.  (~(compute-target-ai-asert dcon con.built broken bc-dual-puzzle:helpers) 2 tip-bid)
  ~
::
::  END-TO-END ACCEPTANCE (post-asert) — a correctly-built AI block travels the
::  full +validate-page-without-txs path and is ACCEPTED (target dispatch,
::  AI-normalized heaviness, version, coinbase, timestamp all pass; the AI cert
::  check is deferred to the prover-gated +check-pow). A mis-built AI block
::  (parent/ZK target + ZK-normalized work) is REJECTED. Together: consensus
::  accepts correctly-targeted AI blocks and rejects mis-targeted ones on a live
::  post-asert chain, without the prover.
++  test-ai-block-accepted-post-asert
  ^-  tang
  =/  built  (build-typed-chain:hp ~[%zk %ai %zk])
  =/  ai-page  (make-ai-pow-page:hp tip.built con.built der.built)
  =/  good
    %.  [ai-page ~(timestamp get:page:t ai-page)]
    ~(validate-page-without-txs dcon con.built der.built bc-dual-post:helpers)
  =/  bad-page  (make-ai-pow-garbage-page:hp tip.built)
  =/  bad
    %.  [bad-page ~(timestamp get:page:t bad-page)]
    ~(validate-page-without-txs dcon con.built der.built bc-dual-post:helpers)
  %+  expect-eq  !>([%.y %.n])  !>([-.good -.bad])
::
::  TANDEM RETARGETING — both puzzles' ASERT run in their SUBCHAIN regime at once
::  and each retargets over its OWN block count, independently. The test anchors
::  represent equal work: `ai-target * 2^64 == zk-target`. Comparisons therefore
::  normalize AI targets into the ZK target space. The ASERT time input is the
::  parent median-of-11 (a GLOBAL quantity, equal for both puzzles at the tip), so
::  differences are driven by each puzzle's independent SUBCHAIN COUNT.
::
::  ZK-heavy chain (3 ZK + 1 AI over the same span): the ZK subchain has more
::  blocks per unit time, so the ZK ASERT hardens MORE -> zk-target < ai-target.
++  test-tandem-asert-zk-heavy
  ^-  tang
  =/  t0  (time-in-secs:page:t *@da)
  =/  built
    %-  build-typed-chain-timed:ht
    :~  [%zk (add t0 10)]  [%zk (add t0 20)]  [%zk (add t0 30)]  [%ai (add t0 40)]
    ==
  =/  con  con.built
  =/  tip-bid  ~(digest get:page:t tip.built)
  =/  zk-target  (merge:bignum (~(compute-target-zk-asert dcon con der.built bc-tandem:helpers) 5 tip-bid))
  =/  ai-target  (merge:bignum (~(compute-target-ai-asert dcon con der.built bc-tandem:helpers) 5 tip-bid))
  %+  expect-eq  !>(%.y)  !>((lth zk-target (mul ai-target (bex 64))))
::
::  AI-heavy chain (3 AI + 1 ZK): the reverse — the AI ASERT hardens MORE, so
::  ai-target < zk-target. Confirms each retarget is keyed to its own subchain, not
::  a fixed bias or the global cadence.
++  test-tandem-asert-ai-heavy
  ^-  tang
  =/  t0  (time-in-secs:page:t *@da)
  =/  built
    %-  build-typed-chain-timed:ht
    :~  [%ai (add t0 10)]  [%ai (add t0 20)]  [%ai (add t0 30)]  [%zk (add t0 40)]
    ==
  =/  con  con.built
  =/  tip-bid  ~(digest get:page:t tip.built)
  =/  zk-target  (merge:bignum (~(compute-target-zk-asert dcon con der.built bc-tandem:helpers) 5 tip-bid))
  =/  ai-target  (merge:bignum (~(compute-target-ai-asert dcon con der.built bc-tandem:helpers) 5 tip-bid))
  %+  expect-eq  !>(%.y)  !>((lth (mul ai-target (bex 64)) zk-target))
--
