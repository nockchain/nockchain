::  Tests for hold state logic (Finding 2: Missing Nockchain Blocks)
::
::  These tests exercise the real bridge arms so they track runtime behavior.
::
/=  *  /common/test
/=  t  /common/tx-engine
/=  base-lib  /apps/bridge/base
/=  nock-lib  /apps/bridge/nock
/=  hel  /tests/bridge/helpers
/=  wt  /apps/wallet/lib/types
/=  *  /apps/bridge/types
|%
++  has-stop-effect
  |=  effects=(list effect)
  ^-  ?
  ?~  effects  %.n
  ?:  ?=([%0 %stop * *] i.effects)
    %.y
  $(effects t.effects)
::
++  has-base-withdrawals-pending-effect
  |=  effects=(list effect)
  ^-  ?
  ?~  effects  %.n
  ?:  ?=([%0 %base-block-withdrawals-pending *] i.effects)
    %.y
  $(effects t.effects)
::
::  Settlement referencing unknown nock hash triggers hold.
++  test-hold-unknown-as-of-triggers-hold
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =/  base  ~(. base-lib state)
  =/  unknown-as-of=nock-hash  [0x1 0x2 0x3 0x4 0x5]
  =/  height=@  100
  =/  dest=base-addr  0x1111
  =/  event-id=beid  (from-atom:blist 1)
  =/  settlement=deposit-settlement
    (create-deposit-settlement:hel event-id *nname:t unknown-as-of height dest 1.000.000 5)
  =/  deposit-settlements=(z-map beid deposit-settlement)
    (~(put z-by *(z-map beid deposit-settlement)) event-id settlement)
  =/  blocks=base-blocks
    (make-base-blocks:hel state *(z-map beid withdrawal) deposit-settlements)
  =/  result=process-result
    (base-process-deposit-settlements:base blocks)
  ?>  ?=(%| -.result)
  =/  process-fail=process-fail  +.result
  ?>  ?=(%hold -.process-fail)
  =/  hold=[hash=hash:t height=@]  hold.process-fail
  ;:  weld
    (expect !>(?=(%hold -.process-fail)))
  ::
    %+  expect-eq
      !>(unknown-as-of)
    !>(hash.hold)
  ::
    %+  expect-eq
      !>(height)
    !>(height.hold)
  ==
::  When multiple holds are possible, base picks the greatest height.
++  test-hold-picks-greatest-height
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =/  base  ~(. base-lib state)
  =/  dest=base-addr  0x1111
  =/  as-of-1=nock-hash  [0x1 0x1 0x1 0x1 0x1]
  =/  as-of-2=nock-hash  [0x2 0x2 0x2 0x2 0x2]
  =/  event-1=beid  (from-atom:blist 1)
  =/  event-2=beid  (from-atom:blist 2)
  =/  settlement-1=deposit-settlement
    (create-deposit-settlement:hel event-1 *nname:t as-of-1 100 dest 1.000.000 5)
  =/  settlement-2=deposit-settlement
    (create-deposit-settlement:hel event-2 *nname:t as-of-2 200 dest 1.000.000 6)
  =/  deposit-settlements=(z-map beid deposit-settlement)
    (~(put z-by (~(put z-by *(z-map beid deposit-settlement)) event-1 settlement-1)) event-2 settlement-2)
  =/  blocks=base-blocks
    (make-base-blocks:hel state *(z-map beid withdrawal) deposit-settlements)
  =/  result=process-result
    (base-process-deposit-settlements:base blocks)
  ?>  ?=(%| -.result)
  =/  process-fail=process-fail  +.result
  ?>  ?=(%hold -.process-fail)
  =/  hold=[hash=hash:t height=@]  hold.process-fail
  ;:  weld
    %+  expect-eq
      !>(200)
    !>(height.hold)
  ::
    %+  expect-eq
      !>(as-of-2)
    !>(hash.hold)
  ==
::  Incoming Base batches preflight unknown settlement dependencies before staging.
++  test-incoming-base-blocks-preflight-holds-unknown-settlement
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =/  constants=bridge-constants  (small-constants:hel 1 10 0)
  =.  constants.state  constants
  =.  base-hashchain-next-height.hash-state.state  base-start-height.constants
  =/  dep-name=nname:t  *nname:t
  =/  dep-tx=tx-id:t  *tx-id:t
  =/  dest=base-addr  0x4444
  =/  event-id=beid  (from-atom:blist 30)
  =/  unknown-as-of=nock-hash  [0x4 0x4 0x4 0x4 0x4]
  =/  settlement-event=base-event
    :*  (to-atom:blist event-id)
        [%deposit-processed dep-tx dep-name dest 1.000.000 100 unknown-as-of 9]
    ==
  =/  raw=raw-base-blocks:cause
    :~  [10 0x30 0x0 ~[settlement-event]]
    ==
  =/  base  ~(. base-lib state)
  =/  blocks=base-blocks  (cook-base-blocks:base raw)
  =/  blocks-hash=base-hash  (hash:base-blocks blocks)
  =/  [effects=(list effect) held=bridge-state]
    (incoming-base-blocks:base [raw [~ 0 0x0 *@da]])
  ?>  ?=(^ base-hold.hash-state.held)
  =/  hold=[hash=hash:t height=@]  u.base-hold.hash-state.held
  ;:  weld
    %+  expect-eq
      !>(~)
    !>(effects)
  ::
    %+  expect-eq
      !>(unknown-as-of)
    !>(hash.hold)
  ::
    %+  expect-eq
      !>(100)
    !>(height.hold)
  ::
    (expect !>(?=(~ pending-base-block-commit.hash-state.held)))
  ::
    %+  expect-eq
      !>(10)
    !>(base-hashchain-next-height.hash-state.held)
  ::
    %+  expect-eq
      !>(%.n)
    !>((~(has z-by base-hashchain.hash-state.held) blocks-hash))
  ==
::  Incoming Base batch preflight uses greatest missing Nock height.
++  test-incoming-base-blocks-preflight-picks-greatest-height
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =/  constants=bridge-constants  (small-constants:hel 1 10 0)
  =.  constants.state  constants
  =.  base-hashchain-next-height.hash-state.state  base-start-height.constants
  =/  dep-name=nname:t  *nname:t
  =/  dep-tx=tx-id:t  *tx-id:t
  =/  dest=base-addr  0x5555
  =/  event-1=beid  (from-atom:blist 31)
  =/  event-2=beid  (from-atom:blist 32)
  =/  as-of-1=nock-hash  [0x5 0x5 0x5 0x5 0x5]
  =/  as-of-2=nock-hash  [0x6 0x6 0x6 0x6 0x6]
  =/  settlement-1=base-event
    :*  (to-atom:blist event-1)
        [%deposit-processed dep-tx dep-name dest 1.000.000 100 as-of-1 10]
    ==
  =/  settlement-2=base-event
    :*  (to-atom:blist event-2)
        [%deposit-processed dep-tx dep-name dest 1.000.000 250 as-of-2 11]
    ==
  =/  raw=raw-base-blocks:cause
    :~  [10 0x31 0x0 ~[settlement-1 settlement-2]]
    ==
  =/  base  ~(. base-lib state)
  =/  [effects=(list effect) held=bridge-state]
    (incoming-base-blocks:base [raw [~ 0 0x0 *@da]])
  ?>  ?=(^ base-hold.hash-state.held)
  =/  hold=[hash=hash:t height=@]  u.base-hold.hash-state.held
  ;:  weld
    %+  expect-eq
      !>(~)
    !>(effects)
  ::
    %+  expect-eq
      !>(as-of-2)
    !>(hash.hold)
  ::
    %+  expect-eq
      !>(250)
    !>(height.hold)
  ::
    (expect !>(?=(~ pending-base-block-commit.hash-state.held)))
  ::
    %+  expect-eq
      !>(10)
    !>(base-hashchain-next-height.hash-state.held)
  ==
::  Incoming Base missing as-of stops instead of creating simultaneous holds.
++  test-incoming-base-blocks-stops-before-both-holds
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =/  constants=bridge-constants  (small-constants:hel 1 10 0)
  =.  constants.state  constants
  =.  base-hashchain-next-height.hash-state.state  base-start-height.constants
  =/  nock-hold-value=[hash=base-hash height=@]  [[0xa 0xa 0xa 0xa 0xa] 77]
  =.  nock-hold.hash-state.state  `nock-hold-value
  =/  dep-name=nname:t  *nname:t
  =/  dep-tx=tx-id:t  *tx-id:t
  =/  dest=base-addr  0x7777
  =/  event-id=beid  (from-atom:blist 33)
  =/  unknown-as-of=nock-hash  [0x7 0x7 0x7 0x7 0x7]
  =/  settlement-event=base-event
    :*  (to-atom:blist event-id)
        [%deposit-processed dep-tx dep-name dest 1.000.000 300 unknown-as-of 13]
    ==
  =/  raw=raw-base-blocks:cause
    :~  [10 0x33 0x0 ~[settlement-event]]
    ==
  =/  base  ~(. base-lib state)
  =/  [effects=(list effect) stopped=bridge-state]
    (incoming-base-blocks:base [raw [~ 0 0x0 *@da]])
  ?>  ?=(^ nock-hold.hash-state.stopped)
  =/  stopped-nock-hold=[hash=base-hash height=@]
    u.nock-hold.hash-state.stopped
  ;:  weld
    (expect !>((has-stop-effect effects)))
  ::
    %+  expect-eq
      !>(%.n)
    !>((has-base-withdrawals-pending-effect effects))
  ::
    (expect !>(?=(~ base-hold.hash-state.stopped)))
  ::
    %+  expect-eq
      !>(nock-hold-value)
    !>(stopped-nock-hold)
  ::
    (expect !>(?=(~ pending-base-block-commit.hash-state.stopped)))
  ==
:::  Incoming Nock block missing as-of stops instead of creating simultaneous holds.
++  test-incoming-nockchain-block-stops-before-both-holds
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =.  config.state  test-config:hel
  =.  constants.state  (small-constants:hel 1 0 0)
  =.  nockchain-constants.state  [~ *blockchain-constants:t]
  =/  base-hold-value=[hash=nock-hash height=@]
    [[0xa 0xa 0xa 0xa 0xa] 77]
  =.  base-hold.hash-state.state  `base-hold-value
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
  =/  [build-effects=(list effect) bridge]
    (pok:hel 0 [%0 %create-withdrawal-tx request] bridge)
  ?~  build-effects
    ~|('expected withdrawal-proposal-built effect' !!)
  ?>  ?=([%0 %withdrawal-proposal-built *] i.build-effects)
  =/  proposal=withdrawal-proposal  proposal.i.build-effects
  =/  raw-tx=raw-tx:v1:t  (new:raw-tx:v1:t spends.transaction.proposal)
  =/  tx=tx:t  (new:tx:t raw-tx 0)
  =/  tx-id=tx-id:t  ~(id get:raw-tx:t raw-tx)
  =/  page=page:v1:t  *page:v1:t
  =.  height.page  nockchain-start-height.constants.state
  =.  parent.page  *block-id:t
  =.  digest.page  [0xb 0xb 0xb 0xb 0xb]
  =.  tx-ids.page  (z-silt ~[tx-id])
  =/  txs=(z-map tx-id:t tx:t)
    (~(put z-by *(z-map tx-id:t tx:t)) tx-id tx)
  =/  nock  ~(. nock-lib state)
  =/  nock-cause=nockchain-block:cause  [block=page txs=txs]
  =/  [effects=(list effect) stopped=bridge-state]
    (incoming-nockchain-block:nock [nock-cause [~ 0 0x0 *@da]])
  ?>  ?=(^ base-hold.hash-state.stopped)
  =/  stopped-base-hold=[hash=nock-hash height=@]
    u.base-hold.hash-state.stopped
  ;:  weld
    (expect !>((has-stop-effect effects)))
  ::
    (expect !>(?=(~ nock-hold.hash-state.stopped)))
  ::
    %+  expect-eq
      !>(base-hold-value)
    !>(stopped-base-hold)
  ::
    %+  expect-eq
      !>(nockchain-start-height.constants.state)
    !>(nock-hashchain-next-height.hash-state.stopped)
  ==
::  Any hold causes handle-cause to not emit a stop effect.
++  test-hold-no-stop-handle-cause
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =/  base  ~(. base-lib state)
  =/  unknown-as-of=nock-hash  [0x3 0x3 0x3 0x3 0x3]
  =/  dest=base-addr  0x2222
  =/  event-id=beid  (from-atom:blist 3)
  =/  settlement=deposit-settlement
    (create-deposit-settlement:hel event-id *nname:t unknown-as-of 101 dest 1.000.000 7)
  =/  deposit-settlements=(z-map beid deposit-settlement)
    (~(put z-by *(z-map beid deposit-settlement)) event-id settlement)
  =/  blocks=base-blocks
    (make-base-blocks:hel state *(z-map beid withdrawal) deposit-settlements)
  =/  result=process-result
    (base-process-deposit-settlements:base blocks)
  ?>  ?=(%| -.result)
  =/  process-fail=process-fail  +.result
  ?>  ?=(%hold -.process-fail)
  =/  hold=[hash=hash:t height=@]  hold.process-fail
  =/  state-held=bridge-state  state
  =.  base-hold.hash-state.state-held  `hold
  =/  brg  (brg:hel)
  =/  bridge  (lod:hel state-held brg)
  =/  [effects=(list effect) bridge]
    (pok:hel 0 [%0 %cfg-load ~] bridge)
  =/  new-state=bridge-state  (inner-state:hel bridge)
  =/  is-stop=?
    ?~  effects  %.n
    ?=([%0 %stop * *] i.effects)
  =/  hold-still-set=?  ?=(^ base-hold.hash-state.new-state)
  ;:  weld
    (expect !>(!is-stop))
  ::
    (expect !>(hold-still-set))
  ==
::  Base hold clears when the referenced nock block arrives.
++  test-hold-base-clears-on-block-arrival
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =/  height=@  nockchain-start-height.constants.state
  =/  block-id=block-id:t  [0x9 0x9 0x9 0x9 0x9]
  =/  page=page:v1:t  *page:v1:t
  =.  height.page  height
  =.  parent.page  *block-id:t
  =.  digest.page  block-id
  =.  tx-ids.page  *(z-set tx-id:t)
  =/  txs=(z-map tx-id:t tx:t)  *(z-map tx-id:t tx:t)
  =/  expected-block=nock-block
    :*  %nock
        version=%0
        height
        block-id
        deposits=*(z-map nname:t deposit)
        withdrawal-settlements=*(z-map nname:t withdrawal-settlement)
        prev=last-nock-block.hash-state.state
    ==
  =/  hold-hash=nock-hash  (hash:nock-block expected-block)
  =/  state-held=bridge-state  state
  =.  base-hold.hash-state.state-held  `[hold-hash height]
  =/  nock  ~(. nock-lib state-held)
  =/  nock-cause=nockchain-block:cause  [block=page txs=txs]
  =/  [effects=(list effect) new-state=bridge-state]
    (incoming-nockchain-block:nock [nock-cause [~ 0 0x0 *@da]])
  =/  hold-cleared=?  ?=(~ base-hold.hash-state.new-state)
  (expect !>(hold-cleared))
::  Nock hold clears when the referenced base block arrives.
++  test-hold-nock-clears-on-block-arrival
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =/  constants=bridge-constants  (small-constants:hel 1 10 0)
  =.  constants.state  constants
  =/  height=@  base-start-height.constants.state
  =.  base-hashchain-next-height.hash-state.state  height
  =/  block-id=base-block-id  0x1
  =/  parent-id=base-block-id  0x0
  =/  raw=raw-base-blocks:cause
    :~  [height block-id parent-id ~]
    ==
  =/  base  ~(. base-lib state)
  =/  blocks=base-blocks  (cook-base-blocks:base raw)
  =/  hold-hash=base-hash  (hash:base-blocks blocks)
  =/  state-held=bridge-state  state
  =.  nock-hold.hash-state.state-held  `[hold-hash height]
  =/  base-held  ~(. base-lib state-held)
  =/  [effects=(list effect) staged=bridge-state]
    (incoming-base-blocks:base-held [raw [~ 0 0x0 *@da]])
  ?>  ?=(^ pending-base-block-commit.hash-state.staged)
  =/  pending=pending-base-block-commit-data
    u.pending-base-block-commit.hash-state.staged
  =/  metadata=pending-base-block-withdrawals  metadata.pending
  =/  ack=base-block-commit-ack
    [blocks-hash.metadata first-height.metadata last-height.metadata]
  =/  base-staged  ~(. base-lib staged)
  =/  [ack-effects=(list effect) new-state=bridge-state]
    (commit-base-block-withdrawals:base-staged ack)
  =/  hold-cleared=?  ?=(~ nock-hold.hash-state.new-state)
  (expect !>(hold-cleared))
::
++  test-base-deposit-settlement-commits-only-after-ack
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =/  constants=bridge-constants  (small-constants:hel 1 10 0)
  =.  constants.state  constants
  =.  base-hashchain-next-height.hash-state.state  base-start-height.constants
  =/  dep-name=nname:t  *nname:t
  =/  dep-tx=tx-id:t  *tx-id:t
  =/  dest=base-addr  0x3333
  =/  amount=coins  1.000.000
  =/  dep=deposit
    (create-deposit:hel dep-tx dep-name `dest amount 5)
  =/  deposits=(z-map nname:t deposit)
    (~(put z-by *(z-map nname:t deposit)) dep-name dep)
  =/  [nb=nock-block nock-state=bridge-state]
    (add-nockchain-blocks:hel state deposits *(z-map nname:t withdrawal-settlement))
  =/  as-of=nock-hash  (hash:nock-block nb)
  =.  unsettled-deposits.hash-state.nock-state
    (~(put z-bi unsettled-deposits.hash-state.nock-state) as-of dep-name dep)
  =/  event-id=beid  (from-atom:blist 20)
  =/  settlement-event=base-event
    :*  (to-atom:blist event-id)
        [%deposit-processed dep-tx dep-name dest amount height.nb as-of 7]
    ==
  =/  raw=raw-base-blocks:cause
    :~  [10 0x20 0x0 ~[settlement-event]]
    ==
  =/  base  ~(. base-lib nock-state)
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
  =/  has-unsettled=?
    (~(has z-bi unsettled-deposits.hash-state.committed) as-of dep-name)
  ;:  weld
    (expect !>((~(has z-bi unsettled-deposits.hash-state.staged) as-of dep-name)))
  ::
    (expect !>(?=(^ pending-base-block-commit.hash-state.staged)))
  ::
    %+  expect-eq
      !>(~)
    !>(ack-effects)
  ::
    (expect !>(!has-unsettled))
  ::
    (expect !>(?=(~ pending-base-block-commit.hash-state.committed)))
  ::
    (expect !>((~(has z-by base-hashchain.hash-state.committed) blocks-hash.pending)))
  ==
--
