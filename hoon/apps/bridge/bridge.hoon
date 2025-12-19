::  base bridge nockapp
::
::    implements the nockchain side of a federated 3-of-5 multisig
::    bridge to base l2. detects v1 bridge transactions, coordinates
::    bundle proposals through round-robin, collects signatures from
::    bridge nodes, and submits bundles to base for token minting.
::
/=  t  /common/tx-engine
/=  base-lib  /apps/bridge/base
/=  nock-lib  /apps/bridge/nock
/=  *   /common/zeke
/=  *  /common/zoon
/=  *  /common/zose
/=  *  /common/wrapper
/=  *  /apps/bridge/types
/=  dumb  /apps/dumbnet/lib/types
::
=>
|%
++  moat  (keep bridge-state)
::
::  +generate-test-config: create testing configuration
::
::  generates a default node-config for testing with 5 localhost
::  nodes. uses dummy keys and addresses. in production, nodes
::  load real configuration from config files.
::
++  generate-test-config
  ^-  node-config
  ::  create deterministic test keys that ensure node 0 is always proposer at height 0
  ::  by making its nockchain pubkey lexicographically smallest
  =/  test-seckeys=(list schnorr-seckey:t)
    :~  (from-atom:schnorr-seckey:t 0x1.0000.0000.0000.0000)
        (from-atom:schnorr-seckey:t 0x2.0000.0000.0000.0000)
        (from-atom:schnorr-seckey:t 0x3.0000.0000.0000.0000)
        (from-atom:schnorr-seckey:t 0x4.0000.0000.0000.0000)
        (from-atom:schnorr-seckey:t 0x5.0000.0000.0000.0000)
    ==
  =/  test-pubkeys=(list schnorr-pubkey:t)
    %+  turn  test-seckeys
    |=  seckey=schnorr-seckey:t
    %-  ch-scal:affine:curve:cheetah
    :*  (t8-to-atom:belt-schnorr:cheetah seckey)
        a-gen:curve:cheetah
    ==
  ::  compute PKHs (public key hashes) from pubkeys
  =/  test-pkhs=(list hash:t)
    %+  turn  test-pubkeys
    |=  pubkey=schnorr-pubkey:t
    (hash:schnorr-pubkey:t pubkey)
  ::  verify that node 0's pkh is lexicographically smallest
  ::  and reorder if necessary to ensure deterministic proposer selection
  =/  pkh-b58-strings=(list @t)
    %+  turn  test-pkhs
    |=  pkh=hash:t
    (to-b58:hash:t pkh)
  =/  sorted-indices=(list @ud)
    %+  sort  (gulf 0 4)
    |=  [a=@ud b=@ud]
    =/  str-a=@t  (snag a pkh-b58-strings)
    =/  str-b=@t  (snag b pkh-b58-strings)
    (lth str-a str-b)
  ::  reorder nodes so that the lexicographically smallest pkh is at index 0
  =/  reordered-seckeys=(list schnorr-seckey:t)
    %+  turn  sorted-indices
    |=  idx=@ud
    (snag idx test-seckeys)
  =/  reordered-pkhs=(list hash:t)
    %+  turn  sorted-indices
    |=  idx=@ud
    (snag idx test-pkhs)
  =/  test-nodes=(list node-info)
    :~  [ip='localhost:8001' eth-pubkey=0x1111 nock-pkh=(snag 0 reordered-pkhs)]
        [ip='localhost:8002' eth-pubkey=0x2222 nock-pkh=(snag 1 reordered-pkhs)]
        [ip='localhost:8003' eth-pubkey=0x3333 nock-pkh=(snag 2 reordered-pkhs)]
        [ip='localhost:8004' eth-pubkey=0x4444 nock-pkh=(snag 3 reordered-pkhs)]
        [ip='localhost:8005' eth-pubkey=0x5555 nock-pkh=(snag 4 reordered-pkhs)]
    ==
  :*  0
      test-nodes
      0xdead.beef
      (snag 0 reordered-seckeys)
  ==
::
++  bridge
  |_  state=bridge-state
  +*  base  ~(. base-lib state)
      nock  ~(. nock-lib state)
  ::
  ++  handle-cause
    |=  [=cause rest=[=wire eny=@ our=@ux now=@da]]
    ^-  [(list effect) bridge-state]
    ?>  ?=(%0 -.cause)
    ~&  %handle-cause
    ?^  stop.state
      =+  base-hash-b58=(to-b58:hash:t hash.base.u.stop.state)
      =+  nock-hash-b58=(to-b58:hash:t hash.nock.u.stop.state)
      =/  msg=@t
        ;:  (cury cat 3)
            'bridge was stopped. no causes will be processed. last known good base blocks hash: '
            base-hash-b58
            '. last known good nock block hash: '
            nock-hash-b58
            '.'
        ==
      ~>  %slog.[0 msg]
      [~ state]
    ?:  ?|  ?=(^ base-hold.hash-state.state)
            ?=(^ nock-hold.hash-state.state)
        ==
      [[%0 %stop 'hold detected. we do not handle holds at the moment, so we treat them as a stop condition' (get-stop-info state)]~ state]
    ::  virtualize the cause handler to catch crashes that may not have been caught.
    =;  result
      ?-    -.result
          %|
        =/  msg=@t  (cat 3 'bridge kernel: crashed when handling cause: ' +<.cause)
        %-  (slog p.result)
        [[%0 %stop msg (get-stop-info state)]~ state]
     ::
          %&
        p.result
      ==
    %-  mule
    |.
    ?-    +<.cause
        %cfg-load             (config-load config.cause)
        %set-constants        (set-constants constants.cause)
        %stop                 [~ state(stop `last.cause)]
        %start                [~ state(stop ~)]
        %base-blocks          (incoming-base-blocks:base +>.cause rest)
        %nockchain-block      (incoming-nockchain-block:nock +>.cause rest)
        %proposed-base-call   (evaluate-base-call +>.cause rest)
        %proposed-nock-tx     (evaluate-proposed-nock-tx +>.cause rest)
    ==
  ++  config-load
    |=  config=(unit node-config)
    ?^  config
      [~ state(config u.config)]
    [~ state]
  ::
  ++  set-constants
    |=  new-constants=bridge-constants
    ^-  [(list effect) bridge-state]
    ::  validate version
    ?.  =(version.new-constants %0)
      ~>  %slog.[0 'set-constants: unsupported version']
      [~ state]
    ::  validate min-signers <= total-signers
    ?:  (gth min-signers.new-constants total-signers.new-constants)
      ~>  %slog.[0 'set-constants: min-signers cannot exceed total-signers']
      [~ state]
    ::  validate min-signers > 0
    ?:  =(min-signers.new-constants 0)
      ~>  %slog.[0 'set-constants: min-signers must be at least 1']
      [~ state]
    ::  validate minimum-event-nocks > 0
    ?:  =(minimum-event-nocks.new-constants 0)
      ~>  %slog.[0 'set-constants: minimum-event-nocks must be greater than 0']
      [~ state]
    ::  validate base-blocks-chunk > 0
    ?:  =(base-blocks-chunk.new-constants 0)
      ~>  %slog.[0 'set-constants: base-blocks-chunk must be greater than 0']
      [~ state]
    ::  all validations passed, update state
    ~>  %slog.[0 'set-constants: constants updated successfully']
    ::  update hashchain next-heights if they're still at old defaults
    ::  (i.e., bridge hasn't started processing blocks yet)
    =/  old-nock-start  nockchain-start-height.constants.state
    =/  old-base-start  base-start-height.constants.state
    =/  new-state  state(constants new-constants)
    =?  nock-hashchain-next-height.hash-state.new-state
      =(nock-hashchain-next-height.hash-state.state old-nock-start)
    nockchain-start-height.new-constants
    =?  base-hashchain-next-height.hash-state.new-state
      =(base-hashchain-next-height.hash-state.state old-base-start)
    base-start-height.new-constants
    [~ new-state]
  ::
  ::  +evaluate-base-call:
  ::    Invoke this function only after confirming that every proposed deposit corresponds
  ::    exactly to an unsettled deposit in the hash-state, matching both the recipient
  ::    EVM address and the amount.
  ::
  ::    This function moves the proposed deposit from unsettled-deposits to
  ::    unconfirmed-settled-deposits, signalling that the deposit has been seen
  ::    by the node, is considered valid, and is now awaiting confirmation
  ::    base.
  ++  evaluate-base-call
    |=  [proposal=proposed-base-call:cause rest=[=wire eny=@ our=@ux now=@da]]
    ^-  [(list effect) bridge-state]
    ~&  %evaluate-base-call
    =+  old-state=state
    =/  stop-info  (get-stop-info old-state)
    =/  template  |=(msg=@t [%0 %stop msg stop-info])
    |-
    ?~  proposal
      [~ old-state]
    =+  [name as-of nonce]=[name as-of nonce]:i.proposal
    ?:  (gte nonce next-nonce.state)
      ::  Proposed nonces must refer to an already-assigned nonce. This condition
      ::  should never trigger because %proposed-deposit checks if the nonce was already assigned.
      ::  If it triggers, it is a sign of rust driver malfunciton.
      [[(template 'nonce in proposed base call is greater than or equal to next-nonce')]~ old-state]
    ::
    ::  The two conditionals below can be removed if we confirm that the driver is
    ::  checking the pre-conditions before calling this arm. I will keep them here
    ::  for now out of paranoia.
    ?.  (~(has z-bi unsettled-deposits.hash-state.state) as-of name)
      [[(template 'proposed deposit not in unsettled-deposits')]~ old-state]
    ?:  (~(has z-bi unconfirmed-settled-deposits.hash-state.state) as-of name)
      [[(template 'encountered double proposal. proposed deposit already in unconfirmed-settled-deposits.')]~ old-state]
    =+  deposit=(~(got z-bi unsettled-deposits.hash-state.state) as-of name)
    ::
    ::  proposal should have already been checked against deposit through a rust peek
    =.  unsettled-deposits.hash-state.state
      (~(del z-bi unsettled-deposits.hash-state.state) as-of name)
    =.  unconfirmed-settled-deposits.hash-state.state
      (~(put z-bi unconfirmed-settled-deposits.hash-state.state) as-of name deposit)
    $(proposal t.proposal)
  ::
  ++  evaluate-proposed-nock-tx
    |=  [proposal=proposed-nock-tx:cause rest=[=wire eny=@ our=@ux now=@da]]
    ^-  [(list effect) bridge-state]
    ~&  [%evaluate-proposed-nock-tx proposal rest]
    ~|  %todo  !!
  ::
  --
--
::
%-  (moat |)
^-  fort:moat
|_  state=bridge-state
+*  b  ~(. bridge state)
::
::    +load: initialize or restore bridge state
::
::  loads bridge node configuration from file or generates test
::  config if none exists. called on nockapp startup to initialize
::  the bridge state with node identity and network configuration.
::
++  load
  |=  arg=bridge-state
  ^-  bridge-state
  arg
::
::    +peek: read-only queries into bridge state
::
::  handles scry requests to inspect bridge state.
::
++  peek
  |=  arg=path
  ^-  (unit (unit *))
  ~&  bridge-peek+arg
  =/  =(pole)  arg
  ?+    pole  ~
        :: Use this peek to ensure that the bridge is booting in mainnet mode with the correct deployment constants
        [%fakenet ~]    ``!=(constants.state *bridge-constants)
    ::
        [%state ~]       ``state
    ::
        [%hash-state ~]  ``hash-state.state
    ::
        [%constants ~]   ``constants.state
    ::
        [%base-hold ~]
      =+  base-hold=base-hold.hash-state.state
      ?~  base-hold
        ``%.n
      ``(~(has z-by base-hashchain.hash-state.state) hash.u.base-hold)
    ::
        [%nock-hold ~]
      =+  nock-hold=nock-hold.hash-state.state
      ?~  nock-hold
        ``%.n
      ``(~(has z-by nock-hashchain.hash-state.state) hash.u.nock-hold)
    ::
        [%unsettled-deposit-count ~]
      ``~(wyt z-by unsettled-deposits.hash-state.state)
    ::
        [%unconfirmed-settled-deposit-count ~]
      ``~(wyt z-by unconfirmed-settled-deposits.hash-state.state)
    ::
        [%unsettled-withdrawal-count ~]
      ``~(wyt z-by unsettled-withdrawals.hash-state.state)
    ::
        [%unconfirmed-settled-withdrawal-count ~]
      ``~(wyt z-by unconfirmed-settled-withdrawals.hash-state.state)
    ::
        [%nock-hashchain-next-height ~]
      ``nock-hashchain-next-height.hash-state.state
    ::
        [%base-hashchain-next-height ~]
      =/  stored  base-hashchain-next-height.hash-state.state
      =/  start   base-start-height.constants.state
      =/  result  ?:((lth stored start) start stored)
      ~&  base-next-height+[stored=stored start=start result=result]
      ``result
    ::
        [%stop-info ~]
      ``(get-stop-info state)
    ::
        [%proposed-deposit tx-id=@t nock-hash=@t first-name=@t last-name=@t receiver=@t amount-to-mint=@ nonce=@ ~]
      ::  check if deposit exists under key (nock-hash [first-name last-name]). Then check if
      ::  the evm receiver address matches and the amount-to-mint = raw-amount - fee(raw-amount)
      ::  notes:
      ::  - consider storing minted-amount in deposit to simplify the check.
      ::  - do we need to check if the deposit tx-id matches with the tx-id in the proposal?
      ::
      =+  tx-id=(from-b58:hash:t tx-id.pole)
      =+  block-hash=(from-b58:hash:t nock-hash.pole)
      =+  name=(from-b58:nname:t [first-name last-name]:pole)
      =/  receiver=evm-address  (de-base58 (trip receiver.pole))
      ::
      ::  if this condition is hit, it should result in a stop condition because
      ::  it means that this node has already processed this deposit proposal.
      ::  do not call %evaluate-base-call, do not sign the proposal, emit a STOP.
      ?:  (~(has z-bi unconfirmed-settled-deposits.hash-state.state) block-hash name)
        [~ ~ %.n]
      ::  returning [~ ~] means that a deposit corresponding to (block-hash, name) was not found.
      ::  if [~ ~] is returned, do not call %evaluate-base-call and do not sign the proposal.
      ::  This is not a stop condition because it is possible that the node is still syncing.
      ?.  (~(has z-bi unsettled-deposits.hash-state.state) block-hash name)
        [~ ~]
      ::
      ::  If we have the deposit in our hash-state, but the nonce is greater than the next
      ::  nonce, we may be getting asked to sign an invalid nonce.
      ::
      ::  This is a STOP condition because we should have processed the deposit and produced
      ::  the proposal, which resulted in the next-nonce getting incremented, in the same atomic event.
      ?:  (gte nonce.pole next-nonce.state)
        [~ ~ %.n]
      =+  deposit=(~(got z-bi unsettled-deposits.hash-state.state) block-hash name)
      =/  dest-matches=?
        ?~  dest.deposit  %.n
        =(u.dest.deposit receiver)
      =/  amount-matches=?
        =(amount-to-mint.pole amount-to-mint.deposit)
      =/  tx-id-matches=?
        =(tx-id tx-id.deposit)
      ::
      ::  returning %.y means that a matching deposit was present in the hash-state.
      ::  call %evaluate-base-call and sign the proposal.
      ?:  ?&(dest-matches amount-matches tx-id-matches)
        [~ ~ %.y]
      ::
      ::  if this condition is hit, it should result in a stop condition because
      ::  it means the deposit entry under (block-hash, name) exists, but the
      ::  the destination and/or amount does not match. This means that the proposer
      ::  submitted an invalid proposal.
      ::
      ::  this goes without saying, if [~ ~ %.n] is returned:
      ::    - do not call %evaluate-base-call
      ::    - do not sign the proposal
      ::    - return a STOP condition
      [~ ~ %.n]
  ==
::
::    +poke: handle incoming bridge events
::
::  processes all incoming events for the bridge: grpc responses,
::  bridge-specific causes, and node coordination
::  messages. routes to appropriate handlers based on wire and
::  cause type.
::
++  poke
  |=  [=wire eny=@ our=@ux now=@da dat=*]
  ^-  [(list effect) bridge-state]
  =;  res
    ~&  >  "effects: {<-.res>}"
    res
  =/  soft-cause  ((soft cause) dat)
  ?~  soft-cause
    ~&  "bridge: could not mold poke: {<dat>}"  !!
  =/  =cause  u.soft-cause
  =/  tag  +<.cause
  =/  =(pole)  wire
  ~&  >  "poke: saw cause {<;;(@t tag)>} on wire {<wire>}"
  ?+    pole  ~|("unsupported wire: {<wire>}" !!)
    ::
      [%poke src=?(%one-punch %signature) ver=@ *]
    (handle-cause:b cause [wire eny our now])
  ==
::
--
