/=  *  /common/tx-engine
/=  zo  /common/zoon
::  Computes the +fund-address constant for 014-aletheia: the lock-root
::  of a 3-of-4 multisig over the four pkhs in /asert-protocol-lock-fund.txt.
::  The result is a literal hash to be pasted into +fund-address in
::  /common/tx-engine-1.hoon, alongside a link back to this script.
::  Spending the protocol fund post-activation requires three of four
::  signatures from the participants below.
::
::  Re-run after any change to the participant set or threshold:
::    cargo run --profile release --bin hoon -- \
::      <path-to-this-script> <path-to-hoon-search-root>
::
::  The trace prints both the 5-tuple atom representation and the
::  base58 encoding of the resulting fund-address.
=/  pkhs=(list hash)
  :~  (from-b58:hash '7pGXggKU1AWk3d3wqX2kpKUatTqT68Cv8SQfGzGRQvJvYnQBvagSSjT')
      (from-b58:hash '8Mc1U7kdujhPoEwog1BfNsFDtRp8St8UQCHk84iaLdhP4cX9a2CT1MU')
      (from-b58:hash 'DAvp9ffoyNTBqAudZN29qc6s8GZfvvvAGvAfEFrqQsCVgSSSkg1SaSm')
      (from-b58:hash '9LK7wEcQsmRpEot4qFaV9bwjSE9ZD6tB1kWbgNsFkDa2LEpvBV9WGY3')
  ==
=/  participant-set=(z-set:zo hash)  (z-silt:zo pkhs)
::  3-of-4 multisig: a single spend-condition with one ++pkh primitive.
::  The +pkh primitive enforces m-of-n by itself, so we don't need to
::  enumerate the four 3-pkh combinations as separate spend-conditions.
=/  multi-lock=lock
  [%pkh [m=3 participant-set]]~
=/  fund-address=hash  (hash:lock multi-lock)
=/  fund-address-b58=@t  (to-b58:hash fund-address)
~&  fund-address+fund-address
~&  fund-address-b58+fund-address-b58
~&  fund-address-back-to-hash+(from-b58:hash fund-address-b58)
fund-address
