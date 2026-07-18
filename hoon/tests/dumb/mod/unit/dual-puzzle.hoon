::  tests/dumb/mod/unit/dual-puzzle.hoon
::
::    Dual-puzzle (ZK-PoW %2 + AI-PoW %3) consensus mechanism tests.
::
::    Focus: the CROSS-PUZZLE HEAVINESS comparison must not break — an AI
::    block and a ZK block at their respective ASERT anchors must contribute
::    EQUAL work to the single shared accumulated-work fork-choice sum
::    (equal-weight normalization). AI targets live in the 256-bit space (AI ASERT
::    anchor bex 227); +compute-work-ai scales the AI target UP by 2^64 into ZK's
::    ~2^320 space and applies the identical GetBlockProof formula, so exactly:
::      compute-work-ai(T) == compute-work(T * 2^64)
::    and the AI anchor bex 227 contributes exactly the ZK anchor bex 291 work.
::
/=  helpers  /tests/dumb/helpers
/=  dcon     /apps/dumbnet/lib/consensus
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
::  A block at the AI anchor (bex 227) contributes work EQUAL to a block at the
::  ZK anchor (bex 291): the core equal-weight cross-puzzle invariant.
++  test-dual-puzzle-equal-weight-at-anchor
  ^-  tang
  =/  ai-work  (merge:bignum (compute-work-ai:page:t (chunk:bignum ^~((bex 227)))))
  =/  zk-work  (merge:bignum (compute-work:page:t (chunk:bignum ^~((bex 291)))))
  %+  expect-eq  !>(zk-work)  !>(ai-work)
::
::  Equal weight holds at HARDER difficulty too: shift both anchors down by the
::  same number of bits (2x harder) and the works stay equal.
++  test-dual-puzzle-equal-weight-harder
  ^-  tang
  =/  ai-work  (merge:bignum (compute-work-ai:page:t (chunk:bignum ^~((bex 220)))))
  =/  zk-work  (merge:bignum (compute-work:page:t (chunk:bignum ^~((bex 284)))))
  %+  expect-eq  !>(zk-work)  !>(ai-work)
::
::  AI work is MONOTONIC in difficulty: a smaller (harder) AI target yields more
::  work than a larger (easier) one.
++  test-compute-work-ai-monotonic
  ^-  tang
  =/  easy  (merge:bignum (compute-work-ai:page:t (chunk:bignum ^~((bex 227)))))
  =/  hard  (merge:bignum (compute-work-ai:page:t (chunk:bignum ^~((bex 220)))))
  %+  expect-eq  !>(%.y)  !>((gth hard easy))
::
::  The exact scale identity: compute-work-ai(T) == compute-work(T * 2^64) for an
::  arbitrary AI target T. This is the algebraic core of equal weight.
++  test-compute-work-ai-scale-identity
  ^-  tang
  =/  ai-t=@  ^~((bex 240))
  =/  ai   (merge:bignum (compute-work-ai:page:t (chunk:bignum ai-t)))
  =/  zk   (merge:bignum (compute-work:page:t (chunk:bignum (mul ai-t ^~((bex 64))))))
  %+  expect-eq  !>(zk)  !>(ai)
::
::  The AI anchor lives in the 256-bit jackpot space (below 2^256), and scaling it
::  up by 2^64 stays within the ~2^320 max-target domain (never overflows the ZK
::  space, so the shared formula is well-defined).
++  test-compute-work-ai-domain
  ^-  tang
  %+  expect-eq  !>(%.y)
  !>  ?&  (lth ^~((bex 227)) ^~((bex 256)))
          (lth (mul ^~((bex 227)) ^~((bex 64))) (merge:bignum max-target:t))
      ==
::
::  Cross-puzzle accumulated-work SUM: a mixed chain — parent-work + a ZK block
::  at its anchor + an AI block at its anchor — accumulates the SAME as parent +
::  2x the (equal) per-block work. Confirms mixing puzzles in one heaviness total
::  is well-defined and symmetric.
++  test-dual-puzzle-mixed-accumulated-work
  ^-  tang
  =/  parent-work=@  1.000.000
  =/  zk=@  (merge:bignum (compute-work:page:t (chunk:bignum ^~((bex 291)))))
  =/  ai=@  (merge:bignum (compute-work-ai:page:t (chunk:bignum ^~((bex 227)))))
  ::  parent + ZK-then-AI == parent + AI-then-ZK == parent + zk + ai.
  =/  zk-then-ai=@  (add (add parent-work zk) ai)
  =/  ai-then-zk=@  (add (add parent-work ai) zk)
  %+  expect-eq  !>(zk-then-ai)  !>(ai-then-zk)
::
::  Neither puzzle's per-block work is zero at anchor difficulty (a real PoW
::  contribution to heaviness on both sides).
++  test-dual-puzzle-anchor-work-nonzero
  ^-  tang
  =/  ai  (merge:bignum (compute-work-ai:page:t (chunk:bignum ^~((bex 227)))))
  =/  zk  (merge:bignum (compute-work:page:t (chunk:bignum ^~((bex 291)))))
  %+  expect-eq  !>(%.y)  !>(?&((gth ai 0) (gth zk 0)))
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
  =/  expected-work  (add parent-work (merge:bignum (compute-work-ai:page:t expected-target)))
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
  =/  ai-page  (make-ai-pow-page:hp tip.built con.built)
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
::  and each retargets over its OWN block count, independently. bc-tandem gives the
::  two puzzles the SAME anchor target (and 300s ideal / 600s half-life), so the
::  targets are directly comparable. The ASERT time input is the median-of-11 of
::  the parent (a GLOBAL quantity, ~equal for both puzzles at the tip), so the
::  difference between the two targets is driven by the per-puzzle SUBCHAIN COUNT.
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
  %+  expect-eq  !>(%.y)  !>((lth zk-target ai-target))
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
  %+  expect-eq  !>(%.y)  !>((lth ai-target zk-target))
--
