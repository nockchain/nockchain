::  tests/dumb/mod/integration/time-banked-fork.hoon
::
::  Real-kernel coverage of accumulated-work fork choice. A longer branch
::  at the ZK ceiling can have less accumulated work than a shorter branch
::  with tightening targets. Each ceiling-target block contributes the
::  floored minimum work; branch length alone does not determine fork choice.
::
/=  helpers  /tests/dumb/helpers
/=  dcon     /apps/dumbnet/lib/consensus
/=  txe      /common/tx-engine
/=  *        /apps/dumbnet/lib/types
/=  *        /common/zeke
/=  *        /common/test
=>
|%
++  bc-time-banked-fork
  %*  .  bc-pending-provable:helpers
    v1-phase                              1
    blocks-per-epoch                      1.000.000
    ai-pow-activation-height              11
    phase.zk-asert                        11
    anchor-height.zk-asert                10
    anchor-target-atom.zk-asert           ^~((div max-tip5-atom:tip5 (bex 1)))
    ideal-block-time.zk-asert             375
    half-life.zk-asert                    600
    anchor-min-timestamp.zk-asert         0
    phase.zk-asert-post-ai                11
    anchor-height.zk-asert-post-ai        10
    anchor-target-atom.zk-asert-post-ai   ^~((div max-tip5-atom:tip5 (bex 1)))
    ideal-block-time.zk-asert-post-ai     375
    half-life.zk-asert-post-ai            600
    anchor-min-timestamp.zk-asert-post-ai  0
    phase.ai-asert                        11
    anchor-height.ai-asert                10
    anchor-min-timestamp.ai-asert         0
  ==
--
::
|%
++  h  ~(. helpers bc-time-banked-fork)
++  t  ~(. txe bc-time-banked-fork)
::
::  Memoized reads of the live kernel's consensus/derived state; the cast is a
::  static assertion, so repeated calls within a poke cost only the wing walk.
++  live-con
  |=  nockchain=_nockchain:h
  ~+  ^-  consensus-state
  ;;(consensus-state c.internal.outer.nockchain)
::
++  live-der
  |=  nockchain=_nockchain:h
  ~+  ^-  derived-state
  ;;(derived-state d.internal.outer.nockchain)
::
::  Build a ZK candidate with the exact target and accumulated work that the
::  live kernel recomputes before it verifies PoW.
++  build-zk-asert-page
  |=  [parent=page:t ts=@ nockchain=_nockchain:h]
  ^-  page:t
  =/  con=consensus-state  (live-con nockchain)
  =/  der=derived-state    (live-der nockchain)
  =/  height=@  +(~(height get:page:t parent))
  ::  Pre-activation blocks inherit the parent's stored target, matching
  ::  validation's own pre-phase path; the ASERT runs from the phase on.
  =/  target=bignum:bignum:t
    ?:  (lth height dual-puzzle-phase:page:t)
      ~(target get:page:t parent)
    (~(compute-target-zk-asert dcon con der bc-time-banked-fork) height ~(digest get:page:t parent))
  =/  accumulated-work=bignum:bignum:t
    %-  chunk:bignum:t
    %+  add
      (merge:bignum:t ~(accumulated-work get:page:t parent))
    (merge:bignum:t (block-work-at:page:t height %dumb-zkpow target))
  =/  pag=page:t  (make-empty-page:h parent)
  =.  pag
    ?^  -.pag  pag(target target)  pag(target target)
  =.  pag
    ?^  -.pag  pag(accumulated-work accumulated-work)  pag(accumulated-work accumulated-work)
  =.  pag
    ?^  -.pag  pag(timestamp ts)  pag(timestamp ts)
  =.  pag
    ?^  -.pag  pag(digest (compute-digest:page:t pag))  pag(digest (compute-digest:page:t pag))
  pag
::
::  The anchor target is one bit below the ZK ceiling, so a deterministic proof
::  may need a few candidates.  Every retry changes only the timestamp and is
::  bounded; a rejected candidate never changes consensus state.
++  hear-proven-zk
  |=  [parent=page:t ts=@ retries=@ nockchain=_nockchain:h]
  ^-  [page:t _nockchain:h]
  ?>  (lth retries 128)
  =/  pag=page:t  (prove-page:h (build-zk-asert-page parent ts nockchain))
  =/  bid=block-id:t  ~(digest get:page:t pag)
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) pag)
  =/  con=consensus-state  (live-con nockchain)
  ?:  (~(has h-by blocks.con) bid)
    [pag nockchain]
  $(ts +(ts), retries +(retries))
::
::  Chain of `n` ZK blocks at `step`-second spacing through the live kernel.
++  hear-zk-run
  |=  [parent=page:t first-ts=@ step=@ n=@ nockchain=_nockchain:h]
  ^-  [page:t _nockchain:h]
  =/  i=@  1
  =/  tip=page:t  parent
  |-
  ?:  (gth i n)  [tip nockchain]
  =^  next=page:t  nockchain
    (hear-proven-zk tip (add first-ts (mul (dec i) step)) 0 nockchain)
  $(i +(i), tip next)
::
::  The live path is `prove-page` -> `heard-block`: every page has a real ZK
::  proof, target validation, timestamp validation, block admission, and
::  fork-choice update. The longer ceiling-target branch contributes minimum
::  work per capped block and loses to the shorter tightening-target branch.
++  test-time-banked-fork-loses-by-work
  ^-  tang
  =+  [nockchain genesis]=init-nockchain:h
  =/  g=@  ~(timestamp get:page:t genesis)
  ::  Heights 1..10 at 150s spacing: a full MTP window whose median advances
  ::  slower than the 375s ideal, so the branch built on it TIGHTENS.  The
  ::  height-10 block is the shared activation predecessor.
  =^  anchor=page:t  nockchain
    (hear-zk-run genesis (add g 150) 150 10 nockchain)
  =/  anchor-ts=@  ~(timestamp get:page:t anchor)
  ::  The shorter branch keeps the 150s cadence: eight blocks whose ASERT
  ::  targets tighten below the anchor target.
  =^  public-tip=page:t  nockchain
    (hear-zk-run anchor (add anchor-ts 150) 150 8 nockchain)
  ::  The longer branch shares the height-10 anchor and uses fixed later
  ::  timestamps. Its target reaches the ceiling under the same ASERT rules.
  =^  private-tip=page:t  nockchain
    (hear-zk-run anchor (add anchor-ts 1.000.000) 0 16 nockchain)
  =/  public-target=@   (merge:bignum:t ~(target get:page:t public-tip))
  =/  private-target=@  (merge:bignum:t ~(target get:page:t private-tip))
  =/  public-work=@     (merge:bignum:t ~(accumulated-work get:page:t public-tip))
  =/  private-work=@    (merge:bignum:t ~(accumulated-work get:page:t private-tip))
  =/  con=consensus-state  (live-con nockchain)
  =/  der=derived-state    (live-der nockchain)
  ;:  weld
    ::  the shorter branch tightened below the ceiling
    (expect-eq !>(%.y) !>((lth public-target max-target-atom:txe)))
    ::  the longer branch's target reaches the ceiling
    (expect-eq !>(max-target-atom:txe) !>(private-target))
    ::  ...so its blocks earn the floored minimum work
    %+  expect-eq  !>(1)
    !>((merge:bignum:t (~(block-compute-work dcon con der bc-time-banked-fork) private-tip)))
    ::  the ceiling-target branch is longer than the tightening-target branch
    (expect-eq !>(%.y) !>((gth ~(height get:page:t private-tip) ~(height get:page:t public-tip))))
    ::  ...but LIGHTER: discounts do not accumulate into heaviness
    (expect-eq !>(%.y) !>((lth private-work public-work)))
    ::  ...and fork choice follows the work, not the count
    %+  expect-eq
      !>(~(digest get:page:t public-tip))
    !>(~(heaviest-block k-by:h nockchain))
  ==
--
