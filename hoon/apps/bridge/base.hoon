/=  t  /common/tx-engine
/=  *   /common/zeke
/=  *  /common/zoon
/=  *  /common/zose
/=  *  /common/wrapper
/=  *  /apps/bridge/types
/=  dumb  /apps/dumbnet/lib/types
~%  %bridge-base  ..ut  ~
|_  state=bridge-state
++  incoming-base-blocks
  ~%  %incoming-base-blocks  ..incoming-base-blocks  ~
  |=  [raw=raw-base-blocks:cause rest=[=wire eny=@ our=@ux now=@da]]
  ^-  [(list effect) bridge-state]
  ~&  %incoming-base-blocks
  ::
  ::  hold onto old state in case the deposit process fails
  =/  old-state  state
  ::
  =/  stop-info  (get-stop-info old-state)
  ?:  !=(~ pending-base-block-commit.hash-state.state)
    [[%0 %stop 'pending base block commit active, not processing incoming base-blocks' stop-info]~ old-state]
  =/  blocks=base-blocks  (cook-base-blocks raw)
  =/  first=@  first-height.blocks
  =/  chunk=@  base-blocks-chunk.constants.state
  =/  start=@  base-start-height.constants.state
  =/  blocks-hash  (hash:base-blocks blocks)
  ?.  =((dec chunk) (sub last-height.blocks first-height.blocks))
    ::>)  This is a stop condition because it means the driver malfunctioned
    ::>)  Batch must be exactly chunk size (last - first == chunk - 1)
    [[%0 %stop 'driver malfunction: incoming base block chunk is not correct size' stop-info]~ old-state]
  ?:  (lth first start)
    ~&  "received base blocks starting at height {<first>}, bridge starts at height {<start>}."
    [~ state]
  ?^  stop=(validate-base-blocks-sequence blocks)
    [[%0 %stop u.stop stop-info]~ old-state]
  ?^  invalid=(validate-deferred-withdrawal-settlements blocks)
    [[%0 %stop u.invalid stop-info]~ old-state]
  =/  withdrawals=(list nock-withdrawal-request:effect)
    (base-propose-withdrawals blocks)
  =/  pending=pending-base-block-withdrawals
    :*  blocks-hash
        first-height.blocks
        last-height.blocks
        withdrawals
    ==
  =/  stage-result=process-result
    (stage-base-blocks blocks pending)
  ?-    -.stage-result
       %|
    =/  =process-fail  +.stage-result
    ?-    -.process-fail
        %stop
      ::  early stop and roll back to old state if we do not process the base blocks
      [[%0 %stop msg.process-fail stop-info]~ old-state]
    ::
        %hold
      [[%0 %stop 'unexpected base hold while staging base blocks' stop-info]~ old-state]
    ==
   ::
       %&
    =.  state  p.stage-result
    [[%0 %base-block-withdrawals-pending pending]~ state]
  ==
::
++  commit-base-block-withdrawals
  |=  ack=base-block-commit-ack
  ^-  [(list effect) bridge-state]
  =/  old-state  state
  =/  stop-info  (get-stop-info old-state)
  =/  maybe-pending=(unit pending-base-block-commit-data)
    pending-base-block-commit.hash-state.state
  ?~  maybe-pending
    [~ old-state]
  =/  pending=pending-base-block-commit-data  u.maybe-pending
  =/  metadata=pending-base-block-withdrawals  metadata.pending
  ?.  =(blocks-hash.ack blocks-hash.metadata)
    [[%0 %stop 'base block withdrawals commit ack hash mismatch' stop-info]~ old-state]
  ?.  =(first-height.ack first-height.metadata)
    [[%0 %stop 'base block withdrawals commit ack first height mismatch' stop-info]~ old-state]
  ?.  =(last-height.ack last-height.metadata)
    [[%0 %stop 'base block withdrawals commit ack last height mismatch' stop-info]~ old-state]
  =/  commit-result=process-result
    (base-process-deposit-settlements blocks.pending)
  ?-    -.commit-result
      %|
    =/  =process-fail  +.commit-result
    ?-    -.process-fail
        %stop
      [[%0 %stop msg.process-fail stop-info]~ old-state]
    ::
        %hold
      [[%0 %stop 'base block withdrawals commit ack hit base hold' stop-info]~ old-state]
    ==
  ::
      %&
    =.  state  p.commit-result
    =.  pending-base-block-commit.hash-state.state  ~
    =.  state  (commit-base-blocks blocks.pending)
    [~ state]
  ==
::
++  validate-base-blocks-sequence
  |=  blocks=base-blocks
  ^-  (unit @t)
  ?.  =(first-height.blocks base-hashchain-next-height.hash-state.state)
    [~ 'driver malfunction: incoming base blocks start height not equal to next height']
  ?:  ?&  (gte base-start-height.constants.state first-height.blocks)
          (lte base-start-height.constants.state last-height.blocks)
      ==
    ~
  =/  last=base-blocks  (~(got z-by base-hashchain.hash-state.state) last-base-blocks.hash-state.state)
  =/  prev=[bid=bbid parent=bbid]
    (last-block:base-blocks last)
  =/  cur=[bid=bbid parent=bbid]
    (first-block:base-blocks blocks)
  =/  next-height=@  +(first-height.blocks)
  |-
  ?.  =(parent.cur bid.prev)
    [~ 'Invalid base block sequence: parent block ID mismatch']
  ?:  =(next-height +(last-height.blocks))
    ~
  %=  $
    prev  cur
    cur  (~(got z-by blocks.blocks) next-height)
    next-height  +(next-height)
  ==
::
::  cook-base-blocks: convert the base events to a usable form
++  cook-base-blocks
  |=  raw=raw-base-blocks:cause
  ^-  base-blocks
  =|  ret=base-blocks
  |^
  ?~  raw
    ret(prev last-base-blocks.hash-state.state)
  =?  first-height.ret  =(first-height.ret 0)
    height.i.raw
  ::  always update last-height to track the highest block in the batch
  =.  last-height.ret  height.i.raw
  =.  blocks.ret  (~(put z-by blocks.ret) height.i.raw [(from-atom:blist block-id.i.raw) (from-atom:blist parent-block-id.i.raw)])
  =/  [withdrawals=(z-map beid withdrawal) deposit-settlements=(z-map beid deposit-settlement)]
    (cook-base-txs txs.i.raw)
  =.  withdrawals.ret          (~(uni z-by withdrawals.ret) withdrawals)
  =.  deposit-settlements.ret  (~(uni z-by deposit-settlements.ret) deposit-settlements)
  $(raw t.raw)
  ::
  ++  cook-base-txs
    |=  txs=(list base-event)
    ^-  [withdrawals=(z-map beid withdrawal) deposit-settlements=(z-map beid deposit-settlement)]
    =|  ret=[withdrawals=(z-map beid withdrawal) deposit-settlements=(z-map beid deposit-settlement)]
    |-
    ?~  txs  ret
    =.  ret
      ?-    +<.i.txs
          %bridge-node-updated  !!  ::  TODO: one day
          %deposit-processed
        :-  withdrawals.ret
        ::  convert base-event-id to blist for z-map compatibility
        %+  ~(put z-by deposit-settlements.ret)
          (from-atom:blist base-event-id.i.txs)
        :*  (from-atom:blist base-event-id.i.txs)
            nock-note-name.content.i.txs
            as-of.content.i.txs
            block-height.content.i.txs
            recipient.content.i.txs
            amount.content.i.txs
            nonce.content.i.txs
        ==
      ::
          %burn-for-withdrawal
        ?:  (lth amount.content.i.txs (mul minimum-event-nocks.constants.state nicks-per-nock:t))
          ret
        :_  deposit-settlements.ret
        ::  convert base-event-id to blist for z-map compatibility
        %+  ~(put z-by withdrawals.ret)
          (from-atom:blist base-event-id.i.txs)
        :*  (from-atom:blist base-event-id.i.txs)
            lock-root.content.i.txs
            amount-burned=amount.content.i.txs  ::  TODO: what about withdrawal fees on the nock side?
        ==
      ==
    $(txs t.txs)
  --
::
::  +stage-base-blocks:
::    - store the pending base batch until Rust commits derived withdrawals
::
::  returns: [%| effect] if stop condition is hit, otherwise, return [%& state]
::
++  stage-base-blocks
  |=  [blocks=base-blocks metadata=pending-base-block-withdrawals]
  ^-  process-result
  =.  pending-base-block-commit.hash-state.state  `[blocks metadata]
  [%& state]
::
::  +commit-base-blocks:
::    - update hash-state to reflect a previously staged base block batch
::
++  commit-base-blocks
  |=  blocks=base-blocks
  ^-  bridge-state
  =/  base-blocks-hash  (hash:base-blocks blocks)
  =.  base-hashchain.hash-state.state
    %+  ~(put z-by base-hashchain.hash-state.state)
      base-blocks-hash
    blocks
  =.  last-base-blocks.hash-state.state  base-blocks-hash
  =.  base-hashchain-next-height.hash-state.state
    %+  add
      base-hashchain-next-height.hash-state.state
    base-blocks-chunk.constants.state
  =?  unsettled-withdrawals.hash-state.state  !=(~ withdrawals.blocks)
    %-  ~(put z-by unsettled-withdrawals.hash-state.state)
    [base-blocks-hash withdrawals.blocks]
  =.  state  (reconcile-deferred-withdrawal-settlements base-blocks-hash)
  state
::
:::  +validate-deferred-withdrawal-settlements:
:::    Validate Nockchain settlements that arrived before this Base batch.
:::    Validation runs before Rust persists proposals, so already-settled
:::    withdrawals are never emitted as new requests.
++  validate-deferred-withdrawal-settlements
  |=  latest-blocks=base-blocks
  ^-  (unit @t)
  =/  as-of=base-hash  (hash:base-blocks latest-blocks)
  ?.  (~(has z-by deferred-withdrawal-settlements.hash-state.state) as-of)
    ~
  =/  settlements=(list [nname:t withdrawal-settlement])
    ~(tap z-by (~(got z-by deferred-withdrawal-settlements.hash-state.state) as-of))
  =/  seen=(z-set beid)  *(z-set beid)
  |-
  ?~  settlements  ~
  =/  [name=nname:t settlement=withdrawal-settlement]  i.settlements
  ?.  =(name nname.settlement)
    [~ 'failed to reconcile withdrawal settlement: note name does not match map key']
  =/  event-id=beid  counterpart.settlement
  ?:  (~(has z-in seen) event-id)
    [~ 'failed to reconcile withdrawal settlement: duplicate counterpart event']
  =/  maybe-counterpart=(unit withdrawal)
    (~(get z-by withdrawals.latest-blocks) event-id)
  ?~  maybe-counterpart
    [~ 'failed to reconcile withdrawal settlement: counterpart event not found in as-of base block']
  ?.  (check-withdrawal-settlement u.maybe-counterpart settlement)
    [~ 'failed to reconcile withdrawal settlement: counterpart does not match settlement']
  $(settlements t.settlements, seen (~(put z-in seen) event-id))
::
++  reconcile-deferred-withdrawal-settlements
  |=  as-of=base-hash
  ^-  bridge-state
  ?.  (~(has z-by deferred-withdrawal-settlements.hash-state.state) as-of)
    state
  =/  settlements=(list [nname:t withdrawal-settlement])
    ~(tap z-by (~(got z-by deferred-withdrawal-settlements.hash-state.state) as-of))
  |-
  ?~  settlements
    =.  deferred-withdrawal-settlements.hash-state.state
      (~(del z-by deferred-withdrawal-settlements.hash-state.state) as-of)
    state
  =/  settlement=withdrawal-settlement  +.i.settlements
  =.  unsettled-withdrawals.hash-state.state
    (~(del z-bi unsettled-withdrawals.hash-state.state) [as-of counterpart.settlement])
  $(settlements t.settlements)
::
:::  +base-process-deposit-settlements: confirm the deposits in the latest base block batch
::
++  base-process-deposit-settlements
  |=  latest-blocks=base-blocks
  ^-  process-result
  =+  settlements=~(tap z-by deposit-settlements.latest-blocks)
  |-
  ?~  settlements
    [%& state]
  =/  [event-id=beid settlement=deposit-settlement]
    i.settlements
  =/  [name=nname:t as-of=nock-hash]  [counterpart as-of]:settlement
  ?.  (~(has z-by nock-hashchain.hash-state.state) as-of)
    =.  deferred-deposit-settlements.hash-state.state
      %-  ~(put z-bi deferred-deposit-settlements.hash-state.state)
      [as-of event-id settlement]
    $(settlements t.settlements)
  =/  counterpart=deposit
    =+  block-with-deposit=(~(got z-by nock-hashchain.hash-state.state) as-of)
    (~(got z-by deposits.block-with-deposit) name)
  ::
  ::  find the corresponding unsettled deposit in the hash-state.
  ::  we do not require the bridge node to have seen the proposal prior to observing
  ::  the deposit settlement.
  ::    - if bridge node has seen proposal, the deposit will be in the unsettled deposit set.
  ::    - if the unsettled deposit is not the unsettled deposit set, this is a STOP condition.
  ?.  (has-unsettled-deposit as-of name)
    [%| [%stop 'failed to process deposit settlement: cannot find unsettled deposit in state']]
  ?.  (check-deposit-settlement counterpart settlement)
    [%| [%stop 'failed to process deposit settlement: counterpart does not match settlement']]
  ::
  ::  now that the deposit settled on base, delete it from the tracked state
  =.  unsettled-deposits.hash-state.state
    (~(del z-bi unsettled-deposits.hash-state.state) [as-of name])
  $(settlements t.settlements)
::
++  has-unsettled-deposit
  |=  [as-of=nock-hash name=nname:t]
  (~(has z-bi unsettled-deposits.hash-state.state) as-of name)
::
::
:::  JOIE: when proposing withdrawals, attach height at the end of the batch window to the note-data
++  base-propose-withdrawals
  |=  latest-blocks=base-blocks
  ^-  (list nock-withdrawal-request:effect)
  =/  base-hash  (hash:base-blocks latest-blocks)
  %+  murn  ~(tap z-by withdrawals.latest-blocks)
  |=  [=beid =withdrawal]
  ?:  (has-deferred-withdrawal-settlement base-hash beid)
    ~
  %-  some
  :*  (to-atom:blist beid)
      dest.withdrawal
      amount-burned.withdrawal
      last-height.latest-blocks
      base-hash
  ==
::
++  has-deferred-withdrawal-settlement
  |=  [as-of=base-hash =beid]
  ^-  ?
  ?.  (~(has z-by deferred-withdrawal-settlements.hash-state.state) as-of)
    %.n
  %+  lien
    ~(tap z-by (~(got z-by deferred-withdrawal-settlements.hash-state.state) as-of))
  |=  [name=nname:t settlement=withdrawal-settlement]
  =(beid counterpart.settlement)
--
