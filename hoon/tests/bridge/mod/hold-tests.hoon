::  Tests for deferred settlement replay and cross-chain state invariants.
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
:::  A Base settlement whose Nockchain block has not arrived is persisted.
++  test-base-unknown-settlement-is-deferred
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =.  nock-hashchain-next-height.hash-state.state  100
  =/  base  ~(. base-lib state)
  =/  unknown-as-of=nock-hash  [0x1 0x2 0x3 0x4 0x5]
  =/  event-id=beid  (from-atom:blist 1)
  =/  settlement=deposit-settlement
    (create-deposit-settlement:hel event-id *nname:t unknown-as-of 100 0x1111 1.000.000 5)
  =/  deposit-settlements=(z-map beid deposit-settlement)
    (~(put z-by *(z-map beid deposit-settlement)) event-id settlement)
  =/  blocks=base-blocks
    (make-base-blocks:hel state *(z-map beid withdrawal) deposit-settlements)
  =/  result=process-result
    (base-process-deposit-settlements:base blocks)
  ?>  ?=(%& -.result)
  =/  new-state=bridge-state  p.result
  ;:  weld
    (expect !>((~(has z-bi deferred-deposit-settlements.hash-state.new-state) unknown-as-of event-id)))
  ::
    (expect !>(?=(~ base-hold.hash-state.new-state)))
  ::
    (expect !>(?=(~ nock-hold.hash-state.new-state)))
  ==
::
:::  Unknown settlement dependencies no longer prevent a Base batch commit.
++  test-incoming-base-settlement-commits-without-hold
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =.  constants.state  (small-constants:hel 1 10 0)
  =.  nock-hashchain-next-height.hash-state.state  100
  =.  base-hashchain-next-height.hash-state.state  10
  =/  event-id=beid  (from-atom:blist 30)
  =/  unknown-as-of=nock-hash  [0x4 0x4 0x4 0x4 0x4]
  =/  settlement-event=base-event
    :*  (to-atom:blist event-id)
        [%deposit-processed *tx-id:t *nname:t 0x4444 1.000.000 100 unknown-as-of 9]
    ==
  =/  raw=raw-base-blocks:cause
    :~  [10 0x30 0x0 ~[settlement-event]]
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
  ;:  weld
    (expect !>(?=(~ ack-effects)))
  ::
    (expect !>((~(has z-bi deferred-deposit-settlements.hash-state.committed) unknown-as-of event-id)))
  ::
    (expect !>((~(has z-by base-hashchain.hash-state.committed) blocks-hash.pending)))
  ::
    %+  expect-eq
      !>(11)
    !>(base-hashchain-next-height.hash-state.committed)
  ::
    (expect !>(?=(~ pending-base-block-commit.hash-state.committed)))
  ::
    (expect !>(?=(~ base-hold.hash-state.committed)))
  ::
    (expect !>(?=(~ nock-hold.hash-state.committed)))
  ==
::
:::  A deferred Base settlement is reconciled when its Nockchain block arrives,
:::  and the already-settled deposit is not proposed again.
++  test-deferred-deposit-reconciles-on-nock-arrival
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =/  name=nname:t  *nname:t
  =/  dep=deposit
    (create-deposit:hel [0x1 0x1 0x1 0x1 0x1] name `0x1234 1.000.000 5)
  =/  deposits=(z-map nname:t deposit)
    (~(put z-by *(z-map nname:t deposit)) name dep)
  =/  block=nock-block
    (produce-nock-block:hel state deposits *(z-map nname:t withdrawal-settlement))
  =/  as-of=nock-hash  (hash:nock-block block)
  =/  event-id=beid  (from-atom:blist 2)
  =/  settlement=deposit-settlement
    (create-deposit-settlement:hel event-id name as-of height.block 0x1234 1.000.000 6)
  =.  unsettled-deposits.hash-state.state
    (~(put z-bi unsettled-deposits.hash-state.state) as-of name dep)
  =.  deferred-deposit-settlements.hash-state.state
    (~(put z-bi deferred-deposit-settlements.hash-state.state) as-of event-id settlement)
  =/  nock  ~(. nock-lib state)
  =/  result=process-result
    (nockchain-process-deferred-deposit-settlements:nock block)
  ?>  ?=(%& -.result)
  =/  reconciled=bridge-state  p.result
  =/  nock-reconciled  ~(. nock-lib reconciled)
  =/  [requests=(list nock-deposit-request:effect) final=bridge-state]
    (nockchain-propose-deposits:nock-reconciled block)
  ;:  weld
    (expect !>(?=(~ requests)))
  ::
    (expect !>(?=(~ (~(get z-by deferred-deposit-settlements.hash-state.final) as-of))))
  ::
    %+  expect-eq
      !>(%.n)
    !>((~(has z-bi unsettled-deposits.hash-state.final) as-of name))
  ==
::
:::  A Nockchain settlement that arrives first suppresses the later withdrawal
:::  proposal and is reconciled when its Base batch arrives.
++  test-deferred-withdrawal-filters-base-proposal
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =/  event-id=beid  (from-atom:blist 3)
  =/  dest=nock-lock-root  *nock-lock-root
  =/  wd=withdrawal
    (create-withdrawal:hel event-id dest 10.000.000)
  =/  withdrawals=(z-map beid withdrawal)
    (~(put z-by *(z-map beid withdrawal)) event-id wd)
  =/  blocks=base-blocks
    (make-base-blocks:hel state withdrawals *(z-map beid deposit-settlement))
  =/  as-of=base-hash  (hash:base-blocks blocks)
  =/  name=nname:t  *nname:t
  =/  settlement=withdrawal-settlement
    :*  [0x2 0x2 0x2 0x2 0x2]
        name
        event-id
        last-height.blocks
        as-of
        dest
        7.000.000
    ==
  =.  deferred-withdrawal-settlements.hash-state.state
    (~(put z-bi deferred-withdrawal-settlements.hash-state.state) as-of name settlement)
  =/  base  ~(. base-lib state)
  =/  invalid=(unit @t)
    (validate-deferred-withdrawal-settlements:base blocks)
  =/  requests=(list nock-withdrawal-request:effect)
    (base-propose-withdrawals:base blocks)
  =/  committed=bridge-state
    (commit-base-blocks:base blocks)
  ;:  weld
    (expect !>(?=(~ invalid)))
  ::
    (expect !>(?=(~ requests)))
  ::
    (expect !>(?=(~ (~(get z-by deferred-withdrawal-settlements.hash-state.committed) as-of))))
  ::
    %+  expect-eq
      !>(%.n)
    !>((~(has z-bi unsettled-withdrawals.hash-state.committed) as-of event-id))
  ==
::
:::  Start clears future dependency holds left by the old state machine.
++  test-start-clears-legacy-holds
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =.  nock-hold.hash-state.state  `[[0xa 0xa 0xa 0xa 0xa] 1]
  =.  base-hold.hash-state.state  `[[0xb 0xb 0xb 0xb 0xb] 50.000]
  =.  stop.state  `(get-stop-info state)
  =/  resumed=bridge-state  (resume-bridge-state state)
  ;:  weld
    (expect-eq !>(~) !>(stop.resumed))
  ::
    (expect-eq !>(~) !>(base-hold.hash-state.resumed))
  ::
    (expect-eq !>(~) !>(nock-hold.hash-state.resumed))
  ==
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
  =.  nockchain-start-height.constants.state  46.810
  =/  name=nname:t
    :*  [0xf480.0376.e5c6.138d 0x9a4c.e7c6.94db.95f1 0x6c18.a134.f480.fde0 0xbe1c.4b92.e6d4.61d0 0x6c6d.671d.8d73.ef3b]
        [0xf68c.c7dd.f2ba.7818 0x828a.9a6d.3dcf.f822 0x409f.62b1.3f56.88d9 0x46ea.2f97.f8f8.c4d7 0x561d.0332.2829.9954]
        ~
    ==
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
:::  Unknown settlement dependencies must point to an unprocessed Nock height.
++  test-stale-deposit-settlement-is-not-deferred
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =.  nock-hashchain-next-height.hash-state.state  101
  =/  event-id=beid  (from-atom:blist 101)
  =/  as-of=nock-hash  [0x101 0x101 0x101 0x101 0x101]
  =/  settlement=deposit-settlement
    (create-deposit-settlement:hel event-id *nname:t as-of 100 0x1111 1.000.000 1)
  =/  settlements=(z-map beid deposit-settlement)
    (~(put z-by *(z-map beid deposit-settlement)) event-id settlement)
  =/  blocks=base-blocks
    (make-base-blocks:hel state *(z-map beid withdrawal) settlements)
  =/  base  ~(. base-lib state)
  =/  result=process-result  (base-process-deposit-settlements:base blocks)
  ?>  ?=(%| -.result)
  =/  fail=process-fail  +.result
  ;:  weld
    (expect !>(?=(%stop -.fail)))
  ::
    (expect !>(?=(~ (~(get z-by deferred-deposit-settlements.hash-state.state) as-of))))
  ==
:::
:::  Full settlement identity is checked before Rust may persist withdrawals.
++  test-invalid-base-settlement-stops-before-persistence
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =.  constants.state  (small-constants:hel 1 10 0)
  =.  base-hashchain-next-height.hash-state.state  10
  =/  name=nname:t  *nname:t
  =/  dep=deposit
    (create-deposit:hel *tx-id:t name `0x2222 1.000.000 5)
  =/  deposits=(z-map nname:t deposit)
    (~(put z-by *(z-map nname:t deposit)) name dep)
  =/  [block=nock-block nock-state=bridge-state]
    (add-nockchain-blocks:hel state deposits *(z-map nname:t withdrawal-settlement))
  =/  as-of=nock-hash  (hash:nock-block block)
  =.  unsettled-deposits.hash-state.nock-state
    (~(put z-bi unsettled-deposits.hash-state.nock-state) as-of name dep)
  =/  event-id=beid  (from-atom:blist 102)
  =/  settlement-event=base-event
    :*  (to-atom:blist event-id)
        [%deposit-processed *tx-id:t name 0x2222 1.000.000 +(height.block) as-of 1]
    ==
  =/  raw=raw-base-blocks:cause
    ~[[10 0x102 0x0 ~[settlement-event]]]
  =/  base  ~(. base-lib nock-state)
  =/  [effects=(list effect) returned=bridge-state]
    (incoming-base-blocks:base [raw [~ 0 0x0 *@da]])
  ;:  weld
    (expect !>((has-stop-effect effects)))
  ::
    (expect !>(!(has-base-withdrawals-pending-effect effects)))
  ::
    (expect-eq !>(nock-state) !>(returned))
  ::
    (expect !>(?=(~ pending-base-block-commit.hash-state.returned)))
  ==
:::
:::  A known source hash with no matching note is a stop, not a runtime crash.
++  test-known-deposit-settlement-missing-counterpart-stops
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =/  [block=nock-block known-state=bridge-state]
    (add-nockchain-blocks:hel state *(z-map nname:t deposit) *(z-map nname:t withdrawal-settlement))
  =/  as-of=nock-hash  (hash:nock-block block)
  =/  event-id=beid  (from-atom:blist 103)
  =/  settlement=deposit-settlement
    (create-deposit-settlement:hel event-id *nname:t as-of height.block 0x3333 1.000.000 1)
  =/  settlements=(z-map beid deposit-settlement)
    (~(put z-by *(z-map beid deposit-settlement)) event-id settlement)
  =/  blocks=base-blocks
    (make-base-blocks:hel known-state *(z-map beid withdrawal) settlements)
  =/  base  ~(. base-lib known-state)
  =/  result=process-result  (base-process-deposit-settlements:base blocks)
  ?>  ?=(%| -.result)
  =/  fail=process-fail  +.result
  (expect !>(?=(%stop -.fail)))
:::
:::  A note cannot settle under one hash and later appear under another hash.
++  test-deferred-deposit-wrong-hash-stops-on-counterpart
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =/  name=nname:t  *nname:t
  =/  dep=deposit
    (create-deposit:hel *tx-id:t name `0x4444 1.000.000 5)
  =/  deposits=(z-map nname:t deposit)
    (~(put z-by *(z-map nname:t deposit)) name dep)
  =/  block=nock-block
    (produce-nock-block:hel state deposits *(z-map nname:t withdrawal-settlement))
  =/  actual-as-of=nock-hash  (hash:nock-block block)
  =/  wrong-as-of=nock-hash  [0x104 0x104 0x104 0x104 0x104]
  =/  event-id=beid  (from-atom:blist 104)
  =/  settlement=deposit-settlement
    (create-deposit-settlement:hel event-id name wrong-as-of height.block 0x4444 1.000.000 1)
  =.  unsettled-deposits.hash-state.state
    (~(put z-bi unsettled-deposits.hash-state.state) actual-as-of name dep)
  =.  deferred-deposit-settlements.hash-state.state
    (~(put z-bi deferred-deposit-settlements.hash-state.state) wrong-as-of event-id settlement)
  =/  nock  ~(. nock-lib state)
  =/  result=process-result
    (nockchain-process-deferred-deposit-settlements:nock block)
  ?>  ?=(%| -.result)
  =/  fail=process-fail  +.result
  (expect !>(?=(%stop -.fail)))
:::
:::  A withdrawal counterpart under a different hash stops before persistence.
++  test-deferred-withdrawal-wrong-hash-stops-before-persistence
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =.  constants.state  (small-constants:hel 1 10 0)
  =.  base-hashchain-next-height.hash-state.state  10
  =/  event-id=beid  (from-atom:blist 105)
  =/  dest=nock-lock-root  *nock-lock-root
  =/  burn-event=base-event
    :*  (to-atom:blist event-id)
        [%burn-for-withdrawal 0x5555 10.000.000 dest]
    ==
  =/  raw=raw-base-blocks:cause
    ~[[10 0x105 0x0 ~[burn-event]]]
  =/  wrong-as-of=base-hash  [0x105 0x105 0x105 0x105 0x105]
  =/  name=nname:t  *nname:t
  =/  settlement=withdrawal-settlement
    :*  *tx-id:t
        name
        event-id
        10
        wrong-as-of
        dest
        7.000.000
    ==
  =.  deferred-withdrawal-settlements.hash-state.state
    (~(put z-bi deferred-withdrawal-settlements.hash-state.state) wrong-as-of name settlement)
  =/  base  ~(. base-lib state)
  =/  [effects=(list effect) returned=bridge-state]
    (incoming-base-blocks:base [raw [~ 0 0x0 *@da]])
  ;:  weld
    (expect !>((has-stop-effect effects)))
  ::
    (expect !>(!(has-base-withdrawals-pending-effect effects)))
  ::
    (expect-eq !>(state) !>(returned))
  ::
    (expect !>(?=(~ pending-base-block-commit.hash-state.returned)))
  ==
:::
:::  Unknown withdrawal dependencies must point to an unprocessed Base batch.
++  test-stale-withdrawal-settlement-is-not-deferred
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =.  base-hashchain-next-height.hash-state.state  124
  =/  as-of=base-hash  [0x106 0x106 0x106 0x106 0x106]
  =/  settlement=withdrawal-settlement
    :*  *tx-id:t
        *nname:t
        (from-atom:blist 106)
        123
        as-of
        *nock-lock-root
        7.000.000
    ==
  =/  settlements=(z-map nname:t withdrawal-settlement)
    (~(put z-by *(z-map nname:t withdrawal-settlement)) nname.settlement settlement)
  =/  block=nock-block
    (produce-nock-block:hel state *(z-map nname:t deposit) settlements)
  =/  nock  ~(. nock-lib state)
  =/  result=process-result
    (nockchain-process-withdrawal-settlements:nock block)
  ?>  ?=(%| -.result)
  =/  fail=process-fail  +.result
  ;:  weld
    (expect !>(?=(%stop -.fail)))
  ::
    (expect !>(?=(~ (~(get z-by deferred-withdrawal-settlements.hash-state.state) as-of))))
  ==
:::
:::  A known Base hash also binds the settlement's batch-end height.
++  test-known-withdrawal-settlement-batch-end-mismatch-stops
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =.  constants.state  (small-constants:hel 1 10 0)
  =/  event-id=beid  (from-atom:blist 107)
  =/  dest=nock-lock-root  *nock-lock-root
  =/  wd=withdrawal  (create-withdrawal:hel event-id dest 10.000.000)
  =/  withdrawals=(z-map beid withdrawal)
    (~(put z-by *(z-map beid withdrawal)) event-id wd)
  =/  blocks=base-blocks
    (make-base-blocks:hel state withdrawals *(z-map beid deposit-settlement))
  =/  as-of=base-hash  (hash:base-blocks blocks)
  =.  base-hashchain.hash-state.state
    (~(put z-by base-hashchain.hash-state.state) as-of blocks)
  =.  unsettled-withdrawals.hash-state.state
    (~(put z-bi unsettled-withdrawals.hash-state.state) as-of event-id wd)
  =/  settlement=withdrawal-settlement
    :*  *tx-id:t
        *nname:t
        event-id
        +(last-height.blocks)
        as-of
        dest
        7.000.000
    ==
  =/  settlements=(z-map nname:t withdrawal-settlement)
    (~(put z-by *(z-map nname:t withdrawal-settlement)) nname.settlement settlement)
  =/  block=nock-block
    (produce-nock-block:hel state *(z-map nname:t deposit) settlements)
  =/  nock  ~(. nock-lib state)
  =/  result=process-result
    (nockchain-process-withdrawal-settlements:nock block)
  ?>  ?=(%| -.result)
  =/  fail=process-fail  +.result
  (expect !>(?=(%stop -.fail)))
:::
:::  Counterpart-first deposit replay rejects a later cross-hash settlement.
++  test-existing-deposit-counterpart-rejects-wrong-hash-settlement
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =/  name=nname:t  *nname:t
  =/  dep=deposit
    (create-deposit:hel *tx-id:t name `0x108 1.000.000 5)
  =/  deposits=(z-map nname:t deposit)
    (~(put z-by *(z-map nname:t deposit)) name dep)
  =/  block=nock-block
    (produce-nock-block:hel state deposits *(z-map nname:t withdrawal-settlement))
  =/  actual-as-of=nock-hash  (hash:nock-block block)
  =.  unsettled-deposits.hash-state.state
    (~(put z-bi unsettled-deposits.hash-state.state) actual-as-of name dep)
  =/  event-id=beid  (from-atom:blist 108)
  =/  wrong-as-of=nock-hash  [0x108 0x108 0x108 0x108 0x108]
  =/  settlement=deposit-settlement
    (create-deposit-settlement:hel event-id name wrong-as-of +(height.block) 0x108 1.000.000 1)
  =/  settlements=(z-map beid deposit-settlement)
    (~(put z-by *(z-map beid deposit-settlement)) event-id settlement)
  =/  blocks=base-blocks
    (make-base-blocks:hel state *(z-map beid withdrawal) settlements)
  =/  base  ~(. base-lib state)
  =/  result=process-result  (base-process-deposit-settlements:base blocks)
  ?>  ?=(%| -.result)
  =/  fail=process-fail  +.result
  (expect !>(?=(%stop -.fail)))
:::
:::  Counterpart-first withdrawal replay rejects a later cross-hash settlement.
++  test-existing-withdrawal-counterpart-rejects-wrong-hash-settlement
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =.  constants.state  (small-constants:hel 1 10 0)
  =.  base-hashchain-next-height.hash-state.state  11
  =/  event-id=beid  (from-atom:blist 109)
  =/  dest=nock-lock-root  *nock-lock-root
  =/  wd=withdrawal  (create-withdrawal:hel event-id dest 10.000.000)
  =/  withdrawals=(z-map beid withdrawal)
    (~(put z-by *(z-map beid withdrawal)) event-id wd)
  =/  blocks=base-blocks
    (make-base-blocks:hel state withdrawals *(z-map beid deposit-settlement))
  =/  actual-as-of=base-hash  (hash:base-blocks blocks)
  =.  base-hashchain.hash-state.state
    (~(put z-by base-hashchain.hash-state.state) actual-as-of blocks)
  =.  unsettled-withdrawals.hash-state.state
    (~(put z-bi unsettled-withdrawals.hash-state.state) actual-as-of event-id wd)
  =/  wrong-as-of=base-hash  [0x109 0x109 0x109 0x109 0x109]
  =/  settlement=withdrawal-settlement
    :*  *tx-id:t
        *nname:t
        event-id
        11
        wrong-as-of
        dest
        7.000.000
    ==
  =/  settlements=(z-map nname:t withdrawal-settlement)
    (~(put z-by *(z-map nname:t withdrawal-settlement)) nname.settlement settlement)
  =/  block=nock-block
    (produce-nock-block:hel state *(z-map nname:t deposit) settlements)
  =/  nock  ~(. nock-lib state)
  =/  result=process-result
    (nockchain-process-withdrawal-settlements:nock block)
  ?>  ?=(%| -.result)
  =/  fail=process-fail  +.result
  (expect !>(?=(%stop -.fail)))
:::
:::  Valid deposit settlement order changes effects, not final kernel state.
++  test-deposit-arrival-orders-converge
  ^-  tang
  =/  initial=bridge-state  *bridge-state
  =/  name=nname:t  *nname:t
  =/  dep=deposit
    (create-deposit:hel *tx-id:t name `0x110 1.000.000 5)
  =/  deposits=(z-map nname:t deposit)
    (~(put z-by *(z-map nname:t deposit)) name dep)
  =/  block=nock-block
    (produce-nock-block:hel initial deposits *(z-map nname:t withdrawal-settlement))
  =/  as-of=nock-hash  (hash:nock-block block)
  =/  event-id=beid  (from-atom:blist 110)
  =/  settlement=deposit-settlement
    (create-deposit-settlement:hel event-id name as-of height.block 0x110 1.000.000 1)
  =/  settlements=(z-map beid deposit-settlement)
    (~(put z-by *(z-map beid deposit-settlement)) event-id settlement)
  =/  settlement-blocks=base-blocks
    (make-base-blocks:hel initial *(z-map beid withdrawal) settlements)
  ::
  ::  Counterpart first, then settlement.
  =/  [counterpart-block=nock-block counterpart-state=bridge-state]
    (add-nockchain-blocks:hel initial deposits *(z-map nname:t withdrawal-settlement))
  =.  unsettled-deposits.hash-state.counterpart-state
    (~(put z-bi unsettled-deposits.hash-state.counterpart-state) as-of name dep)
  =/  counterpart-base  ~(. base-lib counterpart-state)
  =/  counterpart-result=process-result
    (base-process-deposit-settlements:counterpart-base settlement-blocks)
  ?>  ?=(%& -.counterpart-result)
  =/  counterpart-final=bridge-state  p.counterpart-result
  ::
  ::  Settlement first, then counterpart.
  =/  initial-base  ~(. base-lib initial)
  =/  settlement-result=process-result
    (base-process-deposit-settlements:initial-base settlement-blocks)
  ?>  ?=(%& -.settlement-result)
  =/  deferred-state=bridge-state  p.settlement-result
  =/  [deferred-block=nock-block deferred-counterpart-state=bridge-state]
    (add-nockchain-blocks:hel deferred-state deposits *(z-map nname:t withdrawal-settlement))
  ?>  =(counterpart-block deferred-block)
  =.  unsettled-deposits.hash-state.deferred-counterpart-state
    (~(put z-bi unsettled-deposits.hash-state.deferred-counterpart-state) as-of name dep)
  =/  deferred-nock  ~(. nock-lib deferred-counterpart-state)
  =/  deferred-result=process-result
    (nockchain-process-deferred-deposit-settlements:deferred-nock deferred-block)
  ?>  ?=(%& -.deferred-result)
  =/  deferred-final=bridge-state  p.deferred-result
  (expect-eq !>(hash-state.counterpart-final) !>(hash-state.deferred-final))
:::
:::  Valid withdrawal settlement order changes effects, not final kernel state.
++  test-withdrawal-arrival-orders-converge
  ^-  tang
  =/  initial=bridge-state  *bridge-state
  =.  constants.initial  (small-constants:hel 1 10 0)
  =.  base-hashchain-next-height.hash-state.initial  10
  =/  event-id=beid  (from-atom:blist 111)
  =/  dest=nock-lock-root  *nock-lock-root
  =/  wd=withdrawal  (create-withdrawal:hel event-id dest 10.000.000)
  =/  withdrawals=(z-map beid withdrawal)
    (~(put z-by *(z-map beid withdrawal)) event-id wd)
  =/  withdrawal-blocks=base-blocks
    (make-base-blocks:hel initial withdrawals *(z-map beid deposit-settlement))
  =/  as-of=base-hash  (hash:base-blocks withdrawal-blocks)
  =/  settlement=withdrawal-settlement
    :*  *tx-id:t
        *nname:t
        event-id
        last-height.withdrawal-blocks
        as-of
        dest
        7.000.000
    ==
  =/  settlements=(z-map nname:t withdrawal-settlement)
    (~(put z-by *(z-map nname:t withdrawal-settlement)) nname.settlement settlement)
  =/  settlement-block=nock-block
    (produce-nock-block:hel initial *(z-map nname:t deposit) settlements)
  ::
  ::  Counterpart first, then settlement.
  =/  initial-base  ~(. base-lib initial)
  =/  counterpart-state=bridge-state
    (commit-base-blocks:initial-base withdrawal-blocks)
  =/  counterpart-nock  ~(. nock-lib counterpart-state)
  =/  counterpart-result=process-result
    (nockchain-process-withdrawal-settlements:counterpart-nock settlement-block)
  ?>  ?=(%& -.counterpart-result)
  =/  counterpart-final=bridge-state  p.counterpart-result
  ::
  ::  Settlement first, then counterpart.
  =/  initial-nock  ~(. nock-lib initial)
  =/  settlement-result=process-result
    (nockchain-process-withdrawal-settlements:initial-nock settlement-block)
  ?>  ?=(%& -.settlement-result)
  =/  deferred-state=bridge-state  p.settlement-result
  =/  deferred-base  ~(. base-lib deferred-state)
  ?^  invalid=(validate-deferred-withdrawal-settlements:deferred-base withdrawal-blocks)
    ~|  invalid  !!
  =/  deferred-final=bridge-state
    (commit-base-blocks:deferred-base withdrawal-blocks)
  (expect-eq !>(hash-state.counterpart-final) !>(hash-state.deferred-final))
:::
:::  A block cannot settle one Base event twice through different note names.
++  test-duplicate-withdrawal-counterpart-in-block-stops
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =.  constants.state  (small-constants:hel 1 10 0)
  =.  base-hashchain-next-height.hash-state.state  11
  =/  event-id=beid  (from-atom:blist 112)
  =/  dest=nock-lock-root  *nock-lock-root
  =/  wd=withdrawal  (create-withdrawal:hel event-id dest 10.000.000)
  =/  withdrawals=(z-map beid withdrawal)
    (~(put z-by *(z-map beid withdrawal)) event-id wd)
  =/  blocks=base-blocks
    (make-base-blocks:hel state withdrawals *(z-map beid deposit-settlement))
  =/  actual-as-of=base-hash  (hash:base-blocks blocks)
  =.  base-hashchain.hash-state.state
    (~(put z-by base-hashchain.hash-state.state) actual-as-of blocks)
  =.  unsettled-withdrawals.hash-state.state
    (~(put z-bi unsettled-withdrawals.hash-state.state) actual-as-of event-id wd)
  =/  name-a=nname:t  *nname:t
  =/  name-b=nname:t
    [[0x112 0x112 0x112 0x112 0x112] [0x212 0x212 0x212 0x212 0x212] ~]
  =/  ordering-map=(z-map nname:t @)  *(z-map nname:t @)
  =.  ordering-map  (~(put z-by ordering-map) name-a 0)
  =.  ordering-map  (~(put z-by ordering-map) name-b 0)
  =/  ordered=(list [nname:t @])  ~(tap z-by ordering-map)
  ?>  ?=(^ ordered)
  ?>  ?=(^ t.ordered)
  =/  [known-name=nname:t ignored-a=@]  i.ordered
  =/  [unknown-name=nname:t ignored-b=@]  i.t.ordered
  =/  known=withdrawal-settlement
    :*  *tx-id:t
        known-name
        event-id
        last-height.blocks
        actual-as-of
        dest
        7.000.000
    ==
  =/  wrong-as-of=base-hash  [0x112 0x212 0x312 0x412 0x512]
  =/  unknown=withdrawal-settlement
    :*  *tx-id:t
        unknown-name
        event-id
        11
        wrong-as-of
        dest
        7.000.000
    ==
  =/  settlements=(z-map nname:t withdrawal-settlement)
    *(z-map nname:t withdrawal-settlement)
  =.  settlements  (~(put z-by settlements) known-name known)
  =.  settlements  (~(put z-by settlements) unknown-name unknown)
  =/  block=nock-block
    (produce-nock-block:hel state *(z-map nname:t deposit) settlements)
  =/  nock  ~(. nock-lib state)
  =/  result=process-result
    (nockchain-process-withdrawal-settlements:nock block)
  ?>  ?=(%| -.result)
  =/  fail=process-fail  +.result
  (expect !>(?=(%stop -.fail)))
:::
--
