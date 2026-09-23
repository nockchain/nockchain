::  tests/dumb/mod/unit/dual-puzzle.hoon
::
::    Dual-puzzle (ZK-PoW + AI-PoW %4) consensus mechanism tests.
::
::    Focus: fork choice must price both puzzles in shared hardware units and
::    must not reward a difficulty discount. Once both are live every block
::    contributes expected work at its own target, priced per puzzle in
::    height-selected ZK work-unit equivalents.
::    ASERT block shares follow each lane's ideal interval, while normalized
::    work rates follow the declared capacity anchors.
::
/=  helpers  /tests/dumb/helpers
/=  dcon     /apps/dumbnet/lib/consensus
/=  dder     /apps/dumbnet/lib/derived
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
::  priced per puzzle in height-selected ZK work-unit equivalents. ZK blocks
::  keep the pre-activation formula exactly; AI blocks contribute
::  2^256/(target+1) MAC-equivalents over the selected exchange rate.
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
    ::  AI contributes its MAC-equivalents in ZK work-unit equivalents
    %+  expect-eq  !>(ai-w)
    !>((merge:bignum (ai-pow-work:page:pt ~(height get:page:t tip.ai-built) ~(target get:page:t tip.ai-built))))
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
    (expect-eq !>(w2) !>((merge:bignum (ai-pow-work:page:pt ~(height get:page:t tip.built) ~(target get:page:t tip.built)))))
    ::  ...and the two rules genuinely differ here, so both pins are meaningful
    (expect-eq !>(%.y) !>(!=(w1 w2)))
  ==
:::
::  Mainnet schedules use whole-second intervals. The 214s ZK interval is the
::  nearest integer to 1,500 / 7, so the target share is 30% AI and 70% ZK
::  within the fixed 150s global cadence.
++  test-mainnet-dual-puzzle-schedule
  ^-  tang
  =/  mainnet  *blockchain-constants:txe
  ;:  weld
    (expect-eq !>(126.000) !>(ai-pow-activation-height.mainnet))
    (expect-eq !>(126.000) !>(phase.zk-asert-post-ai.mainnet))
    (expect-eq !>(125.999) !>(anchor-height.zk-asert-post-ai.mainnet))
    (expect-eq !>(214) !>(ideal-block-time.zk-asert-post-ai.mainnet))
    (expect-eq !>((div (mul 375 (bex 291)) 214)) !>(anchor-target-atom.zk-asert-post-ai.mainnet))
    (expect-eq !>(126.000) !>(phase.ai-asert.mainnet))
    (expect-eq !>(125.999) !>(anchor-height.ai-asert.mainnet))
    (expect-eq !>(500) !>(ideal-block-time.ai-asert.mainnet))
    (expect-eq !>((bex 192)) !>(anchor-target-atom.ai-asert.mainnet))
  ==
::
::  Version %5 re-anchors both lanes at height 147,500. AI and ZK swap their
::  Logos ideal intervals: 214s AI / 500s ZK gives 70.028% / 29.972% and keeps
::  the combined cadence at 149.86s.
++  test-mainnet-v5-dual-puzzle-schedule
  ^-  tang
  =/  mainnet  *blockchain-constants:txe
  =/  mt  ~(. txe mainnet)
  =/  dc  ~(. dcon *consensus-state *derived-state mainnet)
  =/  zk-before  (need (active-asert-anchor:dc %zk 147.499))
  =/  ai-before  (need (active-asert-anchor:dc %ai 147.499))
  =/  zk-v5  (need (active-asert-anchor:dc %zk 147.500))
  =/  ai-v5  (need (active-asert-anchor:dc %ai 147.500))
  ;:  weld
    (expect-eq !>(147.500) !>(zk-pow-v5-phase:page:mt))
    (expect-eq !>(214) !>(ideal-block-time.zk-before))
    (expect-eq !>(500) !>(ideal-block-time.zk-v5))
    (expect-eq !>(500) !>(ideal-block-time.ai-before))
    (expect-eq !>(214) !>(ideal-block-time.ai-v5))
    (expect-eq !>(147.500) !>(activation-height.zk-v5))
    (expect-eq !>(147.500) !>(activation-height.ai-v5))
  ==
::
::  The first child at the version-%5 boundary starts both ASERT lineages at
::  their new anchor target. No pre-cutover derived lineage is required, but
::  the shared predecessor's puzzle-keyed median timestamp must be present.
++  test-mainnet-v5-first-child-uses-new-asert-anchors
  ^-  tang
  =/  mainnet  *blockchain-constants:txe
  =/  mt  ~(. txe mainnet)
  =/  parent-id=block-id:t  *block-id:t
  =/  anchor-min-ts=@  123.456
  =/  timestamps=(h-map block-id:t @)
    (~(put h-by *(h-map block-id:t @)) parent-id anchor-min-ts)
  =/  caches=(map @tas (h-map block-id:t @))
    *(map @tas (h-map block-id:t @))
  =.  caches  (~(put by caches) %zk timestamps)
  =.  caches  (~(put by caches) %ai timestamps)
  =/  con=consensus-state  *consensus-state
  =.  asert-anchor-min-timestamps.con  caches
  =/  dc  ~(. dcon con *derived-state mainnet)
  =/  zk-target=@
    (merge:bignum (compute-target-zk-asert:dc 147.500 parent-id))
  =/  ai-target=@
    (merge:bignum (compute-target-ai-asert:dc 147.500 parent-id))
  %+  expect-eq
    !>([zk-pow-v5-zk-anchor-target:page:mt zk-pow-v5-ai-anchor-target:page:mt])
  !>([zk-target ai-target])
::
::  Accepting either puzzle at height 147,500 discards all Logos lineage
::  counters and heads. The reset happens once: height 147,501 extends the
::  freshly-created branch-local state rather than zeroing it again.
++  test-mainnet-v5-derived-lineages-reset-once
  ^-  tang
  =/  mainnet  *blockchain-constants:txe
  =/  mt  ~(. txe mainnet)
  =/  hm  ~(. helpers mainnet)
  =/  parent=page:t  default-genesis-page:hm
  =.  parent
    ?^  -.parent
      parent(height 147.499, digest *block-id:t)
    parent(height 147.499, digest *block-id:t)
  =/  parent-id=block-id:t  ~(digest get:page:mt parent)
  =/  con=consensus-state  *consensus-state
  =.  blocks.con
    (~(put h-by blocks.con) parent-id (to-local-page:page:mt parent))
  =/  prior-state=puzzle-asert-state
    [zk-count=91 ai-count=73 zk-head=`parent-id ai-head=`parent-id]
  =/  prior=derived-state  *derived-state
  =.  puzzle-asert-states.prior
    (~(put h-by puzzle-asert-states.prior) parent-id prior-state)
  =/  zk-page=page:t  (make-empty-page:hm parent)
  =/  zk-id=block-id:t  ~(digest get:page:mt zk-page)
  =/  zk-derived=derived-state
    (~(update-puzzle-asert-state dder prior mainnet) con zk-page)
  =/  zk-state=puzzle-asert-state
    (~(got h-by puzzle-asert-states.zk-derived) zk-id)
  =/  ai-page=page:t  (make-empty-page:hm parent)
  =.  ai-page
    ?^  -.ai-page
      ai-page
    ai-page(pow `(sample-ai-pow-artifact:hm 4))
  =.  ai-page
    ?^  -.ai-page
      ai-page(digest (compute-digest:page:mt ai-page))
    ai-page(digest (compute-digest:page:mt ai-page))
  =/  ai-id=block-id:t  ~(digest get:page:mt ai-page)
  =/  ai-derived=derived-state
    (~(update-puzzle-asert-state dder prior mainnet) con ai-page)
  =/  ai-state=puzzle-asert-state
    (~(got h-by puzzle-asert-states.ai-derived) ai-id)
  =/  after-con=consensus-state  con
  =.  blocks.after-con
    (~(put h-by blocks.after-con) ai-id (to-local-page:page:mt ai-page))
  =/  after-page=page:t  (make-empty-page:hm ai-page)
  =/  after-id=block-id:t  ~(digest get:page:mt after-page)
  =/  after-derived=derived-state
    (~(update-puzzle-asert-state dder ai-derived mainnet) after-con after-page)
  =/  after-state=puzzle-asert-state
    (~(got h-by puzzle-asert-states.after-derived) after-id)
  %+  expect-eq
    !>  :*  [1 0 `zk-id ~]
            [0 1 ~ `ai-id]
            [1 1 `after-id `ai-id]
        ==
  !>  [zk-state ai-state after-state]
::
::  The version-%5 ASERT anchors price 2,000 RTX 5090 ZK miners and
::  10 ExaMAC/s of AI-PoW capacity.
++  test-mainnet-v5-anchor-calibration
  ^-  tang
  =/  mt  ~(. txe *blockchain-constants:txe)
  =/  zk-work=@
    %-  merge:bignum
    (block-work-at:page:mt 147.500 %dumb-zkpow (chunk:bignum zk-pow-v5-zk-anchor-target:page:mt))
  =/  ai-work=@
    %-  merge:bignum
    (block-work-at:page:mt 147.500 %ai-pow (chunk:bignum zk-pow-v5-ai-anchor-target:page:mt))
  ;:  weld
    (expect-eq !>(1.028.807) !>(zk-pow-v5-mac-equivalents-per-zk-hash:page:mt))
    (expect-eq !>(2.000) !>(zk-pow-v5-reference-zk-gpu-count:page:mt))
    (expect-eq !>(388.800.000) !>(zk-pow-v5-reference-zk-hashes-per-gpu-second:page:mt))
    (expect-eq !>(777.600.000.000) !>(zk-pow-v5-reference-zk-hashes-per-second:page:mt))
    (expect-eq !>(10.000.000.000.000.000.000) !>(zk-pow-v5-reference-ai-macs-per-second:page:mt))
    %+  expect-eq
      !>(5.493.793.810.273.389.665.851.259.858.908.398.676.672.107.719.039.465.617.368.341.183.437.285.024.349.963.580)
    !>(zk-pow-v5-zk-anchor-target:page:mt)
    %+  expect-eq
      !>(54.108.452.914.633.736.179.238.778.041.442.947.594.985.974.142.822.693.476)
    !>(zk-pow-v5-ai-anchor-target:page:mt)
    (expect-eq !>(777.599.999.999) !>((div zk-work 500)))
    (expect-eq !>(9.719.996.073.121) !>((div ai-work 214)))
  ==
::
:::  ZK weight is continuous across the activation boundary: a post-activation
:::  ZK block contributes exactly what the pre-activation formula gives on the
:::  same target. KAT: at the mainnet post-activation anchor the expected work
::  is 306,374,333 attempts.
++  test-zk-work-continuous-at-activation
  ^-  tang
  =/  mt  ~(. txe *blockchain-constants:txe)
  =/  mainnet  *blockchain-constants:txe
  =/  anchor-bn  (chunk:bignum anchor-target-atom.zk-asert-post-ai.mainnet)
  =/  post-w=@  (merge:bignum (block-work-at:page:mt 126.000 %dumb-zkpow anchor-bn))
  =/  pre-w=@   (merge:bignum (compute-work:page:mt anchor-bn))
  ;:  weld
    (expect-eq !>(pre-w) !>(post-w))
    (expect-eq !>(306.374.333) !>(post-w))
  ==
:::
::  The AI ASERT anchor sets the puzzle's LAUNCH BLOCK INTERVAL and prices its
::  launch weight. An %ai-pow target prices one MAC-equivalent, so 2^256/anchor
::  is the expected MAC-equivalents per block; bex 192 is 2^64 of them, about a
::  hundred consumer GPUs at the 500s ideal.
++  test-ai-anchor-sets-the-launch-block-interval
  ^-  tang
  =/  mt  ~(. txe *blockchain-constants:txe)
  =/  mainnet  *blockchain-constants:txe
  =/  anchor  anchor-target-atom.ai-asert.mainnet
  %+  weld
    (expect-eq !>(64) !>((sub 256 (dec (met 0 anchor)))))
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
:::  ZK and AI anchors preserve the calibrated lane work rates at their revised
:::  214s and 500s ideal intervals.
++  test-post-ai-asert-anchors-calibrate-revised-cadence
  ^-  tang
  =/  mainnet  *blockchain-constants:txe
  ;:  weld
    %+  expect-eq
      !>((div (mul 375 (bex 291)) 214))
    !>(anchor-target-atom.zk-asert-post-ai.mainnet)
    %+  expect-eq
      !>((bex 192))
    !>(anchor-target-atom.ai-asert.mainnet)
  ==
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
:::  A block that reaches its puzzle's ceiling contributes the floored minimum,
:::  so a discounted branch cannot win by count. At the launch anchors one block
:::  of either puzzle is within 2.4x of the other, so neither puzzle's blocks are
:::  systematically orphaned at calibration; a capped block is worth less than
:::  any honest anchor block of either puzzle.
++  test-single-block-cannot-outweigh-a-run
  ^-  tang
  =/  mt  ~(. txe *blockchain-constants:txe)
  =/  mainnet  *blockchain-constants:txe
  =/  zk-anchor-w=@  (merge:bignum (block-work-at:page:mt 126.000 %dumb-zkpow (chunk:bignum anchor-target-atom.zk-asert-post-ai.mainnet)))
  =/  ai-anchor-w=@  (merge:bignum (block-work-at:page:mt 126.000 %ai-pow (chunk:bignum anchor-target-atom.ai-asert.mainnet)))
  =/  zk-cap-w=@  (merge:bignum (block-work-at:page:mt 126.000 %dumb-zkpow (chunk:bignum max-target-atom:mt)))
  =/  ai-cap-w=@  (merge:bignum (block-work-at:page:mt 126.000 %ai-pow (chunk:bignum max-ai-target-atom:mt)))
  ;:  weld
    ::  anchor blocks are within 3x of each other (about 2.338x)
    (expect-eq !>(%.y) !>((lth zk-anchor-w (mul 3 ai-anchor-w))))
    (expect-eq !>(%.y) !>((lth ai-anchor-w (mul 3 zk-anchor-w))))
    ::  capped blocks contribute the floored minimum
    (expect-eq !>(1) !>(zk-cap-w))
    (expect-eq !>(1) !>(ai-cap-w))
    ::  ...so a capped block is worth less than any honest anchor block
    (expect-eq !>(%.y) !>((lth zk-cap-w ai-anchor-w)))
    (expect-eq !>(%.y) !>((lth ai-cap-w zk-anchor-w)))
  ==
::
::  Per-block work at the Logos anchors uses the historical exchange rate.
::  Version %5 switches only blocks at and above height 147,500 to the public
::  Tip5-hash calibration.
++  test-anchor-work-is-exchange-rate-priced
  ^-  tang
  =/  mt  ~(. txe *blockchain-constants:txe)
  =/  mainnet  *blockchain-constants:txe
  ;:  weld
    (expect-eq !>(25.750.000.000) !>(mac-equivalents-per-zk-attempt:page:mt))
    (expect-eq !>(25.750.000.000) !>((mac-equivalents-per-zk-work-unit-at:page:mt 147.499)))
    (expect-eq !>(1.028.807) !>((mac-equivalents-per-zk-work-unit-at:page:mt 147.500)))
    %+  expect-eq  !>(306.374.333)
    !>((merge:bignum (block-work-at:page:mt 126.000 %dumb-zkpow (chunk:bignum anchor-target-atom.zk-asert-post-ai.mainnet))))
    %+  expect-eq  !>(716.378.410)
    !>((merge:bignum (block-work-at:page:mt 126.000 %ai-pow (chunk:bignum anchor-target-atom.ai-asert.mainnet))))
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
::  walks the production consensus promotion route for a post-activation
::  AI-PoW block (GHSA-g3g3-q33c-6fwh).
::
::    A block whose transactions arrive late takes the pending route:
::    +heard-block admits it via +validate-page-without-txs,
::    +add-pending-block parks it in `.pending-blocks`, and when the
::    transaction arrives +accept-page promotes it through
::    +accept-pending-block. The promoted block must carry the same `%4`
::    proof-version entry the direct +accept-block path writes. Before the
::    fix the promotion route omitted the write, so +block-id-to-proof-version
::    fell back to the height-derived legacy ZK version and misclassified
::    the block. The final control asserts +block-compute-work reads the
::    certificate artifact directly, so fork-choice heaviness stays AI-priced
::    either way.
++  test-pending-ai-block-promotion-records-proof-version
  ^-  tang
  =/  pt  ~(. txe bc-dual-post:helpers)
  ::  Height-3 chain accepted through the DIRECT path (block-versions
  ::  populated; the height-2 AI block carries its `%4` entry).
  =/  built  (build-typed-chain:hp ~[%zk %ai %zk])
  ::  Height-4 post-activation AI page whose transaction has NOT arrived yet.
  =/  ai-page=page:t  (make-ai-pow-page:hp tip.built con.built der.built)
  ::  A transaction id the page carries but whose raw tx is absent.
  =/  late-tx-id=tx-id:t  *hash:t
  =.  ai-page
    ?^  -.ai-page
      ai-page(tx-ids (~(put z-in tx-ids.ai-page) late-tx-id))
      ai-page(tx-ids (~(put z-in tx-ids.ai-page) late-tx-id))
  =.  ai-page
    ?^  -.ai-page
      ai-page(digest (compute-digest:page:pt ai-page))
      ai-page(digest (compute-digest:page:pt ai-page))
  =/  bid=block-id:t  ~(digest get:page:pt ai-page)
  ::  heard-block step 1: header validation passes without the raw tx.
  =/  header-ok=?  -:(~(validate-page-without-txs dcon con.built der.built bc-dual-post:helpers) ai-page ~(timestamp get:page:pt ai-page))
  ?>  header-ok
  ::  heard-block step 2: the missing transaction parks the block in pending.
  =/  [missing=(list tx-id:t) pending=consensus-state]
    (~(add-pending-block dcon con.built der.built bc-dual-post:helpers) ai-page)
  ?>  (lte 1 (lent missing))
  ?>  (~(has h-by pending-blocks.pending) bid)
  ::  The transaction arrives: +accept-page routes the still-pending block
  ::  through +accept-pending-block, the production promotion route.
  =/  promoted=consensus-state
    (~(accept-page dcon pending der.built bc-dual-post:helpers) ai-page *tx-acc:t *@da)
  ?>  (~(has h-by blocks.promoted) bid)
  ?>  ?!  (~(has h-by pending-blocks.promoted) bid)
  ::  Setup guard: the DIRECT path recorded `%4` for its height-2 AI block,
  ::  so +accept-block's conditional write is the only thing the promotion
  ::  route lacks. If this fails the fixture is wrong, not the consensus.
  =/  direct-ai-bid=block-id:t  (~(got z-by heaviest-chain.der.built) 2)
  ?>  ?=([~ %4] (~(get h-by block-versions.con.built) direct-ai-bid))
  =/  pv=proof-version:sp
    (~(block-id-to-proof-version dcon promoted der.built bc-dual-post:helpers) bid)
  =/  aw=@
    (merge:bignum (~(block-compute-work dcon promoted der.built bc-dual-post:helpers) ai-page))
  ;:  weld
    ::  the invariant: the promoted block carries the same `%4` entry the
    ::  direct path writes (this fails on the unfixed promotion route).
    %+  expect-eq  !>(%.y)
    !>(?=([~ %4] (~(get h-by block-versions.promoted) bid)))
    ::  consequence: +block-id-to-proof-version identifies the promoted
    ::  block as `%4` rather than the legacy height-derived ZK version.
    %+  expect-eq  !>(%4)
    !>(pv)
    ::  control: fork choice reads the certificate artifact directly, so
    ::  heaviness stays AI-priced even while the entry was missing.
    %+  expect-eq  !>(aw)
    !>((merge:bignum (ai-pow-work:page:pt ~(height get:page:pt ai-page) ~(target get:page:pt ai-page))))
  ==

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
  =/  expected-work  (add parent-work (merge:bignum (ai-pow-work:page:t ~(height get:page:t ai-cand) expected-target)))
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
