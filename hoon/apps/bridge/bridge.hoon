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
        %cfg-load             (config-load config.cause)
        %set-constants        (set-constants constants.cause)
        %stop                 [~ state(stop `last.cause)]
        %base-blocks          (incoming-base-blocks:base +>.cause rest)
        %nockchain-block      (incoming-nockchain-block:nock +>.cause rest)
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
  ++  evaluate-proposed-nock-tx
    |=  [proposal=proposed-nock-tx:cause rest=[=wire eny=@ our=@ux now=@da]]
    ^-  [(list effect) bridge-state]
    ~&  [%evaluate-proposed-nock-tx proposal rest]
    ~|  %todo  !!
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
  ?:  ?=(%1 -.old)
    old
  ~>  %slog.[0 'bridge: +load state upgrade required']
  ?-  -.old
    %0  $(old state-0-1)
  ==
  ::
  ++  state-0-1
    ^-  bridge-state
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
        [%nock-hashchain-deposits-since-height start-height=@t ~]
      =/  start=@  (slav %ud start-height.pole)
      =|  reqs=(list nock-deposit-request:effect)
      =/  cur-hash=nock-hash  last-nock-block.hash-state.state
      ?:  =(*hash:t last-nock-block.hash-state.state)
        [~ ~]
      =/  cur-block=nock-block
        (~(got z-by nock-hashchain.hash-state.state) cur-hash)
      |-
      =/  blk=nock-block  cur-block
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
        [%nock-hashchain-next-height ~]
      ``nock-hashchain-next-height.hash-state.state
    ::
        [%base-hashchain-next-height ~]
      =/  stored  base-hashchain-next-height.hash-state.state
      =/  start   base-start-height.constants.state
      =/  result  ?:((lth stored start) start stored)
      ``result
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
