/=  t  /common/tx-engine
/=  *   /common/zeke
/=  *  /common/zoon
/=  *  /common/zose
/=  *  /common/wrapper
/=  *  /apps/bridge/types
/=  dumb  /apps/dumbnet/lib/types
|_  state=bridge-state
++  incoming-nockchain-block
  |=  [nockchain-block=nockchain-block:cause rest=[=wire eny=@ our=@ux now=@da]]
  ^-  [(list effect) bridge-state]
  ~&  %incoming-nockchain
  ::~&  [%incoming-nockchain-block rest]
  ~|  %txs-provided-check
  ::  save old-state in case we need to revert after an error
  =/  old-state  state
  ::
  ::  avoiding ?^ because it gives too much information to compiler about the shape of base-hold
  ::  if there is a hold, do not process
  ?:  !=(~ nock-hold.hash-state.state)
    ~>  %slog.[0 'nock hold active, not processing incoming nockchain-block']
    [~ old-state]
  =/  stop-info  (get-stop-info old-state)
  ?.  ?=(%1 -.block.nockchain-block)
    ~>  %slog.[0 'ignoring v0 block, bridge starts after v0 cutover']
    [~ state]
  ?:  !=(tx-ids.block.nockchain-block ~(key z-by txs.nockchain-block))
    [[%0 %stop 'tx-ids mismatch txs in nockchain block' stop-info]~ old-state]
  =/  block-height=@  height.block.nockchain-block
  =/  start=@  nockchain-start-height.constants.state
  ?:  (lth block-height start)
    ~&  "received nockchain block at height {<block-height>}, bridge starts at height {<start>}."
    [~ old-state]
  ?^  stop=(validate-nockchain-page-sequence block.nockchain-block)
    [[%0 %stop u.stop stop-info]~ old-state]
  =/  [latest-block=nock-block process-block=process-result]
    (process-nockchain-block block.nockchain-block txs.nockchain-block)
  ?-    -.process-block
      %|
    =/  =process-fail  +.process-block
    ?-  -.process-fail
      %stop  [[%0 %stop msg.process-fail stop-info]~ old-state]
      %hold  [~ old-state(nock-hold.hash-state `hold.process-fail)]
    ==
  ::
      %&
    ::  if process block was successful, update state and carry on
    =.  state  p.process-block
    =?  base-hold.hash-state.state  ?=(^ base-hold.hash-state.state)
      =+  nock-hash=(hash:nock-block latest-block)
      ?:  =(nock-hash hash.u.base-hold.hash-state.state)  ~
      base-hold.hash-state.state
    =/  current-height=@ud  ~(height get:page:t last-block.state)
    ::
    ::  If there are no signature requests, we will not submit a proposal.
    ::  Note that even blocks with deposits could result in no signature requests
    ::  because the deposits may be issued to malformed evm addresses.
    ::
    ::  Base recipient addresses are represented as (unit base-addr) where the null
    ::  case represents a malformed address.
    ::
    ::  If any deposit is issued to a malformed address, we do not process it.
    ::  We instead keep the deposited funds in the bridge address.
    ::
    =^  eth-sig-requests  state
      (nockchain-propose-deposits latest-block)
    ?~  eth-sig-requests
      [~ state]
    =/  deposit-effects=(list effect)
      ~[[%0 %commit-nock-deposits eth-sig-requests]]
    ~&  eth-sig-requests+eth-sig-requests
    [deposit-effects state]
  ==
::
::  check if nockchain page belongs to hashchain
++  validate-nockchain-page-sequence
  |=  =page:v1:t
  ^-  (unit @t)
  =/  height  ~(height get:page:t page)
  ?.  =(height.page nock-hashchain-next-height.hash-state.state)
    ~&  %driver-malfunction-received-block-with-height-greater-than-next-height
    ~&  [received+height.page expected+nock-hashchain-next-height.hash-state.state]
     [~ 'received block with height not equal to next height']
  ?:  =(height.page nockchain-start-height.constants.state)
    ~
  =/  last-nock-block
    (~(got z-by nock-hashchain.hash-state.state) last-nock-block.hash-state.state)
  ::
  ::  This condition should never ever trigger if the state machine is working correctly
  ?.  =(height.last-nock-block (dec nock-hashchain-next-height.hash-state.state))
    ~&  %fatal-last-nock-block-is-not-decrement-of-next-nock-hashchain-height
    [~ 'fatal: height of last block in hashchain is not (next-height - 1)']
  ?.  =(block-id.last-nock-block parent.page)
    [~ 'hashchain reorg: parent of incoming block is not the last block in the hashchain']
  ~
::
++  process-nockchain-block
  |=  [block=page:t txs=(z-map tx-id:t tx:t)]
  ^-  [nock-block process-result]
  |^
  ?:  ?=(^ -.block)
    ::  we should not be processing blocks that were mined prior to the bridge cutover.
    ~|  %v0-block-received  !!
  =+  [deposits withdrawal-settlements]=process-nock-txs
  =/  nock-blk=nock-block
    :*  %nock
        %0
        height.block
        digest.block
        deposits
        withdrawal-settlements
        ::  if it's the first block in the hash chain, prev will point to [0x0 0x0 0x0 0x0 0x0]
        ::  this is okay.
        prev=last-nock-block.hash-state.state
    ==
  =/  nock-blk-hash  (hash:nock-block nock-blk)
  =.  last-block.state  block
  =.  nock-hashchain.hash-state.state
    %+  ~(put z-by nock-hashchain.hash-state.state)
      nock-blk-hash
    nock-blk
  =.  last-nock-block.hash-state.state  nock-blk-hash
  =.  nock-hashchain-next-height.hash-state.state
    +(nock-hashchain-next-height.hash-state.state)
  =.  hash-state.state
    %+  roll
      ~(tap z-by deposits.nock-blk)
    |=  [[name=nname:t =deposit] hash-state=_hash-state.state]
    =.  unsettled-deposits.hash-state
      %-  ~(put z-bi unsettled-deposits.hash-state)
      [nock-blk-hash name deposit]
    hash-state
  =?  last-nock-deposit-height.state  !=(~ deposits.nock-blk)
    height.nock-blk
  [nock-blk (nockchain-process-withdrawal-settlements nock-blk)]
  ::
  ++  process-nock-txs
    ^-  [deposits=(z-map nname deposit) withdrawal-settlements=(z-map nname withdrawal-settlement)]
    =/  tx-list  ~(tap z-by txs)
    =|  ret=[deposits=(z-map nname deposit) withdrawal-settlements=(z-map nname withdrawal-settlement)]
    |-
    ?~  tx-list  ret
    =*  tx-id  p.i.tx-list
    =*  tx    q.i.tx-list
    ?:  (is-bridge-deposit-tx tx)
      ::  produce a deposit
      ::
      ~&  bridge-deposit-detected+tx
      =/  maybe-intent=(unit deposit-intent)
        (extract-deposit-intent tx)
      ~&  maybe-intent+maybe-intent
      ?~  maybe-intent
        $(tx-list t.tx-list)
      =.  deposits.ret
        (~(put z-by deposits.ret) name.u.maybe-intent [tx-id [name recipient amount-to-mint fee]:u.maybe-intent])
      $(tx-list t.tx-list)
    ?:  (is-bridge-withdrawal-tx tx)
      =/  withdraw-info=(unit [recipient=nock-lock-root name=nname:t amount=@ base-batch-end=@ as-of=base-hash counterpart=beid])
        (extract-withdrawal-info tx)
      ?~  withdraw-info
          ::  just skip it
        $(tx-list t.tx-list)
      =/  w-settle=withdrawal-settlement
        :*  tx-id
            name.u.withdraw-info
            counterpart.u.withdraw-info
            base-batch-end.u.withdraw-info
            as-of.u.withdraw-info
            recipient.u.withdraw-info
            amount.u.withdraw-info
        ==
      =.  withdrawal-settlements.ret
        (~(put z-by withdrawal-settlements.ret) name.u.withdraw-info w-settle)
      $(tx-list t.tx-list)
    $(tx-list t.tx-list)
  ::
  ::    +is-bridge-deposit-tx: detect bridge transactions
  ::
  ::  returns %.y if a transaction is a bridge deposit. checks that
  ::  the transaction is v1 and has %bridge field in note-data of
  ::  at least one output. the %bridge field contains [%base (list belt)]
  ::  where the list is the based representation of the evm recipient.
  ::
  ++  is-bridge-deposit-tx
    |=  =tx:t
    ^-  ?
    ?.  ?=(%1 -.tx)  %.n
    %+  lien  ~(tap z-in outputs.tx)
    |=  out=output:v1:t
    ?>  ?=(@ -.note.out)
    =/  =note-data:t  note-data.note.out
    (~(has z-by note-data) %bridge)
  ::
  ::    +extract-deposit-intent: parse bridge transaction data
  ::
  ::  extracts the recipient evm address and amount from a bridge
  ::  deposit transaction. searches outputs for %bridge field
  ::  containing [%0 %base evm-address-based], converts the based address
  ::  to raw evm format, and calculates total amount from spends.
  ::  returns ~ if the tx output doesn't go to the proper address or
  ::  the note-data doesn't have a %bridge entry.
  ::
  ++  extract-deposit-intent
    |=  =tx:t
    ^-  (unit deposit-intent)
    ?>  ?=(%1 -.tx)
    =/  bridge-output=(unit output:v1:t)
      =/  outputs-list=(list output:v1:t)
        ~(tap z-in outputs.tx)
      |-  ^-  (unit output:v1:t)
      ::  if there is no match, return ~
      ?~  outputs-list  ~
      =/  out=output:v1:t  i.outputs-list
      ?.  ?=(@ -.note.out)
        $(outputs-list t.outputs-list)
      ~&  output-note+note.out
      =/  =note-data:t  note-data.note.out
      ?:  (lth assets.note.out (mul minimum-event-nocks.constants.state nicks-per-nock:t))
        ~>  %slog.[0 'deposit-does-not-meet-minimum-requirement']
        $(outputs-list t.outputs-list)
      ?:  ?&  (~(has z-by note-data) %bridge)
              =(-.name.note.out (first:nname:v1:t bridge-lock-root.config.state))
          ==
        `out
      $(outputs-list t.outputs-list)
    ?~  bridge-output
      ~>  %slog.[0 'bridge data output note first name does not match bridge-lock-root first name']
      ~
    ~&  bridge-output+bridge-output
    ?>  ?=(@ -.note.u.bridge-output)  :: assert v1 output
    =/  =note-data:t  note-data.note.u.bridge-output
    ::  we already checked that the %bridge entry exists in the note data
    =/  bridge-data  (~(got z-by note-data) %bridge)
    ::  NOTE: the whole bridge will crash if someone puts a faulty bridge
    ::  note-data together without mole virtualizing the recipient processing.
    ::  validate bridge data format: [%0 %base evm-address-based]
    =/  recipient=(unit evm-address)
      %-  mole
      |.
      =+  deposit-data=;;(bridge-deposit-data bridge-data)
      ::  convert from based representation to raw EVM address
      (based-to-evm-address addr.deposit-data)
    ?~  recipient
      ~>  %slog.[0 'Encountered malformed evm recipient address. Deposited nocks will remain in bridge nockchain wallet.']
      ~
    ~&  recipient+recipient
    =/  deposit-total  assets.note.u.bridge-output
    ::
    =/  deposit-fee=@  (calculate:bridge-fee deposit-total nicks-fee-per-nock.constants.state)
    =/  amount-to-mint=@
      (sub deposit-total deposit-fee)
    ::  amount that we are minting as a result of this deposit should be positive
    ?:  (gth amount-to-mint 0)
      `[name.note.u.bridge-output recipient amount-to-mint deposit-fee]
    ~
  --
::
::  +nockchain-process-withdrawal-settlements:
::    processes unsettled withdrawals in new nockchain block
::    unsettled withdrawals track the gross/pre-fee amount burned on Base,
::    while settlements carry the net/post-fee amount disbursed on Nockchain.
::    kernel reconciliation only enforces identity and basic amount bounds.
::    TODO: once withdrawals are implemented, we need to emit holds for withdrawal settlements that we have not
::    processed the corresponding withdrawal for.
++  nockchain-process-withdrawal-settlements
  |=  latest=nock-block
  ^-  process-result
  =/  settlements  ~(tap z-by withdrawal-settlements.latest)
  =/  hold  nock-hold.hash-state.state
  |-
  ?~  settlements
    ?~  hold  [%& state]
    [%| [%hold u.hold]]
  =/  [name=nname:t settlement=withdrawal-settlement]
    i.settlements
  =/  [=beid as-of=base-hash height=@]  [counterpart as-of base-batch-end]:settlement
  ?.  (~(has z-by base-hashchain.hash-state.state) as-of)
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
  ::  If there is a hold, do not process the settlement
  ?.  =(~ hold)
    $(settlements t.settlements)
  ::
  ::  find the corresponding unsettled withdrawal in the hash-state.
  ::  we do not require the bridge node to have seen the proposal prior to observing
  ::  the withdrawal settlement.
  ::    - if bridge node has seen proposal, the withdrawal will be in the unsettled withdrawal set.
  ::    - if the unsettled deposit is not the unsettled deposit set, this is a STOP condition.
  ?.  (has-unsettled-withdrawal as-of beid)
    [%| [%stop 'failed to process withdrawal settlement: cannot find unsettled withdrawal in state']]
  =+  block-with-withdrawal=(~(got z-by base-hashchain.hash-state.state) as-of)
  =/  maybe-counterpart=(unit withdrawal)
    (~(get z-by withdrawals.block-with-withdrawal) beid)
  ?~  maybe-counterpart
    [%| [%stop 'failed to process withdrawal settlement: counterpart event not found in as-of base block']]
  =/  counterpart=withdrawal
    u.maybe-counterpart
  ?.  (check-withdrawal-settlement counterpart settlement)
    [%| [%stop 'failed to process withdrawal settlement: counterpart does not match settlement']]
  ::
  ::  now that the withdrawal settled on nock, delete it from the tracked state
  =.  unsettled-withdrawals.hash-state.state
    (~(del z-bi unsettled-withdrawals.hash-state.state) [as-of beid])
  $(settlements t.settlements)
::
++  has-unsettled-withdrawal
  |=  [as-of=base-hash =beid]
  (~(has z-bi unsettled-withdrawals.hash-state.state) as-of beid)
::
++  check-withdrawal-settlement
  |=  $:  counterpart=withdrawal
          settlement=withdrawal-settlement
      ==
  =/  dest-matches=?
    =(dest.settlement dest.counterpart)
  ::  counterpart tracks the gross/pre-fee burn amount, while settlement
  ::  carries the net/post-fee disbursed amount. exact fee correctness is
  ::  validated in Rust proposal acceptance, so kernel only enforces bounds.
  =/  amount-in-bounds=?
    ?&  (gth settled-amount.settlement 0)
        (lth settled-amount.settlement amount-burned.counterpart)
    ==
  ?.  dest-matches
    ~>  %slog.[0 'settlement destination does not match withdrawal destination']  %.n
  ?.  amount-in-bounds
    ~>  %slog.[0 'settlement amount is out of bounds for withdrawal']  %.n
  %.y
::
::  +nockchain-propose-deposits:
::    This arm only gets called if its our turn to propose and there are deposits in the newst nock block.
++  nockchain-propose-deposits
  |=  =nock-block
  ^-  [(list nock-deposit-request:effect) bridge-state]
  =+  block-hash=(hash:^nock-block nock-block)
  =/  requests=(list nock-deposit-request:effect)
    %+  murn
      ~(tap z-by deposits.nock-block)
    |=  [name=nname =deposit]
    ::  if the recipient is malformed, we keep the funds in the bridge nock address
    ?~  dest.deposit  ~
    ::  NOTE: as-of must be block-hash (hash of nock-block structure), NOT block-id (page digest).
    ::  Deposits are stored in unsettled-deposits keyed by block-hash, so peers must use
    ::  block-hash to look them up during validation.
    %-  some
    :*  tx-id.deposit
        name
        u.dest.deposit
        amount-to-mint.deposit
        height.nock-block
        block-hash
    ==
  ::
  ::  flop requests because they are getting prepended in the +roll
  [(flop requests) state]
::
++  is-bridge-withdrawal-tx
  |=  =tx:t
  ^-  ?
  ?.  ?=(%1 -.tx)  %.n
  =/  spent-from-bridge
    %+  levy  ~(tap z-by spends.raw-tx.tx)
    |=  [note-name=nname:t spend=spend-v1:t]
    ^-  ?
    ::  NOTE: must be spent from bridge
    =(-.note-name (first:nname:v1:t bridge-lock-root.config.state))
  =/  output-has-counterpart
    %+  lien  ~(tap z-in outputs.tx)
    |=  out=output:v1:t
    ?>  ?=(@ -.note.out)
    =/  =note-data:t  note-data.note.out
    ::  check for packed withdrawal metadata key.
    ?>  (lth %bridge-w p)
    (~(has z-by note-data) %bridge-w)
  ?&(spent-from-bridge output-has-counterpart)
::
++  extract-withdrawal-info
  |=  =tx:t
  ^-  (unit [recipient=nock-lock-root name=nname:t amount=@ base-batch-end=@ as-of=base-hash counterpart=beid])
  ?>  ?=(%1 -.tx)
  =/  bridge-output=(unit output:v1:t)
    =/  outputs-list=(list output:v1:t)
      ~(tap z-in outputs.tx)
    |-  ^-  (unit output:v1:t)
    ?~  outputs-list  ~
    =/  out=output:v1:t  i.outputs-list
    ?.  ?=(@ -.note.out)
      $(outputs-list t.outputs-list)
    =/  =note-data:t  note-data.note.out
    ?.  (~(has z-by note-data) %bridge-w)
      $(outputs-list t.outputs-list)
    `out
  ?~  bridge-output
    ~
  ?>  ?=(@ -.note.u.bridge-output)  :: assert v1 output
  =/  =note-data:t  note-data.note.u.bridge-output
  ::  we already checked that these entries exist in the note data
  =/  withdraw-info  ((soft withdraw-info) (~(got z-by note-data) %bridge-w))
  ?~  withdraw-info
    ~&  'withdraw note data malformed'  ~
  =/  base-block-hash=base-hash  base-hash.u.withdraw-info
  =/  =beid  beid.u.withdraw-info
  =/  base-batch-end=@  base-batch-end.u.withdraw-info
  =/  recipient=nock-lock-root  lock-root.u.withdraw-info
  =/  amount-disbursed  assets.note.u.bridge-output
  ::  amount sent should be positive
  ?:  (gth amount-disbursed 0)
    `[recipient name.note.u.bridge-output amount-disbursed base-batch-end base-block-hash beid]
  ~
--
