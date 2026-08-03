::  tests/dumb/mod/integration/asert-activation.hoon
::
::    activation-boundary integration tests for aserti3-2d.
::    builds a chain with a low asert-phase, runs it across the boundary,
::    and cross-checks consensus's +compute-target-asert against a direct
::    call into lib/asert for the same inputs. phase 2 of 014-aletheia
::    pins the anchor's median-of-11 as a hardcoded
::    `asert-anchor-min-timestamp` field on blockchain-constants, so the
::    test bc must encode the value the test chain would produce at the
::    anchor — median of 5 timestamps at 600s spacing from the genesis
::    timestamp (= `time-in-secs *@da`), i.e. T0 + 1.200.
/=  helpers  /tests/dumb/helpers
/=  asert  /apps/dumbnet/lib/asert
/=  dcon  /apps/dumbnet/lib/consensus
/=  txe  /common/tx-engine
/=  *  /common/h-zoon
/=  *  /common/zeke
/=  *  /common/test
::
=>
|%
::  bc-asert: constants with a very low asert-phase so we can reach it in
::    tests. asert-anchor-min-timestamp pins the median-of-11 the test
::    chain produces at anchor-height=4 with 600s/block spacing starting
::    at default-genesis-timestamp = *@da.
++  bc-asert
  %*  .  default-bc:helpers
    blocks-per-epoch            1.000.000     :: avoid epoch boundary inside test
    v1-phase                    5             :: must be <= asert-phase
    asert-phase                 5
    asert-anchor-height         4
    asert-anchor-target-atom    ^~((div max-tip5-atom:tip5 (bex 14)))
    asert-ideal-block-time      150
    asert-half-life             43.200
    asert-anchor-min-timestamp  (add (time-in-secs:page:txe *@da) 1.200)
  ==
--
::
|%
++  h  ~(. helpers bc-asert)
++  t  ~(. txe bc-asert)
::
::  +test-asert-wrapper-matches-library: after building 4 blocks (reaching
::    the anchor at height 4), calling the consensus wrapper
::    +compute-target-asert for height=5 must yield the same result as a
::    direct call into +compute-target:asert with the same inputs. at this
::    moment the parent *is* the anchor, so parent-min-ts and anchor-min-ts
::    resolve to the same value. post-phase-2: anchor-min-ts is the bc
::    constant, not a walk result.
++  test-asert-wrapper-matches-library
  =/  bc  bc-asert
  =/  con  (initial-consensus-state-custom:h bc)
  =^  par=page:t  con  (add-n-pages:h 4 con default-retain:h)
  =/  parent-digest  ~(digest get:page:t par)
  =/  parent-min-ts  (~(got h-by min-timestamps.con) parent-digest)
  =/  anchor-min-ts  asert-anchor-min-timestamp.bc
  =/  got-bn
    (~(compute-target-asert dcon con bc) %zk 5 parent-digest)
  =/  expected-atom
    %-  compute-target:asert
    :*  asert-anchor-target-atom.bc
        anchor-min-ts
        asert-anchor-height.bc
        parent-min-ts
        5
        asert-ideal-block-time.bc
        asert-half-life.bc
        max-target-atom.bc
    ==
  (expect-eq !>(expected-atom) !>((merge:bignum got-bn)))
::
::  +test-asert-wrapper-past-anchor: after building to height 5 (one block
::    past the anchor), the wrapper at height=6 uses a parent at height 5
::    whose min-timestamps differs from the anchor's min-of-11. exercises
::    the post-anchor path where anchor-min-ts (bc constant) and
::    parent-min-ts (chain state) diverge.
++  test-asert-wrapper-past-anchor
  =/  bc  bc-asert
  =/  con  (initial-consensus-state-custom:h bc)
  =^  par=page:t  con  (add-n-pages:h 5 con default-retain:h)
  =/  parent-digest  ~(digest get:page:t par)
  =/  parent-min-ts  (~(got h-by min-timestamps.con) parent-digest)
  =/  anchor-min-ts  asert-anchor-min-timestamp.bc
  =/  got-bn
    (~(compute-target-asert dcon con bc) %zk 6 parent-digest)
  =/  expected-atom
    %-  compute-target:asert
    :*  asert-anchor-target-atom.bc
        anchor-min-ts
        asert-anchor-height.bc
        parent-min-ts
        6
        asert-ideal-block-time.bc
        asert-half-life.bc
        max-target-atom.bc
    ==
  (expect-eq !>(expected-atom) !>((merge:bignum got-bn)))
::
::  Fixed anchor timestamps are consensus constants, not retained-state lookups.
++  test-asert-fixed-anchor-does-not-walk-state
  =/  bc  bc-asert
  =/  con  (initial-consensus-state-custom:h bc)
  =^  par=page:t  con  (add-n-pages:h 5 con default-retain:h)
  =/  parent-digest  ~(digest get:page:t par)
  =/  parent-min-ts  (~(got h-by min-timestamps.con) parent-digest)
  =.  con  con(blocks *(h-map block-id:t local-page:t))
  =/  got-bn
    (~(compute-target-asert dcon con bc) %zk 6 parent-digest)
  =/  expected-atom
    %-  compute-target:asert
    :*  asert-anchor-target-atom.bc
        asert-anchor-min-timestamp.bc
        asert-anchor-height.bc
        parent-min-ts
        6
        asert-ideal-block-time.bc
        asert-half-life.bc
        max-target-atom.bc
    ==
  (expect-eq !>(expected-atom) !>((merge:bignum got-bn)))
::  +test-asert-anchor-min-ts-matches-observed: the bc-pinned constant
::    must equal the median-of-11 the consensus state actually wrote for
::    the canonical anchor block when it was accepted. this is the load-
::    bearing invariant of the phase-2 cutover: pinning the constant
::    correctly preserves bit-for-bit continuity vs the phase-1 walk.
++  test-asert-anchor-min-ts-matches-observed
  =/  bc  bc-asert
  =/  con  (initial-consensus-state-custom:h bc)
  =^  par=page:t  con  (add-n-pages:h 4 con default-retain:h)
  =/  anchor-digest  ~(digest get:page:t par)
  =/  observed       (~(got h-by min-timestamps.con) anchor-digest)
  (expect-eq !>(observed) !>(asert-anchor-min-timestamp.bc))
::
::  +test-asert-wrapper-activation-identity: production-semantic pin for
::    the activation boundary. child-height = anchor-height + 1 with the
::    parent == anchor implies exponent = 0, factor = radix, target =
::    anchor_target exactly. this pins the wrapper against a value
::    derived externally from Eq. (2), rather than against a library
::    call on the same inputs (as +test-asert-wrapper-matches-library
::    does). covers audit item 9.
++  test-asert-wrapper-activation-identity
  =/  bc  bc-asert
  =/  con  (initial-consensus-state-custom:h bc)
  =^  par=page:t  con  (add-n-pages:h 4 con default-retain:h)
  =/  parent-digest  ~(digest get:page:t par)
  =/  got-bn
    (~(compute-target-asert dcon con bc) %zk 5 parent-digest)
  (expect-eq !>(asert-anchor-target-atom.bc) !>((merge:bignum got-bn)))
::
:::
::  The %zk schedule starts at the canonical anchor and changes at 112.500.
::  The first child can read its parent timestamp directly, but every later
::  lookup must use the accepted branch's O(1) timestamp cache.  Missing cache
::  state fails closed instead of walking retained ancestry.
++  test-asert-reanchor-requires-cached-timestamp
  =/  bc  bc-asert
  =/  con  (initial-consensus-state-custom:h bc)
  =^  par=page:t  con  (add-n-pages:h 4 con default-retain:h)
  =/  before=(unit asert-anchor:dcon)
    (~(active-asert-anchor dcon con bc) %zk 112.499)
  =/  active=(unit asert-anchor:dcon)
    (~(active-asert-anchor dcon con bc) %zk 112.500)
  =/  wrong-type=(unit asert-anchor:dcon)
    (~(active-asert-anchor dcon con bc) %other 112.500)
  =/  expected-original=asert-anchor:dcon
    [asert-phase.bc asert-anchor-target-atom.bc `asert-anchor-min-timestamp.bc]
  =/  expected-reanchor=asert-anchor:dcon
    [112.500 (div max-target-atom.bc (mul 3.000.000 asert-ideal-block-time.bc)) ~]
  =/  anchor=page:t
    ?^  -.par
      par(height 112.499)
    par(height 112.499)
  =/  anchor-id=block-id:t  ~(digest get:page:t anchor)
  =/  anchor-min-ts=@  (~(got h-by min-timestamps.con) anchor-id)
  =/  raw-child=page:t  (make-empty-page:h par)
  =/  child=page:t
    ?^  -.raw-child
      raw-child(height 112.500)
    raw-child(height 112.500)
  =/  child-id=block-id:t  ~(digest get:page:t child)
  =/  recovered-target
    (~(compute-target-asert dcon con bc) %zk 112.500 anchor-id)
  =.  con  (~(update-asert-anchor-min-timestamps dcon con bc) %zk child)
  =.  blocks.con
    (~(put h-by blocks.con) anchor-id (to-local-page:page:t anchor))
  =.  blocks.con
    (~(put h-by blocks.con) child-id (to-local-page:page:t child))
  =/  uncached
    con(asert-anchor-min-timestamps (~(del by asert-anchor-min-timestamps.con) %zk))
  =/  cached-no-blocks
    con(blocks *(h-map block-id:t local-page:t))
  =/  got-bn
    (~(compute-target-asert dcon con bc) %zk 112.500 anchor-id)
  =/  expected-target
    (div max-target-atom.bc (mul 3.000.000 asert-ideal-block-time.bc))
  ;:  weld
    %+  expect-fail
      |.  (~(get-asert-anchor-min-timestamp dcon uncached bc) %zk 112.499 child-id)
    ~
    %+  expect-eq
      !>  :*  %.y
                %.y
                %.y
                expected-target
                expected-target
                anchor-min-ts
            ==
    !>  :*  =(before `expected-original)
              =(active `expected-reanchor)
              =(wrong-type ~)
              (merge:bignum recovered-target)
              (merge:bignum got-bn)
              (~(get-asert-anchor-min-timestamp dcon cached-no-blocks bc) %zk 112.499 child-id)
          ==
  ==
::
::  Zoe re-pins ASERT at the same height that selects proof version %3.  The
::  target is the original Aletheia 2^291 target, not a newly calibrated
::  approximation, so the first Zoe block and zero-drift baseline return to
::  about 536.9 million expected attempts.
++  test-zoe-asert-reanchor-restores-aletheia-target
  =/  bc  bc-asert
  =/  production-bc  default-bc:helpers
  =/  con  (initial-consensus-state-custom:h bc)
  =^  par=page:t  con  (add-n-pages:h 4 con default-retain:h)
  =/  before=(unit asert-anchor:dcon)
    (~(active-asert-anchor dcon con bc) %zk (dec proof-version-3-start:dcon))
  =/  active=(unit asert-anchor:dcon)
    (~(active-asert-anchor dcon con bc) %zk proof-version-3-start:dcon)
  =/  after=(unit asert-anchor:dcon)
    (~(active-asert-anchor dcon con bc) %zk +(proof-version-3-start:dcon))
  =/  expected-before=asert-anchor:dcon
    [112.500 (div max-target-atom.bc (mul 3.000.000 asert-ideal-block-time.bc)) ~]
  =/  expected-active=asert-anchor:dcon
    [proof-version-3-start:dcon asert-anchor-target-atom.bc ~]
  =/  anchor=page:t
    ?^  -.par
      par(height (dec proof-version-3-start:dcon))
    par(height (dec proof-version-3-start:dcon))
  =/  anchor-id=block-id:t  ~(digest get:page:t anchor)
  =/  anchor-min-ts=@  (~(got h-by min-timestamps.con) anchor-id)
  =/  first-bn
    (~(compute-target-asert dcon con bc) %zk proof-version-3-start:dcon anchor-id)
  =.  con  (~(update-asert-anchor-min-timestamps dcon con bc) %zk anchor)
  =/  child=page:t  (make-empty-page:h anchor)
  =/  child-id=block-id:t  ~(digest get:page:t child)
  =.  min-timestamps.con
    (~(put h-by min-timestamps.con) child-id anchor-min-ts)
  =.  con  (~(update-asert-anchor-min-timestamps dcon con bc) %zk child)
  =/  grandchild=page:t  (make-empty-page:h child)
  =/  grandchild-id=block-id:t  ~(digest get:page:t grandchild)
  =.  min-timestamps.con
    (~(put h-by min-timestamps.con) grandchild-id anchor-min-ts)
  ::  This update takes the recursive cache-to-cache propagation branch.
  =.  con  (~(update-asert-anchor-min-timestamps dcon con bc) %zk grandchild)
  =.  blocks.con
    (~(put h-by blocks.con) anchor-id (to-local-page:page:t anchor))
  =.  blocks.con
    (~(put h-by blocks.con) child-id (to-local-page:page:t child))
  =.  blocks.con
    (~(put h-by blocks.con) grandchild-id (to-local-page:page:t grandchild))
  =/  expected-next=@
    %-  compute-target:asert
    :*  asert-anchor-target-atom.bc
        anchor-min-ts
        (dec proof-version-3-start:dcon)
        anchor-min-ts
        +(proof-version-3-start:dcon)
        asert-ideal-block-time.bc
        asert-half-life.bc
        max-target-atom.bc
    ==
  =/  cached-no-blocks
    con(blocks *(h-map block-id:t local-page:t))
  =/  uncached
    con(asert-anchor-min-timestamps (~(del by asert-anchor-min-timestamps.con) %zk))
  =/  next-bn
    (~(compute-target-asert dcon cached-no-blocks bc) %zk +(proof-version-3-start:dcon) child-id)
  =/  expected-after-next=@
    %-  compute-target:asert
    :*  asert-anchor-target-atom.bc
        anchor-min-ts
        (dec proof-version-3-start:dcon)
        anchor-min-ts
        +(+(proof-version-3-start:dcon))
        asert-ideal-block-time.bc
        asert-half-life.bc
        max-target-atom.bc
    ==
  =/  after-next-bn
    (~(compute-target-asert dcon cached-no-blocks bc) %zk +(+(proof-version-3-start:dcon)) grandchild-id)
  ;:  weld
    %+  expect-fail
      |.  (~(compute-target-asert dcon uncached bc) %zk +(+(proof-version-3-start:dcon)) grandchild-id)
    ~
    %+  expect-eq
      !>  :*  `expected-before
                `expected-active
                `expected-active
                asert-anchor-target-atom.bc
                expected-next
                expected-after-next
                (bex 291)
            ==
    !>  :*  before
              active
              after
              (merge:bignum first-bn)
              (merge:bignum next-bn)
              (merge:bignum after-next-bn)
              asert-anchor-target-atom.production-bc
          ==
  ==
--
