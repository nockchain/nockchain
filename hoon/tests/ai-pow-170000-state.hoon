::  Bounded accepted-state prerequisite for the node boundary smoke.
::  Compile with Honk in arbitrary mode; AI_POW_170000_STATE_JAM names this trap.
::  Retain a synthetic 169997 predecessor and the required storage metadata.
::  Import with PoW enabled and finish init before submitting current proofs.
::  A custom 169998 pin makes both honest lanes and a shorter/heavier fork
::  affordable. The 170000 AI target is production exact; ZK uses the new
::  100e9 hashes/s calibration and 500-second cadence with max-target=2^384,
::  NOT mainnet target difficulty. The historical 56e9 reference stays intact.
::  Both production height gates remain active.
/=  txe  /common/tx-engine
/=  fixtures  /tests/dumb/mod/unit/consensus
/=  *  /apps/dumbnet/lib/types
/=  *  /common/h-zoon
=/  bc=blockchain-constants:txe
  %*  .  *blockchain-constants:txe
    phase.zk-asert-post-ai                 169.998
    anchor-height.zk-asert-post-ai         169.997
    anchor-target-atom.zk-asert-post-ai    (bex 320)
    anchor-min-timestamp.zk-asert-post-ai  1
    phase.ai-asert                         169.998
    anchor-height.ai-asert                 169.997
    anchor-target-atom.ai-asert            (dec (bex 232))
    anchor-min-timestamp.ai-asert          1
    ai-pow-activation-height               169.998
    update-candidate-interval              ~s1
  ==
=/  f  ~(. fixtures bc)
=/  state=kernel-state  (accepted-load-state:f 169.997 %.y 123.456)
=/  t  ~(. txe constants.state)
=.  puzzle-asert-states.d.state  *(h-map block-id:t puzzle-asert-state)
=/  completed=kernel-state  state
::  Each retained page starts an epoch at its stored predecessor.
=.  epoch-start.c.completed
  %-  ~(run h-by blocks.c.completed)
  |=  local=local-page:t
  =/  pag=page:t  (to-page:local-page:t local)
  ?:  =(~(height get:page:t pag) 0)  ~(digest get:page:t pag)
  ~(parent get:page:t pag)
=.  targets.c.completed
  %-  ~(run h-by blocks.c.completed)
  |=  local=local-page:t
  ~(target get:page:t (to-page:local-page:t local))
::  This fixture has no transaction prehistory; every prerequisite balance is
::  explicitly empty, including the tip used by the real miner after import.
=.  balance.c.completed
  %-  ~(run h-by blocks.c.completed)
  |=  pag=local-page:t
  *(h-map nname:t nnote:t)
=.  init.a.completed  %.y
=.  btc-data.c.completed  ``*btc-hash:t
[%0 ~ completed]
