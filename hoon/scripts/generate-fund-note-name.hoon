/=  *  /common/tx-engine
/=  zo  /common/zoon
::  Computes the on-chain *first-name* of the protocol-fund coinbase notes
::  (014-aletheia). Each post-activation coinbase puts +fund-address into the
::  coinbase-split map; +make-name:coinbase then wraps that split key as a
::  single %pkh primitive plus the coinbase timelock and takes the nname:
::
::    note-lock      = ~[[%pkh m=1 {fund-address}] coinbase-tim-lp]
::    note-lock-root = (hash:lock note-lock)
::    first-name     = (first:nname note-lock-root)
::
::  The *first-name* (-.name) is identical for every fund coinbase note
::  (it does not depend on the parent block), and it is the value the v1
::  lock check funnels through (+lock-hash:nnote-1 -> +check:check-context).
::  So this single constant identifies every fund note for spend purposes.
::
::  Re-run after any change to the participant set, the threshold, the lock
::  structure, or coinbase-timelock-min:
::    cargo run --release --bin hoon -- \
::      open/hoon/scripts/generate-fund-note-name.hoon open/hoon
::
=/  pkhs=(list hash)
  :~  (from-b58:hash '7pGXggKU1AWk3d3wqX2kpKUatTqT68Cv8SQfGzGRQvJvYnQBvagSSjT')
      (from-b58:hash '8Mc1U7kdujhPoEwog1BfNsFDtRp8St8UQCHk84iaLdhP4cX9a2CT1MU')
      (from-b58:hash 'DAvp9ffoyNTBqAudZN29qc6s8GZfvvvAGvAfEFrqQsCVgSSSkg1SaSm')
      (from-b58:hash '9LK7wEcQsmRpEot4qFaV9bwjSE9ZD6tB1kWbgNsFkDa2LEpvBV9WGY3')
  ==
=/  participant-set=(z-set:zo hash)  (z-silt:zo pkhs)
::  3-of-4 multisig lock-root == the value pinned in +fund-address.
=/  fund-addr=hash  (hash:lock [%pkh [m=3 participant-set]]~)
::  Reconstruct the coinbase note lock exactly as +make-name:coinbase does.
::  fund-note-timelock-min is the canonical relative age committed by these notes.
=/  fund-pkh-set=(z-set:zo hash)  (z-silt:zo ~[fund-addr])
=/  pkh-prim=lock-primitive  [%pkh [m=1 fund-pkh-set]]
=/  tim-prim=lock-primitive  [%tim [rel=[min=`fund-note-timelock-min max=~] abs=[min=~ max=~]]]
=/  note-lock=lock  ~[pkh-prim tim-prim]
=/  note-lock-root=hash  (hash:lock note-lock)
=/  fund-note-firstname=hash  (first:nname note-lock-root)
~&  fund-address+(to-b58:hash fund-addr)
~&  note-lock-root+(to-b58:hash note-lock-root)
~&  fund-note-firstname+(to-b58:hash fund-note-firstname)
fund-note-firstname
