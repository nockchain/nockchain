::  tests/dumb/mod/unit/dual-puzzle.hoon
::
::    Dual-puzzle (ZK-PoW %3 + AI-PoW %4) consensus mechanism tests.
::
::    Focus: fork choice must not favour either puzzle at calibration and must
::    not reward a discount. Once both are live every block contributes the
::    expected work at its own target, priced per puzzle in
::    ZKPoW-attempt-equivalents at the +mac-equivalents-per-zk-attempt
::    exchange rate. At the launch anchors both lanes produce the same heaviness
::    per second, so a block of one is worth a block of the other; a block
::    whose target is cheap for its puzzle earns proportionally less, so an
::    ASERT discount can never subsidize a reorg.
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
++  hc  ~(. helpers bc-dual-repin-cache:helpers)
++  hp  ~(. helpers bc-dual-post:helpers)
++  ht  ~(. helpers bc-tandem:helpers)
::
::  Post-activation heaviness is the expected work at the block's own target,
::  priced per puzzle in ZKPoW-attempt-equivalents. ZK blocks keep the
::  pre-activation formula exactly; AI blocks contribute 2^256/(target+1)
::  MAC-equivalents over the +mac-equivalents-per-zk-attempt exchange rate.
++  test-post-activation-work-is-puzzle-priced
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
  ;:  weld
    ::  ZK keeps the pre-activation formula on its own target
    %+  expect-eq  !>(zk-w)
    !>((merge:bignum (compute-work:page:pt ~(target get:page:t tip.zk-built))))
    ::  AI contributes its MAC-equivalents in attempt-equivalents
    %+  expect-eq  !>(ai-w)
    !>((merge:bignum (ai-pow-work:page:pt ~(target get:page:t tip.ai-built))))
  ==
::
::  ...and the weight tracks difficulty: of two AI blocks whose ASERT targets
::  differ, the one with the EASIER target contributes LESS. A branch that
::  retargets to a cheaper target earns less fork-choice credit per block, so
::  an ASERT discount can never subsidize a reorg.
++  test-post-activation-weight-tracks-target
  ^-  tang
  =/  one  (build-typed-chain:hp ~[%zk %ai])
  =/  two  (build-typed-chain:hp ~[%zk %ai %ai])
  =/  t1=@  (merge:bignum ~(target get:page:t tip.one))
  =/  t2=@  (merge:bignum ~(target get:page:t tip.two))
  =/  w1
    %-  merge:bignum
    (~(block-compute-work dcon con.one der.one bc-dual-post:helpers) tip.one)
  =/  w2
    %-  merge:bignum
    (~(block-compute-work dcon con.two der.two bc-dual-post:helpers) tip.two)
  ::  the test chain runs behind its ideal, so the ASERT eases the target
  ?>  (gth t2 t1)
  %+  expect-eq  !>(%.y)  !>((lth w2 w1))
::
::  Per-puzzle pricing starts at +dual-puzzle-phase and NO EARLIER. That is
::  the height of the ZK re-pin / AI ASERT introduction, NOT
::  `ai-pow-activation-height`: admission can be configured below the re-pin,
::  and until the re-pin a block still accumulates the ZK formula on its own
::  target, whatever puzzle produced it.
::
::  Here admission is height 1 but the phases are height 2, so the height-1 AI
::  block keeps +compute-work on its own target while the height-2 AI block is
::  priced in MAC-equivalents.
++  test-puzzle-pricing-starts-at-the-asert-phase-not-admission
  ^-  tang
  =/  pt  ~(. txe bc-dual-post:helpers)
  =/  built  (build-typed-chain:hp ~[%ai %ai])
  =/  h1=page:t  (to-page:local-page:t (~(got h-by blocks.con.built) ~(parent get:page:t tip.built)))
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
    ::  height 1 (pre-phase): the ZK formula on the block's own target
    (expect-eq !>(w1) !>((merge:bignum (compute-work:page:pt ~(target get:page:t h1)))))
    ::  height 2 (post-phase): MAC-equivalents over the exchange rate
    (expect-eq !>(w2) !>((merge:bignum (ai-pow-work:page:pt ~(target get:page:t tip.built)))))
    ::  ...and the two rules genuinely differ here, so both pins are meaningful
    (expect-eq !>(%.y) !>(!=(w1 w2)))
  ==
::
::  ZK weight is continuous across the activation boundary: a post-activation
::  ZK block contributes exactly what the pre-activation formula gives on the
::  same target. KAT: at the mainnet post-activation anchor (2^291) that is
::  2^29 - 1 attempts.
++  test-zk-work-continuous-at-activation
  ^-  tang
  =/  mt  ~(. txe *blockchain-constants:txe)
  =/  mainnet  *blockchain-constants:txe
  =/  anchor-bn  (chunk:bignum anchor-target-atom.zk-asert-post-ai.mainnet)
  =/  post-w=@  (merge:bignum (block-work-at:page:mt 114.300 %dumb-zkpow anchor-bn))
  =/  pre-w=@   (merge:bignum (compute-work:page:mt anchor-bn))
  ;:  weld
    (expect-eq !>(pre-w) !>(post-w))
    (expect-eq !>(536.870.911) !>(post-w))
  ==
::
::  The AI ASERT anchor sets the puzzle's LAUNCH BLOCK INTERVAL and prices its
::  launch weight. An %ai-pow target prices one MAC-equivalent, so 2^256/anchor
::  is the expected MAC-equivalents per block; bex 193 is 2^63 of them, about a
::  hundred consumer GPUs at the 250s ideal.
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
::  Cross-puzzle accumulated-work over a MIXED chain: each block adds the
::  expected work at its own target for its own puzzle, so a chain's total is
::  the per-puzzle sum, whatever order the puzzles produced the blocks in.
++  test-dual-puzzle-mixed-accumulated-work
  ^-  tang
  =/  built  (build-typed-chain:hp ~[%zk %zk %ai])
  =/  h2=page:t  (to-page:local-page:t (~(got h-by blocks.con.built) ~(parent get:page:t tip.built)))
  =/  h1=page:t  (to-page:local-page:t (~(got h-by blocks.con.built) ~(parent get:page:t h2)))
  =/  work  |=(pag=page:t (merge:bignum (~(block-compute-work dcon con.built der.built bc-dual-post:helpers) pag)))
  =/  sum=@
    :(add (merge:bignum ~(accumulated-work get:page:t h1)) (work h2) (work tip.built))
  %+  expect-eq  !>(sum)  !>((merge:bignum ~(accumulated-work get:page:t tip.built)))
::
::  A block that reaches its puzzle's ceiling contributes the floored minimum,
::  so a discounted branch cannot win by count. At the launch anchors one block
::  of either puzzle is within 1.5x of the other, so neither puzzle orphans the
::  other's blocks at calibration; but a capped block is worth less than any
::  honest anchor block of either puzzle.
++  test-single-block-cannot-outweigh-a-run
  ^-  tang
  =/  mt  ~(. txe *blockchain-constants:txe)
  =/  mainnet  *blockchain-constants:txe
  =/  zk-anchor-w=@  (merge:bignum (block-work-at:page:mt 114.300 %dumb-zkpow (chunk:bignum anchor-target-atom.zk-asert-post-ai.mainnet)))
  =/  ai-anchor-w=@  (merge:bignum (block-work-at:page:mt 114.300 %ai-pow (chunk:bignum anchor-target-atom.ai-asert.mainnet)))
  =/  zk-cap-w=@  (merge:bignum (block-work-at:page:mt 114.300 %dumb-zkpow (chunk:bignum max-target-atom:mt)))
  =/  ai-cap-w=@  (merge:bignum (block-work-at:page:mt 114.300 %ai-pow (chunk:bignum max-ai-target-atom:mt)))
  ;:  weld
    ::  anchor blocks are within 2x of each other (1.499x exactly)
    (expect-eq !>(%.y) !>((lth zk-anchor-w (mul 2 ai-anchor-w))))
    (expect-eq !>(%.y) !>((lth ai-anchor-w (mul 2 zk-anchor-w))))
    ::  capped blocks contribute the floored minimum
    (expect-eq !>(1) !>(zk-cap-w))
    (expect-eq !>(1) !>(ai-cap-w))
    ::  ...so a capped block is worth less than any honest anchor block
    (expect-eq !>(%.y) !>((lth zk-cap-w ai-anchor-w)))
    (expect-eq !>(%.y) !>((lth ai-cap-w zk-anchor-w)))
  ==
::
::  Per-block work at the launch anchors, in ZKPoW-attempt-equivalents. The
::  exchange rate is +mac-equivalents-per-zk-attempt, from the reference-GPU
::  co-benchmark (see tx-engine).
++  test-anchor-work-is-exchange-rate-priced
  ^-  tang
  =/  mt  ~(. txe *blockchain-constants:txe)
  =/  mainnet  *blockchain-constants:txe
  ;:  weld
    (expect-eq !>(25.750.000.000) !>(mac-equivalents-per-zk-attempt:page:mt))
    %+  expect-eq  !>(536.870.911)
    !>((merge:bignum (block-work-at:page:mt 114.300 %dumb-zkpow (chunk:bignum anchor-target-atom.zk-asert-post-ai.mainnet))))
    %+  expect-eq  !>(358.189.205)
    !>((merge:bignum (block-work-at:page:mt 114.300 %ai-pow (chunk:bignum anchor-target-atom.ai-asert.mainnet))))
  ==
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
  =/  expected-work  (add parent-work (merge:bignum (ai-pow-work:page:t expected-target)))
  %+  expect-eq
    !>([(merge:bignum expected-target) expected-work])
  !>  :-  (merge:bignum ~(target get:page:t ai-cand))
      (merge:bignum ~(accumulated-work get:page:t ai-cand))
::
::  The AI pin is cached through every accepted branch, including a ZK block.
::  A first AI block therefore recovers its timestamp without becoming an
::  ad-hoc anchor.
++  test-ai-repin-cache-populates-on-zk
  ^-  tang
  =/  built  (build-typed-chain:hc ~[%zk])
  =/  tip-id=block-id:t  ~(digest get:page:t tip.built)
  =/  ai-timestamps=(h-map block-id:t @)
    (need (~(get by asert-anchor-min-timestamps.con.built) %ai))
  (expect-eq !>(%.y) !>((~(has h-by ai-timestamps) tip-id)))
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
