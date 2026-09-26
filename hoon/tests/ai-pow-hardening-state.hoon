::  Native boundary/restart prerequisite, NOT a replayed or validated history.
::  Compile with hoonc; AI_POW_HARDENING_STATE_JAM names the compiled trap.
::  Evaluating the trap returns raw outer state, imported by the CURRENT kernel.
::
::  The sparse ancestry is genesis -> Zoe 119400 -> 154498. These three pages
::  have synthetic proof envelopes and empty balances. They are never submitted
::  as new blocks. All later pages are proved and admitted by the Rust test.
::  In particular, NO synthetic page at/after 154499 bypasses the live boundary.
::
::  A custom-network pin at 154499 makes a small Legacy AI proof affordable.
::  max-target-atom=2^384 makes real ZK proofs affordable through the immutable
::  154500 reset. Neither changes the fixed AI target or harden-phase=154500.
::  PoW remains enabled, including during every subsequent current-kernel load.
/=  txe  /common/tx-engine
/=  *  /apps/dumbnet/lib/types
/=  *  /common/h-zoon
=/  bc=blockchain-constants:txe
  %*  .  *blockchain-constants:txe
    check-pow-flag                         %.y
    max-target-atom                        (bex 384)
    v1-phase                               1
    phase.zk-asert                         1
    anchor-height.zk-asert                 0
    anchor-target-atom.zk-asert            (bex 384)
    anchor-min-timestamp.zk-asert          1
    phase.zk-asert-post-ai                 154.499
    anchor-height.zk-asert-post-ai         154.498
    anchor-target-atom.zk-asert-post-ai    (bex 384)
    anchor-min-timestamp.zk-asert-post-ai  1
    phase.ai-asert                         154.499
    anchor-height.ai-asert                 154.498
    anchor-target-atom.ai-asert            (dec (bex 232))
    anchor-min-timestamp.ai-asert          1
    ai-pow-activation-height               154.499
    update-candidate-interval              ~s1
  ==
=/  t  ~(. txe bc)
=/  initial=kernel-state  *kernel-state
=.  constants.initial  bc
=.  init.a.initial  %.n
=.  mining.m.initial  %.n
=.  genesis-seal.c.initial  `[0 *hash:t]
=.  btc-data.c.initial  ``*btc-hash:t
=/  built=[state=kernel-state parent=block-id:t]
  %+  roll  `(list @ud)`~[0 119.400 154.498]
  |=  [height=@ud state=_initial parent=block-id:t]
  =/  base=proof:t  *proof:t
  =/  prf=proof:t
    ?:  =(height 0)  base
    ?:  =(height 119.400)  [%3 objects.base hashes.base read-index.base]
    [%5 objects.base hashes.base read-index.base]
  =/  pag=page:v1:t
    %*  .  *page:v1:t
      height     height
      parent     parent
      timestamp  1
      pow        `prf
    ==
  =.  pag  pag(digest (compute-digest:page:t pag))
  =/  bid=block-id:t  digest.pag
  =.  blocks.c.state
    (~(put h-by blocks.c.state) bid (to-local-page:page:t pag))
  =.  balance.c.state
    (~(put h-by balance.c.state) bid *(h-map nname:t nnote:t))
  =.  min-timestamps.c.state  (~(put h-by min-timestamps.c.state) bid 1)
  =.  epoch-start.c.state  (~(put h-by epoch-start.c.state) bid bid)
  =.  targets.c.state  (~(put h-by targets.c.state) bid target.pag)
  =.  block-versions.c.state  (~(put h-by block-versions.c.state) bid version.prf)
  =.  heaviest-block.c.state  `bid
  =.  heaviest-chain.d.state  (~(put z-by heaviest-chain.d.state) height bid)
  =.  highest-block-height.d.state  `height
  [state bid]
[%0 ~ state.built]
