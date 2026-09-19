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
:::  The closed mainnet lineage range reconciles by its unique canonical note
:::  even when the note's historical hashchain entry was already discarded.
:::  The same hash mismatch outside the immutable nonce range still stops.
++  test-mainnet-pre-repair-deferred-lineage-range
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =.  nockchain-start-height.constants.state  46.810
  =.  base-start-height.constants.state  39.694.000
  =/  legacy-name=nname:t  mainnet-legacy-deposit-name
  =/  name=nname:t
    [-.legacy-name [0x211 0x212 0x213 0x214 0x215] ~]
  =/  dep=deposit
    (create-deposit:hel [0x221 0x222 0x223 0x224 0x225] name `0x1234 1.000.000 5)
  =/  deposits=(z-map nname:t deposit)
    (~(put z-by *(z-map nname:t deposit)) name dep)
  =/  block=nock-block
    :*  %nock
        %0
        48.015
        [0x231 0x232 0x233 0x234 0x235]
        deposits
        *(z-map nname:t withdrawal-settlement)
        *nock-hash
    ==
  =/  canonical-as-of=nock-hash  (hash:nock-block block)
  =/  historical-as-of=nock-hash
    [0x6ed8.e075.476a.722a 0x251.7f0e.dcf4.5b09 0x53a5.d1f8.80b9.1bac 0xab72.0e9d.a61c.cc4c 0xae0c.cdd4.b817.46b9]
  =/  orphaned-as-of=nock-hash  [0x241 0x242 0x243 0x244 0x245]
  =/  orphaned-as-of-2=nock-hash  [0x251 0x252 0x253 0x254 0x255]
  =/  event-id=beid  (from-atom:blist 529)
  =/  settlement=deposit-settlement
    (create-deposit-settlement:hel event-id name historical-as-of 48.015 0x1234 1.000.000 109)
  =.  last-nock-block.hash-state.state  canonical-as-of
  =.  nock-hashchain-next-height.hash-state.state  48.016
  =.  unsettled-deposits.hash-state.state
    (~(put z-bi unsettled-deposits.hash-state.state) orphaned-as-of name dep)
  =.  unsettled-deposits.hash-state.state
    (~(put z-bi unsettled-deposits.hash-state.state) orphaned-as-of-2 name dep)
  =.  deferred-deposit-settlements.hash-state.state
    (~(put z-bi deferred-deposit-settlements.hash-state.state) historical-as-of event-id settlement)
  =/  nock  ~(. nock-lib state)
  =/  result=process-result
    (nockchain-process-deferred-deposit-settlements:nock block)
  ?>  ?=(%& -.result)
  =/  final=bridge-state  p.result
  =/  future=bridge-state  state
  =.  deferred-deposit-settlements.hash-state.future
    (~(put z-bi *(z-mip nock-hash beid deposit-settlement)) historical-as-of event-id settlement(nonce 530))
  =/  future-nock  ~(. nock-lib future)
  =/  future-result=process-result
    (nockchain-process-deferred-deposit-settlements:future-nock block)
  =/  conflicting=bridge-state  state
  =.  unsettled-deposits.hash-state.conflicting
    (~(put z-bi unsettled-deposits.hash-state.conflicting) orphaned-as-of-2 name dep(amount-to-mint 2.000.000))
  =/  conflicting-nock  ~(. nock-lib conflicting)
  =/  conflicting-result=process-result
    (nockchain-process-deferred-deposit-settlements:conflicting-nock block)
  ;:  weld
    (expect !>(!(~(has z-bi unsettled-deposits.hash-state.final) orphaned-as-of name)))
  ::
    (expect !>(!(~(has z-bi unsettled-deposits.hash-state.final) orphaned-as-of-2 name)))
  ::
    (expect !>(!(~(has z-bi deferred-deposit-settlements.hash-state.final) historical-as-of event-id)))
  ::
    (expect !>(?=(%| -.future-result)))
  ::
    (expect !>(?=(%| -.conflicting-result)))
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
  =/  name=nname:t  mainnet-legacy-deposit-name
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
  =/  old-hash=nock-hash  (hash:nock-block block)
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
    (expect !>((~(has z-by nock-hashchain.hash-state.u.repaired) old-hash)))
  ::
    (expect !>(legacy-tracked))
  ==
::
:::
::  The one finalized pre-repair settlement maps only by its complete immutable
::  Base event and Nock deposit identities, then consumes the canonical entry.
++  test-mainnet-pre-repair-settlement-migration
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =.  nockchain-start-height.constants.state  46.810
  =.  base-start-height.constants.state  39.694.000
  =/  [settlement=deposit-settlement counterpart=deposit]
    mainnet-pre-repair-deposit-settlement
  =/  name=nname:t  counterpart.settlement
  =/  deposits=(z-map nname:t deposit)
    (~(put z-by *(z-map nname:t deposit)) name counterpart)
  =/  canonical=nock-block
    :*  %nock
        %0
        48.325
        [0x121 0x122 0x123 0x124 0x125]
        deposits
        *(z-map nname:t withdrawal-settlement)
        [0x221 0x222 0x223 0x224 0x225]
    ==
  =/  canonical-hash=nock-hash  (hash:nock-block canonical)
  =.  nock-hashchain.hash-state.state
    (~(put z-by *(z-map nock-hash nock-block)) canonical-hash canonical)
  =.  last-nock-block.hash-state.state  canonical-hash
  =.  nock-hashchain-next-height.hash-state.state  48.326
  =.  unsettled-deposits.hash-state.state
    (~(put z-bi unsettled-deposits.hash-state.state) canonical-hash name counterpart)
  =/  settlements=(z-map beid deposit-settlement)
    (~(put z-by *(z-map beid deposit-settlement)) beid.settlement settlement)
  =/  blocks=base-blocks
    (make-base-blocks-at-height:hel state 40.085.800 *(z-map beid withdrawal) settlements)
  =.  last-height.blocks  40.085.899
  =/  base  ~(. base-lib state)
  =/  result=process-result
    (base-process-deposit-settlements:base blocks)
  ?>  ?=(%& -.result)
  =/  final=bridge-state  p.result
  =/  bad-settlement=deposit-settlement
    settlement(nonce +(nonce.settlement))
  =/  bad-settlements=(z-map beid deposit-settlement)
    (~(put z-by *(z-map beid deposit-settlement)) beid.bad-settlement bad-settlement)
  =/  bad-blocks=base-blocks
    (make-base-blocks-at-height:hel state 40.085.800 *(z-map beid withdrawal) bad-settlements)
  =.  last-height.bad-blocks  40.085.899
  =/  bad-result=process-result
    (base-process-deposit-settlements:base bad-blocks)
  =/  wrong-batch=base-blocks
    (make-base-blocks-at-height:hel state 40.085.900 *(z-map beid withdrawal) settlements)
  =.  last-height.wrong-batch  40.085.999
  =/  wrong-batch-result=process-result
    (base-process-deposit-settlements:base wrong-batch)
  ;:  weld
    (expect !>(!(~(has z-bi unsettled-deposits.hash-state.final) canonical-hash name)))
  ::
    (expect !>(?=(%| -.bad-result)))
  ::
    (expect !>(?=(%| -.wrong-batch-result)))
  ==
::
::  Historical mainnet settlements may translate only the closed nonce/height
::  range and still require one exact canonical counterpart and value match.
++  test-mainnet-pre-repair-lineage-range
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =.  nockchain-start-height.constants.state  46.810
  =.  base-start-height.constants.state  39.694.000
  =/  legacy-name=nname:t  mainnet-legacy-deposit-name
  =/  name=nname:t
    [-.legacy-name [0x201 0x202 0x203 0x204 0x205] ~]
  =/  counterpart=deposit
    (create-deposit:hel [0x206 0x207 0x208 0x209 0x20a] name `0x1234 1.000.000 5)
  =/  deposits=(z-map nname:t deposit)
    (~(put z-by *(z-map nname:t deposit)) name counterpart)
  =/  canonical=nock-block
    :*  %nock
        %0
        48.390
        [0x20b 0x20c 0x20d 0x20e 0x20f]
        deposits
        *(z-map nname:t withdrawal-settlement)
        *nock-hash
    ==
  =/  canonical-as-of=nock-hash  (hash:nock-block canonical)
  =/  historical-as-of=nock-hash
    [0x29da.7958.6518.bd7c 0xdf61.a8d6.2a33.eee2 0xd283.7fe4.9afb.bf19 0x2aca.87e7.216f.950a 0xce79.4b62.5f43.abe8]
  =/  event-id=beid  (from-atom:blist 121)
  =/  settlement=deposit-settlement
    (create-deposit-settlement:hel event-id name historical-as-of 48.390 0x1234 1.000.000 121)
  =.  nock-hashchain.hash-state.state
    (~(put z-by nock-hashchain.hash-state.state) canonical-as-of canonical)
  =.  last-nock-block.hash-state.state  canonical-as-of
  =.  nock-hashchain-next-height.hash-state.state  48.391
  =.  unsettled-deposits.hash-state.state
    (~(put z-bi unsettled-deposits.hash-state.state) canonical-as-of name counterpart)
  =/  settlements=(z-map beid deposit-settlement)
    (~(put z-by *(z-map beid deposit-settlement)) event-id settlement)
  =/  blocks=base-blocks
    (make-base-blocks-at-height:hel state 40.102.300 *(z-map beid withdrawal) settlements)
  =.  last-height.blocks  40.102.399
  =/  base  ~(. base-lib state)
  =/  result=process-result
    (base-process-deposit-settlements:base blocks)
  ?>  ?=(%& -.result)
  =/  final=bridge-state  p.result
  =/  future-settlement=deposit-settlement  settlement(nonce 530)
  =/  future-settlements=(z-map beid deposit-settlement)
    (~(put z-by *(z-map beid deposit-settlement)) event-id future-settlement)
  =/  future-blocks=base-blocks
    (make-base-blocks-at-height:hel state 40.102.300 *(z-map beid withdrawal) future-settlements)
  =.  last-height.future-blocks  40.102.399
  =/  future-result=process-result
    (base-process-deposit-settlements:base future-blocks)
  =/  wrong-height=deposit-settlement  settlement(nock-height 48.391)
  =/  wrong-height-settlements=(z-map beid deposit-settlement)
    (~(put z-by *(z-map beid deposit-settlement)) event-id wrong-height)
  =/  wrong-height-blocks=base-blocks
    (make-base-blocks-at-height:hel state 40.102.300 *(z-map beid withdrawal) wrong-height-settlements)
  =.  last-height.wrong-height-blocks  40.102.399
  =/  wrong-height-result=process-result
    (base-process-deposit-settlements:base wrong-height-blocks)
  ;:  weld
    (expect !>(!(~(has z-bi unsettled-deposits.hash-state.final) canonical-as-of name)))
  ::
    (expect !>(?=(%| -.future-result)))
  ::
    (expect !>(?=(%| -.wrong-height-result)))
  ==
::
::  Signers whose Base cursor already passed the one pre-repair event converge
::  on startup, but only from the complete immutable deposit identity.
++  test-mainnet-pre-repair-startup-reconciliation
  ^-  tang
  =/  state=bridge-state  *bridge-state
  =.  nockchain-start-height.constants.state  46.810
  =.  base-start-height.constants.state  39.694.000
  =.  base-hashchain-next-height.hash-state.state  40.085.900
  =/  [settlement=deposit-settlement counterpart=deposit]
    mainnet-pre-repair-deposit-settlement
  =/  name=nname:t  counterpart.settlement
  =/  deposits=(z-map nname:t deposit)
    (~(put z-by *(z-map nname:t deposit)) name counterpart)
  =/  canonical=nock-block
    :*  %nock
        %0
        48.325
        [0x121 0x122 0x123 0x124 0x125]
        deposits
        *(z-map nname:t withdrawal-settlement)
        [0x221 0x222 0x223 0x224 0x225]
    ==
  =/  canonical-hash=nock-hash  (hash:nock-block canonical)
  =.  nock-hashchain.hash-state.state
    (~(put z-by *(z-map nock-hash nock-block)) canonical-hash canonical)
  =.  last-nock-block.hash-state.state  canonical-hash
  =.  nock-hashchain-next-height.hash-state.state  48.326
  =.  unsettled-deposits.hash-state.state
    (~(put z-bi unsettled-deposits.hash-state.state) canonical-hash name counterpart)
  =/  no-event=bridge-state  state
  =/  settlements=(z-map beid deposit-settlement)
    (~(put z-by *(z-map beid deposit-settlement)) beid.settlement settlement)
  =/  event-batch=base-blocks
    (make-base-blocks-at-height:hel state 40.085.800 *(z-map beid withdrawal) settlements)
  =.  last-height.event-batch  40.085.899
  =/  event-batch-hash=base-hash  (hash:base-blocks event-batch)
  =.  base-hashchain.hash-state.state
    (~(put z-by *(z-map base-hash base-blocks)) event-batch-hash event-batch)
  =.  last-base-blocks.hash-state.state  event-batch-hash
  =/  before=bridge-state  state
  =.  base-hashchain-next-height.hash-state.before  40.085.800
  =/  before-base  ~(. base-lib before)
  =/  before-result=(unit bridge-state)
    (reconcile-mainnet-pre-repair-deposit:before-base ~)
  ?~  before-result
    ~|('expected pre-event state to remain valid' !!)
  =/  no-event-base  ~(. base-lib no-event)
  =/  no-event-result=(unit bridge-state)
    (reconcile-mainnet-pre-repair-deposit:no-event-base ~)
  =/  consumed-no-event=bridge-state  no-event
  =.  unsettled-deposits.hash-state.consumed-no-event
    (~(del z-bi unsettled-deposits.hash-state.consumed-no-event) [canonical-hash name])
  =/  consumed-no-event-base  ~(. base-lib consumed-no-event)
  =/  consumed-no-event-result=(unit bridge-state)
    (reconcile-mainnet-pre-repair-deposit:consumed-no-event-base ~)
  =/  brg  (brg:hel)
  =/  no-event-bridge  (lod:hel no-event brg)
  =/  stopped-bridge  no-event-bridge
  =^  start-effects=(list effect)  stopped-bridge
    (pok:hel 0 [%0 %start ~] no-event-bridge)
  =/  stop-peek=(unit (unit *))
    (peek:stopped-bridge [%stop-state ~])
  ?>  ?=(^ stop-peek)
  ?>  ?=(^ u.stop-peek)
  =/  stopped=?  ;;(? u.u.stop-peek)
  =/  base  ~(. base-lib state)
  =/  reconciled=(unit bridge-state)
    (reconcile-mainnet-pre-repair-deposit:base ~)
  ?~  reconciled
    ~|('expected exact finalized settlement to reconcile on startup' !!)
  =/  reconciled-state=bridge-state  u.reconciled
  =/  again-base  ~(. base-lib reconciled-state)
  =/  again=(unit bridge-state)
    (reconcile-mainnet-pre-repair-deposit:again-base ~)
  ?~  again
    ~|('expected completed startup reconciliation to be idempotent' !!)
  =/  consumed-with-deferred=bridge-state  state
  =.  unsettled-deposits.hash-state.consumed-with-deferred
    (~(del z-bi unsettled-deposits.hash-state.consumed-with-deferred) [canonical-hash name])
  =.  deferred-deposit-settlements.hash-state.consumed-with-deferred
    %-  ~(put z-bi deferred-deposit-settlements.hash-state.consumed-with-deferred)
    [as-of.settlement beid.settlement settlement]
  =/  consumed-base  ~(. base-lib consumed-with-deferred)
  =/  consumed-result=(unit bridge-state)
    (reconcile-mainnet-pre-repair-deposit:consumed-base ~)
  ?~  consumed-result
    ~|('expected exact consumed settlement residue to reconcile on startup' !!)
  =/  consumed-state=bridge-state  u.consumed-result
  =/  bad=bridge-state  state
  =/  bad-counterpart=deposit
    counterpart(fee +(fee.counterpart))
  =.  unsettled-deposits.hash-state.bad
    (~(put z-bi unsettled-deposits.hash-state.bad) canonical-hash name bad-counterpart)
  =/  bad-base  ~(. base-lib bad)
  =/  bad-result=(unit bridge-state)
    (reconcile-mainnet-pre-repair-deposit:bad-base ~)
  =/  deferred=bridge-state  state
  =.  deferred-deposit-settlements.hash-state.deferred
    (~(put z-bi deferred-deposit-settlements.hash-state.deferred) as-of.settlement beid.settlement settlement)
  =/  deferred-base  ~(. base-lib deferred)
  =/  deferred-result=(unit bridge-state)
    (reconcile-mainnet-pre-repair-deposit:deferred-base ~)
  ;:  weld
    %+  expect-eq
      !>(before)
    !>(u.before-result)
  ::
    (expect !>(?=(~ no-event-result)))
  ::
    (expect !>(?=(~ consumed-no-event-result)))
  ::
    (expect !>((has-stop-effect start-effects)))
  ::
    (expect !>(stopped))
  ::
    (expect !>(!(~(has z-bi unsettled-deposits.hash-state.reconciled-state) canonical-hash name)))
  ::
    %+  expect-eq
      !>(reconciled-state)
    !>(u.again)
  ::
    %+  expect-eq
      !>(reconciled-state)
    !>(consumed-state)
  ::
    (expect !>(?=(~ bad-result)))
  ::
    (expect !>(?=(~ deferred-result)))
  ==
:::
:::
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
