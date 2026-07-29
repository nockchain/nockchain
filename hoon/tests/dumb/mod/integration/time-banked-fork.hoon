::  tests/dumb/mod/integration/time-banked-fork.hoon
::
::  A real-kernel reproduction of the delayed-fork ASERT failure.  The shared
::  ancestor has a complete median-time-past window.  A private branch then
::  supplies six legal far-future timestamps, moves MTP to that time, reaches
::  the ZK ASERT ceiling, and wins fork choice solely by adding blocks.
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
    half-life.zk-asert                    43.200
    anchor-min-timestamp.zk-asert         0
    phase.zk-asert-post-ai                11
    anchor-height.zk-asert-post-ai        10
    anchor-target-atom.zk-asert-post-ai   ^~((div max-tip5-atom:tip5 (bex 1)))
    ideal-block-time.zk-asert-post-ai     375
    half-life.zk-asert-post-ai            43.200
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
::  Build a ZK candidate with the exact post-activation target and flat
::  accumulated work that the live kernel recomputes before it verifies PoW.
++  build-zk-asert-page
  |=  [parent=page:t ts=@ nockchain=_nockchain:h]
  ^-  page:t
  =/  con=consensus-state  (live-con nockchain)
  =/  der=derived-state    (live-der nockchain)
  =/  height=@  +(~(height get:page:t parent))
  =/  target=bignum:bignum:t
    (~(compute-target-zk-asert dcon con der bc-time-banked-fork) height ~(digest get:page:t parent))
  =/  accumulated-work=bignum:bignum:t
    %-  chunk:bignum:t
    %+  add
      (merge:bignum:t ~(accumulated-work get:page:t parent))
    (merge:bignum:t dual-puzzle-block-work:page:t)
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
  ?>  (lth retries 64)
  =/  pag=page:t  (prove-page:h (build-zk-asert-page parent ts nockchain))
  =/  bid=block-id:t  ~(digest get:page:t pag)
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) pag)
  =/  con=consensus-state  (live-con nockchain)
  ?:  (~(has h-by blocks.con) bid)
    [pag nockchain]
  $(ts +(ts), retries +(retries))
::
::  The live path is `prove-page` -> `heard-block`: every page has a real ZK
::  proof, target validation, timestamp validation, block admission, and
::  fork-choice update.  The private branch's timestamps are legal under the
::  current rules, yet its seventh post-anchor target reaches the ceiling.
++  test-time-banked-fork-wins-by-count
  ^-  tang
  =+  [nockchain genesis]=init-nockchain:h
  ::  Heights 1..10 create the pre-activation MTP window and the shared
  ::  activation predecessor at height 10.
  =^  shared=(list page:t)  nockchain
    (add-n-pages-integration:h genesis 10 nockchain)
  =/  anchor=page:t  (snag 9 shared)
  =/  anchor-ts=@  ~(timestamp get:page:t anchor)
  ::  Honest height 11 establishes the public tip.  Its target is still below
  ::  the ceiling because its parent MTP remains near the anchor clock.
  =^  public-page=page:t  nockchain
    (hear-proven-zk anchor (add anchor-ts 600) 0 nockchain)
  ::  The attacker starts from the same height-10 anchor much later.  Six
  ::  timestamps near one common wall-clock time flip the 11-block MTP; the
  ::  seventh child therefore sees that elapsed interval with only six virtual
  ::  ZK blocks in its ASERT history.
  =/  future=@  (add anchor-ts 1.000.000)
  =^  private1=page:t  nockchain  (hear-proven-zk anchor future 0 nockchain)
  =^  private2=page:t  nockchain  (hear-proven-zk private1 future 0 nockchain)
  =^  private3=page:t  nockchain  (hear-proven-zk private2 future 0 nockchain)
  =^  private4=page:t  nockchain  (hear-proven-zk private3 future 0 nockchain)
  =^  private5=page:t  nockchain  (hear-proven-zk private4 future 0 nockchain)
  =^  private6=page:t  nockchain  (hear-proven-zk private5 future 0 nockchain)
  =^  private7=page:t  nockchain  (hear-proven-zk private6 future 0 nockchain)
  =/  public-target=@  (merge:bignum:t ~(target get:page:t public-page))
  =/  attacker-target=@  (merge:bignum:t ~(target get:page:t private7))
  ;:  weld
    (expect-eq !>(%.y) !>((lth public-target max-target-atom:txe)))
    (expect-eq !>(max-target-atom:txe) !>(attacker-target))
    ::  Each post-activation block carries the same work, so the capped-target
    ::  private branch wins once released even though its target is easier.  A
    ::  fork-choice rule that prices difficulty must invert this outcome.
    (expect-eq !>(~(digest get:page:t private7)) !>(~(heaviest-block k-by:h nockchain)))
  ==
--
