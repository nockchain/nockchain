::  bridge node types and utilities
::
::    types and helper functions for the nockchain bridge system.
::
/=  t  /common/tx-engine
/=  *   /common/zeke
/=  *   /common/zoon
/=  *  /common/wrapper
/=  dumb  /apps/dumbnet/lib/types
::>)  TODO: review all hashables in types.hoon
::
|%
::
::    $node-config: bridge node configuration
::
::  contains the identity of this node and the full configuration
::  of all 5 bridge nodes including their network addresses and
::  cryptographic keys for both ethereum and nockchain.
::
+$  node-config
  $:  =node-id                       :: which of the 5 nodes (0-4)
      nodes=(list node-info)         :: all 5 node configs
      my-eth-key=eth-seckey          :: this node's eth private key
      my-nock-key=schnorr-seckey:t   :: this node's nock private key
  ==
::
::    $node-info: information about a bridge node
::
::  contains network address and public keys for both ethereum
::  and nockchain. used for gRPC communication and signature
::  verification.
::
+$  node-info
  $:  ip=@t                          :: node ip/hostname
      eth-pubkey=eth-pubkey          :: ethereum public key
      nock-pkh=hash:t                :: nockchain public key hash
  ==
::
::    $node-id: simple identifier for a node
+$  node-id  @
::
::  $eth-seckey: ethereum secp256k1 secret key
+$  eth-seckey  @
::
::  $eth-pubkey: ethereum secp256k1 public key
+$  eth-pubkey  @
::
::  $eth-signature: ethereum secp256k1 signature
+$  eth-signature
  $:  r=@ux
      s=@ux
      v=@ud
  ==
::
::  $evm-address: raw 20-byte ethereum address
+$  evm-address  @ux
::
::    $evm-address-based: ethereum address in based representation
::
::  stored as list of base field elements for note-data compatibility
::
+$  evm-address-based  [@ux @ux @ux]
::
++  base-addr  evm-address
::
::  base-event-id is base-tx-id + log index.
::
::  every base tx has like an array of events
::  the log index is the index into that array
::  so the tx-id + that index will give you a unique id for every event
::
++  base-event-id  @
++  base-tx-id  @
::
::  $blist: based list - lossless representation of arbitrary atoms as based values
::
::    atoms from Base chain (event IDs, tx IDs, block IDs) can exceed the
::    base field prime p, which causes crashes in z-map operations since
::    tip5 hash requires based values for both keys and values.
::
::    blist stores atoms as a list of based field elements, which is:
::    - lossless (can convert back to original atom)
::    - safe for z-map keys and values (all elements < p)
::    - compatible with tip5 hashing
::
::    modeled after atom-to-digest/digest-to-atom in ztd/three.hoon
::
++  blist
  =<  form
  |%
  +$  form  $+(blist (list @))
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    leaf+form
  ::
  ::  +from-atom: convert any atom to a based list (lossless)
  ++  from-atom
    |=  n=@
    ^-  form
    ?:  (based n)  ~[n]
    =/  [q=@ r=@]  (dvr n p)
    [r $(n q)]
  ::
  ::  +to-atom: convert a based list back to an atom (inverse of +from-atom)
  ++  to-atom
    |=  l=form
    ^-  @
    ?~  l  0
    %+  roll  (flop l)
    |=  [belt=@ acc=@]
    (add belt (mul p acc))
  ::
  ::  +valid: check all elements are based
  ++  valid
    |=  l=form
    ^-  ?
    (levy l based)
  --
::
::  semantic aliases for blist - use these in type signatures for clarity
::
++  beid  blist  ::  base event id (Base chain tx ID added to log index as a based list)
++  btid  blist  ::  based tx id (Base chain tx ID as based list)
++  bbid  blist  ::  based block id (Base chain block ID as based list)
::
::  alises so we can differentiate between nock and base blocks hashes
++  nock-hash  $+(nock-hash hash:t)
++  base-hash  $+(base-hash hash:t)
::
::  TODO: should probably be called lock-root?
++  nock-addr  hash:t
::
::
++  coins  coins:t
::
++  tx-id  tx-id:t
::
++  block-id  block-id:t
::
++  nname  nname:t
::
::
++  base-block-id  @
::
++  nock-pubkey  schnorr-pubkey:t
::
::  // Solidity Events
::    event DepositProcessed(
::        bytes32 indexed txId,
::        // TODO need nname
::        address indexed recipient,
::        uint256 amount,
::        uint256 blockHeight,
::        bytes32 asOf
::    );
::
::    event BridgeNodeUpdated(
::        uint256 indexed index,
::        address indexed oldNode,
::        address indexed newNode
::    );
::
::    event BurnForWithdrawal(
::        address indexed burner,
::        uint256 amount,
::        bytes32 indexed lockRoot,
::    );
::
+$  base-event
  $:  =base-event-id
      $=  content
      $%  [%deposit-processed nock-tx-id=tx-id nock-note-name=nname recipient=base-addr amount=@ block-height=@ as-of=hash:t nonce=@]
          [%bridge-node-updated ~]
          [%burn-for-withdrawal burner=base-addr amount=@ lock-root=hash:t]
      ==
  ==
::
:::    hashchain molds
:::
+$  min-signers  $~(3 @)
+$  total-signers  $~(5 @)
+$  minimum-event-nocks  $~(100.000 @)  ::  100,000 nock event = 300 nock fee
+$  nicks-fee-per-nock  $~(195 @)  ::  2^16 * 0.003 = 196.6, rounded down to nearest factor of 5 for easy division between the bridge nodes
+$  base-blocks-chunk  $~(100 @)
+$  base-start-height  $~(39.694.000 @)
::>)  !TODO!: set this to the proper cutoff for the bridge to start accepting deposits
+$  nockchain-start-height  $~(46.810 @)
::
++  bridge-lock-root-default
  (from-b58:hash:t 'AcsPkuhXQoGeEsF91yynpm1kcW17PQ2Z1MEozgx7YnDPkZwrtzLuuqd')
+$  bridge-lock-root  $~(bridge-lock-root-default hash:t)
::
++  bridge-constants
  =<  form
  |%
  +$  form
    $+  bridge-constants
    $:  version=%0
        =min-signers
        =total-signers
        =minimum-event-nocks
        =nicks-fee-per-nock
        =base-blocks-chunk
        =base-start-height
        =nockchain-start-height
    ==
  --
::
+$  stop-info  [base=[hash=base-hash height=@] nock=[hash=nock-hash height=@]]
::
::  $bridge-state: state of the bridge
+$  bridge-state
  $:  %0
      config=node-config                                    ::  node configuration
      constants=bridge-constants                            ::  static bridge parameters
      =hash-state                                           ::  hashlogged cross-chain state
      next-nonce=$~(1 @)                                    ::  nonce to track processed deposits
      last-block=page:t                                     ::  for determining proposer
      =bridge-lock-root                                     ::  script hash: receive address for bridge deposits
      stop=(unit stop-info)                                 ::  flag to stop the bridge. populated with last known good block hashes if stop is true.
  ==
++  get-stop-info
  |=  state=bridge-state
  ^-  stop-info
  =+  hs=hash-state.state
  :-  :-  last-base-blocks.hs
      ?:  =(0 base-hashchain-next-height.hs)  0
      (dec base-hashchain-next-height.hs)
  :-  last-nock-block.hs
  ?:  =(0 nock-hashchain-next-height.hs)  0
  (dec nock-hashchain-next-height.hs)
::
+$  process-result  (each bridge-state process-fail)
+$  process-fail
  $%  [%stop msg=@t]
      [%hold hold=[=hash:t height=@]]
  ==
::
:::
++  hash-state
  =<  form
  |%
  +$  form
    $+  hash-state
    $:  version=%0
        ::
        ::  hashchains
        last-nock-block=nock-hash
        last-base-blocks=base-hash
        nock-hashchain=(z-map nock-hash nock-block)
        base-hashchain=(z-map base-hash base-blocks)
        ::
        ::
        ::  nock-hold blocks the advancement of nock hashchain until the
        ::  the base block with the specified hash is processed
        nock-hold=(unit [hash=base-hash height=@])
        ::
        ::  base-hold blocks the advancement of base hashchain until the
        ::  the nock block with the specified hash is processed
        base-hold=(unit [hash=nock-hash height=@])
        ::
        :: Next Nockchain block height required for the hashchain
        nock-hashchain-next-height=nockchain-start-height
        ::
        :: Next highest-in-the-batch height required for the BASE hashchain
        base-hashchain-next-height=base-start-height
        ::
        ::  TODO: track the as-of cursor note for the bridge and aggregate all assets here
        bridge-treasury-note=nname
        ::
        ::  TODO: track the new bridge deposit notes since last withdrawal
        bridge-deposit-notes=(z-set nname)
        ::
        ::  track unsettled asset allocations
        ::
        ::  For each hashchain we need two sets:
        ::
        ::  unsettled-deposits tracks deposits
        ::  which have confirmed on Nockchain but for which our node has never
        ::  seen a settlement transaction.
        ::
        ::  If a deposit is not in this set we will not sign a settlement transaction for it.
        ::
        ::  unconfirmed-settled-deposits tracks deposits for which we have seen a deposit user action,
        ::  but which we have not yet seen confirmed on BASE. This allows us to
        ::  validate the BASE consensus.
        ::
        ::  unsettled-withdrawals and unconfirmed-settled-withdrawals work similarly,
        ::  but for withdrawals initiated on BASE and settled on Nockchain
        ::
        ::  unsettled-deposits are tracked post-fee, so the amount recorded
        ::  is the exact amount which should be minted in Base NOCK
        ::
        ::  deposits are removed from this set when we propose, sign, or
        ::  observe a transaction settling them, even if not posted to or
        ::  confirmed on BASE
        ::
        ::  Populated by nock hashchain: when a nock block is confirmed,
        ::  deposit notes are added to this set
        unsettled-deposits=(z-mip nock-hash nname:t deposit)
        ::
        ::  unconfirmed-settled-deposits may have a signed transaction settling,
        ::  but not yet confirmed on BASE
        ::
        ::  deposits are removed from this set when the deposit settlement is
        ::  confirmed on BASE
        ::
        ::  Populated by deposits removed from unsettled deposits
        ::
        ::  Note that during BASE hashchain validation, we may encounter a
        ::  a deposit settlement which we never observed prior to confirmation,
        ::  so if a deposit is not in this set 'unsettled-deposits' should also
        ::  be checked and the deposit should be removed from that set and *not*
        ::  added to this one
        unconfirmed-settled-deposits=(z-mip nock-hash nname:t deposit)
        ::
        ::  unsettled-withdrawals are tracked pre-fee, so the amount recorded
        ::  is exact amount which is burned on Base NOCK, but not the exact amount which will be received on nockchain
        ::
        ::  withdrawals are removed from this set when we propose, sign, or
        ::  observe a transaction settling them, even if not posted to or
        ::  confirmed on Nockchain
        unsettled-withdrawals=(z-mip base-hash beid withdrawal)
        ::
        ::  unconfirmed-settled-withdrawals may have a signed transaction settling,
        ::  but not yet confirmed on Nockchain
        ::
        ::  withdrawals are removed from this set when the withdrawal settlement is
        ::  confirmed on Nockchain
        unconfirmed-settled-withdrawals=(z-mip base-hash beid withdrawal)
    ==
  --
:::
++  nock-block
  =<  form
  |%
  +$  form
    $+  nock-block
    $:  %nock
        version=%0
        height=@
        =block-id
        deposits=(z-map nname:t deposit)  ::  deposit request
        withdrawal-settlements=(z-map nname:t withdrawal-settlement)
        prev=nock-hash
    ==
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    :*  [%leaf %nock]
        [%leaf version.form]
        [%leaf height.form]
        [%hash block-id.form]
        (hashable-deposits:deposit deposits.form)
        (hashable-withdrawal-settlements:withdrawal-settlement withdrawal-settlements.form)
        [%hash prev.form]
    ==
  ::
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  --
:::
++  base-blocks
  =<  form
  |%
  +$  form
    $+  base-blocks
    $:  %base
        version=%0
        first-height=@
        last-height=@
        ::>) TODO: check the sequence of hashes and that the parent of the
        ::  first block in a new batch matches the last block in the previous batch
        blocks=(z-map @ [bid=bbid parent=bbid])
        withdrawals=(z-map beid withdrawal)
        deposit-settlements=(z-map beid deposit-settlement)
        prev=base-hash
    ==
  ++  hashable-blocks
    |=  blk-map=(z-map @ [bid=bbid parent=bbid])
    ^-  hashable:tip5
    %-  hashable-block-list
    ~(tap z-by blk-map)
  ++  hashable-block-list
    |=  entries=(list [@ [bid=bbid parent=bbid]])
    ^-  hashable:tip5
    ?~  entries  leaf+~
    :-  (hashable-block-pair i.entries)
    $(entries t.entries)
  ++  hashable-block-pair
    |=  (pair @ [bid=bbid parent=bbid])
    ^-  hashable:tip5
    ::  blist elements are already based, safe to use directly
    :*  leaf+p
        (hashable:bbid bid.q)
        (hashable:bbid parent.q)
    ==
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    :*
      [%leaf %base]
      [%leaf version.form]
      [%leaf first-height.form]
      [%leaf last-height.form]
      (hashable-blocks:base-blocks blocks.form)
      (hashable-withdrawals:withdrawal withdrawals.form)
      (hashable-deposit-settlements:deposit-settlement deposit-settlements.form)
      [%hash prev.form]
    ==
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  ::
  ++  first-block
    |=  =form
    (~(got z-by blocks.form) first-height.form)
  ::
  ++  last-block
    |=  =form
    (~(got z-by blocks.form) last-height.form)
  ::
  ++  nth-block
    |=  [=form n=@]
    ?>  ?&  (gte n first-height.form)
            (lte n last-height.form)
        ==
    (~(got z-by blocks.form) n)
  --
:::
++  deposit
  =<  form
  |%
  +$  form
    $+  deposit
    $:  =tx-id
        =nname
        dest=(unit base-addr)
        amount-to-mint=coins
        fee=coins
    ==
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    :*  hash+tx-id.form
        (hashable:nname nname.form)
        (hashable-dest dest.form)
        leaf+amount-to-mint.form
        leaf+fee.form
    ==
  ++  hashable-dest
    |=  dest=(unit base-addr)
    ^-  hashable:tip5
    ?~  dest
      leaf+~
    leaf+(evm-address-to-based u.dest)
  ++  hashable-deposits
    |=  mp=(z-map nname form)
    ^-  hashable:tip5
    %-  hashable-deposit-list
    ~(tap z-by mp)
  ++  hashable-deposit-list
    |=  entries=(list [nname form])
    ^-  hashable:tip5
    ?~  entries  leaf+~
    :-  (hashable-deposit-pair i.entries)
    $(entries t.entries)
  ++  hashable-deposit-pair
    |=  (pair nname form)
    ^-  hashable:tip5
    :*  (hashable:nname p)
        (hashable q)
    ==
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  --
++  withdrawal
  =<  form
  |%
  +$  form
    $+  withdrawal
    $:  =beid
        dest=nock-addr
        amount-burned=coins
        fee=coins
    ==
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    ::>)  TODO: check hashable
    :*  (hashable:beid beid.form)
        hash+dest.form
        leaf+amount-burned.form
        leaf+fee.form
    ==
  ++  hashable-withdrawals
    |=  mp=(z-map beid form)
    ^-  hashable:tip5
    %-  hashable-withdrawal-list
    ~(tap z-by mp)
  ++  hashable-withdrawal-list
    |=  entries=(list [beid form])
    ^-  hashable:tip5
    ?~  entries  leaf+~
    :-  (hashable-withdrawal-pair i.entries)
    $(entries t.entries)
  ++  hashable-withdrawal-pair
    |=  (pair beid form)
    ^-  hashable:tip5
    :*  (hashable:beid p)
        (hashable q)
    ==
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  --
::
++  bridge-fee
  =<  form
  |%
  +$  form
    $+  bridge-fee  @
  ++  calculate
    |=  [nicks=@ nicks-fee-per-nock=@]
    ^-  @
    =/  [nocks=@ nicks=@]  (dvr nicks nicks-per-nock:t)
    ::  round up to the nearest nock if there is a remainder
    =?  nocks  (gth nicks 0)
      +(nocks)
    (mul nicks-fee-per-nock nocks)
  --
:::
::  data format for bridge deposit, stored under %bridge key in note-data map.
+$  bridge-deposit-data
 $:  %0
     [%base addr=evm-address-based]
 ==
::
+$  deposit-intent  [name=nname recipient=(unit evm-address) amount-to-mint=coins fee=coins]
::
++  deposit-settlement
  =<  form
  |%
  +$  form
    $+  deposit-settlement
    [=beid data-part]
  ::
  +$  data-part
    $:  counterpart=nname:t
        as-of=nock-hash
        nock-height=@
        dest=base-addr
        settled-amount=coins
        nonce=@
    ==
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    :*  (hashable:beid beid.form)
        (hashable:nname counterpart.form)
        hash+as-of.form
        leaf+nock-height.form
        leaf+(evm-address-to-based dest.form)
        leaf+settled-amount.form
        leaf+nonce.form
    ==
  ++  hashable-deposit-settlements
    |=  mp=(z-map beid form)
    ^-  hashable:tip5
    %-  hashable-settlement-list
    ~(tap z-by mp)
  ++  hashable-settlement-list
    |=  entries=(list [beid form])
    ^-  hashable:tip5
    ?~  entries  leaf+~
    :-  (hashable-settlement-pair i.entries)
    $(entries t.entries)
  ++  hashable-settlement-pair
    |=  (pair beid form)
    ^-  hashable:tip5
    :*  (hashable:beid p)
        (hashable q)
    ==
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  --
:::
++  withdrawal-settlement
  =<  form
  |%
  +$  form
    $+  withdrawal-settlement
    $:  =tx-id
        =nname
        counterpart=beid
        as-of=base-hash
        dest=nock-addr
        settled-amount=coins
        nock-tx-fee=coins
    ==
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    :*  hash+tx-id.form
        (hashable:nname nname.form)
        (hashable:beid counterpart.form)
        hash+as-of.form
        hash+dest.form
        leaf+settled-amount.form
        leaf+nock-tx-fee.form
    ==
  ++  hashable-withdrawal-settlements
    |=  mp=(z-map nname form)
    ^-  hashable:tip5
    %-  hashable-settlement-list
    ~(tap z-by mp)
  ++  hashable-settlement-list
    |=  entries=(list [nname form])
    ^-  hashable:tip5
    ?~  entries  leaf+~
    :-  (hashable-settlement-pair i.entries)
    $(entries t.entries)
  ++  hashable-settlement-pair
    |=  (pair nname form)
    ^-  hashable:tip5
    :*  (hashable:nname p)
        (hashable q)
    ==
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  --
::    +active-proposer: determine which node should propose
::
::  computes which bridge node should propose the bundle at a given
::  block height. uses deterministic ordering by sorting node ids
::  by their nockchain public keys, then rotating based on height
::  modulo 5. ensures all nodes agree on proposer at any height.
::
++  active-proposer
  |=  [height=@ud config=node-config]
  ^-  @ud
  =/  node-pairs=(list [@ud node-info])
    %+  turn  (gulf 0 4)
    |=  idx=@ud
    [idx (snag idx nodes.config)]
  =/  node-map=(z-map @ud node-info)
    (z-malt node-pairs)
  =/  sorted-pubkeys=(list @ud)
    %+  sort  (gulf 0 4)
    |=  [a=@ud b=@ud]
    =/  node-a=node-info  (~(got z-by node-map) a)
    =/  node-b=node-info  (~(got z-by node-map) b)
    =/  pkh-a=@t  (to-b58:hash:t nock-pkh.node-a)
    =/  pkh-b=@t  (to-b58:hash:t nock-pkh.node-b)
    (lth pkh-a pkh-b)
  =+  rotation=(mod height 5)
  (snag rotation sorted-pubkeys)
::
::    +active-verifiers: determine primary verification nodes
::
::  returns the two nodes immediately after the proposer in the
::  rotation order. these nodes have primary responsibility for
::  verification, though all nodes can sign. uses same deterministic
::  ordering as active-proposer to ensure byzantine fault tolerance.
::
++  active-verifiers
  |=  [height=@ud config=node-config]
  ^-  (list @ud)
  =/  node-pairs=(list [@ud node-info])
    %+  turn  (gulf 0 4)
    |=  idx=@ud
    [idx (snag idx nodes.config)]
  =/  node-map=(z-map @ud node-info)
    (malt node-pairs)
  =/  sorted-pubkeys=(list @ud)
    %+  sort  (gulf 0 4)
    |=  [a=@ud b=@ud]
    =/  node-a=node-info  (~(got z-by node-map) a)
    =/  node-b=node-info  (~(got z-by node-map) b)
    =/  pkh-a=@t  (to-b58:hash:t nock-pkh.node-a)
    =/  pkh-b=@t  (to-b58:hash:t nock-pkh.node-b)
    (lth pkh-a pkh-b)
  =+  rotation=(mod height 5)
  =/  proposer=@ud  (snag rotation sorted-pubkeys)
  =/  verifier-1=@ud  (snag (add rotation 1) sorted-pubkeys)
  =/  verifier-2=@ud  (snag (add rotation 2) sorted-pubkeys)
  :~  verifier-1
      verifier-2
  ==
::
::    +is-my-turn: check if this node should propose
::
::  returns %.y if this node is the active proposer at the given
::  height, %.n otherwise. used by nodes to determine when to
::  create bundle proposals from pending deposits.
::
++  is-my-turn
  |=  [height=@ud config=node-config]
  ^-  ?
  =/  proposer=@ud  (active-proposer height config)
  ~&  [%is-my-turn-check height=height node-id=node-id.config active-proposer=proposer rotation=(mod height 5)]
  =(node-id.config proposer)
::
::    +is-verifier: check if this node is a primary verifier
::
::  returns %.y if this node is one of the two primary verifiers
::  at the given height. verifiers have first responsibility to
::  validate and sign proposals, though all nodes may participate.
::
++  is-verifier
  |=  [height=@ud config=node-config]
  ^-  ?
  %+  lien  (active-verifiers height config)
  |=  id=@ud
  =(id node-id.config)
::
::    +get-node-by-id: retrieve node info by id
::
::  looks up a node's information by its id (0-4). returns ~
::  if id is out of bounds. used for routing grpc messages
::  and signature verification.
::
++  get-node-by-id
  |=  [config=node-config id=@ud]
  ^-  (unit node-info)
  ?:  (lth id (lent nodes.config))
    `(snag id nodes.config)
  ~
::
::    +evm-address-to-based: convert evm address to based field
::
::  converts a 20-byte ethereum address to a list of base field
::  elements (belts) for storage in note-data. splits the 160-bit
::  address into 3 base field elements of characteristic p = 2^64 - 2^32 + 1
::  by converting the address to a base-p number. this ensures all data in note-data
::  uses based arithmetic.
::
++  evm-address-to-based
  |=  addr=evm-address
  ^-  evm-address-based
  =/  [q=@ a=@]  (dvr addr p)
  =/  [q=@ b=@]  (dvr q p)
  =/  [q=@ c=@]  (dvr q p)
  ?.  =(q 0)  ~|  %invalid-evm-address  !!
  [a b c]
::
::    +based-to-evm-address: convert based field to evm address
::
::  converts a list of base field elements back to a 20-byte
::  ethereum address. takes exactly three chunks,
::  then reconstructs the address from the 32-bit chunks. inverse
::  of evm-address-to-based.
::
++  based-to-evm-address
  |=  addr=evm-address-based
  ^-  evm-address
  =+  [a=@ux b=@ux c=@ux]=addr
  ?.  ?&  (based a)
          (based b)
          (based c)
      ==
    ~|  %evm-address-has-not-based-entries  !!
  =/  p2  (mul p p)
  :(add a (mul p b) (mul p2 c))
::
++  cause
  =<  form
  |%
  +$  form
    $+  cause
    $:  %0
      $%  [%cfg-load config=(unit node-config)]
          [%set-constants constants=bridge-constants]
          [%base-blocks raw-base-blocks]
          [%nockchain-block nockchain-block]
          [%proposed-base-call proposed-base-call]
          [%proposed-nock-tx proposed-nock-tx]  ::  TODO: fill in when we do withdrawals
          [%stop last=stop-info]
          [%start ~]
      ==
    ==
  ::
  +$  raw-base-blocks  (list [height=@ block-id=base-block-id parent-block-id=base-block-id txs=(list base-event)])
  ::
  +$  nockchain-block  [block=page:t txs=(z-map tx-id:t tx:t)]
  ::
  +$  proposed-base-call  (list eth-signature-request:effect)
  ::
  +$  proposed-nock-tx  raw-tx:t
  ::
  +$  base-call-sig  [sig=eth-signature data=@]
  --
::
++  effect
  =<  form
  |%
  +$  form
    $+  effect
    $:  %0
      $%  [%base-call base-call]
          [%assemble-base-call data-part:deposit-settlement]
          [%eth-signature-request eth-signature-request]
          ::  broadcast signature requests to peer nodes for signing
          [%propose-base-call reqs=(list eth-signature-request)]
          [%nockchain-tx nockchain-tx]
          [%propose-nockchain-tx propose-nockchain-tx]
          [%grpc grpc-effect]
          [%stop reason=cord last=stop-info]
      ==
    ==
  ::
  +$  base-call  [sigs=(list eth-signature) data=@]
  ::
  ::      bytes32 (nockchain) txId,
  ::      bytes name,
  ::      address recipient,
  ::      uint256 amount,
  ::      uint256 blockHeight,
  ::      bytes32 asOf,
  ::
  ::  proposal hash is computed as:
  ::  keccak256(abi.encode(txId, name, recipient, amount, blockHeight, asOf))
  ::
  +$  eth-signature-request
    [tx-id=tx-id:t name=nname:t recipient=base-addr amount=@ block-height=@ as-of=nock-hash nonce=@]
  ::
  +$  propose-base-call  [peer=nock-pubkey req=eth-signature-request]
  ::
  +$  nockchain-tx  [tx=raw-tx:t]
  ::
  +$  propose-nockchain-tx  [peer=nock-pubkey data=@]
  ::
  +$  grpc-effect
    $%  [%peek pid=@ typ=@tas =path]
        [%call ip=@t method=@tas data=*]
    ==
  --
--
