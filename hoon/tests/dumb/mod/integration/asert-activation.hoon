::  tests/dumb/mod/integration/asert-activation.hoon
::
::    activation-boundary integration tests for aserti3-2d.
::    builds a chain with a low phase.zk-asert, runs it across the boundary,
::    and cross-checks consensus's +compute-target-asert against a direct
::    call into lib/asert for the same inputs. phase 2 of 014-aletheia
::    pins the anchor's median-of-11 as a hardcoded
::    `anchor-min-timestamp.zk-asert` field on blockchain-constants, so the
::    test bc must encode the value the test chain would produce at the
::    anchor — median of 5 timestamps at 600s spacing from the genesis
::    timestamp (= `time-in-secs *@da`), i.e. T0 + 1.200.
/=  helpers  /tests/dumb/helpers
/=  asert  /apps/dumbnet/lib/asert
/=  dcon  /apps/dumbnet/lib/consensus
/=  dt  /apps/dumbnet/lib/types
/=  txe  /common/tx-engine
/=  *  /common/h-zoon
/=  *  /common/zeke
/=  *  /common/test
::
=>
|%
::  bc-asert: constants with a very low phase.zk-asert so we can reach it in
::    tests. anchor-min-timestamp.zk-asert pins the median-of-11 the test
::    chain produces at anchor-height=4 with 600s/block spacing starting
::    at default-genesis-timestamp = *@da.
++  bc-asert
  %*  .  default-bc:helpers
    blocks-per-epoch            1.000.000     :: avoid epoch boundary inside test
    v1-phase                    5             :: must be <= phase.zk-asert
    phase.zk-asert                 5
    anchor-height.zk-asert         4
    anchor-target-atom.zk-asert    ^~((div max-tip5-atom:tip5 (bex 14)))
    ideal-block-time.zk-asert      150
    half-life.zk-asert             43.200
    anchor-min-timestamp.zk-asert  (add (time-in-secs:page:txe *@da) 1.200)
  ==
--
::
|%
++  h  ~(. helpers bc-asert)
++  t  ~(. txe bc-asert)
::  +der: pre-activation derived-state (read-only extra arg for consensus door)
++  der  ^-  derived-state:dt  *derived-state:dt
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
  =/  anchor-min-ts  anchor-min-timestamp.zk-asert.bc
  =/  got-bn
    (~(compute-target-asert dcon con der bc) %zk 5 parent-digest)
  =/  expected-atom
    %-  compute-target:asert
    :*  anchor-target-atom.zk-asert.bc
        anchor-min-ts
        anchor-height.zk-asert.bc
        parent-min-ts
        5
        ideal-block-time.zk-asert.bc
        half-life.zk-asert.bc
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
  =/  anchor-min-ts  anchor-min-timestamp.zk-asert.bc
  =/  got-bn
    (~(compute-target-asert dcon con der bc) %zk 6 parent-digest)
  =/  expected-atom
    %-  compute-target:asert
    :*  anchor-target-atom.zk-asert.bc
        anchor-min-ts
        anchor-height.zk-asert.bc
        parent-min-ts
        6
        ideal-block-time.zk-asert.bc
        half-life.zk-asert.bc
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
    (~(compute-target-asert dcon con der bc) %zk 6 parent-digest)
  =/  expected-atom
    %-  compute-target:asert
    :*  anchor-target-atom.zk-asert.bc
        anchor-min-timestamp.zk-asert.bc
        anchor-height.zk-asert.bc
        parent-min-ts
        6
        ideal-block-time.zk-asert.bc
        half-life.zk-asert.bc
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
  (expect-eq !>(observed) !>(anchor-min-timestamp.zk-asert.bc))
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
    (~(compute-target-asert dcon con der bc) %zk 5 parent-digest)
  (expect-eq !>(anchor-target-atom.zk-asert.bc) !>((merge:bignum got-bn)))
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
    (~(active-asert-anchor dcon con der bc) %zk 112.499)
  =/  active=(unit asert-anchor:dcon)
    (~(active-asert-anchor dcon con der bc) %zk 112.500)
  =/  wrong-type=(unit asert-anchor:dcon)
    (~(active-asert-anchor dcon con der bc) %other 112.500)
  =/  expected-original=asert-anchor:dcon
    [phase.zk-asert.bc anchor-target-atom.zk-asert.bc `anchor-min-timestamp.zk-asert.bc]
  =/  expected-reanchor=asert-anchor:dcon
    [112.500 (div max-target-atom.bc (mul 3.000.000 ideal-block-time.zk-asert.bc)) ~]
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
    (~(compute-target-asert dcon con der bc) %zk 112.500 anchor-id)
  =.  con  (~(update-asert-anchor-min-timestamps dcon con der bc) %zk child)
  =/  got-bn
    (~(compute-target-asert dcon con der bc) %zk 112.500 anchor-id)
  =/  expected-target
    (div max-target-atom.bc (mul 3.000.000 ideal-block-time.zk-asert.bc))
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
            (~(get-asert-anchor-min-timestamp dcon con der bc) %zk 112.499 child-id)
        ==
--
