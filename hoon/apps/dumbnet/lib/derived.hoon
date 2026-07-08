/=  dk  /apps/dumbnet/lib/types
/=  dumb-transact  /common/tx-engine
/=  *  /common/h-zoon
::
~%  %dumb-derived  ..ut  ~
|_  [d=derived-state:dk =blockchain-constants:dumb-transact]
+*  t  ~(. dumb-transact blockchain-constants)
::  +update: update metadata derived from consensus state
++  update
  ~/  %update
  |=  [c=consensus-state:dk pag=page:t]
  ^-  derived-state:dk
  ::  update highest height
  =.  d  (update-highest ~(height get:page:t pag))
  ::  populate per-puzzle ASERT anchor caches if this block crosses an
  ::  activation boundary. Deterministic O(1) lookup: the cache entry
  ::  is read directly from consensus state's min-timestamps + targets
  ::  maps keyed by the parent block-id.
  =.  d  (populate-zk-asert-post-ai-anchor c pag)
  :: update view of heaviest chain
  =/  heaviest-page=page:t
    ?:  =(~ heaviest-block.c)
      pag  :: genesis block
    (to-page:local-page:t (~(got h-by blocks.c) (need heaviest-block.c)))
  =/  next-parent=block-id:t    ~(digest get:page:t heaviest-page)
  =/  next-height=page-number:t  ~(height get:page:t heaviest-page)
  |-
  ?:  =((~(get z-by heaviest-chain.d) next-height) `next-parent)
    ::  heaviest chain is accurate
    ::TODO check there aren't any blocks at page-numbers higher than
    ::the page-number of the heaviest block?
    d
  ::  heaviest chain is wrong, start revising
  =.  heaviest-chain.d
    (~(put z-by heaviest-chain.d) next-height next-parent)
  ?:  =(*page-number:t next-height)
    ::  genesis block was put into heaviest-chain, so we're done
    d
  %=  $
    next-height   (dec next-height)
    next-parent  ~(parent get:local-page:t (~(got h-by blocks.c) next-parent))
  ==
::
::  +populate-zk-asert-post-ai-anchor: when accepting the first block
::  at height >= ai-pow-activation-height, cache the regime-2 anchor
::  values from the parent block (at activation-height - 1). Idempotent:
::  if cache is already populated OR the new block is pre-activation,
::  returns d unchanged.
++  populate-zk-asert-post-ai-anchor
  |=  [c=consensus-state:dk pag=page:t]
  ^-  derived-state:dk
  ?^  cached-zk-asert-post-ai-anchor.d  d  ::  already cached, idempotent
  =/  block-height=@  ~(height get:page:t pag)
  ?:  (lth block-height ai-pow-activation-height.blockchain-constants)
    d  ::  pre-activation: nothing to cache yet
  =/  parent-bid=block-id:t  ~(parent get:page:t pag)
  =/  parent-min-ts-opt  (~(get h-by min-timestamps.c) parent-bid)
  =/  parent-target-opt  (~(get h-by targets.c) parent-bid)
  ?~  parent-min-ts-opt  d
  ?~  parent-target-opt  d
  =/  anchor=cached-asert-anchor:dk
    [min-ts=u.parent-min-ts-opt target-atom=(merge:bignum:t u.parent-target-opt)]
  d(cached-zk-asert-post-ai-anchor `anchor)
++  update-highest
  ~/  %update-highest
  |=  height=page-number:t
  =/  new-highest
    ?~  highest-block-height.d  height
    ?:  (gth height u.highest-block-height.d)
      height
    u.highest-block-height.d
  =.  highest-block-height.d  `new-highest
  d
::
::  Any genesis-seal that does not contain the realnet genesis message is considered fake
::  If the seal is not set, then we check the genesis block itself
::  If there is no genesis block, we return ~
++  is-mainnet
  ~/  %is-mainnet
  |=  c=consensus-state:dk
  ^-  (unit ?)
  ?~  genesis-seal.c
    ?^  genesis-id=(~(get z-by heaviest-chain.d) 0)
      =+  genesis=(~(get h-by blocks.c) u.genesis-id)
      ?~  genesis
        ~
      `=((hash:page-msg:t ~(msg get:local-page:t u.genesis)) realnet-genesis-msg:dk)
    ~
  `=(realnet-genesis-msg:dk msg-hash.u.genesis-seal.c)
--
