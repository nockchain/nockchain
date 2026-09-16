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
:::  A successful lineage repair preserves the new Nock hold in the same poke.
++  test-incoming-nockchain-block-preserves-hold-after-repair
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =.  config.state  test-config:hel
  =.  constants.state  (small-constants:hel 1 0 0)
  =.  nockchain-constants.state  [~ *blockchain-constants:t]
  =/  old-hash-1=nock-hash  [0x11 0x11 0x11 0x11 0x11]
  =/  old-hash-2=nock-hash  [0x22 0x22 0x22 0x22 0x22]
  =/  block-1=nock-block
    :*  %nock
        %0
        0
        [0x61 0x62 0x63 0x64 0x65]
        *(z-map nname:t deposit)
        *(z-map nname:t withdrawal-settlement)
        *nock-hash
    ==
  =/  canonical-hash-1=nock-hash  (hash:nock-block block-1)
  =/  block-2=nock-block
    :*  %nock
        %0
        1
        [0x71 0x72 0x73 0x74 0x75]
        *(z-map nname:t deposit)
        *(z-map nname:t withdrawal-settlement)
        old-hash-1
    ==
  =/  canonical-block-2=nock-block  block-2(prev canonical-hash-1)
  =/  canonical-hash-2=nock-hash  (hash:nock-block canonical-block-2)
  =.  nock-hashchain.hash-state.state
    %+  ~(put z-by (~(put z-by *(z-map nock-hash nock-block)) old-hash-1 block-1))
      old-hash-2
    block-2
  =.  last-nock-block.hash-state.state  old-hash-2
  =.  nock-hashchain-next-height.hash-state.state  2
  =.  base-hold.hash-state.state  `[canonical-hash-2 1]
  =/  selected=selected-withdrawal-note
    (create-selected-withdrawal-note:hel config.state [0x81 0x82 0x83 0x84 0x85] 15.000.000)
  =/  id=withdrawal-id
    [[0x91 0x92 0x93 0x94 0x95] 8]
  =/  request=create-withdrawal-tx
    :*  id
        [0xa1 0xa2 0xa3 0xa4 0xa5]
        3.000.000
        10.000.000
        25
        0
        [10 [0xb1 0xb2 0xb3 0xb4 0xb5]]
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
  =.  height.page  2
  =.  parent.page  block-id.block-2
  =.  digest.page  [0xc1 0xc2 0xc3 0xc4 0xc5]
  =.  tx-ids.page  (z-silt ~[tx-id])
  =/  txs=(z-map tx-id:t tx:t)
    (~(put z-by *(z-map tx-id:t tx:t)) tx-id tx)
  =/  nock  ~(. nock-lib state)
  =/  nock-cause=nockchain-block:cause  [block=page txs=txs]
  =/  [effects=(list effect) repaired=bridge-state]
    (incoming-nockchain-block:nock [nock-cause [~ 0 0x0 *@da]])
  ?>  ?=(^ nock-hold.hash-state.repaired)
  =/  hold=[hash=base-hash height=@]  u.nock-hold.hash-state.repaired
  ;:  weld
    (expect !>(?=(~ effects)))
  ::
    (expect !>(?=(~ base-hold.hash-state.repaired)))
  ::
    %+  expect-eq
      !>(canonical-hash-2)
    !>(last-nock-block.hash-state.repaired)
  ::
    %+  expect-eq
      !>(2)
    !>(nock-hashchain-next-height.hash-state.repaired)
  ::
    %+  expect-eq
      !>(-.id)
    !>(hash.hold)
  ::
    %+  expect-eq
      !>(25)
    !>(height.hold)
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
::  Stale hash keys and prev links are rebuilt before clearing a Base hold.
++  test-stale-nock-hashchain-repair
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =/  old-hash-1=nock-hash  [0x11 0x11 0x11 0x11 0x11]
  =/  old-hash-2=nock-hash  [0x22 0x22 0x22 0x22 0x22]
  =/  dep-name=nname:t
    [[0x31 0x32 0x33 0x34 0x35] [0x41 0x42 0x43 0x44 0x45] ~]
  =/  dep=deposit
    (create-deposit:hel [0x51 0x52 0x53 0x54 0x55] dep-name `0x1234 1.000.000 2)
  =/  deposits=(z-map nname:t deposit)
    (~(put z-by *(z-map nname:t deposit)) dep-name dep)
  =/  block-1=nock-block
    :*  %nock
        %0
        1
        [0x61 0x62 0x63 0x64 0x65]
        *(z-map nname:t deposit)
        *(z-map nname:t withdrawal-settlement)
        *nock-hash
    ==
  =/  canonical-hash-1=nock-hash  (hash:nock-block block-1)
  =/  block-2=nock-block
    :*  %nock
        %0
        2
        [0x71 0x72 0x73 0x74 0x75]
        deposits
        *(z-map nname:t withdrawal-settlement)
        old-hash-1
    ==
  =/  canonical-block-2=nock-block  block-2(prev canonical-hash-1)
  =/  canonical-hash-2=nock-hash  (hash:nock-block canonical-block-2)
  =.  nock-hashchain.hash-state.state
    %+  ~(put z-by (~(put z-by *(z-map nock-hash nock-block)) old-hash-1 block-1))
      old-hash-2
    block-2
  =.  last-nock-block.hash-state.state  old-hash-2
  =.  nock-hashchain-next-height.hash-state.state  3
  =.  base-hold.hash-state.state  `[canonical-hash-2 2]
  =.  unsettled-deposits.hash-state.state
    (~(put z-bi *(z-mip nock-hash nname:t deposit)) old-hash-2 dep-name dep)
  =/  nock  ~(. nock-lib state)
  =/  repaired=(unit bridge-state)  (repair-stale-base-hold:nock ~)
  ?~  repaired
    ~|('expected stale nock hashchain repair to succeed' !!)
  =/  new-state=bridge-state  u.repaired
  =/  stored-block-2=nock-block
    (~(got z-by nock-hashchain.hash-state.new-state) canonical-hash-2)
  =/  old-chain-key-remains=?
    (~(has z-by nock-hashchain.hash-state.new-state) old-hash-2)
  =/  old-unsettled-key-remains=?
    (~(has z-bi unsettled-deposits.hash-state.new-state) old-hash-2 dep-name)
  ;:  weld
    (expect !>(?=(~ base-hold.hash-state.new-state)))
  ::
    %+  expect-eq
      !>(canonical-hash-2)
    !>(last-nock-block.hash-state.new-state)
  ::
    %+  expect-eq
      !>(canonical-hash-1)
    !>(prev.stored-block-2)
  ::
    (expect !>((~(has z-by nock-hashchain.hash-state.new-state) canonical-hash-1)))
  ::
    (expect !>((~(has z-by nock-hashchain.hash-state.new-state) canonical-hash-2)))
  ::
    (expect !>(!old-chain-key-remains))
  ::
    (expect !>((~(has z-bi unsettled-deposits.hash-state.new-state) canonical-hash-2 dep-name)))
  ::
    (expect !>(!old-unsettled-key-remains))
  ==
::
::  A hold is never cleared when rebuilding cannot produce its target hash.
++  test-stale-nock-hashchain-repair-rejects-unknown-target
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =/  old-hash=nock-hash  [0x81 0x82 0x83 0x84 0x85]
  =/  block=nock-block
    :*  %nock
        %0
        1
        [0x91 0x92 0x93 0x94 0x95]
        *(z-map nname:t deposit)
        *(z-map nname:t withdrawal-settlement)
        *nock-hash
    ==
  =.  nock-hashchain.hash-state.state
    (~(put z-by *(z-map nock-hash nock-block)) old-hash block)
  =.  last-nock-block.hash-state.state  old-hash
  =.  nock-hashchain-next-height.hash-state.state  2
  =.  base-hold.hash-state.state  `[[0xa1 0xa2 0xa3 0xa4 0xa5] 1]
  =/  nock  ~(. nock-lib state)
  =/  repaired=(unit bridge-state)  (repair-stale-base-hold:nock ~)
  (expect !>(?=(~ repaired)))
::
::  Fresh mainnet replay restores the one deposit accepted below today's minimum.
++  test-mainnet-legacy-deposit-restored
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =/  name=nname:t
    :*  [0xf480.0376.e5c6.138d 0x9a4c.e7c6.94db.95f1 0x6c18.a134.f480.fde0 0xbe1c.4b92.e6d4.61d0 0x6c6d.671d.8d73.ef3b]
        [0xf68c.c7dd.f2ba.7818 0x828a.9a6d.3dcf.f822 0x409f.62b1.3f56.88d9 0x46ea.2f97.f8f8.c4d7 0x561d.0332.2829.9954]
        ~
    ==
  =.  bridge-lock-root.config.state  -.name
  =/  block=nock-block
    :*  %nock
        %0
        46.849
        [0xea58.5f21.dd2b.1c45 0xa800.c0cb.33d7.31e1 0x74d7.9cc6.c9ae.2c02 0x29c.34b8.66c4.de58 0xeac3.e1ca.0329.b3fb]
        *(z-map nname:t deposit)
        *(z-map nname:t withdrawal-settlement)
        *nock-hash
    ==
  =/  nock  ~(. nock-lib state)
  =/  restored=nock-block  (restore-mainnet-legacy-deposit:nock block)
  =/  legacy=deposit  (~(got z-by deposits.restored) name)
  =/  old-hash=nock-hash  [0xd1 0xd2 0xd3 0xd4 0xd5]
  =/  restored-hash=nock-hash  (hash:nock-block restored)
  =.  nock-hashchain.hash-state.state
    (~(put z-by *(z-map nock-hash nock-block)) old-hash block)
  =.  last-nock-block.hash-state.state  old-hash
  =.  nock-hashchain-next-height.hash-state.state  46.850
  =.  base-hold.hash-state.state  `[restored-hash 46.849]
  =/  repair-nock  ~(. nock-lib state)
  =/  repaired=(unit bridge-state)  (repair-stale-base-hold:repair-nock ~)
  ?~  repaired
    ~|('expected legacy lineage repair to succeed' !!)
  =/  legacy-tracked=?
    (~(has z-bi unsettled-deposits.hash-state.u.repaired) restored-hash name)
  ;:  weld
    %+  expect-eq
      !>(99.702.430)
    !>(amount-to-mint.legacy)
  ::
    %+  expect-eq
      !>(297.570)
    !>(fee.legacy)
  ::
    %+  expect-eq
      !>(`(unit base-addr)`[~ 0x4be0.28f3.ed83.7add.fcb5.233f.0af7.6b61.5947.e5b4])
    !>(dest.legacy)
  ::
    %+  expect-eq
      !>(restored)
    !>((restore-mainnet-legacy-deposit:nock restored))
  ::
    (expect !>(legacy-tracked))
  ==
::
--
