/=  *  /common/test
/=  zo  /common/zoon
/=  base-lib  /apps/bridge/base
/=  bridge-ker  /apps/bridge/bridge
/=  nock-lib  /apps/bridge/nock
/=  dhel  /tests/dumb/helpers
/=  hel  /tests/bridge/helpers
/=  *  /apps/bridge/types
/=  wt  /apps/wallet/lib/types
/=  txb-lib  /apps/wallet/lib/tx-builder
/=  wutils  /apps/wallet/lib/utils
|%
++  make-withdrawal
  |=  [event-id=beid recipient=nock-lock-root amount=coins]
  ^-  withdrawal
  :*  event-id
      recipient
      amount
  ==
::
++  make-withdrawal-proposal
  |=  $:  as-of=base-hash
          event-id=beid
          recipient=nock-lock-root
          amount=coins
          amount-burned=coins
          base-batch-end=@
      ==
  ^-  withdrawal-proposal
  :*  [as-of (to-atom:blist event-id)]
      recipient
      amount
      amount-burned
      base-batch-end
      0
      [1 *block-id:t]
      ~[*nname:t]
      *transaction:wt
  ==
::
++  make-burn-event
  |=  [event-id=beid burner=base-addr amount=coins lock-root=nock-lock-root]
  ^-  base-event
  :*  (to-atom:blist event-id)
      [%burn-for-withdrawal burner amount lock-root]
  ==
::
++  setup-unsettled-withdrawal
  |=  [event-id=beid recipient=nock-lock-root amount=coins]
  ^-  [state=bridge-state as-of=base-hash wd=withdrawal]
  =/  state=bridge-state  *bridge-state
  =/  constants=bridge-constants  (small-constants:hel 1 10 0)
  =.  constants.state  constants
  =.  base-hashchain-next-height.hash-state.state  base-start-height.constants
  =/  wd=withdrawal  (make-withdrawal event-id recipient amount)
  =|  withdrawals=(z-map beid withdrawal)
  =.  withdrawals
    (~(put z-by withdrawals) event-id wd)
  =/  blocks=base-blocks
    (make-base-blocks:hel state withdrawals *(z-map beid deposit-settlement))
  =/  as-of=base-hash  (hash:base-blocks blocks)
  =.  base-hashchain.hash-state.state
    (~(put z-by base-hashchain.hash-state.state) as-of blocks)
  =.  last-base-blocks.hash-state.state  as-of
  =.  unsettled-withdrawals.hash-state.state
    (~(put z-bi unsettled-withdrawals.hash-state.state) as-of event-id wd)
  [state as-of wd]
::
++  make-withdrawal-settlement
  |=  $:  event-id=beid
          as-of=base-hash
          recipient=nock-lock-root
          amount=coins
          base-batch-end=@
      ==
  ^-  withdrawal-settlement
  :*  *tx-id:t
      *nname:t
      event-id
      base-batch-end
      as-of
      recipient
      amount
  ==
::
++  has-stop-effect
  |=  effects=(list effect)
  ^-  ?
  ?~  effects  %.n
  ?=([%0 %stop * *] i.effects)
::
++  test-burn-for-withdrawal-enters-state-and-emits-request
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =/  constants=bridge-constants  (small-constants:hel 1 10 0)
  =.  constants.state  constants
  =/  event-id=beid  (from-atom:blist 4)
  =/  recipient=nock-lock-root  *hash:t
  =/  amount=coins  1.000.000
  =/  burn-event=base-event
    (make-burn-event event-id 0x1111 amount recipient)
  =/  raw=raw-base-blocks:cause
    :~  [10 0x1 0x0 ~[burn-event]]
    ==
  =/  base  ~(. base-lib state)
  =/  cooked=base-blocks  (cook-base-blocks:base raw)
  =/  as-of=base-hash  (hash:base-blocks cooked)
  =/  expected-withdrawal=withdrawal
    (make-withdrawal event-id recipient amount)
  =/  expected-withdrawals=(list [beid withdrawal])
    ~[[event-id expected-withdrawal]]
  =/  cooked-withdrawals=(list [beid withdrawal])
    ~(tap z-by withdrawals.cooked)
  =/  expected-requests=(list nock-withdrawal-request:effect)
    ~[[(to-atom:blist event-id) recipient amount 10 as-of]]
  =/  requests=(list nock-withdrawal-request:effect)
    (base-propose-withdrawals:base cooked)
  ;:  weld
    %+  expect-eq
      !>(expected-withdrawals)
    !>(cooked-withdrawals)
  ::
    %+  expect-eq
      !>(expected-requests)
    !>(requests)
  ==
::
++  test-base-block-withdrawal-batch-stages-pending-before-ack
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =/  constants=bridge-constants  (small-constants:hel 1 10 0)
  =.  constants.state  constants
  =.  base-hashchain-next-height.hash-state.state  base-start-height.constants
  =/  event-id=beid  (from-atom:blist 15)
  =/  recipient=nock-lock-root  *hash:t
  =/  amount=coins  1.800.000
  =/  burn-event=base-event
    (make-burn-event event-id 0x1111 amount recipient)
  =/  raw=raw-base-blocks:cause
    :~  [10 0x10 0x0 ~[burn-event]]
    ==
  =/  base  ~(. base-lib state)
  =/  [effects=(list effect) staged=bridge-state]
    (incoming-base-blocks:base [raw [~ 0 0x0 *@da]])
  ?~  effects
    ~|('expected base-block-withdrawals-pending effect' !!)
  ?>  ?=([%0 %base-block-withdrawals-pending *] i.effects)
  =/  pending=pending-base-block-withdrawals  pending.i.effects
  =/  expected-requests=(list nock-withdrawal-request:effect)
    ~[[(to-atom:blist event-id) recipient amount 10 blocks-hash.pending]]
  ;:  weld
    %+  expect-eq
      !>(10)
    !>(base-hashchain-next-height.hash-state.staged)
  ::
    (expect !>(?=(^ pending-base-block-commit.hash-state.staged)))
  ::
    %+  expect-eq
      !>(expected-requests)
    !>(withdrawals.pending)
  ::
    %+  expect-eq
      !>(%.n)
    !>((~(has z-by base-hashchain.hash-state.staged) blocks-hash.pending))
  ::
    %+  expect-eq
      !>(%.n)
    !>((~(has z-bi unsettled-withdrawals.hash-state.staged) blocks-hash.pending event-id))
  ==
::
++  test-base-block-withdrawal-ack-commits-and-clears-pending
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =/  constants=bridge-constants  (small-constants:hel 1 10 0)
  =.  constants.state  constants
  =.  base-hashchain-next-height.hash-state.state  base-start-height.constants
  =/  event-id=beid  (from-atom:blist 16)
  =/  recipient=nock-lock-root  *hash:t
  =/  amount=coins  1.900.000
  =/  burn-event=base-event
    (make-burn-event event-id 0x1111 amount recipient)
  =/  raw=raw-base-blocks:cause
    :~  [10 0x11 0x0 ~[burn-event]]
    ==
  =/  base  ~(. base-lib state)
  =/  [effects=(list effect) staged=bridge-state]
    (incoming-base-blocks:base [raw [~ 0 0x0 *@da]])
  ?~  effects
    ~|('expected base-block-withdrawals-pending effect' !!)
  ?>  ?=([%0 %base-block-withdrawals-pending *] i.effects)
  =/  pending=pending-base-block-withdrawals  pending.i.effects
  =/  ack=base-block-commit-ack
    [blocks-hash.pending first-height.pending last-height.pending]
  =/  base-staged  ~(. base-lib staged)
  =/  [ack-effects=(list effect) committed=bridge-state]
    (commit-base-block-withdrawals:base-staged ack)
  =/  base-committed  ~(. base-lib committed)
  =/  [duplicate-effects=(list effect) after-duplicate=bridge-state]
    (commit-base-block-withdrawals:base-committed ack)
  ;:  weld
    %+  expect-eq
      !>(~)
    !>(ack-effects)
  ::
    %+  expect-eq
      !>(11)
    !>(base-hashchain-next-height.hash-state.committed)
  ::
    (expect !>(?=(~ pending-base-block-commit.hash-state.committed)))
  ::
    (expect !>((~(has z-by base-hashchain.hash-state.committed) blocks-hash.pending)))
  ::
    (expect !>((~(has z-bi unsettled-withdrawals.hash-state.committed) blocks-hash.pending event-id)))
  ::
    %+  expect-eq
      !>(~)
    !>(duplicate-effects)
  ::
    %+  expect-eq
      !>(11)
    !>(base-hashchain-next-height.hash-state.after-duplicate)
  ::
    (expect !>(?=(~ pending-base-block-commit.hash-state.after-duplicate)))
  ==
::
++  test-empty-base-block-batch-still-requires-ack
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =/  constants=bridge-constants  (small-constants:hel 1 10 0)
  =.  constants.state  constants
  =.  base-hashchain-next-height.hash-state.state  base-start-height.constants
  =/  raw=raw-base-blocks:cause
    :~  [10 0x12 0x0 ~]
    ==
  =/  base  ~(. base-lib state)
  =/  [effects=(list effect) staged=bridge-state]
    (incoming-base-blocks:base [raw [~ 0 0x0 *@da]])
  ?~  effects
    ~|('expected base-block-withdrawals-pending effect' !!)
  ?>  ?=([%0 %base-block-withdrawals-pending *] i.effects)
  =/  pending=pending-base-block-withdrawals  pending.i.effects
  ?>  ?=(~ withdrawals.pending)
  =/  ack=base-block-commit-ack
    [blocks-hash.pending first-height.pending last-height.pending]
  =/  base-staged  ~(. base-lib staged)
  =/  [ack-effects=(list effect) committed=bridge-state]
    (commit-base-block-withdrawals:base-staged ack)
  ;:  weld
    %+  expect-eq
      !>(10)
    !>(base-hashchain-next-height.hash-state.staged)
  ::
    %+  expect-eq
      !>(~)
    !>(ack-effects)
  ::
    %+  expect-eq
      !>(11)
    !>(base-hashchain-next-height.hash-state.committed)
  ::
    (expect !>(?=(~ pending-base-block-commit.hash-state.committed)))
  ::
    (expect !>((~(has z-by base-hashchain.hash-state.committed) blocks-hash.pending)))
  ==
::
++  test-base-block-ack-mismatch-stops
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =/  constants=bridge-constants  (small-constants:hel 1 10 0)
  =.  constants.state  constants
  =.  base-hashchain-next-height.hash-state.state  base-start-height.constants
  =/  raw=raw-base-blocks:cause
    :~  [10 0x13 0x0 ~]
    ==
  =/  base  ~(. base-lib state)
  =/  [effects=(list effect) staged=bridge-state]
    (incoming-base-blocks:base [raw [~ 0 0x0 *@da]])
  ?~  effects
    ~|('expected base-block-withdrawals-pending effect' !!)
  ?>  ?=([%0 %base-block-withdrawals-pending *] i.effects)
  =/  pending=pending-base-block-withdrawals  pending.i.effects
  =/  base-staged  ~(. base-lib staged)
  =/  bad-hash-ack=base-block-commit-ack
    [[0x99 0x98 0x97 0x96 0x95] first-height.pending last-height.pending]
  =/  [bad-hash-effects=(list effect) bad-hash-state=bridge-state]
    (commit-base-block-withdrawals:base-staged bad-hash-ack)
  =/  bad-first-ack=base-block-commit-ack
    [blocks-hash.pending +(first-height.pending) last-height.pending]
  =/  [bad-first-effects=(list effect) bad-first-state=bridge-state]
    (commit-base-block-withdrawals:base-staged bad-first-ack)
  =/  bad-last-ack=base-block-commit-ack
    [blocks-hash.pending first-height.pending +(last-height.pending)]
  =/  [bad-last-effects=(list effect) bad-last-state=bridge-state]
    (commit-base-block-withdrawals:base-staged bad-last-ack)
  ;:  weld
    (expect !>((has-stop-effect bad-hash-effects)))
  ::
    (expect !>((has-stop-effect bad-first-effects)))
  ::
    (expect !>((has-stop-effect bad-last-effects)))
  ==
::
++  test-second-base-block-batch-while-pending-stops
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =/  constants=bridge-constants  (small-constants:hel 1 10 0)
  =.  constants.state  constants
  =.  base-hashchain-next-height.hash-state.state  base-start-height.constants
  =/  raw=raw-base-blocks:cause
    :~  [10 0x14 0x0 ~]
    ==
  =/  base  ~(. base-lib state)
  =/  [effects=(list effect) staged=bridge-state]
    (incoming-base-blocks:base [raw [~ 0 0x0 *@da]])
  ?~  effects
    ~|('expected base-block-withdrawals-pending effect' !!)
  =/  base-staged  ~(. base-lib staged)
  =/  [second-effects=(list effect) second-state=bridge-state]
    (incoming-base-blocks:base-staged [raw [~ 0 0x0 *@da]])
  (expect !>((has-stop-effect second-effects)))
::
++  test-withdrawal-settlement-clears-state
  ^-  tang
  =/  event-id=beid  (from-atom:blist 5)
  =/  recipient=nock-lock-root  *hash:t
  =/  amount=coins  900.000
  =/  [state=bridge-state as-of=base-hash wd=withdrawal]
    (setup-unsettled-withdrawal event-id recipient amount)
  =/  settlement=withdrawal-settlement
    (make-withdrawal-settlement event-id as-of recipient 850.000 10)
  =/  settlements=(z-map nname:t withdrawal-settlement)
    (~(put z-by *(z-map nname:t withdrawal-settlement)) nname.settlement settlement)
  =/  latest=nock-block
    (produce-nock-block:hel state *(z-map nname:t deposit) settlements)
  =/  nock  ~(. nock-lib state)
  =/  result=process-result
    (nockchain-process-withdrawal-settlements:nock latest)
  ?>  ?=(%& -.result)
  =/  new-state=bridge-state  +.result
  =/  has-unsettled=?
    (~(has z-bi unsettled-withdrawals.hash-state.new-state) as-of event-id)
  (expect !>(!has-unsettled))
::
++  test-withdrawal-settlement-unknown-as-of-is-deferred
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =.  base-hashchain-next-height.hash-state.state  123
  =/  recipient=nock-lock-root  *hash:t
  =/  as-of=base-hash  [0x7 0x7 0x7 0x7 0x7]
  =/  settlement=withdrawal-settlement
    (make-withdrawal-settlement (from-atom:blist 6) as-of recipient 700.000 123)
  =/  settlements=(z-map nname:t withdrawal-settlement)
    (~(put z-by *(z-map nname:t withdrawal-settlement)) nname.settlement settlement)
  =/  latest=nock-block
    (produce-nock-block:hel state *(z-map nname:t deposit) settlements)
  =/  nock  ~(. nock-lib state)
  =/  result=process-result
    (nockchain-process-withdrawal-settlements:nock latest)
  ?>  ?=(%& -.result)
  =/  new-state=bridge-state  p.result
  ;:  weld
    (expect !>((~(has z-bi deferred-withdrawal-settlements.hash-state.new-state) as-of nname.settlement)))
  ::
    (expect !>(?=(~ nock-hold.hash-state.new-state)))
  ==
::
++  test-withdrawal-settlement-mismatch-stops
  ^-  tang
  =/  event-id=beid  (from-atom:blist 7)
  =/  recipient=nock-lock-root  *hash:t
  =/  amount=coins  1.100.000
  =/  [state=bridge-state as-of=base-hash wd=withdrawal]
    (setup-unsettled-withdrawal event-id recipient amount)
  =/  settlement=withdrawal-settlement
    (make-withdrawal-settlement event-id as-of recipient +(amount) 10)
  =/  settlements=(z-map nname:t withdrawal-settlement)
    (~(put z-by *(z-map nname:t withdrawal-settlement)) nname.settlement settlement)
  =/  latest=nock-block
    (produce-nock-block:hel state *(z-map nname:t deposit) settlements)
  =/  nock  ~(. nock-lib state)
  =/  result=process-result
    (nockchain-process-withdrawal-settlements:nock latest)
  ?>  ?=(%| -.result)
  =/  fail=process-fail  +.result
  ?>  ?=(%stop -.fail)
  ;:  weld
    %+  expect-eq
      !>('failed to process withdrawal settlement: counterpart does not match settlement')
    !>(msg.fail)
  ==
::
++  test-withdrawal-settlement-zero-amount-stops
  ^-  tang
  =/  event-id=beid  (from-atom:blist 8)
  =/  recipient=nock-lock-root  *hash:t
  =/  amount=coins  1.200.000
  =/  [state=bridge-state as-of=base-hash wd=withdrawal]
    (setup-unsettled-withdrawal event-id recipient amount)
  =/  settlement=withdrawal-settlement
    (make-withdrawal-settlement event-id as-of recipient 0 10)
  =/  settlements=(z-map nname:t withdrawal-settlement)
    (~(put z-by *(z-map nname:t withdrawal-settlement)) nname.settlement settlement)
  =/  latest=nock-block
    (produce-nock-block:hel state *(z-map nname:t deposit) settlements)
  =/  nock  ~(. nock-lib state)
  =/  result=process-result
    (nockchain-process-withdrawal-settlements:nock latest)
  ?>  ?=(%| -.result)
  =/  fail=process-fail  +.result
  ?>  ?=(%stop -.fail)
  ;:  weld
    %+  expect-eq
      !>('failed to process withdrawal settlement: counterpart does not match settlement')
    !>(msg.fail)
  ==
::
++  test-withdrawal-settlement-equal-amount-stops
  ^-  tang
  =/  event-id=beid  (from-atom:blist 9)
  =/  recipient=nock-lock-root  *hash:t
  =/  amount=coins  1.300.000
  =/  [state=bridge-state as-of=base-hash wd=withdrawal]
    (setup-unsettled-withdrawal event-id recipient amount)
  =/  settlement=withdrawal-settlement
    (make-withdrawal-settlement event-id as-of recipient amount 10)
  =/  settlements=(z-map nname:t withdrawal-settlement)
    (~(put z-by *(z-map nname:t withdrawal-settlement)) nname.settlement settlement)
  =/  latest=nock-block
    (produce-nock-block:hel state *(z-map nname:t deposit) settlements)
  =/  nock  ~(. nock-lib state)
  =/  result=process-result
    (nockchain-process-withdrawal-settlements:nock latest)
  ?>  ?=(%| -.result)
  =/  fail=process-fail  +.result
  ?>  ?=(%stop -.fail)
  ;:  weld
    %+  expect-eq
      !>('failed to process withdrawal settlement: counterpart does not match settlement')
    !>(msg.fail)
  ==
::
++  test-withdrawal-settlement-greater-amount-stops
  ^-  tang
  =/  event-id=beid  (from-atom:blist 10)
  =/  recipient=nock-lock-root  *hash:t
  =/  amount=coins  1.400.000
  =/  [state=bridge-state as-of=base-hash wd=withdrawal]
    (setup-unsettled-withdrawal event-id recipient amount)
  =/  settlement=withdrawal-settlement
    (make-withdrawal-settlement event-id as-of recipient +(amount) 10)
  =/  settlements=(z-map nname:t withdrawal-settlement)
    (~(put z-by *(z-map nname:t withdrawal-settlement)) nname.settlement settlement)
  =/  latest=nock-block
    (produce-nock-block:hel state *(z-map nname:t deposit) settlements)
  =/  nock  ~(. nock-lib state)
  =/  result=process-result
    (nockchain-process-withdrawal-settlements:nock latest)
  ?>  ?=(%| -.result)
  =/  fail=process-fail  +.result
  ?>  ?=(%stop -.fail)
  ;:  weld
    %+  expect-eq
      !>('failed to process withdrawal settlement: counterpart does not match settlement')
    !>(msg.fail)
  ==
::
++  test-withdrawal-settlement-destination-mismatch-stops
  ^-  tang
  =/  event-id=beid  (from-atom:blist 11)
  =/  recipient=nock-lock-root  *hash:t
  =/  wrong-recipient=nock-lock-root  [0xaa 0xbb 0xcc 0xdd 0xee]
  =/  amount=coins  1.500.000
  =/  [state=bridge-state as-of=base-hash wd=withdrawal]
    (setup-unsettled-withdrawal event-id recipient amount)
  =/  settlement=withdrawal-settlement
    (make-withdrawal-settlement event-id as-of wrong-recipient 1.250.000 10)
  =/  settlements=(z-map nname:t withdrawal-settlement)
    (~(put z-by *(z-map nname:t withdrawal-settlement)) nname.settlement settlement)
  =/  latest=nock-block
    (produce-nock-block:hel state *(z-map nname:t deposit) settlements)
  =/  nock  ~(. nock-lib state)
  =/  result=process-result
    (nockchain-process-withdrawal-settlements:nock latest)
  ?>  ?=(%| -.result)
  =/  fail=process-fail  +.result
  ?>  ?=(%stop -.fail)
  ;:  weld
    %+  expect-eq
      !>('failed to process withdrawal settlement: counterpart does not match settlement')
    !>(msg.fail)
  ==
::
++  test-withdrawal-settlement-missing-counterpart-stops
  ^-  tang
  =/  event-id=beid  (from-atom:blist 12)
  =/  other-event-id=beid  (from-atom:blist 13)
  =/  recipient=nock-lock-root  *hash:t
  =/  amount=coins  1.600.000
  =/  [state=bridge-state as-of=base-hash wd=withdrawal]
    (setup-unsettled-withdrawal event-id recipient amount)
  =|  withdrawals=(z-map beid withdrawal)
  =.  withdrawals
    (~(put z-by withdrawals) other-event-id (make-withdrawal other-event-id recipient amount))
  =/  blocks=base-blocks
    (make-base-blocks:hel state withdrawals *(z-map beid deposit-settlement))
  =.  base-hashchain.hash-state.state
    (~(put z-by base-hashchain.hash-state.state) as-of blocks)
  =/  settlement=withdrawal-settlement
    (make-withdrawal-settlement event-id as-of recipient 1.450.000 10)
  =/  settlements=(z-map nname:t withdrawal-settlement)
    (~(put z-by *(z-map nname:t withdrawal-settlement)) nname.settlement settlement)
  =/  latest=nock-block
    (produce-nock-block:hel state *(z-map nname:t deposit) settlements)
  =/  nock  ~(. nock-lib state)
  =/  result=process-result
    (nockchain-process-withdrawal-settlements:nock latest)
  ?>  ?=(%| -.result)
  =/  fail=process-fail  +.result
  ?>  ?=(%stop -.fail)
  ;:  weld
    %+  expect-eq
      !>('failed to process withdrawal settlement: counterpart event not found in as-of base block')
    !>(msg.fail)
  ==
::
++  test-withdrawal-settlement-missing-unsettled-stops
  ^-  tang
  =/  event-id=beid  (from-atom:blist 14)
  =/  recipient=nock-lock-root  *hash:t
  =/  amount=coins  1.700.000
  =/  [state=bridge-state as-of=base-hash wd=withdrawal]
    (setup-unsettled-withdrawal event-id recipient amount)
  =.  unsettled-withdrawals.hash-state.state
    (~(del z-bi unsettled-withdrawals.hash-state.state) [as-of event-id])
  =/  settlement=withdrawal-settlement
    (make-withdrawal-settlement event-id as-of recipient 1.550.000 10)
  =/  settlements=(z-map nname:t withdrawal-settlement)
    (~(put z-by *(z-map nname:t withdrawal-settlement)) nname.settlement settlement)
  =/  latest=nock-block
    (produce-nock-block:hel state *(z-map nname:t deposit) settlements)
  =/  nock  ~(. nock-lib state)
  =/  result=process-result
    (nockchain-process-withdrawal-settlements:nock latest)
  ?>  ?=(%| -.result)
  =/  fail=process-fail  +.result
  ?>  ?=(%stop -.fail)
  ;:  weld
    %+  expect-eq
      !>('failed to process withdrawal settlement: cannot find unsettled withdrawal in state')
    !>(msg.fail)
  ==
::
++  test-create-withdrawal-tx-emits-built-proposal
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =.  config.state  test-config:hel
  =.  constants.state  (small-constants:hel 1 0 0)
  =.  nockchain-constants.state  [~ *blockchain-constants:t]
  =/  selected=selected-withdrawal-note
    (create-selected-withdrawal-note:hel config.state [0x11 0x12 0x13 0x14 0x15] 15.000.000)
  =/  id=withdrawal-id
    [[0x21 0x22 0x23 0x24 0x25] 8]
  =/  request=create-withdrawal-tx
    :*  id
        [0x31 0x32 0x33 0x34 0x35]
        3.000.000
        10.000.000
        25
        0
        [10 [0x41 0x42 0x43 0x44 0x45]]
        7.000.000
        ~[selected]
    ==
  =/  brg  (brg:hel)
  =/  bridge  (lod:hel state brg)
  =/  [effects=(list effect) bridge]
    (pok:hel 0 [%0 %create-withdrawal-tx request] bridge)
  ?~  effects
    ~|('expected withdrawal-proposal-built effect' !!)
  ?>  ?=([%0 %withdrawal-proposal-built *] i.effects)
  =/  proposal=withdrawal-proposal  proposal.i.effects
  =/  =raw-tx:v1:t  (new:raw-tx:v1:t spends.transaction.proposal)
  =/  tx=tx:t  (new:tx:t raw-tx 0)
  =/  outs=outputs:t  ~(outputs get:tx:t tx)
  ?>  ?=(%1 -.outs)
  =/  has-bridge-withdrawal-output=?
    %+  lien  ~(tap z-in:zo +.outs)
    |=  out=output:t
    =/  out-note=nnote:t  ~(note get:output:t out)
    ?>  ?=(@ -.out-note)
    (~(has z-by:zo note-data.out-note) %bridge-w)
  ;:  weld
    %+  expect-eq
      !>(~[name.selected])
    !>(selected-notes.proposal)
  ::
    %+  expect-eq
      !>(id)
    !>(id.proposal)
  ::
    (expect !>(has-bridge-withdrawal-output))
  ==
::
++  test-create-withdrawal-tx-accepts-mixed-stub-and-full-inputs
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =.  config.state  test-config:hel
  =.  constants.state  (small-constants:hel 1 0 0)
  =.  nockchain-constants.state  [~ bc-with-fees:dhel]
  =/  selected-stub=selected-withdrawal-note
    (create-selected-withdrawal-note-at-origin:hel config.state [0x91 0x92 0x93 0x94 0x95] 15.000.000 7)
  =/  selected-full=selected-withdrawal-note
    (create-selected-withdrawal-note-at-origin:hel config.state [0xa1 0xa2 0xa3 0xa4 0xa5] 15.000.000 17)
  =/  id=withdrawal-id
    [[0xb1 0xb2 0xb3 0xb4 0xb5] 10]
  =/  request=create-withdrawal-tx
    :*  id
        [0xc1 0xc2 0xc3 0xc4 0xc5]
        13.000.000
        30.000.000
        60
        0
        [25 [0xd1 0xd2 0xd3 0xd4 0xd5]]
        0
        ~[selected-stub selected-full]
    ==
  =/  names=(list nname:t)
    ~[name.selected-stub name.selected-full]
  =/  allowed=(z-set:zo hash:t)
    %-  z-silt:zo
    %+  turn  nodes.config.state
    |=  node=node-info
    nock-pkh.node
  =/  input-lock=lock:t
    [%pkh [m=min-signers.constants.state allowed]]~
  =/  get-note=$-(nname:t nnote:t)
    |=  wanted=nname:t
    ?:  =(wanted name.selected-stub)
      note.selected-stub
    ?:  =(wanted name.selected-full)
      note.selected-full
    ~|('mixed withdrawal selected note missing' !!)
  =/  order=order:wt
    [%bridge-withdrawal base-event-id=base-event-id.id base-hash=as-of.id root=recipient.request base-batch-end=base-batch-end.request gift=amount.request]
  =/  dry-run=transaction:wt
    %:  ~(build txb-lib bc-with-fees:dhel)
      names
      ~[order]
      0
      %.y
      ~[my-nock-key.config.state]
      ~
      get-note
      [~ input-lock]
      %.y
      %asc
      height.snapshot.request
    ==
  =/  wallet-utils  ~(. wutils %.y bc-with-fees:dhel)
  =/  min-fee=coins:t
    %:  spends:estimate-fee:wallet-utils
      spends.dry-run
      inputs.metadata.dry-run
      height.snapshot.request
    ==
  =/  tight-request=create-withdrawal-tx
    request(fee min-fee)
  =/  brg  (brg:hel)
  =/  bridge  (lod:hel state brg)
  =/  [effects=(list effect) bridge]
    (pok:hel 0 [%0 %create-withdrawal-tx tight-request] bridge)
  ?~  effects
    ~|('expected withdrawal-proposal-built effect for exact-min-fee mixed-input withdrawal' !!)
  ?>  ?=([%0 %withdrawal-proposal-built *] i.effects)
  =/  proposal=withdrawal-proposal  proposal.i.effects
  =/  spend-entries=(list [nname:t spend:v1:t])
    ~(tap z-by:zo spends.transaction.proposal)
  =/  actual-fee=coins:t
    (roll-fees:spends:t spends.transaction.proposal)
  =/  counts=[full=@ stub=@]
    %+  roll  spend-entries
    |=  [[* sp=spend:v1:t] acc=[full=@ stub=@]]
    ?>  ?=(%1 -.sp)
    =/  sp1=spend-1:v1:t  +.sp
    ?:  ?=([%full * * *] lmp.witness.sp1)
      [+(full.acc) stub.acc]
    [full.acc +(stub.acc)]
  ;:  weld
    %+  expect-eq
      !>(names)
    !>(selected-notes.proposal)
  ::
    (expect !>((gth min-fee 0)))
  ::
    %+  expect-eq
      !>(min-fee)
    !>(actual-fee)
  ::
    %+  expect-eq
      !>(2)
    !>((lent spend-entries))
  ::
    %+  expect-eq
      !>(1)
    !>(stub.counts)
  ::
    %+  expect-eq
      !>(1)
    !>(full.counts)
  ==
::
::  TODO: add a real sign-tx test that removes only the bridge signer from
::  existing witness entries and asserts %sign-tx restores it.
++  test-sign-tx-is-idempotent-on-built-proposal
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =.  config.state  test-config:hel
  =.  constants.state  (small-constants:hel 1 0 0)
  =.  nockchain-constants.state  [~ *blockchain-constants:t]
  =/  selected=selected-withdrawal-note
    (create-selected-withdrawal-note:hel config.state [0x51 0x52 0x53 0x54 0x55] 15.000.000)
  =/  request=create-withdrawal-tx
    :*  [[0x61 0x62 0x63 0x64 0x65] 9]
        [0x71 0x72 0x73 0x74 0x75]
        3.000.000
        10.000.000
        50
        0
        [10 [0x81 0x82 0x83 0x84 0x85]]
        7.000.000
        ~[selected]
    ==
  =/  brg  (brg:hel)
  =/  bridge  (lod:hel state brg)
  =/  [build-effects=(list effect) bridge]
    (pok:hel 0 [%0 %create-withdrawal-tx request] bridge)
  ?~  build-effects
    ~|('expected withdrawal-proposal-built effect' !!)
  ?>  ?=([%0 %withdrawal-proposal-built *] i.build-effects)
  =/  built=withdrawal-proposal  proposal.i.build-effects
  =/  [sign-effects=(list effect) bridge]
    (pok:hel 1 [%0 %sign-tx built] bridge)
  ?~  sign-effects
    ~|('expected withdrawal-tx-signed effect' !!)
  ?>  ?=([%0 %withdrawal-tx-signed *] i.sign-effects)
  =/  signed=withdrawal-proposal  proposal.i.sign-effects
  %+  expect-eq
    !>(built)
  !>(signed)
--
