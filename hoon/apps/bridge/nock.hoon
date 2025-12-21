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
    =?  nock-hold.hash-state.state  ?=(^ nock-hold.hash-state.state)
      =+  nock-hash=(hash:nock-block latest-block)
      ?:  =(nock-hash hash.u.nock-hold.hash-state.state)  ~
      nock-hold.hash-state.state
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
    ~&  eth-sig-requests+eth-sig-requests
    [[%0 %propose-base-call eth-sig-requests]~ state]
  ==
::
::  check if nockchain page belongs to hashchain
++  validate-nockchain-page-sequence
  |=  =page:v1:t
  ^-  (unit @t)
  =/  height  ~(height get:page:t page)
  ?.  =(height.page nock-hashchain-next-height.hash-state.state)
    ~&  %driver-malfunction-received-block-with-height-greater-than-next-height
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
      ::  crash here. there should be no withdrawals from the bridge address until we implement them.
      ::  the crash will get caught by the virtualization in +handle-cause and a %stop event will be
      ::  emitted.
      ~>  %slog.[0 'fatal: withdrawal tx detected, but withdrawals are disabled.']
      !!
    ::  TODO: revisit when its time to implement withdrawals
    ::    produce a withdrawal settlement
    ::  =/  withdraw-info=(unit [recipient=nock-addr name=nname:t amount=@ as-of=base-hash counterpart-base-event-id=base-event-id])
    ::    (extract-withdrawal-info tx)
    ::  ?~  withdraw-info
    ::    ::  just skip it
    ::    $(tx-list t.tx-list)
    ::  =/  w-settle=withdrawal-settlement
    ::    :*  tx-id
    ::        name.u.withdraw-info
    ::        counterpart-base-event-id.u.withdraw-info
    ::        as-of.u.withdraw-info
    ::        recipient.u.withdraw-info
    ::        amount.u.withdraw-info
    ::        ::  TODO: nock-tx-fee
    ::        *@
    ::    ==
    ::  =.  withdrawal-settlements.ret
    ::    (~(put z-by withdrawal-settlements.ret) name.u.withdraw-info w-settle)
    ::  =.  by-tx.ret
    ::    (~(put z-by by-tx.ret) name.u.withdraw-info tx-id)
    ::  $(tx-list t.tx-list)
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
              =(-.name.note.out (first:nname:v1:t bridge-lock-root.state))
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
::    note that withdrawal settlement will contain the amount of tokens burned minus the fee
::    TODO: once withdrawals are implemented, we need to emit holds for withdrawal settlements that we have not
::    processed the corresponding withdrawal for.
++  nockchain-process-withdrawal-settlements
  |=  latest=nock-block
  ^-  process-result
  ?^  withdrawal-settlements.latest
    [%| [%stop 'withdrawal settlement detected but withdrawals are not permitted']]
  [%& state]
::
::  +nockchain-propose-deposits:
::    This arm only gets called if its our turn to propose and there are deposits in the newst nock block.
++  nockchain-propose-deposits
  |=  =nock-block
  ^-  [(list eth-signature-request:effect) bridge-state]
  =+  block-hash=(hash:^nock-block nock-block)
  =^  requests=(list eth-signature-request:effect)  state
    %+  roll
      ~(tap z-by deposits.nock-block)
    |=  $:  [name=nname =deposit]
            [requests=(list eth-signature-request:effect) state=_state]
        ==
    ::  if the recipient is malformed, we keep the funds in the bridge nock address
    ?~  dest.deposit
      [requests state]
    =.  unsettled-deposits.hash-state.state
      (~(del z-bi unsettled-deposits.hash-state.state) block-hash name)
    =.  unconfirmed-settled-deposits.hash-state.state
      (~(put z-bi unconfirmed-settled-deposits.hash-state.state) block-hash name deposit)
    :_  state(next-nonce +(next-nonce.state))
    :_  requests
    ::  NOTE: as-of must be block-hash (hash of nock-block structure), NOT block-id (page digest).
    ::  Deposits are stored in unsettled-deposits keyed by block-hash, so peers must use
    ::  block-hash to look them up during validation.
    :*  tx-id.deposit
        name
        u.dest.deposit
        amount-to-mint.deposit
        height.nock-block
        block-hash
        next-nonce.state
    ==
  ::
  ::  flop requests because they are getting prepended in the +roll and the
  ::  contract requires that deposits are processed in ascending nonce order
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
    =(-.note-name (first:nname:v1:t bridge-lock-root.state))
  =/  output-has-counterpart
    %+  lien  ~(tap z-in outputs.tx)
    |=  out=output:v1:t
    ?>  ?=(@ -.note.out)
    =/  =note-data:t  note-data.note.out
    ::  check for some kind of bridge key that contains as-of (base hash) and counterpart-base-tx-id
    ?>  ?&  (lth %ba-blk p)
            (lth %ba-eid p)
        ==
    ?&  (~(has z-by note-data) %ba-blk)
        (~(has z-by note-data) %ba-eid)
    ==
  ?&(spent-from-bridge output-has-counterpart)
::
++  extract-withdrawal-info
  ::>)  TODO: extract fee
  |=  =tx:t
  ^-  (unit [recipient=nock-addr name=nname:t amount=@ as-of=base-hash counterpart-base-event-id=base-event-id])
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
    ?.  ?&  (~(has z-by note-data) %ba-blk)
            (~(has z-by note-data) %ba-eid)
        ==
      $(outputs-list t.outputs-list)
    `out
  ?~  bridge-output
    ~|  %no-bridge-data-in-tx  !!
  ?>  ?=(@ -.note.u.bridge-output)  :: assert v1 output
  =/  =note-data:t  note-data.note.u.bridge-output
  ::  we already checked that these entries exist in the note data
  =/  base-block-hash  (~(got z-by note-data) %ba-blk)
  =/  base-event-id  (~(got z-by note-data) %ba-eid)
  =/  recipient=nock-addr
    =+  lock-data=(~(got z-by note-data) %lock)
    ?~  soft-lock=((soft spend-condition:t) +.lock-data)
      ~|  %lock-data-malformed-in-tx-output  !!
    ?.  =((lent u.soft-lock) 1)
      ~|  %more-than-one-lock-primitive-in-output-lock  !!
    =+  maybe-pkh=(head u.soft-lock)
    ?.  ?=(%pkh -.maybe-pkh)
      ~|  %lock-in-outputs-not-pkh  !!
    =+  receivers=~(tap z-in h.maybe-pkh)
    ?>  ?&  =(1 (lent receivers))
            =(1 m.maybe-pkh)
        ==
    (head receivers)
  =/  amount-disbursed  assets.note.u.bridge-output
  =/  as-of=(unit base-hash)
    ((soft base-hash) base-block-hash)
  =/  counterpart-base-tx=(unit @)
    ((soft @) base-event-id)
  ?~  as-of
    ~
  ?~  counterpart-base-tx
    ~
  ::  amount sent should be positive
  ?:  (gth amount-disbursed 0)
    `[recipient name.note.u.bridge-output amount-disbursed u.as-of u.counterpart-base-tx]
  ~
--
