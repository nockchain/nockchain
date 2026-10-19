::  tests/dumb/bythos-gating.hoon
/=  tx-engine  /common/tx-engine
/=  helpers  /tests/dumb/helpers
/=  zo  /common/zoon
/=  hz  /common/h-zoon
/=  *  /common/test
|_  constants=blockchain-constants:tx-engine
+*  t  ~(. tx-engine constants)
    h  ~(. helpers constants)
::
++  test-bythos-lock-merkle-proof-gating
  ^-  tang
  =/  bc=blockchain-constants:tx-engine
    %*  .  constants
      bythos-phase  10
      coinbase-timelock-min  0
    ==
  =/  tb  ~(. tx-engine bc)
  =/  tim-prim=lock-primitive:tb  tim-lp:coinbase:tb
  =/  sc=spend-condition:tb  ~[tim-prim]
  =/  lock=lock:tb  sc
  =/  lock-root=hash:tb  (hash:lock:tb lock)
  =/  parent-first=hash:tb  (first:nname:tb lock-root)
  =/  lmp-stub=lock-merkle-proof:tb
    (build-lock-merkle-proof-stub:lock:tb lock 1)
  =/  lmp-full=lock-merkle-proof:tb
    (build-lock-merkle-proof-full:lock:tb lock 1)
  =/  witness-stub=witness:tb
    %*  .  *witness:tb
      lmp  lmp-stub
      pkh  *(z-map:zo hash:tb [schnorr-pubkey:tb schnorr-signature:tb])
      hax  *(z-map:zo hash:tb *)
      tim  ~
    ==
  =/  witness-full=witness:tb
    %*  .  *witness:tb
      lmp  lmp-full
      pkh  *(z-map:zo hash:tb [schnorr-pubkey:tb schnorr-signature:tb])
      hax  *(z-map:zo hash:tb *)
      tim  ~
    ==
  =/  ctx-pre-full=check-context:tb
    :*  9
        0
        *hash:tb
        witness-full
        bythos-phase.bc
        ~
    ==
  =/  ctx-pre-stub=check-context:tb
    :*  9
        0
        *hash:tb
        witness-stub
        bythos-phase.bc
        ~
    ==
  =/  ctx-post-full=check-context:tb
    :*  10
        0
        *hash:tb
        witness-full
        bythos-phase.bc
        ~
    ==
  =/  ctx-post-stub=check-context:tb
    :*  10
        0
        *hash:tb
        witness-stub
        bythos-phase.bc
        ~
    ==
  =/  res-pre-full=?  (check:check-context:tb [ctx-pre-full parent-first])
  =/  res-pre-stub=?  (check:check-context:tb [ctx-pre-stub parent-first])
  =/  res-post-full=?  (check:check-context:tb [ctx-post-full parent-first])
  =/  res-post-stub=?  (check:check-context:tb [ctx-post-stub parent-first])
  %+  expect-eq
    !>  [%.n %.y %.y %.y]
  !>  [res-pre-full res-pre-stub res-post-full res-post-stub]
::
++  test-bythos-fee-gating
  ^-  tang
  =/  bc=blockchain-constants:tx-engine
    %*  .  constants
      bythos-phase  10
      base-fee  256
      input-fee-divisor  4
      coinbase-timelock-min  0
    ==
  =/  tb  ~(. tx-engine bc)
  =/  hb  ~(. helpers bc)
  =/  note-a=nnote:tb  (make-simple-note:v1:hb p:default-keys-1:hb 1.000.000)
  =/  note-b=nnote:tb  (make-simple-note:v1:hb p:default-keys-2:hb 1.000.000)
  ?>  ?=(@ -.note-a)
  ?>  ?=(@ -.note-b)
  =/  spend-a=spend-v1:tb
    %:  simple-from-note:spend-v1:tb
      p:default-keys-1:hb
      p:default-keys-3:hb
      note-a
    ==
  =/  spend-b=spend-v1:tb
    %:  simple-from-note:spend-v1:tb
      p:default-keys-2:hb
      p:default-keys-3:hb
      note-b
    ==
  =/  sps=spends:tb
    (~(put z-by:zo *spends:tb) ~(name get:nnote:tb note-a) spend-a)
  =.  sps  (~(put z-by:zo sps) ~(name get:nnote:tb note-b) spend-b)
  =/  pre-seed-words=@  (count-seed-words:spends:tb [sps 9])
  =/  post-seed-words=@  (count-seed-words:spends:tb [sps 10])
  =/  pre-witness-words=@  (count-witness-words:spends:tb [sps 9])
  =/  post-witness-words=@  (count-witness-words:spends:tb [sps 10])
  =/  pre-fee=coins:tb  (calculate-min-fee:spends:tb [sps 9])
  =/  post-fee=coins:tb  (calculate-min-fee:spends:tb [sps 10])
  =/  pre-base-fee=coins:tb  (mul 2 base-fee.bc)
  =/  expected-pre=coins:tb
    (max (mul (add pre-seed-words pre-witness-words) pre-base-fee) min-fee.data.bc)
  =/  expected-post=coins:tb
    (max (add (mul post-seed-words base-fee.bc) (div (mul post-witness-words base-fee.bc) input-fee-divisor.bc)) min-fee.data.bc)
  ;:  weld
    %+  expect-eq
      !>  %.y
    !>  (gth pre-seed-words post-seed-words)
  ::
    %+  expect-eq
      !>  pre-witness-words
    !>  post-witness-words
  ::
    %+  expect-eq
      !>  %.y
    !>  (gth pre-fee post-fee)
  ::
    %+  expect-eq
      !>  [expected-pre expected-post]
    !>  [pre-fee post-fee]
  ==
::
::  REGRESSION TESTS for bythos gating bug
::
::  These tests verify that validate-with-context properly enforces bythos
::  gating. The bug was that validate-with-context in tx-engine-1.hoon calls
::  check:check-context directly from tx-engine-1, bypassing the wrapper in
::  tx-engine.hoon that does bythos gating.
::
::  A transaction with a full LMP ([%full * * *]) should be REJECTED when
::  processed at a block height before bythos-phase.
::
++  test-bythos-validate-with-context-full-lmp-rejected-pre-bythos
  ::  Test that validate-with-context rejects full LMP before bythos-phase
  ::
  ::  This test creates a v1 note, builds a spend with a FULL lock-merkle-proof,
  ::  and calls validate-with-context at a height BEFORE bythos-phase.
  ::  This should FAIL (return %.n) because full LMPs are not allowed pre-bythos.
  ::
  ^-  tang
  =/  bc=blockchain-constants:tx-engine
    %*  .  constants
      bythos-phase  100       ::  bythos activates at height 100
      v1-phase  1             ::  v1 activates at height 1
      coinbase-timelock-min  0
      base-fee  0
      input-fee-divisor  1
      data  [2.048 0]
    ==
  =/  tb  ~(. tx-engine bc)
  =/  hb  ~(. helpers bc)
  ::  create a v1 note
  =/  note=nnote:tb  (make-simple-note:v1:hb p:default-keys-1:hb 1.000.000)
  ?>  ?=(@ -.note)
  ::  build spend-1 manually with FULL lock-merkle-proof
  =/  parent-lock=lock:tb  (lock-from-sig:tb p:default-keys-1:hb)
  =/  recipient-lock=lock:tb  (lock-from-sig:tb p:default-keys-2:hb)
  ::  FORCE a full LMP (this is the key: normally the code would use stub pre-bythos)
  =/  parent-lmp=lock-merkle-proof:tb
    (build-lock-merkle-proof-full:lock:tb parent-lock 1)
  ::  verify we actually have a full LMP
  ?>  ?=([%full *] parent-lmp)
  ::  build seeds for recipient lock
  =/  lock-root=hash:tb  (hash:lock:tb recipient-lock)
  =/  sed=seed:v1:tb
    (simple:seed-v1:tb lock-root assets.note (hash:nnote:tb note))
  =/  seeds=(z-set:zo seed:v1:tb)  (~(put z-in:zo *(z-set:zo seed:v1:tb)) sed)
  =|  wit=witness:tb
  =/  sp=spend-1:v1:tb
    %*  .  *spend-1:v1:tb
      witness  wit(lmp parent-lmp)
      seeds    seeds
      fee      0
    ==
  ::  sign the spend using tx-engine helper
  =.  sp  (sign:spend-1:v1:tb sp s:default-keys-1:hb)
  =/  spend=spend:v1:tb  [%1 sp]
  =/  sps=spends:tb
    (~(put z-by:zo *spends:tb) ~(name get:nnote:tb note) spend)
  ::  create a balance with the note
  =/  balance=(h-map:hz nname:tb nnote:tb)
    (~(put h-by:hz *(h-map:hz nname:tb nnote:tb)) name.note note)
  ::  call validate-with-context at height 50 (BEFORE bythos-phase of 100)
  ::  This should REJECT the transaction because full LMP is used pre-bythos
  =/  result=(reason:tb ~)
    %-  validate-with-context:spends:tb
    [balance sps 50 max-size.data.bc bythos-phase.bc]
  ::  Expected: %.n (rejected) because full LMP before bythos-phase
  %+  expect-eq
    !>  %.n
  !>  ?=(%.y -.result)
::
++  test-bythos-validate-with-context-stub-lmp-accepted-pre-bythos
  ::  Test that validate-with-context accepts stub LMP before bythos-phase
  ::
  ::  This is the CONTROL test: stub LMPs should always be accepted.
  ::  This test should pass both before and after the fix.
  ::
  ^-  tang
  =/  bc=blockchain-constants:tx-engine
    %*  .  constants
      bythos-phase  100
      v1-phase  1
      coinbase-timelock-min  0
      base-fee  0
      input-fee-divisor  1
      data  [2.048 0]
    ==
  =/  tb  ~(. tx-engine bc)
  =/  hb  ~(. helpers bc)
  ::  create a v1 note
  =/  note=nnote:tb  (make-simple-note:v1:hb p:default-keys-1:hb 1.000.000)
  ?>  ?=(@ -.note)
  ::  build spend-1 with STUB lock-merkle-proof (the normal pre-bythos case)
  =/  parent-lock=lock:tb  (lock-from-sig:tb p:default-keys-1:hb)
  =/  recipient-lock=lock:tb  (lock-from-sig:tb p:default-keys-2:hb)
  =/  parent-lmp=lock-merkle-proof:tb
    (build-lock-merkle-proof-stub:lock:tb parent-lock 1)
  ::  verify we have a stub LMP (not full - full starts with %full tag)
  ?>  ?!(?=([%full *] parent-lmp))
  ::  build seeds for recipient lock
  =/  lock-root=hash:tb  (hash:lock:tb recipient-lock)
  =/  sed=seed:v1:tb
    (simple:seed-v1:tb lock-root assets.note (hash:nnote:tb note))
  =/  seeds=(z-set:zo seed:v1:tb)  (~(put z-in:zo *(z-set:zo seed:v1:tb)) sed)
  =|  wit=witness:tb
  =/  sp=spend-1:v1:tb
    %*  .  *spend-1:v1:tb
      witness  wit(lmp parent-lmp)
      seeds    seeds
      fee      0
    ==
  ::  sign the spend using tx-engine helper
  =.  sp  (sign:spend-1:v1:tb sp s:default-keys-1:hb)
  =/  spend=spend:v1:tb  [%1 sp]
  =/  sps=spends:tb
    (~(put z-by:zo *spends:tb) ~(name get:nnote:tb note) spend)
  ::  create a balance with the note
  =/  balance=(h-map:hz nname:tb nnote:tb)
    (~(put h-by:hz *(h-map:hz nname:tb nnote:tb)) name.note note)
  ::  call validate-with-context at height 50 (before bythos-phase)
  ::  This SHOULD be accepted because stub LMP is always allowed
  =/  result=(reason:tb ~)
    %-  validate-with-context:spends:tb
    [balance sps 50 max-size.data.bc bythos-phase.bc]
  ::  Expected: %.y (accepted) because stub LMP is always valid
  %+  expect-eq
    !>  %.y
  !>  ?=(%.y -.result)
::
++  test-bythos-validate-with-context-full-lmp-accepted-post-bythos
  ::  Test that validate-with-context accepts full LMP at/after bythos-phase
  ::
  ::  This is the CONTROL test: full LMPs should be accepted post-bythos.
  ::  This test should pass both before and after the fix.
  ::
  ^-  tang
  =/  bc=blockchain-constants:tx-engine
    %*  .  constants
      bythos-phase  100
      v1-phase  1
      coinbase-timelock-min  0
      base-fee  0
      input-fee-divisor  1
      data  [2.048 0]
    ==
  =/  tb  ~(. tx-engine bc)
  =/  hb  ~(. helpers bc)
  ::  create a v1 note
  =/  note=nnote:tb  (make-simple-note:v1:hb p:default-keys-1:hb 1.000.000)
  ?>  ?=(@ -.note)
  ::  build spend-1 with FULL lock-merkle-proof
  =/  parent-lock=lock:tb  (lock-from-sig:tb p:default-keys-1:hb)
  =/  recipient-lock=lock:tb  (lock-from-sig:tb p:default-keys-2:hb)
  =/  parent-lmp=lock-merkle-proof:tb
    (build-lock-merkle-proof-full:lock:tb parent-lock 1)
  ?>  ?=([%full *] parent-lmp)
  ::  build seeds for recipient lock
  =/  lock-root=hash:tb  (hash:lock:tb recipient-lock)
  =/  sed=seed:v1:tb
    (simple:seed-v1:tb lock-root assets.note (hash:nnote:tb note))
  =/  seeds=(z-set:zo seed:v1:tb)  (~(put z-in:zo *(z-set:zo seed:v1:tb)) sed)
  =|  wit=witness:tb
  =/  sp=spend-1:v1:tb
    %*  .  *spend-1:v1:tb
      witness  wit(lmp parent-lmp)
      seeds    seeds
      fee      0
    ==
  ::  sign the spend using tx-engine helper
  =.  sp  (sign:spend-1:v1:tb sp s:default-keys-1:hb)
  =/  spend=spend:v1:tb  [%1 sp]
  =/  sps=spends:tb
    (~(put z-by:zo *spends:tb) ~(name get:nnote:tb note) spend)
  ::  create a balance with the note
  =/  balance=(h-map:hz nname:tb nnote:tb)
    (~(put h-by:hz *(h-map:hz nname:tb nnote:tb)) name.note note)
  ::  call validate-with-context at height 100 (AT bythos-phase)
  ::  This SHOULD be accepted because full LMP is allowed at/after bythos
  =/  result=(reason:tb ~)
    %-  validate-with-context:spends:tb
    [balance sps 100 max-size.data.bc bythos-phase.bc]
  ::  Expected: %.y (accepted) because height >= bythos-phase
  %+  expect-eq
    !>  %.y
  !>  ?=(%.y -.result)
::
++  test-bythos-tx-acc-process-full-lmp-rejected-pre-bythos
  ::  Test that tx-acc:process rejects full LMP transactions before bythos-phase
  ::
  ::  This tests the consensus processing path (tx-acc:process -> v1-to-v1 ->
  ::  validate-with-context). This is one of the three usage sites mentioned
  ::  in the code review.
  ::
  ^-  tang
  =/  bc=blockchain-constants:tx-engine
    %*  .  constants
      bythos-phase  100
      v1-phase  1
      coinbase-timelock-min  0
      base-fee  0
      input-fee-divisor  1
      data  [2.048 0]
    ==
  =/  tb  ~(. tx-engine bc)
  =/  hb  ~(. helpers bc)
  ::  create a v1 note
  =/  note=nnote:tb  (make-simple-note:v1:hb p:default-keys-1:hb 1.000.000)
  ?>  ?=(@ -.note)
  ::  build spend-1 with FULL lock-merkle-proof
  =/  parent-lock=lock:tb  (lock-from-sig:tb p:default-keys-1:hb)
  =/  recipient-lock=lock:tb  (lock-from-sig:tb p:default-keys-2:hb)
  =/  parent-lmp=lock-merkle-proof:tb
    (build-lock-merkle-proof-full:lock:tb parent-lock 1)
  ?>  ?=([%full *] parent-lmp)
  =/  lock-root=hash:tb  (hash:lock:tb recipient-lock)
  =/  sed=seed:v1:tb
    (simple:seed-v1:tb lock-root assets.note (hash:nnote:tb note))
  =/  seeds=(z-set:zo seed:v1:tb)  (~(put z-in:zo *(z-set:zo seed:v1:tb)) sed)
  =|  wit=witness:tb
  =/  sp=spend-1:v1:tb
    %*  .  *spend-1:v1:tb
      witness  wit(lmp parent-lmp)
      seeds    seeds
      fee      0
    ==
  ::  sign the spend using tx-engine helper
  =.  sp  (sign:spend-1:v1:tb sp s:default-keys-1:hb)
  =/  spend=spend:v1:tb  [%1 sp]
  =/  sps=spends:tb
    (~(put z-by:zo *spends:tb) ~(name get:nnote:tb note) spend)
  ::  create raw-tx
  =/  raw=raw-tx:v1:tb  (new:raw-tx:v1:tb sps)
  ::  create tx-acc with balance containing the note, at height 50 (pre-bythos)
  =/  balance=(h-map:hz nname:tb nnote:tb)
    (~(put h-by:hz *(h-map:hz nname:tb nnote:tb)) name.note note)
  =/  tac=tx-acc:tb  (new:tx-acc:tb `balance 50)
  ::  process the transaction - should be REJECTED pre-bythos
  =/  result=(reason:tb tx-acc:tb)
    (process:tx-acc:tb tac raw)
  ::  Expected: %.n (rejected) because full LMP before bythos-phase
  ::  BUG: Currently returns %.y because validate-with-context lacks bythos gating
  %+  expect-eq
    !>  %.n
  !>  ?=(%.y -.result)
--
