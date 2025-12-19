/=  t  /common/tx-engine
/=  *   /common/zeke
/=  *  /common/zoon
/=  *  /common/zose
/=  *  /common/wrapper
/=  *  /apps/bridge/types
/=  dumb  /apps/dumbnet/lib/types
|_  state=bridge-state
++  incoming-base-blocks
  |=  [raw=raw-base-blocks:cause rest=[=wire eny=@ our=@ux now=@da]]
  ^-  [(list effect) bridge-state]
  ~&  %incoming-base-blocks
  ::~&  [%incoming-base-blocks raw rest]
  ::
  ::  hold onto old state in case the deposit process fails
  =/  old-state  state
  =/  stop-info  (get-stop-info old-state)
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
  =+  process-blocks=(process-base-blocks blocks)
  ?-    -.process-blocks
       %|
    =/  =process-fail  +.process-blocks
    ?-    -.process-fail
        %stop
      ::  early stop and roll back to old state if we do not process the base blocks
      ::  this happens when we encounter a %hold or %stop condition.
      [[%0 %stop msg.process-fail stop-info]~ old-state]
    ::
        %hold
      [~ old-state(base-hold.hash-state `hold.process-fail)]
    ==
   ::
       %&
    ::  update state if blocks get processed without stop condition or hold
    =.  state  p.process-blocks
    ::  check if we need to remove the hold
    =?  nock-hold.hash-state.state  ?=(^ nock-hold.hash-state.state)
      ?:  =(blocks-hash hash.u.nock-hold.hash-state.state)  ~
      nock-hold.hash-state.state
    ::
    =/  current-height=@ud  ~(height get:page:t last-block.state)
    ::
    ::  NOTE: This should always be true until we implement withdrawals
    ?:  =(~ withdrawals.blocks)
      [~ state]
    ?~  maybe-proposal=(base-propose-withdrawals blocks)
      [~ state]
    :: TODO: when we implement withdrawals, we will emit a real propose effect
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
        :_  deposit-settlements.ret
        ::  convert base-event-id to blist for z-map compatibility
        %+  ~(put z-by withdrawals.ret)
          (from-atom:blist base-event-id.i.txs)
        :*  (from-atom:blist base-event-id.i.txs)
            lock-root.content.i.txs
            amount-burned=amount.content.i.txs  ::  TODO: what about withdrawal fees on the nock side?
            fee=*coins:t
        ==
      ==
    $(txs t.txs)
  --
::
::  +process-base-blocks:
::    - update hash-state to reflect new base blocks
::    - process unsettled deposits
::
::  returns: [%| effect] if stop condition is hit, otherwise, return [%& state]
::
++  process-base-blocks
  |=  blocks=base-blocks
  ^-  process-result
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
  (base-process-deposit-settlements blocks)
::
::  +base-process-deposit-settlements: confirm the deposits in the latest base block batch
++  base-process-deposit-settlements
  |=  latest-blocks=base-blocks
  ^-  process-result
  =+  settlements=~(tap z-by deposit-settlements.latest-blocks)
  =/  hold  base-hold.hash-state.state
  |-
  ?~  settlements
    ?~  hold  [%& state]
    [%| [%hold u.hold]]
  =/  [event-id=beid settlement=deposit-settlement]
    i.settlements
  =/  [name=nname:t as-of=nock-hash height=@]  [counterpart as-of nock-height]:settlement
  ?.  (lth nonce.settlement next-nonce.state)
    [%| [%stop 'nonce in deposit settlement is not less than next nonce']]
  ?.  (~(has z-by nock-hashchain.hash-state.state) as-of)
   ::  this means that we still have not processed the nockchain deposit tx
   ::  corresponding to the settlement. put a hold on it. if there is already a
   ::  hold, pick the hold with the greatest height.
    %=    $
        settlements
      t.settlements
    ::
        hold
      ?~  hold  `[as-of height]
      ?:  (lte height height.u.hold)  hold
      `[as-of height]
    ==
  ::
  ::  If there are any holds, do not process the settlement
  ?.  =(~ hold)
    $(settlements t.settlements)
  =/  counterpart=deposit
    =+  block-with-deposit=(~(got z-by nock-hashchain.hash-state.state) as-of)
    (~(got z-by deposits.block-with-deposit) name)
  ::
  ::  find the corresponding unsettled deposit in the hash-state.
  ::  we do not require the bridge node to have seen the proposal prior to observing
  ::  the deposit settlement.
  ::    - if bridge node has seen proposal, the deposit will be in the unconfirmed-settled set.
  ::    - if bridge node has not seen proposal, the deposit will be in the unsettled deposit set.
  ::    - if the unsettled deposit is in neither, this is a STOP condition.
  ?.  (has-unsettled-deposit as-of name)
    [%| [%stop 'failed to process deposit settlement: cannot find unsettled deposit in state']]
  ?.  (check-deposit-settlement counterpart settlement)
    [%| [%stop 'failed to process deposit settlement: counterpart does not match settlement']]
  ::
  ::  now that the deposit settled on base, delete it from the tracked state
  ::  we delete from both the unconfirmed and unsettled deposit sets because
  ::  the deposit it could have been in either.
  =.  unconfirmed-settled-deposits.hash-state.state
    (~(del z-bi unconfirmed-settled-deposits.hash-state.state) [as-of name])
  =.  unsettled-deposits.hash-state.state
    (~(del z-bi unsettled-deposits.hash-state.state) [as-of name])
  $(settlements t.settlements)
::
++  has-unsettled-deposit
  |=  [as-of=nock-hash name=nname:t]
  ?|  (~(has z-bi unconfirmed-settled-deposits.hash-state.state) as-of name)
      (~(has z-bi unsettled-deposits.hash-state.state) as-of name)
  ==
::
++  check-deposit-settlement
  |=  $:  counterpart=deposit
          settlement=deposit-settlement
      ==
  =/  dest-matches=?
    ?~  dest.counterpart  %.n
    =(dest.settlement u.dest.counterpart)
  =/  amount-matches=?
    =(amount-to-mint.counterpart settled-amount.settlement)
  ?.  dest-matches
    ~>  %slog.[0 'settlement destination does not match deposit destination']  %.n
  ?.  amount-matches
    ~>  %slog.[0 'settlement amount does not match deposit amount']  %.n
  %.y
::
++  base-propose-withdrawals
  |=  latest-blocks=base-blocks
  ^-  (unit effect)
  ::>)  TODO: when we implement withdrawals, return %nock-proposal request if there are qualified withdrawals
  ~|  %todo  !!
--
