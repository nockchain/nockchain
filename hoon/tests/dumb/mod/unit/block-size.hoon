::  tests/dumb/mod/unit/block-size.hoon
::
::    Regression tests for block-size accounting.
::
::    The v1 $page prepends a `version` head that v0 lacks, so its layout is
::    [version digest pow parent ...] versus v0's [digest pow parent ...] --
::    every field is one axis deeper. +compute-size-without-txs jams the page
::    minus its digest and proof (both are variable-length but bounded, and
::    are accounted for by +max-size constants instead). v0 jams `+>.pag`
::    (axis 7 = [parent ...]); the v1 arm copied that literally, but on a v1
::    page axis 7 is [pow parent ...], so it wrongly folded the proof into the
::    jammed size. A mining candidate carries pow=~ while the mined block
::    carries the full proof, so the miner's +candidate-block-below-max-size
::    guard (run on the candidate) disagreed with consensus +check-size (run
::    on the mined page): a miner would mine a block it then self-rejected as
::    %block-too-large, and the chain could not advance. The fix jams
::    `+>+.pag` (axis 15 = [parent ...]), excluding version, digest, AND pow.
::
::    These tests pin that property: the size of a page excluding its txs must
::    not depend on whether the proof is present.
::
/=  txe  /common/tx-engine
/=  *  /common/zoon
/=  *  /common/test
|_  constants=blockchain-constants:txe
+*  t  ~(. txe constants)
::
::  +sample-v1-page: a v1 page with non-trivial header content and no proof,
::  exactly as a fresh mining candidate has it.
++  sample-v1-page
  ^-  page:v1:t
  =/  a-hash=hash:t  *hash:t
  =/  p=page:v1:t  *page:v1:t
  %=    p
    height    102.009
    tx-ids    (~(put z-in *(z-set tx-id:t)) a-hash)
    coinbase  (~(put z-by *coinbase-split:v1:t) a-hash 123.456)
  ==
::
::  +test-compute-size-ignores-pow: a mining candidate (pow=~) and the same
::  block once mined (pow set to a proof) must size identically. Before the
::  fix these differed by ~the size of the proof, wedging block production.
++  test-compute-size-ignores-pow
  ^-  tang
  =/  candidate=page:v1:t  sample-v1-page
  =/  mined=page:v1:t  candidate(pow (some *proof:t))
  =/  size-candidate=@  (compute-size-without-txs:page:t candidate)
  =/  size-mined=@  (compute-size-without-txs:page:t mined)
  ;:  weld
    ::  the proof must not change the accounted size, so the miner guard and
    ::  consensus check-size agree.
    (expect-eq !>(size-candidate) !>(size-mined))
    ::  a full proof's worth of bits is still reserved via the max-size
    ::  constant, so the fix did not simply stop budgeting for the proof.
    (expect !>((gte size-candidate max-size:proof:t)))
  ==
::
::  +test-compute-size-ignores-pow-empty: same invariant on a bare page, so
::  the property does not depend on header contents.
++  test-compute-size-ignores-pow-empty
  ^-  tang
  =/  candidate=page:v1:t  *page:v1:t
  =/  mined=page:v1:t  candidate(pow (some *proof:t))
  (expect-eq
    !>((compute-size-without-txs:page:t candidate))
    !>((compute-size-without-txs:page:t mined)))
--
