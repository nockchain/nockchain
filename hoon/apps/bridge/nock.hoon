/=  t  /common/tx-engine
/=  *   /common/zeke
/=  *  /common/zoon
/=  *  /common/zose
/=  *  /common/wrapper
/=  *  /apps/bridge/types
/=  dumb  /apps/dumbnet/lib/types
~%  %bridge-nock  ..ut  ~
|_  state=bridge-state
++  incoming-nockchain-block
  ~%  %incoming-nockchain-block  ..incoming-nockchain-block  ~
  |=  [nockchain-block=nockchain-block:cause rest=[=wire eny=@ our=@ux now=@da]]
  ^-  [(list effect) bridge-state]
  ~&  %incoming-nockchain
  ::~&  [%incoming-nockchain-block rest]
  ~|  %txs-provided-check
  ::  save old-state in case we need to revert after an error
  =/  old-state  state
  ::
  ?:  !=(~ pending-base-block-commit.hash-state.state)
    ~>  %slog.[0 'base block commit pending, not processing incoming nockchain-block']
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
        %stop
      [[%0 %stop msg.process-fail stop-info]~ old-state]
    ::
        %hold
      [[%0 %stop 'unexpected nock hold while processing nockchain block' stop-info]~ old-state]
    ==
  ::
      %&
    ::  if process block was successful, update state and carry on
    =.  state  p.process-block
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
::  Mainnet nonce 14 settled a 100,000,000-nick deposit from block 46,849
::  before the minimum became 100,000 NOCK. Preserve that canonical block
::  content when replaying or repairing the recursive Nock hashchain.
++  restore-mainnet-legacy-deposit
  |=  block=nock-block
  ^-  nock-block
  =/  expected-block-id=block-id:t
    [0xea58.5f21.dd2b.1c45 0xa800.c0cb.33d7.31e1 0x74d7.9cc6.c9ae.2c02 0x29c.34b8.66c4.de58 0xeac3.e1ca.0329.b3fb]
  =/  name=nname:t  mainnet-legacy-deposit-name
  ?.  ?&  =(46.849 height.block)
           =(expected-block-id block-id.block)
           =(46.810 nockchain-start-height.constants.state)
           =(-.name (first:nname:v1:t bridge-lock-root.config.state))
       ==
    block
  ?:  (~(has z-by deposits.block) name)
    block
  =/  recipient=base-addr  0x4be0.28f3.ed83.7add.fcb5.233f.0af7.6b61.5947.e5b4
  =/  legacy=deposit
    :*  [0x62a3.2805.7e94.5a0a 0xdff6.74b9.94a9.563e 0xb432.72d6.4a88.e1e4 0x7df8.12ab.a7c3.9a6a 0x7879.dcf7.13da.5670]
        name
        `recipient
        99.702.430
        297.570
    ==
  block(deposits (~(put z-by deposits.block) name legacy))
::
::
++  repair-stale-base-hold
  |=  ~
  ^-  (unit bridge-state)
  =/  maybe-hold  base-hold.hash-state.state
  ?~  maybe-hold  ~
  =/  hold=[hash=nock-hash height=@]  u.maybe-hold
  =/  old-chain  nock-hashchain.hash-state.state
  ?:  (~(has z-by old-chain) hash.hold)
    `state(base-hold.hash-state ~)
  ?.  (lth height.hold nock-hashchain-next-height.hash-state.state)
    ~
  =/  maybe-entries
    ^-  (unit (list [nock-hash nock-block]))
    =/  cursor=nock-hash  last-nock-block.hash-state.state
    =/  chronological=(list [nock-hash nock-block])  ~
    |-
    ?:  =(cursor *nock-hash)
      `chronological
    =/  maybe-block  (~(get z-by old-chain) cursor)
    ?~  maybe-block  ~
    $(cursor prev.u.maybe-block, chronological [[cursor u.maybe-block] chronological])
  ?~  maybe-entries  ~
  =/  rebuilt
    ^-  [new-chain=(z-map nock-hash nock-block) old-to-new=(z-map nock-hash nock-hash) new-last=nock-hash added-deposits=(z-mip nock-hash nname:t deposit)]
    =/  entries=(list [nock-hash nock-block])  u.maybe-entries
    =/  new-chain=(z-map nock-hash nock-block)
      *(z-map nock-hash nock-block)
    =/  old-to-new=(z-map nock-hash nock-hash)
      *(z-map nock-hash nock-hash)
    =/  new-last=nock-hash  *nock-hash
    =/  added-deposits=(z-mip nock-hash nname:t deposit)
      *(z-mip nock-hash nname:t deposit)
    |-
    ?~  entries  [new-chain old-to-new new-last added-deposits]
    =/  old-hash=nock-hash  -.i.entries
    =/  relinked=nock-block  +.i.entries(prev new-last)
    =/  rebuilt-block=nock-block
      (restore-mainnet-legacy-deposit relinked)
    =/  new-hash=nock-hash  (hash:nock-block rebuilt-block)
    =/  original-block=nock-block  +.i.entries
    =/  preserve-old=?
      ?&  =(old-hash (hash:nock-block original-block))
          ?|  =(46.849 height.original-block)
              (~(has z-by deferred-deposit-settlements.hash-state.state) old-hash)
              %+  lien  ~(tap z-by deposits.original-block)
              |=  [name=nname:t ignored=deposit]
              (~(has z-bi unsettled-deposits.hash-state.state) old-hash name)
          ==
      ==
    =?  new-chain  preserve-old
      (~(put z-by new-chain) old-hash original-block)
    =/  newly-added=(z-map nname:t deposit)
      (~(dif z-by deposits.rebuilt-block) deposits.relinked)
    =.  added-deposits
      %+  roll  ~(tap z-by newly-added)
      |=  [[name=nname:t =deposit] acc=_added-deposits]
      (~(put z-bi acc) new-hash name deposit)
    $(entries t.entries, new-chain (~(put z-by new-chain) new-hash rebuilt-block), old-to-new (~(put z-by old-to-new) old-hash new-hash), new-last new-hash, added-deposits added-deposits)
  ?.  (~(has z-by new-chain.rebuilt) hash.hold)
    ~
  =/  maybe-unsettled
    ^-  (unit (z-mip nock-hash nname:t deposit))
    =/  entries=(list [nock-hash [nname:t deposit]])
      ~(tap z-bi unsettled-deposits.hash-state.state)
    =/  new-unsettled=(z-mip nock-hash nname:t deposit)
      added-deposits.rebuilt
    |-
    ?~  entries  `new-unsettled
    =/  old-hash=nock-hash  -.i.entries
    =/  maybe-new-hash  (~(get z-by old-to-new.rebuilt) old-hash)
    ?~  maybe-new-hash  ~
    =/  name=nname:t  -.+.i.entries
    =/  =deposit  +.+.i.entries
    $(entries t.entries, new-unsettled (~(put z-bi new-unsettled) u.maybe-new-hash name deposit))
  ?~  maybe-unsettled  ~
  =.  nock-hashchain.hash-state.state  new-chain.rebuilt
  =.  last-nock-block.hash-state.state  new-last.rebuilt
  =.  unsettled-deposits.hash-state.state  u.maybe-unsettled
  =.  base-hold.hash-state.state  ~
  `state
::
::
::  check if nockchain page belongs to hashchain
++  validate-nockchain-page-sequence
  ~%  %validate-nockchain-page-sequence  ..validate-nockchain-page-sequence  ~
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
  ~%  %process-nockchain-block  ..process-nockchain-block  ~
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
  =.  nock-blk  (restore-mainnet-legacy-deposit nock-blk)
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
  =/  deferred-result=process-result
    (nockchain-process-deferred-deposit-settlements nock-blk)
  ?-  -.deferred-result
      %|  [nock-blk deferred-result]
      %&
    =.  state  p.deferred-result
    [nock-blk (nockchain-process-withdrawal-settlements nock-blk)]
  ==
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
      ~&  bridge-deposit-detected+tx-id
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
++  unsettled-deposits-by-counterpart
  |=  name=nname:t
  ^-  (list [nock-hash deposit])
  %+  murn
    ~(tap z-bi unsettled-deposits.hash-state.state)
  |=  [tracked-as-of=nock-hash [tracked-name=nname:t tracked=deposit]]
  ?.  =(name tracked-name)
    ~
  `[tracked-as-of tracked]
::
::  Resolve the finite mainnet event range finalized against the pre-repair
::  lineage. All value-bearing fields still bind to one canonical counterpart.
++  mainnet-pre-repair-lineage-source-block
  |=  [settlement=deposit-settlement latest=nock-block]
  ^-  (unit nock-block)
  ?.  (mainnet-pre-repair-lineage-settlement constants.state settlement)
    ~
  =/  name=nname:t  counterpart.settlement
  =/  tracked=(list [nock-hash deposit])
    (unsettled-deposits-by-counterpart name)
  ?~  tracked  ~
  ?^  t.tracked  ~
  =/  tracked-as-of=nock-hash  -.i.tracked
  =/  current-as-of=nock-hash  (hash:nock-block latest)
  =/  maybe-block=(unit nock-block)
    ?:  =(tracked-as-of current-as-of)
      `latest
    (~(get z-by nock-hashchain.hash-state.state) tracked-as-of)
  ?~  maybe-block  ~
  =/  block=nock-block  u.maybe-block
  ?.  =(tracked-as-of (hash:nock-block block))
    ~
  ?.  =(nock-height.settlement height.block)
    ~
  =/  maybe-counterpart=(unit deposit)
    (~(get z-by deposits.block) name)
  ?~  maybe-counterpart  ~
  ?.  =(+.i.tracked u.maybe-counterpart)
    ~
  ?.  (check-deposit-settlement +.i.tracked settlement)
    ~
  `block
:::
++  mainnet-finalized-orphaned-lineage-settlement
  |=  settlement=deposit-settlement
  ^-  ?
  =/  bridge-root=hash:t
    [0xf480.0376.e5c6.138d 0x9a4c.e7c6.94db.95f1 0x6c18.a134.f480.fde0 0xbe1c.4b92.e6d4.61d0 0x6c6d.671d.8d73.ef3b]
  ?&  =(46.810 nockchain-start-height.constants.state)
      =(39.694.000 base-start-height.constants.state)
      =(bridge-root -.counterpart.settlement)
      (gte nonce.settlement 14)
      (lte nonce.settlement 529)
      (gte nock-height.settlement 46.849)
      (lte nock-height.settlement 146.586)
  ==
:::
++  mainnet-pre-repair-orphaned-lineage-settlement
  |=  [settlement=deposit-settlement tracked=(list [nock-hash deposit])]
  ^-  ?
  ?.  (mainnet-finalized-orphaned-lineage-settlement settlement)
    %.n
  ?~  tracked  %.n
  =/  expected=deposit  +.i.tracked
  ?.  %+  levy  `(list [nock-hash deposit])`tracked
      |=  [ignored=nock-hash candidate=deposit]
      =(expected candidate)
    %.n
  (check-deposit-settlement expected settlement)
::
++  deferred-deposit-settlement-source-block
  |=  [settlement=deposit-settlement latest=nock-block]
  ^-  (unit nock-block)
  =/  current-as-of=nock-hash  (hash:nock-block latest)
  ?:  =(as-of.settlement current-as-of)
    `latest
  =/  direct=(unit nock-block)
    (~(get z-by nock-hashchain.hash-state.state) as-of.settlement)
  ?^  direct  direct
  (mainnet-pre-repair-lineage-source-block settlement latest)
::
:::  +validate-deferred-deposit-settlements:
:::    Deferred Base settlements are future dependencies. They must remain
:::    globally unique by Nock note and bind to the exact source block.
++  validate-deferred-deposit-settlements
  |=  latest=nock-block
  ^-  (unit @t)
  =/  current-height=@  height.latest
  =/  current-as-of=nock-hash  (hash:nock-block latest)
  =/  settlements=(list [nock-hash [beid deposit-settlement]])
    ~(tap z-bi deferred-deposit-settlements.hash-state.state)
  =/  seen=(z-set nname:t)  *(z-set nname:t)
  |-
  ?~  settlements  ~
  =/  [as-of=nock-hash [event-id=beid settlement=deposit-settlement]]
    i.settlements
  ?.  =(event-id beid.settlement)
    [~ 'failed to reconcile deposit settlement: event id does not match map key']
  ?.  =(as-of as-of.settlement)
    [~ 'failed to reconcile deposit settlement: as-of hash does not match map key']
  =/  name=nname:t  counterpart.settlement
  ?:  (~(has z-in seen) name)
    [~ 'failed to reconcile deposit settlement: duplicate counterpart note']
  =/  next-seen=(z-set nname:t)  (~(put z-in seen) name)
  =/  tracked=(list [nock-hash deposit])
    (unsettled-deposits-by-counterpart name)
  =/  maybe-block=(unit nock-block)
    (deferred-deposit-settlement-source-block settlement latest)
  ?~  maybe-block
    ?:  (mainnet-pre-repair-orphaned-lineage-settlement settlement tracked)
      $(settlements t.settlements, seen next-seen)
    ~&  [%unreconciled-mainnet-lineage (mainnet-finalized-orphaned-lineage-settlement settlement) (lent tracked) settlement tracked]
    ?^  tracked
      [~ 'failed to reconcile deposit settlement: counterpart note is tracked under an unknown as-of hash']
    ?:  (~(has z-by deposits.latest) name)
      [~ 'failed to reconcile deposit settlement: counterpart note found under a different as-of hash']
    ?:  (lte nock-height.settlement current-height)
      [~ 'failed to reconcile deposit settlement: unknown as-of hash is behind the Nockchain cursor']
    $(settlements t.settlements, seen next-seen)
  =/  block=nock-block  u.maybe-block
  ?.  =(nock-height.settlement height.block)
    [~ 'failed to reconcile deposit settlement: Nockchain height does not match as-of block']
  =/  maybe-counterpart=(unit deposit)
    (~(get z-by deposits.block) name)
  ?~  maybe-counterpart
    [~ 'failed to reconcile deposit settlement: counterpart note not found in as-of nock block']
  ?~  tracked
    [~ 'failed to reconcile deposit settlement: cannot find unsettled deposit in state']
  ?^  t.tracked
    [~ 'failed to reconcile deposit settlement: counterpart note is tracked more than once']
  ?.  =(+.i.tracked u.maybe-counterpart)
    [~ 'failed to reconcile deposit settlement: tracked counterpart does not match as-of block']
  ?.  (check-deposit-settlement +.i.tracked settlement)
    [~ 'failed to reconcile deposit settlement: counterpart does not match settlement']
  $(settlements t.settlements, seen next-seen)
:::
:::
:::  +nockchain-process-deferred-deposit-settlements:
:::    Reconcile Base settlements that arrived before their Nockchain block.
++  nockchain-process-deferred-deposit-settlements
  |=  latest=nock-block
  ^-  process-result
  ?^  invalid=(validate-deferred-deposit-settlements latest)
    [%| [%stop u.invalid]]
  =/  settlements=(list [nock-hash [beid deposit-settlement]])
    ~(tap z-bi deferred-deposit-settlements.hash-state.state)
  |-
  ?~  settlements  [%& state]
  =/  [as-of=nock-hash [event-id=beid settlement=deposit-settlement]]
    i.settlements
  =/  name=nname:t  counterpart.settlement
  =/  tracked=(list [nock-hash deposit])
    (unsettled-deposits-by-counterpart name)
  =/  maybe-block=(unit nock-block)
    (deferred-deposit-settlement-source-block settlement latest)
  =/  resolved=?
    ?^  maybe-block
      %.y
    (mainnet-pre-repair-orphaned-lineage-settlement settlement tracked)
  ?.  resolved
    $(settlements t.settlements)
  ?~  tracked
    [%| [%stop 'failed to reconcile deposit settlement: validated counterpart disappeared from state']]
  =.  unsettled-deposits.hash-state.state
    %+  roll  `(list [nock-hash deposit])`tracked
    |=  [[tracked-as-of=nock-hash ignored=deposit] acc=_unsettled-deposits.hash-state.state]
    (~(del z-bi acc) [tracked-as-of name])
  =.  deferred-deposit-settlements.hash-state.state
    (~(del z-bi deferred-deposit-settlements.hash-state.state) [as-of event-id])
  $(settlements t.settlements)
::
:::  +nockchain-process-withdrawal-settlements:
:::    Reconcile settlements immediately when their Base batch is known;
:::    otherwise persist them until that batch arrives.
++  nockchain-process-withdrawal-settlements
  |=  latest=nock-block
  ^-  process-result
  =/  settlements  ~(tap z-by withdrawal-settlements.latest)
  =/  seen=(z-set beid)  *(z-set beid)
  |-
  ?~  settlements  [%& state]
  =/  [name=nname:t settlement=withdrawal-settlement]
    i.settlements
  ?.  =(name nname.settlement)
    [%| [%stop 'failed to process withdrawal settlement: note name does not match map key']]
  =/  [event-id=beid as-of=base-hash]  [counterpart as-of]:settlement
  ?:  (~(has z-in seen) event-id)
    [%| [%stop 'failed to process withdrawal settlement: duplicate counterpart event']]
  =/  next-seen=(z-set beid)  (~(put z-in seen) event-id)
  ?:  (has-deferred-withdrawal-counterpart event-id)
    [%| [%stop 'failed to process withdrawal settlement: counterpart event already has a deferred settlement']]
  ?.  (~(has z-by base-hashchain.hash-state.state) as-of)
    ?:  (has-unsettled-withdrawal-counterpart-under-other-hash as-of event-id)
      [%| [%stop 'failed to process withdrawal settlement: counterpart event is tracked under a different as-of hash']]
    ?:  (lth base-batch-end.settlement base-hashchain-next-height.hash-state.state)
      [%| [%stop 'failed to process withdrawal settlement: unknown as-of hash is behind the Base cursor']]
    =.  deferred-withdrawal-settlements.hash-state.state
      %-  ~(put z-bi deferred-withdrawal-settlements.hash-state.state)
      [as-of name settlement]
    $(settlements t.settlements, seen next-seen)
  ::
  ::  find the corresponding unsettled withdrawal in the hash-state.
  ::  we do not require the bridge node to have seen the proposal prior to observing
  ::  the withdrawal settlement.
  ::    - if bridge node has seen proposal, the withdrawal will be in the unsettled withdrawal set.
  ::    - if the unsettled withdrawal is not in the unsettled withdrawal set, this is a STOP condition.
  ?.  (has-unsettled-withdrawal as-of event-id)
    [%| [%stop 'failed to process withdrawal settlement: cannot find unsettled withdrawal in state']]
  =+  block-with-withdrawal=(~(got z-by base-hashchain.hash-state.state) as-of)
  ?.  =(base-batch-end.settlement last-height.block-with-withdrawal)
    [%| [%stop 'failed to process withdrawal settlement: Base batch end does not match as-of batch']]
  =/  maybe-counterpart=(unit withdrawal)
    (~(get z-by withdrawals.block-with-withdrawal) event-id)
  ?~  maybe-counterpart
    [%| [%stop 'failed to process withdrawal settlement: counterpart event not found in as-of base block']]
  =/  counterpart=withdrawal
    u.maybe-counterpart
  ?.  (check-withdrawal-settlement counterpart settlement)
    [%| [%stop 'failed to process withdrawal settlement: counterpart does not match settlement']]
  ::
  ::  now that the withdrawal settled on nock, delete it from the tracked state
  =.  unsettled-withdrawals.hash-state.state
    (~(del z-bi unsettled-withdrawals.hash-state.state) [as-of event-id])
  $(settlements t.settlements, seen next-seen)
::
++  has-unsettled-withdrawal-counterpart-under-other-hash
  |=  [as-of=base-hash event-id=beid]
  ^-  ?
  %+  lien
    ~(tap z-bi unsettled-withdrawals.hash-state.state)
  |=  [tracked-as-of=base-hash [tracked-event-id=beid tracked=withdrawal]]
  ?&  =(event-id tracked-event-id)
      !=(as-of tracked-as-of)
  ==
:::
++  has-deferred-withdrawal-counterpart
  |=  event-id=beid
  ^-  ?
  %+  lien
    ~(tap z-bi deferred-withdrawal-settlements.hash-state.state)
  |=  [deferred-as-of=base-hash [name=nname:t settlement=withdrawal-settlement]]
  =(event-id counterpart.settlement)
:::
++  has-unsettled-withdrawal
  |=  [as-of=base-hash =beid]
  (~(has z-bi unsettled-withdrawals.hash-state.state) as-of beid)
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
    |=  [name=nname:t =deposit]
    ?.  (~(has z-bi unsettled-deposits.hash-state.state) block-hash name)
      ~
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
