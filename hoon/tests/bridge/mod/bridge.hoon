/=  *  /common/test
/=  *  /common/zoon
/=  t  /common/tx-engine
/=  nock-lib  /apps/bridge/nock
/=  hel  /tests/bridge/helpers
/=  *  /apps/bridge/types
|%
++  incoming-nockchain-block-requires-all-txs-present
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =/  nock  ~(. nock-lib state)
  =/  tx-id=tx-id:t  *tx-id:t
  =/  tx-ids=(z-set tx-id:t)  (z-silt ~[tx-id])
  =/  page=page:v1:t  *page:v1:t
  =.  tx-ids.page  tx-ids
  =/  txs=(z-map tx-id:t tx:t)  *(z-map tx-id:t tx:t)
  =/  nock-cause=nockchain-block:cause  [block=page txs=txs]
  =/  [effs=(list effect) new-state=bridge-state]
    (incoming-nockchain-block:nock [nock-cause [~ 0 0x0 *@da]])
  =/  is-stop=?
    ?~  effs  %.n
    ?=(%stop +<+>.i.effs)
  %+  weld
    (expect !>(is-stop))
  %+  expect-eq
    !>(state)
  !>(new-state)
::
++  nockchain-propose-deposits-moves-only-valid-recipients
  ^-  tang
  =/  name=nname:t  *nname:t
  =/  good-dest=base-addr  0x1111
  =/  deposit-tx-id=tx-id:t  *tx-id:t
  =/  deposit-bad=deposit
    :*  deposit-tx-id
        name
        ~
        1.000.000
        50.000
    ==
  =/  deposit-good=deposit
    :*  deposit-tx-id
        name
        [~ good-dest]
        2.000.000
        60.000
    ==
  =/  state-0=bridge-state  *bridge-state
  =/  empty-withdrawal-settlements=(z-map nname:t withdrawal-settlement)
    *(z-map nname:t withdrawal-settlement)
  ::
  ::  block 1: malformed recipient => no request, deposit stays unsettled
  =/  deposits-1=(z-map nname:t deposit)
    (~(put z-by *(z-map nname:t deposit)) name deposit-bad)
  =/  [blk-1=nock-block state-1a=bridge-state]
    (add-nockchain-blocks:hel state-0 deposits-1 empty-withdrawal-settlements)
  =/  hash-1=nock-hash  last-nock-block.hash-state.state-1a
  =.  unsettled-deposits.hash-state.state-1a
    (~(put z-bi unsettled-deposits.hash-state.state-1a) hash-1 name deposit-bad)
  =/  nock-1  ~(. nock-lib state-1a)
  =/  [reqs-1=(list nock-deposit-request:effect) state-1=bridge-state]
    (nockchain-propose-deposits:nock-1 blk-1)
  ::
  ::  block 2: valid recipient => request emitted, deposit moves to unconfirmed
  =/  deposits-2=(z-map nname:t deposit)
    (~(put z-by *(z-map nname:t deposit)) name deposit-good)
  =/  [blk-2=nock-block state-1b=bridge-state]
    (add-nockchain-blocks:hel state-1 deposits-2 empty-withdrawal-settlements)
  =/  hash-2=nock-hash  last-nock-block.hash-state.state-1b
  =.  unsettled-deposits.hash-state.state-1b
    (~(put z-bi unsettled-deposits.hash-state.state-1b) hash-2 name deposit-good)
  =/  nock-2  ~(. nock-lib state-1b)
  =/  [reqs-2=(list nock-deposit-request:effect) state-2=bridge-state]
    (nockchain-propose-deposits:nock-2 blk-2)
  ::
  =/  valid-req=?
    ?~  reqs-2  %.n
    ?&  =(as-of.i.reqs-2 hash-2)
        =(recipient.i.reqs-2 good-dest)
    ==
  ;:  weld
    (expect-null !>(reqs-1))
  ::
    (expect !>((~(has z-bi unsettled-deposits.hash-state.state-1) hash-1 name)))
  ::
    (expect !>(valid-req))
  ::
    %+  expect-eq
      !>(%.n)
    !>((~(has z-bi unsettled-deposits.hash-state.state-2) hash-2 name))
  ==
::
++  test-peek-unsettled-deposits-returns-requests
  ^-  tang
  =/  name=nname:t  *nname:t
  =/  good-dest=base-addr  0x1111
  =/  deposit-tx-id=tx-id:t  *tx-id:t
  =/  =deposit
    :*  deposit-tx-id
        name
        [~ good-dest]
        2.000.000
        60.000
    ==
  =/  state-0=bridge-state  *bridge-state
  =/  deposits=(z-map nname:t ^deposit)
    (~(put z-by *(z-map nname:t ^deposit)) name deposit)
  =/  empty-withdrawal-settlements=(z-map nname:t withdrawal-settlement)
    *(z-map nname:t withdrawal-settlement)
  =/  [blk=nock-block state-1=bridge-state]
    (add-nockchain-blocks:hel state-0 deposits empty-withdrawal-settlements)
  =/  block-hash=nock-hash  last-nock-block.hash-state.state-1
  =.  unsettled-deposits.hash-state.state-1
    (~(put z-bi unsettled-deposits.hash-state.state-1) block-hash name deposit)
  =/  brg  (brg:hel)
  =/  bridge  (lod:hel state-1 brg)
  =/  peek-res=(unit (unit *))
    (peek:bridge [%unsettled-deposits ~])
  ?>  ?=(^ peek-res)
  ?>  ?=(^ u.peek-res)
  =/  reqs=(list nock-deposit-request:effect)
    ;;((list nock-deposit-request:effect) u.u.peek-res)
  =/  ok=?
    ?~  reqs  %.n
    ?&  =(tx-id.i.reqs deposit-tx-id)
        =(name.i.reqs name)
        =(recipient.i.reqs good-dest)
        =(amount.i.reqs 2.000.000)
        =(block-height.i.reqs height.blk)
        =(as-of.i.reqs block-hash)
    ==
  (expect !>(ok))
::
++  test-peek-nock-hashchain-deposits-includes-valid-deposits
  ^-  tang
  =/  name=nname:t  *nname:t
  =/  good-dest=base-addr  0x1111
  =/  deposit-tx-id=tx-id:t  *tx-id:t
  =/  =deposit
    :*  deposit-tx-id
        name
        [~ good-dest]
        2.000.000
        60.000
    ==
  =/  state-0=bridge-state  *bridge-state
  =/  deposits=(z-map nname:t ^deposit)
    (~(put z-by *(z-map nname:t ^deposit)) name deposit)
  =/  empty-withdrawal-settlements=(z-map nname:t withdrawal-settlement)
    *(z-map nname:t withdrawal-settlement)
  =/  [blk=nock-block state-1=bridge-state]
    (add-nockchain-blocks:hel state-0 deposits empty-withdrawal-settlements)
  =/  block-hash=nock-hash  last-nock-block.hash-state.state-1
  =/  brg  (brg:hel)
  =/  bridge  (lod:hel state-1 brg)
  =/  peek-res=(unit (unit *))
    (peek:bridge [%nock-hashchain-deposits ~])
  ?>  ?=(^ peek-res)
  ?>  ?=(^ u.peek-res)
  =/  reqs=(list nock-deposit-request:effect)
    ;;((list nock-deposit-request:effect) u.u.peek-res)
  =/  matches=(list nock-deposit-request:effect)
    %+  murn  reqs
    |=  r=nock-deposit-request:effect
    ?:  ?&  =(tx-id.r deposit-tx-id)
            =(name.r name)
            =(recipient.r good-dest)
            =(amount.r 2.000.000)
            =(block-height.r height.blk)
            =(as-of.r block-hash)
        ==
      (some r)
    ~
  (expect !>(?=(^ matches)))
++  test-load-upgrades-pre-logos-state
  ^-  tang
  =/  current=blockchain-constants:t  *blockchain-constants:t
  =/  legacy-constants=blockchain-constants-v1-pre-ai:dumb
    :*  v1-phase.current
        bythos-phase.current
        data.current
        base-fee.current
        input-fee-divisor.current
        :*  max-block-size.current
            blocks-per-epoch.current
            target-epoch-duration.current
            update-candidate-interval.current
            max-future-timestamp.current
            min-past-blocks.current
            genesis-target-atom.current
            max-target-atom.current
            check-pow-flag.current
            coinbase-timelock-min.current
            pow-len.current
            max-coinbase-split.current
            first-month-coinbase-min.current
        ==
        phase.zk-asert.current
        anchor-height.zk-asert.current
        anchor-target-atom.zk-asert.current
        ideal-block-time.zk-asert.current
        half-life.zk-asert.current
        anchor-min-timestamp.zk-asert.current
    ==
  =/  base=bridge-state  *bridge-state
  =/  old=bridge-state-3
    :*  %3
        config.base
        constants.base
        [~ legacy-constants]
        hash-state.base
        last-nock-deposit-height.base
        last-block.base
        stop.base
    ==
  =/  loaded=bridge-state  (upgrade-pre-logos-state old)
  ;:  weld
    (expect-eq !>(%4) !>(-.loaded))
    (expect-eq !>([~ current]) !>(nockchain-constants.loaded))
  ==
--
