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
/=  *  /common/zeke
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
++  sample-ai-pow-cert
  |=  cert-len=@ud
  ^-  ai-pow-certificate:t
  =/  cert-data=@
    ?:  =(0 cert-len)  0
    (bex (dec (mul 8 cert-len)))
  =/  cert=ai-pow-certificate:t  *ai-pow-certificate:t
  cert(version 1, certificate [%bytes cert-len cert-data])
::
++  sample-ai-pow-artifact
  |=  cert-len=@ud
  ^-  pow-artifact:t
  [%ai-pow [4 (bex 31)] (sample-ai-pow-cert cert-len)]
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
  %+  expect-eq
    !>((compute-size-without-txs:page:t candidate))
    !>((compute-size-without-txs:page:t mined))
::
::  +test-ai-pow-size-charges-actual-artifact: AI artifacts are charged by
::  their actual jam size instead of the legacy fixed 90,000-byte proof weight.
++  test-ai-pow-size-charges-actual-artifact
  ^-  tang
  =/  small-art=*  (sample-ai-pow-artifact 4)
  =/  large-art=*  (sample-ai-pow-artifact 256)
  =/  base=page:t  sample-v1-page
  =/  small-page=page:t
    ?^  -.base
      base
    base(pow `[%ai-pow [4 (bex 31)] (sample-ai-pow-cert 4)])
  =/  large-page=page:t
    ?^  -.base
      base
    base(pow `[%ai-pow [4 (bex 31)] (sample-ai-pow-cert 256)])
  =/  small-size=@  (compute-size-without-txs:page:t small-page)
  =/  large-size=@  (compute-size-without-txs:page:t large-page)
  =/  page-delta=@  (sub large-size small-size)
  =/  artifact-delta=@
    %+  sub
      (compute-size-jam `*`large-art)
    (compute-size-jam `*`small-art)
  ;:  weld
    (expect !>((gth page-delta 0)))
    (expect-eq !>(artifact-delta) !>(page-delta))
  ==
:::
:::  +test-ai-pow-resource-allows-zero-padded-byte-atoms: declared byte lengths
:::  are consensus-visible. The atom may have a shorter canonical byte length
:::  when the declared bytes end in zero.
++  test-ai-pow-resource-allows-zero-padded-byte-atoms
  ^-  tang
  =/  cert=ai-pow-certificate:t  *ai-pow-certificate:t
  =/  base=page:t  sample-v1-page
  =/  padded-page=page:t
    ?^  -.base
      base
    base(pow `[%ai-pow [4 0] cert(version 1, certificate [%bytes 4 0])])
  =/  ref-page=page:t
    ?^  -.base
      base
    base(pow `[%ai-pow [4 1] cert(version 1, certificate [%bytes 4 1])])
  =/  padded-size=@  (compute-size-without-txs:page:t padded-page)
  =/  ref-size=@  (compute-size-without-txs:page:t ref-page)
  (expect !>((lth padded-size (add ref-size 1.000))))
--
