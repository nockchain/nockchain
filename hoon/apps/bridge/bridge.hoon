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
/=  tx-builder  /apps/wallet/lib/tx-builder
/=  wt  /apps/wallet/lib/types
/=  *   /common/zeke
/=  zo  /common/zoon
/=  *  /common/zose
/=  *  /common/wrapper
/=  *  /apps/bridge/types
/=  dumb  /apps/dumbnet/lib/types
::
=>
|%
++  moat  (keep bridge-state)
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
    ?:  ?=(%start +<.cause)
      =/  msg=@t  'bridge stop state removed. resuming cause processing.'
      ~>  %slog.[0 msg]
      [~ state(stop ~)]
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
    ?:  ?&  ?=(^ base-hold.hash-state.state)
            ?=(^ nock-hold.hash-state.state)
        ==
      [[%0 %stop 'fatal: hold on both nock and base detected' (get-stop-info state)]~ state]
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
        %cfg-load
      (config-load config.cause)
      ::
        %set-constants
      (set-constants constants.cause)
      ::
        %set-blockchain-constants
      (set-blockchain-constants constants.cause)
      ::
        %stop
      [~ state(stop `last.cause)]
      ::
        %base-blocks
      (incoming-base-blocks:base +>.cause rest)
      ::
        %base-block-withdrawals-committed
      (commit-base-block-withdrawals:base ack.cause)
      ::
        %nockchain-block
      (incoming-nockchain-block:nock +>.cause rest)
      ::
        %create-withdrawal-tx
      (evaluate-create-withdrawal-tx +>.cause rest)
      ::
        %sign-tx
      (evaluate-sign-tx +>.cause rest)
      ::
        %proposed-nock-tx
      (evaluate-proposed-nock-tx +>.cause rest)
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
  ++  set-blockchain-constants
    |=  new-constants=blockchain-constants:t
    ^-  [(list effect) bridge-state]
    ~>  %slog.[0 'set-blockchain-constants: stored connected node blockchain constants']
    [~ state(nockchain-constants [~ new-constants])]
  ::
  ++  withdrawal-signer-pkh
    ^-  hash:t
    %-  hash:schnorr-pubkey:t
    %-  from-sk:schnorr-pubkey:t
    (to-atom:schnorr-seckey:t my-nock-key.config.state)
  ::
  ++  selected-note-names
    |=  selected=(list selected-withdrawal-note)
    ^-  (list nname:t)
    %+  turn  selected
    |=  picked=selected-withdrawal-note
    name.picked
  ::
  ++  get-selected-note
    |=  [selected=(list selected-withdrawal-note) wanted=nname:t]
    ^-  nnote:t
    |-
    ?~  selected
      ~|('withdrawal selected note missing from create-withdrawal-tx request' !!)
    ?:  =(name.i.selected wanted)
      note.i.selected
    $(selected t.selected)
  ::
  ++  build-withdrawal-order
    |=  request=create-withdrawal-tx
    ^-  order:wt
    [%bridge-withdrawal base-event-id=base-event-id.id.request base-hash=as-of.id.request root=recipient.request base-batch-end=base-batch-end.request gift=amount.request]
  ::
  ++  bridge-withdrawal-spend-condition
    ^-  (unit spend-condition:t)
    =/  allowed=(z-set:zo hash:t)
      %-  z-silt:zo
      %+  turn  nodes.config.state
      |=  node=node-info
      nock-pkh.node
    =/  lock=spend-condition:t
      [%pkh [m=min-signers.constants.state allowed]]~
    ?:  =((hash:lock:t lock) bridge-lock-root.config.state)
      (some lock)
    ~>  %slog.[0 'configured bridge signer set does not hash to bridge-lock-root']
    ~
  ::
  ++  sign-withdrawal-transaction
    |=  =transaction:wt
    ^-  transaction:wt
    =/  =witness-data:wt  witness-data.transaction
    ?>  ?&  ?=(%1 -.witness-data)
            ?=(%1 -.inputs.metadata.transaction)
        ==
    =/  =spends:v1:t  spends.transaction
    =/  signer-pkh=hash:t  withdrawal-signer-pkh
    ::  Pull the first `%pkh` primitive for each input note. This signer helper
    ::  only adds PKH signatures, so notes without a PKH primitive are skipped.
    =/  pkh-lps=(z-map nname:t pkh:v1:t)
      %-  ~(rep z-by p.inputs.metadata.transaction)
      |=  $:  [k=nname:t v=spend-condition:t]
              acc=(z-map nname:t pkh:v1:t)
          ==
      ?~  v
        acc
      ?:  ?=(%pkh -.i.v)
        (~(put z-by acc) k +.i.v)
      $(v t.v)
    =.  witness-data
      :-  %1
      %-  ~(rep z-by spends)
      |=  $:  [name=nname:t =spend:v1:t]
              wd=(z-map nname:t witness:t)
          ==
      ?>  ?=(%1 -.spend)
      =+  curr-witness=(~(got z-by p.witness-data) name)
      ?~  required=(~(get z-by pkh-lps) name)
        (~(put z-by wd) name curr-witness)
      =+  curr-pkh=u.required
      ::  Only add a signature when this signer is in the allowed PKH set, has
      ::  not already signed, and the witness is still short of the PKH quorum.
      =+  num-signed=~(wyt z-by pkh.curr-witness)
      ?:  (~(has z-by pkh.curr-witness) signer-pkh)
        (~(put z-by wd) name curr-witness)
      ?.  (~(has z-in h.curr-pkh) signer-pkh)
        (~(put z-by wd) name curr-witness)
      ?.  (lth num-signed m.curr-pkh)
        (~(put z-by wd) name curr-witness)
      =+  sig-hash=(sig-hash:spend-1:v1:t +.spend)
      %+  ~(put z-by wd)  name
      (sign:witness:t curr-witness my-nock-key.config.state sig-hash)
    transaction(witness-data witness-data)
  ::
  ++  evaluate-proposed-nock-tx
    |=  [proposal=withdrawal-proposal rest=[=wire eny=@ our=@ux now=@da]]
    ^-  [(list effect) bridge-state]
    ~&  [%evaluate-proposed-nock-tx proposal rest]
    ~>  %slog.[0 'proposed-nock-tx validation/tracking is runtime-owned; kernel state unchanged']
    [~ state]
  ::
  ++  evaluate-create-withdrawal-tx
    |=  [request=create-withdrawal-tx rest=[=wire eny=@ our=@ux now=@da]]
    ^-  [(list effect) bridge-state]
    ~&  [%evaluate-create-withdrawal-tx request rest]
    ?~  nockchain-constants.state
      ~>  %slog.[0 'create-withdrawal-tx requires boot-time blockchain constants']
      [~ state]
    =/  names=(list nname:t)  (selected-note-names selected-notes.request)
    =/  sign-keys=(list schnorr-seckey:t)  ~[my-nock-key.config.state]
    =/  refund-pkh=(unit hash:t)  ~
    =/  maybe-input-lock=(unit spend-condition:t)
      bridge-withdrawal-spend-condition
    =/  supplied-input-lock=(unit lock:t)
      ?~  maybe-input-lock
        ~
      [~ u.maybe-input-lock]
    =/  =transaction:wt
      %:  ~(build tx-builder u.nockchain-constants.state)
        names
        ~[(build-withdrawal-order request)]
        fee.request
        %.n
        sign-keys
        refund-pkh
        |=  wanted=nname:t
        (get-selected-note selected-notes.request wanted)
        supplied-input-lock
        %.y
        %asc
        height.snapshot.request
      ==
    =/  proposal=withdrawal-proposal
      :*  id.request
          recipient.request
          amount.request
          amount-burned.request
          base-batch-end.request
          epoch.request
          snapshot.request
          names
          transaction
      ==
    [~[[%0 %withdrawal-proposal-built proposal]] state]
  ::
  ++  evaluate-sign-tx
    |=  [proposal=withdrawal-proposal rest=[=wire eny=@ our=@ux now=@da]]
    ^-  [(list effect) bridge-state]
    ~&  [%evaluate-sign-tx proposal rest]
    =/  signed=withdrawal-proposal
      proposal(transaction (sign-withdrawal-transaction transaction.proposal))
    [~[[%0 %withdrawal-tx-signed signed]] state]
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
  |=  old=versioned-bridge-state
  ^-  bridge-state
  |^
  |-
  ?:  ?=(%3 -.old)
    old
  ~>  %slog.[0 'bridge: +load state upgrade required']
  ?-  -.old
    %2  $(old state-2-3)
    %1  $(old state-1-2)
    %0  $(old state-0-1)
  ==
  ::
  ++  state-2-3
    ^-  bridge-state
    ?>  ?=(%2 -.old)
    ~>  %slog.[0 'bridge: upgrade state %2 -> %3']
    =/  new-config=node-config
      :*  node-id.config.old
          nodes.config.old
          bridge-lock-root.old
          my-eth-key.config.old
          my-nock-key.config.old
      ==
    =/  new-hash-state=hash-state
      %*  .  *hash-state
          last-nock-block            last-nock-block.hash-state.old
          last-base-blocks           last-base-blocks.hash-state.old
          nock-hashchain             nock-hashchain.hash-state.old
          base-hashchain             base-hashchain.hash-state.old
          nock-hold                  nock-hold.hash-state.old
          base-hold                  base-hold.hash-state.old
          nock-hashchain-next-height  nock-hashchain-next-height.hash-state.old
          base-hashchain-next-height  base-hashchain-next-height.hash-state.old
          unsettled-deposits         unsettled-deposits.hash-state.old
          unsettled-withdrawals      unsettled-withdrawals.hash-state.old
          pending-base-block-commit  ~
      ==
    :*  %3
        new-config
        constants.old
        nockchain-constants.old
        new-hash-state
        last-nock-deposit-height.old
        last-block.old
        stop.old
    ==
  ::
  ++  state-1-2
    ^-  bridge-state-2
    ?>  ?=(%1 -.old)
    ~>  %slog.[0 'bridge: upgrade state %1 -> %2']
    =/  new-hash-state=hash-state-2-old
      %*  .  *hash-state-2-old
          last-nock-block            last-nock-block.hash-state.old
          last-base-blocks           last-base-blocks.hash-state.old
          nock-hashchain             nock-hashchain.hash-state.old
          base-hashchain             base-hashchain.hash-state.old
          nock-hold                  nock-hold.hash-state.old
          base-hold                  base-hold.hash-state.old
          nock-hashchain-next-height  nock-hashchain-next-height.hash-state.old
          base-hashchain-next-height  base-hashchain-next-height.hash-state.old
          unsettled-deposits         unsettled-deposits.hash-state.old
          unsettled-withdrawals      unsettled-withdrawals.hash-state.old
      ==
    :*  %2
        config.old
        constants.old
        ~
        new-hash-state
        last-nock-deposit-height.old
        last-block.old
        bridge-lock-root.old
        stop.old
    ==
  ::
  ++  state-0-1
    ^-  versioned-bridge-state
    ?>  ?=(%0 -.old)
    ~>  %slog.[0 'bridge: upgrade state %0 -> %1']
    =/  new-unsettled-deposits=(z-mip nock-hash nname:t deposit)
      =/  blocks=(z-set nock-hash)
        %-  ~(uni z-in ~(key z-by unconfirmed-settled-deposits.hash-state.old))
        ~(key z-by unsettled-deposits.hash-state.old)
      %-  ~(gas z-by *(z-mip nock-hash nname:t deposit))
      %+  turn  ~(tap z-in blocks)
      |=  as-of=nock-hash
      =+  a=(~(gut z-by unconfirmed-settled-deposits.hash-state.old) as-of ~)
      =+  b=(~(gut z-by unsettled-deposits.hash-state.old) as-of ~)
      [as-of (~(uni z-by a) b)]
    =/  new-hash-state=hash-state-1
      %*  .  *hash-state-1
          last-nock-block   last-nock-block.hash-state.old
          last-base-blocks  last-base-blocks.hash-state.old
          nock-hashchain    nock-hashchain.hash-state.old
          base-hashchain    base-hashchain.hash-state.old
          nock-hold         nock-hold.hash-state.old
          base-hold         base-hold.hash-state.old
          nock-hashchain-next-height  nock-hashchain-next-height.hash-state.old
          base-hashchain-next-height  base-hashchain-next-height.hash-state.old
          unsettled-deposits          new-unsettled-deposits
          unsettled-withdrawals       unsettled-withdrawals.hash-state.old
      ==
    :*  %1
        config.old
        constants.old
        new-hash-state
        nockchain-start-height.constants.old
        last-block.old
        bridge-lock-root.old
        stop.old
    ==
  --
  ::
::    +peek: read-only queries into bridge state
::
::  handles scry requests to inspect bridge state.
::
++  peek
  |=  arg=path
  ^-  (unit (unit *))
  ~&  >>  bridge-peek+arg
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
        [%blockchain-constants ~]
      ?~  nockchain-constants.state
        [~ ~]
      ``u.nockchain-constants.state
    ::
        [%base-hold ~]
      =+  base-hold=base-hold.hash-state.state
      ?~  base-hold
        [~ ~]
      ``u.base-hold
    ::
        [%base-hold-height ~]
      =+  base-hold=base-hold.hash-state.state
      ?~  base-hold
        [~ ~]
      ``height.u.base-hold
    ::
        [%nock-hold ~]
      =+  nock-hold=nock-hold.hash-state.state
      ?~  nock-hold
        [~ ~]
      ``u.nock-hold
    ::
        [%nock-hold-height ~]
      =+  nock-hold=nock-hold.hash-state.state
      ?~  nock-hold
        [~ ~]
      ``height.u.nock-hold
    ::
        [%unsettled-deposit-count ~]
      %-  some  %-  some
      %+  roll  ~(val z-by unsettled-deposits.hash-state.state)
      |=  [m=(z-map nname:t deposit) acc=@]
      (add acc ~(wyt z-by m))
    ::
        [%nock-last-deposit-height ~]
      =/  last=@  last-nock-deposit-height.state
      ``last
    ::
        [%nock-hashchain-deposits ~]
      =/  blocks=(list [as-of=nock-hash block=nock-block])
        ~(tap z-by nock-hashchain.hash-state.state)
      =/  reqs=(list nock-deposit-request:effect)
        %+  roll  blocks
        |=  [[as-of=nock-hash block=nock-block] reqs=(list nock-deposit-request:effect)]
        =/  dep-entries=(list [name=nname:t =deposit])
          ~(tap z-by deposits.block)
        %+  roll  dep-entries
        |=  [[name=nname:t =deposit] reqs=_reqs]
        ?~  dest.deposit  reqs
        :_  reqs
        :*  tx-id.deposit
            name
            u.dest.deposit
            amount-to-mint.deposit
            height.block
            as-of
        ==
      ::  flop not required, but nice because it gives deposits in order of earliest to latest blocks
      ``(flop reqs)
    ::
        [%nock-hashchain-deposits-since-height start-height=@ta ~]
      =/  maybe-start  (rush start-height.pole dim:ag)
      ?~  maybe-start
        ~
      =/  start=@ud  u.maybe-start
      =|  reqs=(list nock-deposit-request:effect)
      =/  cur-hash=nock-hash  last-nock-block.hash-state.state
      ~&  [%peek-nock-hashchain-deposits-since-height start cur-hash]
      ?:  =(*hash:t last-nock-block.hash-state.state)
        [~ ~]
      ~&  [%peek-nock-hashchain-deposits-got start cur-hash]
      =/  cur-block=nock-block
        (~(got z-by nock-hashchain.hash-state.state) cur-hash)
      |-
      =/  blk=nock-block  cur-block
      ~&  [%peek-nock-hashchain-deposits-block start height.blk cur-hash prev.blk]
      ?:  (lth height.blk start)
        ::  flop not required, but nice because it gives deposits in order of earliest to latest blocks
        ``(flop reqs)
      =/  dep-entries=(list [name=nname:t =deposit])
        ~(tap z-by deposits.blk)
      =.  reqs
        %+  roll  dep-entries
        |=  [[name=nname:t =deposit] reqs=_reqs]
        ?~  dest.deposit  reqs
        :_  reqs
        =;  dep=nock-deposit-request:effect  ~&  deposit+dep  dep
        :*  tx-id.deposit
            name
            u.dest.deposit
            amount-to-mint.deposit
            height.blk
            cur-hash
        ==
      ?:  =(*hash:t prev.blk)
        ``(flop reqs)
      ~&  [%peek-nock-hashchain-deposits-prev-got start height.blk prev.blk]
      =.  cur-hash  prev.blk
      =.  cur-block  (~(got z-by nock-hashchain.hash-state.state) cur-hash)
      $(reqs reqs)
    ::
        [%unsettled-deposits ~]
      =/  entries=(list [nock-hash [name=nname:t =deposit]])
        ~(tap z-bi unsettled-deposits.hash-state.state)
      =/  reqs=(list nock-deposit-request:effect)
        %+  murn  entries
        |=  [as-of=nock-hash [name=nname:t =deposit]]
        ?~  dest.deposit  ~
        =/  block=(unit nock-block)
          (~(get z-by nock-hashchain.hash-state.state) as-of)
        ?~  block  ~
        %-  some
        :*  tx-id.deposit
            name
            u.dest.deposit
            amount-to-mint.deposit
            height.u.block
            as-of
        ==
      ``(flop reqs)
    ::
        [%unsettled-withdrawal-count ~]
      %-  some  %-  some
      %+  roll  ~(val z-by unsettled-withdrawals.hash-state.state)
      |=  [m=(z-map beid withdrawal) acc=@]
      (add acc ~(wyt z-by m))
    ::
        [%unsettled-withdrawals ~]
      =/  entries=(list [base-hash [base-event-id=beid =withdrawal]])
        ~(tap z-bi unsettled-withdrawals.hash-state.state)
      =/  reqs=(list nock-withdrawal-request:effect)
        %+  murn  entries
        |=  [as-of=base-hash [base-event-id=beid =withdrawal]]
        =/  block=(unit base-blocks)
          (~(get z-by base-hashchain.hash-state.state) as-of)
        ?~  block  ~
        %-  some
        :*  (to-atom:blist base-event-id)
            dest.withdrawal
            amount-burned.withdrawal
            last-height.u.block
            as-of
        ==
      ``(flop reqs)
    ::
        [%base-hashchain-withdrawals-since-height start-height=@ta ~]
      =/  maybe-start  (rush start-height.pole dim:ag)
      ?~  maybe-start
        ~
      =/  start=@ud  u.maybe-start
      =|  reqs=(list nock-withdrawal-request:effect)
      =/  cur-hash=base-hash  last-base-blocks.hash-state.state
      ~&  [%peek-base-hashchain-withdrawals-since-height start cur-hash]
      ?:  =(*hash:t last-base-blocks.hash-state.state)
        [~ ~]
      ~&  [%peek-base-hashchain-withdrawals-got start cur-hash]
      =/  cur-block=base-blocks
        (~(got z-by base-hashchain.hash-state.state) cur-hash)
      |-
      =/  blk=base-blocks  cur-block
      ~&  [%peek-base-hashchain-withdrawals-block start last-height.blk cur-hash prev.blk]
      ?:  (lth last-height.blk start)
        ``(flop reqs)
      =/  wd-entries=(list [base-event-id=beid =withdrawal])
        ~(tap z-by withdrawals.blk)
      =.  reqs
        %+  roll  wd-entries
        |=  [[base-event-id=beid =withdrawal] reqs=_reqs]
        :_  reqs
        :*  (to-atom:blist base-event-id)
            dest.withdrawal
            amount-burned.withdrawal
            last-height.blk
            cur-hash
        ==
      ?:  =(*hash:t prev.blk)
        ``(flop reqs)
      ~&  [%peek-base-hashchain-withdrawals-prev-got start last-height.blk prev.blk]
      =.  cur-hash  prev.blk
      =.  cur-block  (~(got z-by base-hashchain.hash-state.state) cur-hash)
      $(reqs reqs)
    ::
        [%nock-hashchain-next-height ~]
      ``nock-hashchain-next-height.hash-state.state
    ::
        [%base-hashchain-next-height ~]
      =/  stored  base-hashchain-next-height.hash-state.state
      =/  start   base-start-height.constants.state
      =/  result  ?:((lth stored start) start stored)
      ``result
    ::
        [%pending-base-block-commit ~]
      ?~  pending-base-block-commit.hash-state.state
        [~ ~]
      ``metadata.u.pending-base-block-commit.hash-state.state
    ::
        [%stop-state ~]
      ?~  stop.state
        ``%.n
      ``%.y
    ::
        [%stop-info ~]
      ``(get-stop-info state)
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
