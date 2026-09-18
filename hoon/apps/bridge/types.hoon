::  bridge node types and utilities
::
::    types and helper functions for the nockchain bridge system.
::
/=  t  /common/tx-engine
/=  *   /common/zeke
/=  *   /common/zoon
/=  *  /common/wrapper
/=  wt  /apps/wallet/lib/types
/=  dumb  /apps/dumbnet/lib/types
::>)  TODO: review all hashables in types.hoon
::
~%  %bridge-types  ..ut  ~
|%
::
::    $node-config: bridge node configuration
::
::  contains the identity of this node and the full configuration
::  of all 5 bridge nodes including their network addresses and
::  cryptographic keys for both ethereum and nockchain.
::
+$  node-config-legacy
  $:  =node-id                       :: which of the 5 nodes (0-4)
      nodes=(list node-info)         :: all 5 node configs
      my-eth-key=eth-seckey          :: this node's eth private key
      my-nock-key=schnorr-seckey:t   :: this node's nock private key
  ==
::
+$  node-config
  $:  =node-id                       :: which of the 5 nodes (0-4)
      nodes=(list node-info)         :: all 5 node configs
      =bridge-lock-root              :: active bridge multisig lock root for this environment
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
++  beid  blist  ::  internal base event id key, stored as a based list for z-map safety
++  btid  blist  ::  based tx id (Base chain tx ID as based list)
++  bbid  blist  ::  based block id (Base chain block ID as based list)
+$  epoch  @     ::  withdrawal proposal / attempt index
::
::  alises so we can differentiate between nock and base blocks hashes
++  nock-hash  $+(nock-hash hash:t)
++  base-hash  $+(base-hash hash:t)
::
::  TODO: should probably be called lock-root?
++  nock-lock-root  hash:t
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
+$  bridge-state-0
  $:  %0
      config=node-config-legacy                             ::  node configuration
      constants=bridge-constants                            ::  static bridge parameters
      hash-state=hash-state-0                               ::  hashlogged cross-chain state
      next-nonce=$~(1 @)                                    ::  DEPRECATED: runtime assigns deposit nonces
      last-block=page:t                                     ::  for determining proposer
      =bridge-lock-root                                     ::  script hash: receive address for bridge deposits
      stop=(unit stop-info)                                 ::  flag to stop the bridge. populated with last known good block hashes if stop is true.
  ==
::
+$  bridge-state-1
  $:  %1
      config=node-config-legacy                             ::  node configuration
      constants=bridge-constants                            ::  static bridge parameters
      hash-state=hash-state-1                               ::  hashlogged cross-chain state
      last-nock-deposit-height=@                            ::  last nockchain height containing a deposit (0 = none)
      last-block=page:t                                     ::  for determining proposer
      =bridge-lock-root                                     ::  script hash: receive address for bridge deposits
      stop=(unit stop-info)                                 ::  flag to stop the bridge. populated with last known good block hashes if stop is true.
  ==
::
+$  bridge-state-2
  $:  %2
      config=node-config-legacy                             ::  node configuration
      constants=bridge-constants                            ::  static bridge parameters
      nockchain-constants=(unit blockchain-constants-v1-pre-ai:dumb) ::  pre-Logos node-reported constants
      hash-state=hash-state-2-old                           ::  hashlogged cross-chain state
      last-nock-deposit-height=@                            ::  last nockchain height containing a deposit (0 = none)
      last-block=page:t                                     ::  for determining proposer
      =bridge-lock-root                                     ::  script hash: receive address for bridge deposits
      stop=(unit stop-info)                                 ::  flag to stop the bridge. populated with last known good block hashes if stop is true.
  ==
::
+$  bridge-state-3
  $:  %3
      config=node-config                                    ::  node configuration
      constants=bridge-constants                            ::  static bridge parameters
      nockchain-constants=(unit blockchain-constants-v1-pre-ai:dumb) ::  pre-Logos node-reported constants
      hash-state=hash-state-2                               ::  hashlogged cross-chain state
      last-nock-deposit-height=@                            ::  last nockchain height containing a deposit (0 = none)
      last-block=page:t                                     ::  for determining proposer
      stop=(unit stop-info)                                 ::  flag to stop the bridge. populated with last known good block hashes if stop is true.
  ==
::
+$  bridge-state-4
  $:  %4
      config=node-config                                    ::  node configuration
      constants=bridge-constants                            ::  static bridge parameters
      nockchain-constants=(unit blockchain-constants:t)     ::  node-reported tx-engine constants from boot-time handshake
      hash-state=hash-state-2                               ::  hashlogged cross-chain state
      last-nock-deposit-height=@                            ::  last nockchain height containing a deposit (0 = none)
      last-block=page:t                                     ::  for determining proposer
      stop=(unit stop-info)                                 ::  flag to stop the bridge. populated with last known good block hashes if stop is true.
  ==
::
+$  bridge-state-5
  $:  %5
      config=node-config                                    ::  node configuration
      constants=bridge-constants                            ::  static bridge parameters
      nockchain-constants=(unit blockchain-constants:t)     ::  node-reported tx-engine constants from boot-time handshake
      hash-state=hash-state-3                               ::  hashlogged cross-chain state
      last-nock-deposit-height=@                            ::  last nockchain height containing a deposit (0 = none)
      last-block=page:t                                     ::  for determining proposer
      stop=(unit stop-info)                                 ::  flag to stop the bridge. populated with last known good block hashes if stop is true.
  ==
::
::
+$  versioned-bridge-state
  $%  bridge-state-0
      bridge-state-1
      bridge-state-2
      bridge-state-3
      bridge-state-4
      bridge-state-5
  ==
::
+$  bridge-state  bridge-state-5
++  upgrade-pre-ai-constants
  |=  old=blockchain-constants-v1-pre-ai:dumb
  ^-  blockchain-constants:t
  =/  current=blockchain-constants:t  *blockchain-constants:t
  :*  v1-phase.old
      bythos-phase.old
      data.old
      base-fee.old
      input-fee-divisor.old
      :*  max-block-size.old
          blocks-per-epoch.old
          target-epoch-duration.old
          update-candidate-interval.old
          max-future-timestamp.old
          min-past-blocks.old
          genesis-target-atom.old
          max-target-atom.old
          check-pow-flag.old
          coinbase-timelock-min.old
          pow-len.old
          max-coinbase-split.old
          first-month-coinbase-min.old
      ==
      :*  asert-phase.old
          asert-anchor-height.old
          asert-anchor-target-atom.old
          asert-ideal-block-time.old
          asert-half-life.old
          asert-anchor-min-timestamp.old
      ==
      zk-asert-post-ai.current
      ai-pow-activation-height.current
      ai-asert.current
  ==
::
++  upgrade-pre-logos-state
  |=  old=bridge-state-3
  ^-  bridge-state-4
  =/  upgraded-constants=(unit blockchain-constants:t)
    ?~  nockchain-constants.old
      ~
    [~ (upgrade-pre-ai-constants u.nockchain-constants.old)]
  :*  %4
      config.old
      constants.old
      upgraded-constants
      hash-state.old
      last-nock-deposit-height.old
      last-block.old
      stop.old
  ==
::
++  upgrade-deferred-settlement-state
  |=  old=bridge-state-4
  ^-  bridge-state
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
        deferred-deposit-settlements     *(z-mip nock-hash beid deposit-settlement)
        deferred-withdrawal-settlements  *(z-mip base-hash nname:t withdrawal-settlement)
        pending-base-block-commit  pending-base-block-commit.hash-state.old
    ==
  :*  %5
      config.old
      constants.old
      nockchain-constants.old
      new-hash-state
      last-nock-deposit-height.old
      last-block.old
      stop.old
  ==
::
++  resume-bridge-state
  |=  old=bridge-state
  ^-  bridge-state
  =.  stop.old  ~
  =.  nock-hold.hash-state.old  ~
  =.  base-hold.hash-state.old  ~
  old
::
::
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
+$  hash-state  hash-state-3
++  hash-state-0
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
        ::  unsettled-withdrawals are tracked by the gross/pre-fee amount
        ::  burned on Base. observed settlements carry the net/post-fee amount
        ::  disbursed on Nockchain. kernel reconciliation enforces only basic
        ::  amount bounds, not exact fee correctness.
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
++  hash-state-1
  =<  form
  |%
  +$  form
    $+  hash-state-1
    $:  version=%1
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
        ::  track unsettled asset allocations
        ::
        ::  For each hashchain we need:
        ::
        ::  unsettled-deposits tracks deposits
        ::  which have confirmed on Nockchain but for which our node has never
        ::  seen a settlement transaction.
        ::
        ::  If a deposit is not in this set we will not sign a settlement transaction for it.
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
        ::  unsettled-withdrawals are tracked by the gross/pre-fee amount
        ::  burned on Base. observed settlements carry the net/post-fee amount
        ::  disbursed on Nockchain. kernel reconciliation enforces only basic
        ::  amount bounds, not exact fee correctness.
        ::
        ::  withdrawals are removed from this set when we the transaction settling
        ::  them is posted on nockchain
        unsettled-withdrawals=(z-mip base-hash beid withdrawal)
        ::
    ==
  --
++  hash-state-2-old
  =<  form
  |%
  +$  form
    $+  hash-state-2-old
    $:  version=%2
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
        ::  track unsettled asset allocations
        ::
        ::  For each hashchain we need:
        ::
        ::  unsettled-deposits tracks deposits
        ::  which have confirmed on Nockchain but for which our node has never
        ::  seen a settlement transaction.
        ::
        ::  If a deposit is not in this set we will not sign a settlement transaction for it.
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
        ::  unsettled-withdrawals are tracked by the gross/pre-fee amount
        ::  burned on Base. observed settlements carry the net/post-fee amount
        ::  disbursed on Nockchain. kernel reconciliation enforces only basic
        ::  amount bounds, not exact fee correctness.
        ::
        ::  withdrawals are removed from this set when we the transaction settling
        ::  them is posted on nockchain
        unsettled-withdrawals=(z-mip base-hash beid withdrawal)
    ==
  --
++  hash-state-2
  =<  form
  |%
  +$  form
    $+  hash-state-2
    $:  version=%2
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
        ::  track unsettled asset allocations
        ::
        ::  For each hashchain we need:
        ::
        ::  unsettled-deposits tracks deposits
        ::  which have confirmed on Nockchain but for which our node has never
        ::  seen a settlement transaction.
        ::
        ::  If a deposit is not in this set we will not sign a settlement transaction for it.
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
        ::  unsettled-withdrawals are tracked by the gross/pre-fee amount
        ::  burned on Base. observed settlements carry the net/post-fee amount
        ::  disbursed on Nockchain. kernel reconciliation enforces only basic
        ::  amount bounds, not exact fee correctness.
        ::
        ::  withdrawals are removed from this set when we the transaction settling
        ::  them is posted on nockchain
        unsettled-withdrawals=(z-mip base-hash beid withdrawal)
        ::
        ::  Base batches waiting for Rust to durably persist derived withdrawal
        ::  requests before the hashchain is advanced.
        pending-base-block-commit=(unit pending-base-block-commit-data)
    ==
  --
::
++  hash-state-3
  =<  form
  |%
  +$  form
    $+  hash-state-3
    $:  version=%3
        ::
        ::  hashchains
        last-nock-block=nock-hash
        last-base-blocks=base-hash
        nock-hashchain=(z-map nock-hash nock-block)
        base-hashchain=(z-map base-hash base-blocks)
        ::
        ::  Legacy single-dependency holds. Version %3 no longer creates new
        ::  holds; unresolved cross-chain settlements are persisted below and
        ::  reconciled when their counterpart blocks arrive.
        nock-hold=(unit [hash=base-hash height=@])
        base-hold=(unit [hash=nock-hash height=@])
        ::
        ::  Next Nockchain block height required for the hashchain.
        nock-hashchain-next-height=nockchain-start-height
        ::
        ::  Next highest-in-the-batch height required for the Base hashchain.
        base-hashchain-next-height=base-start-height
        ::
        ::  Confirmed cross-chain events not yet observed as settled.
        unsettled-deposits=(z-mip nock-hash nname:t deposit)
        unsettled-withdrawals=(z-mip base-hash beid withdrawal)
        ::
        ::  Settlements observed before their referenced future counterpart.
        ::  Each value-release counterpart is globally unique across hash
        ::  buckets. Reconciliation requires the exact source hash and position,
        ::  except for the one complete immutable mainnet migration fact defined
        ::  below; unknown stale or cross-hash dependencies stop before effects.
        deferred-deposit-settlements=(z-mip nock-hash beid deposit-settlement)
        deferred-withdrawal-settlements=(z-mip base-hash nname:t withdrawal-settlement)
        ::
        ::  Base batches waiting for Rust to durably persist derived withdrawal
        ::  requests before the hashchain is advanced.
        pending-base-block-commit=(unit pending-base-block-commit-data)
    ==
  --
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
+$  pending-base-block-withdrawals
  $:  blocks-hash=base-hash
      first-height=@
      last-height=@
      withdrawals=(list nock-withdrawal-request:effect)
  ==
::
+$  pending-base-block-commit-data
  $:  blocks=base-blocks
      metadata=pending-base-block-withdrawals
  ==
::
+$  base-block-commit-ack
  $:  blocks-hash=base-hash
      first-height=@
      last-height=@
  ==
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
+$  withdrawal-id  [as-of=base-hash =base-event-id]
+$  withdrawal-snapshot  [height=@ block-id=block-id:t]
++  withdrawal-proposal
  =<  form
  |%
  +$  form
    $+  withdrawal-proposal
    $:  id=withdrawal-id
        recipient=nock-lock-root
        amount=coins
        amount-burned=coins
        base-batch-end=@
        =epoch
        snapshot=withdrawal-snapshot
        selected-notes=(list nname:t)
        transaction=transaction:wt
    ==
  ++  hashable-selected-notes
    |=  inputs=(list nname:t)
    ^-  hashable:tip5
    ?~  inputs  leaf+~
    :-  (hashable:nname i.inputs)
    $(inputs t.inputs)
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    :*  hash+as-of.id.form
        leaf+base-event-id.id.form
        hash+recipient.form
        leaf+amount.form
        leaf+amount-burned.form
        leaf+base-batch-end.form
        leaf+epoch.form
        leaf+height.snapshot.form
        hash+block-id.snapshot.form
        (hashable-selected-notes selected-notes.form)
        :: TODO: check this
        leaf+name.transaction.form
    ==
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  --
+$  selected-withdrawal-note
  $:  name=nname:t
      note=nnote:t
  ==
+$  create-withdrawal-tx
  $:  id=withdrawal-id
      recipient=nock-lock-root
      amount=coins
      amount-burned=coins
      base-batch-end=@
      =epoch
      snapshot=withdrawal-snapshot
      fee=coins
      selected-notes=(list selected-withdrawal-note)
  ==
+$  withdraw-info  [%0 =beid =base-hash lock-root=nock-lock-root base-batch-end=@]
++  withdrawal
  =<  form
  |%
  +$  form
    $+  withdrawal
    $:  =beid
        dest=nock-lock-root
        amount-burned=coins
    ==
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    ::>)  TODO: check hashable
    :*  (hashable:beid beid.form)
        hash+dest.form
        leaf+amount-burned.form
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
::
::  Exact mainnet identity of the note restored at block 46,849.
++  mainnet-legacy-deposit-name
  ^-  nname:t
  :*  [0xf480.0376.e5c6.138d 0x9a4c.e7c6.94db.95f1 0x6c18.a134.f480.fde0 0xbe1c.4b92.e6d4.61d0 0x6c6d.671d.8d73.ef3b]
      [0xf68c.c7dd.f2ba.7818 0x828a.9a6d.3dcf.f822 0x409f.62b1.3f56.88d9 0x46ea.2f97.f8f8.c4d7 0x561d.0332.2829.9954]
      ~
  ==
::
::  This one settlement was finalized before the repaired lineage and before
::  the bounded historical range below. Its complete Base event and Nock
::  deposit identities form a one-shot migration fact.
++  mainnet-pre-repair-deposit-settlement
  ^-  [settlement=deposit-settlement counterpart=deposit]
  =/  name=nname:t
    :*  [0xf480.0376.e5c6.138d 0x9a4c.e7c6.94db.95f1 0x6c18.a134.f480.fde0 0xbe1c.4b92.e6d4.61d0 0x6c6d.671d.8d73.ef3b]
        [0xb7be.bb2f.38cc.3b5d 0x313f.a17d.ecd8.7360 0x5f43.e97c.c3c3.29df 0xe7f0.bbec.e834.6559 0x7ac4.8e78.ae60.b21e]
        ~
    ==
  =/  event-id=beid
    ~[0xc570.d8e0.2619.70ed 0x1ca7.7790.11b0.8188 0x4ea6.2025.9c8f.d321 0x940c.a776.f9d0.b9d7]
  =/  historical-as-of=nock-hash
    :*  0x5215.1cbb.b7b7.6221
        0xb849.bf6c.5152.fdef
        0x12fc.4026.9f99.941f
        0x62aa.8941.0da3.12e8
        0xc0d6.c5c3.a5b1.19f0
    ==
  =/  recipient=base-addr  0x9ff8.e289.ef6b.a33c.6a82.491a.445f.b57a.740d.2150
  =/  settlement=deposit-settlement
    :*  event-id
        name
        historical-as-of
        48.325
        recipient
        9.801.150.000
        120
    ==
  =/  counterpart=deposit
    :*  [0x883c.b79e.ad8b.7eb0 0x7a63.af71.e499.2184 0x2f6d.876b.caec.e4bb 0xb578.65a0.564d.cefa 0x693d.c6c9.1074.25b2]
        name
        `recipient
        9.801.150.000
        29.250.000
    ==
  [settlement counterpart]
::
::  Base deposit nonces 121 through 529 were finalized against the old Nock
::  hash lineage before the repaired chain became authoritative. Base enforces
::  strictly increasing nonces, so this closed range cannot admit a future
::  event. The height and bridge-root bounds are the observed endpoints of the
::  immutable mainnet event range.
++  mainnet-pre-repair-lineage-settlement
  |=  [constants=bridge-constants settlement=deposit-settlement]
  ^-  ?
  =/  bridge-root=hash:t
    [0xf480.0376.e5c6.138d 0x9a4c.e7c6.94db.95f1 0x6c18.a134.f480.fde0 0xbe1c.4b92.e6d4.61d0 0x6c6d.671d.8d73.ef3b]
  ?&  =(46.810 nockchain-start-height.constants)
      =(39.694.000 base-start-height.constants)
      =(bridge-root -.counterpart.settlement)
      (gte nonce.settlement 121)
      (lte nonce.settlement 529)
      (gte nock-height.settlement 48.390)
      (lte nock-height.settlement 146.586)
  ==
:::
++  withdrawal-settlement
  =<  form
  |%
  +$  form
    $+  withdrawal-settlement
    $:  =tx-id
        =nname
        counterpart=beid
        base-batch-end=@
        as-of=base-hash
        dest=nock-lock-root
        ::  net/post-fee amount disbursed on Nockchain
        settled-amount=coins
    ==
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    :*  hash+tx-id.form
        (hashable:nname nname.form)
        (hashable:beid counterpart.form)
        leaf+base-batch-end.form
        hash+as-of.form
        hash+dest.form
        leaf+settled-amount.form
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
++  check-withdrawal-settlement
  |=  $:  counterpart=withdrawal
          settlement=withdrawal-settlement
      ==
  =/  dest-matches=?  =(dest.settlement dest.counterpart)
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
          [%set-blockchain-constants constants=blockchain-constants:t]
          [%base-blocks raw-base-blocks]
          [%base-block-withdrawals-committed ack=base-block-commit-ack]
          [%nockchain-block nockchain-block]
          [%create-withdrawal-tx create-withdrawal-tx]
          [%sign-tx withdrawal-proposal]
          [%proposed-nock-tx withdrawal-proposal]
          [%stop last=stop-info]
          [%start ~]
      ==
    ==
  ::
  +$  raw-base-blocks  (list [height=@ block-id=base-block-id parent-block-id=base-block-id txs=(list base-event)])
  ::
  +$  nockchain-block  [block=page:t txs=(z-map tx-id:t tx:t)]
  ::
  --
::
++  effect
  =<  form
  |%
  +$  form
    $+  effect
    $:  %0
      $%  [%create-withdrawal-txs reqs=(list nock-withdrawal-request)]
          [%base-block-withdrawals-pending pending=pending-base-block-withdrawals]
          [%withdrawal-proposal-built proposal=withdrawal-proposal]
          [%withdrawal-tx-signed proposal=withdrawal-proposal]
          [%commit-nock-deposits reqs=(list nock-deposit-request)]
          [%grpc grpc-effect]
          [%stop reason=cord last=stop-info]
      ==
    ==
  ::
  ::      bytes32 (nockchain) txId,
  ::      bytes name,
  ::      address recipient,
  ::      uint256 amount,
  ::      uint256 blockHeight,
  ::      bytes32 asOf,
  ::
  ::  proposal hash is computed as:
  ::  keccak256(abi.encode(txId, name, recipient, amount, blockHeight, asOf, depositNonce))
  ::
  ::  NOTE: The kernel does not assign deposit nonces. The Rust runtime assigns
  ::  nonces deterministically and constructs the final proposal hash/signature
  ::  request used for contract submission.
  ::
  +$  nock-deposit-request
    [tx-id=tx-id:t name=nname:t recipient=base-addr amount=@ block-height=@ as-of=nock-hash]
  ::
  +$  nock-withdrawal-request
    [=base-event-id recipient=nock-lock-root amount=@ base-batch-end=@ as-of=base-hash]
  ::
  ::
  +$  grpc-effect
    $%  [%peek pid=@ typ=@tas =path]
        [%call ip=@t method=@tas data=*]
    ==
  --
--
