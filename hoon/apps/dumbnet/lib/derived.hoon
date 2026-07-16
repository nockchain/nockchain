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
  :: update view of heaviest chain
  =/  heaviest-page=page:t
    ?:  =(~ heaviest-block.c)
      pag  :: genesis block
    (to-page:local-page:t (~(got h-by blocks.c) (need heaviest-block.c)))
  =/  next-parent=block-id:t    ~(digest get:page:t heaviest-page)
  =/  next-height=page-number:t  ~(height get:page:t heaviest-page)
  ::  must precede the walk, which returns at the first height that agrees
  =.  heaviest-chain.d  (prune-above next-height)
  |-
  ?:  =((~(get z-by heaviest-chain.d) next-height) `next-parent)
    ::  heaviest chain is accurate
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
::  +prune-above: drop every heaviest-chain entry above .tip, restoring the
::  invariant that its keys are exactly 0..tip.
::
::    Heaviness is accumulated-work rather than height, so a reorg onto a
::    shorter heavier chain lowers the tip and leaves entries above it naming
::    the abandoned chain. +release-orphaned-branch reads absence above the tip
::    as proof a block is orphaned, and so depends on this.
::
::    Walks up from tip+1 until a height is absent: O(height dropped), and O(1)
::    when the tip only rises.
++  prune-above
  ~/  %prune-above
  |=  tip=page-number:t
  ^-  (z-map page-number:t block-id:t)
  =/  h=page-number:t  +(tip)
  =/  hc  heaviest-chain.d
  |-
  ^-  (z-map page-number:t block-id:t)
  ?~  (~(get z-by hc) h)  hc
  $(hc (~(del z-by hc) h), h +(h))
::
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
