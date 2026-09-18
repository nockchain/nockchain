/=  zo  /common/zoon
/=  *  /apps/bridge/types
/=  wt  /apps/wallet/lib/types
/=  bridge-ker  /apps/bridge/bridge
/=  whel  /tests/wallet/helpers
|%
::  create deterministic test keys that ensure node 0 is always proposer at height 0
::  by making its nockchain pubkey lexicographically smallest
++  test-config
  ^-  node-config
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
  =/  test-pkhs=(list hash:t)
    %+  turn  test-pubkeys
    |=  pubkey=schnorr-pubkey:t
    (hash:schnorr-pubkey:t pubkey)
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
  =/  reordered-seckeys=(list schnorr-seckey:t)
    %+  turn  sorted-indices
    |=  idx=@ud
    (snag idx test-seckeys)
  =/  reordered-pkhs=(list hash:t)
    %+  turn  sorted-indices
    |=  idx=@ud
    (snag idx test-pkhs)
  =/  bridge-lock-root=hash:t
    =/  lock=spend-condition:t
      [%pkh [m=3 (z-silt:zo reordered-pkhs)]]~
    (hash:lock:t lock)
  =/  test-nodes=(list node-info)
    :~  [ip='localhost:8001' eth-pubkey=0x1111 nock-pkh=(snag 0 reordered-pkhs)]
        [ip='localhost:8002' eth-pubkey=0x2222 nock-pkh=(snag 1 reordered-pkhs)]
        [ip='localhost:8003' eth-pubkey=0x3333 nock-pkh=(snag 2 reordered-pkhs)]
        [ip='localhost:8004' eth-pubkey=0x4444 nock-pkh=(snag 3 reordered-pkhs)]
        [ip='localhost:8005' eth-pubkey=0x5555 nock-pkh=(snag 4 reordered-pkhs)]
    ==
  :*  0
      test-nodes
      bridge-lock-root
      0xdead.beef
      (snag 0 reordered-seckeys)
  ==
::
++  small-constants
  |=  [base-chunk=@ base-start=@ nock-start=@]
  ^-  bridge-constants
  =/  constants  *bridge-constants
  =.  base-blocks-chunk.constants  base-chunk
  =.  minimum-event-nocks.constants  1
  =.  base-start-height.constants  base-start
  =.  nockchain-start-height.constants  nock-start
  constants
::
++  create-deposit-settlement
  |=  $:  =beid
          counterpart=nname:t
          as-of=nock-hash
          nock-height=@
          dest=base-addr
          settled-amount=coins
          nonce=@
      ==
  ^-  deposit-settlement
  [beid [counterpart as-of nock-height dest settled-amount nonce]]
::
++  create-deposit
  |=  $:  =tx-id
          name=nname:t
          dest=(unit base-addr)
          amount-to-mint=coins
          fee=coins
      ==
  ^-  deposit
  :*  tx-id
      name
      dest
      amount-to-mint
      fee
  ==
::
++  create-withdrawal
  |=  $:  =beid
          dest=nock-lock-root
          amount-burned=coins
      ==
  ^-  withdrawal
  :*  beid
      dest
      amount-burned
  ==
::
++  seed-unsettled-withdrawal
  |=  $:  state=bridge-state
          event-id=beid
          dest=nock-lock-root
          amount-burned=coins
      ==
  ^-  [withdrawal-id bridge-state]
  =/  wd=withdrawal
    (create-withdrawal event-id dest amount-burned)
  =|  withdrawals=(z-map beid withdrawal)
  =.  withdrawals
    (~(put z-by withdrawals) event-id wd)
  =/  deposit-settlements=(z-map beid deposit-settlement)
    *(z-map beid deposit-settlement)
  =/  blocks=base-blocks
    (make-base-blocks state withdrawals deposit-settlements)
  =/  as-of=base-hash
    (hash:base-blocks blocks)
  =.  base-hashchain.hash-state.state
    (~(put z-by base-hashchain.hash-state.state) as-of blocks)
  =.  last-base-blocks.hash-state.state  as-of
  =.  unsettled-withdrawals.hash-state.state
    (~(put z-by unsettled-withdrawals.hash-state.state) as-of withdrawals)
  :-  [as-of (to-atom:blist event-id)]
  state
::
++  create-withdrawal-proposal
  |=  $:  id=withdrawal-id
          recipient=nock-lock-root
          amount=coins
          amount-burned=coins
          base-batch-end=@
          epoch=@
      ==
  ^-  withdrawal-proposal
  :*  id
      recipient
      amount
      amount-burned
      base-batch-end
      epoch
      [1 *block-id:t]
      ~
      *transaction:wt
  ==
::
::  make a spendable bridge-owned v1 note using the bridge test config key.
++  create-selected-withdrawal-note
  |=  [config=node-config source-hash=hash:t assets=coins:t]
  ^-  selected-withdrawal-note
  (create-selected-withdrawal-note-at-origin config source-hash assets 0)
::
++  create-selected-withdrawal-note-at-origin
  |=  $:  config=node-config
          source-hash=hash:t
          assets=coins:t
          origin-page=page-number:t
      ==
  ^-  selected-withdrawal-note
  =/  signer=schnorr-pubkey:t
    %-  from-sk:schnorr-pubkey:t
    (to-atom:schnorr-seckey:t my-nock-key.config)
  =/  [name=nname:t note=nnote-1:v1:t]
    (build-v1-note-at-origin:whel source-hash assets [~ signer] origin-page)
  [name note]
::
++  make-base-blocks
  |=  $:  state=bridge-state
          withdrawals=(z-map beid withdrawal)
          deposit-settlements=(z-map beid deposit-settlement)
      ==
  ^-  base-blocks
  (make-base-blocks-at-height state base-start-height.constants.state withdrawals deposit-settlements)
::
++  make-base-blocks-at-height
  |=  $:  state=bridge-state
          height=@
          withdrawals=(z-map beid withdrawal)
          deposit-settlements=(z-map beid deposit-settlement)
      ==
  ^-  base-blocks
  =/  blocks=(z-map @ [bid=bbid parent=bbid])
    *(z-map @ [bid=bbid parent=bbid])
  :*  %base
      version=%0
      first-height=height
      last-height=height
      blocks
      withdrawals
      deposit-settlements
      prev=last-base-blocks.hash-state.state
  ==
::
++  produce-nock-block
  |=  $:  state=bridge-state
          deposits=(z-map nname:t deposit)
          withdrawal-settlements=(z-map nname:t withdrawal-settlement)
      ==
  ^-  nock-block
  =/  chain  nock-hashchain.hash-state.state
  =/  prev-hash  last-nock-block.hash-state.state
  =/  height=@
    ?.  =(*hash:t prev-hash)
      +(height:(~(got z-by chain) prev-hash))
    nockchain-start-height.constants.state
  :*  %nock
      version=%0
      height
      block-id=[`@ux`height 0x0 0x0 0x0 0x0]
      deposits=deposits
      withdrawal-settlements
      prev=prev-hash
  ==
++  add-nockchain-blocks
  |=  $:  state=bridge-state
          deposits=(z-map nname:t deposit)
          withdrawal-settlements=(z-map nname:t withdrawal-settlement)
      ==
  ^-  [nock-block bridge-state]
  =/  chain  nock-hashchain.hash-state.state
  =/  block  (produce-nock-block state deposits withdrawal-settlements)
  =/  block-hash  (hash:nock-block block)
  =.  last-nock-block.hash-state.state  block-hash
  =.  nock-hashchain.hash-state.state  (~(put z-by chain) block-hash block)
  =.  nock-hashchain-next-height.hash-state.state  +(nock-hashchain-next-height.hash-state.state)
  :-  block
  state
::
++  brg
  (bridge-ker *@uvI)
++  input
  input:brg
++  ovum
  ovum:brg
::
++  build-ovum
  |=  cau=cause
  ^-  ovum
  =/  =input  *input
  [/poke/one-punch/0 input(cause cau)]
::
++  lod
  |=  [state=bridge-state brg=_brg]
  ^-  _brg
  =<  .(desk-hash.outer [~ *@uvI])
  =/  outer-state=outer-state:brg
    ;;  outer-state:brg
    [%0 [~ *@uvI] state]
  (load:brg outer-state)
::
++  pok
  |=  [num=@ cau=cause brg=_brg]
  ^-  [(list effect) _brg]
  =^  effs=(list *)  brg
  =<  [- +(desk-hash.outer [~ *@uvI])]
  (poke:brg num (build-ovum cau))
  =/  effects=(list effect)
    %+  murn  effs
    |=  e=*
    =/  maybe=(unit effect)
      ((soft effect) e)
    ?^  maybe  maybe
    ~
  [effects brg]
::
++  inner-state
  |=  brg=_brg
  ^-  bridge-state
  internal:outer:brg
--
