/=  transact  /common/tx-engine
/=  zo  /common/zoon
/=  *  /common/zose
/=  dumb  /apps/dumbnet/lib/types
|%
::    $key: public or private key
::
::   both private and public keys are in serialized cheetah point form
::   they MUST be converted to base58 for export.
::
+$  key
  $~  [%pub p=*@ux]
  $%  [%pub p=@ux]
      [%prv p=@ux]
  ==
::    $coil: key and chaincode
::
::  a wallet consists of a collection of +coil (address and entropy pair). the
::  $cc (called a chain code elsewhere) allows for the deterministic
::  generation of child keys from a parent key without compromising other
::  branches of the hierarchy.
::
::  .key:  public or private key
::  .cc: associated entropy (chain code)
::
++  coil
  =<  form
  |%
    +$  form  [%coil =key =cc]
    ::
    ++  to-b58
      |=  =form
      ^-  [key-b58=@t cc-b58=@t]
      :-  (crip (en:base58:wrap p.key.form))
      (crip (en:base58:wrap cc.form))
  --
::
::    $meta: stored metadata for a key
+$  meta
  $%  coil
      [%label @t]
      [%seed @t]
  ==
::
::    $keys: path indexed map for keys
::
::  path format for keys state:
::
::  /keys                                                        ::  root path (holds nothing in its fil)
::  /keys/[t/master]/[key-type]/m/[coil/key]                     ::  master key path
::  /keys/[t/master]/[key-type]/[ud/index]/[coil/key]            ::  derived key path
::  /keys/[t/master]/[key-type]/[ud/index]/[coil/key]            ::  specific key path
::  /keys/[t/master]/[key-type]/[ud/index]/label/[label/label]   ::  key label path for derived key
::  /keys/[t/master]/[key-type]/m/label/[label/label]            ::  key label path for master key
::  /keys/[t/master]/seed/[seed/seed-phrase]                     ::  seed-phrase path
::
::  Note the terminal entry of the path holds that value, this value is the
::  non-unit `fil` in the $axal definition
::
::  where:
::  - [t/master] is the base58 encoded master public key as @t
::  - m denotes the master key
::  - [ud/index] is the derivation index as @ud
::  - [key-type] is either %pub or %prv
::  - [coil/key] is the key and chaincode pair. key is in serialized
::               format as a @ux, NOT base58.
::  - [seed/seed-phrase] is the seed phrase as a tape
::  - [label/label] is a label value
::
::  master key is stored under 'm'.
::  derived keys use incrementing indices starting from 0 under their master-key and key-type
::  labels are stored as children of their associated keys.
::  seed is a seed phrase and is only stored as a child of [t/master]
::
+$  keys  $+(keys-axal (axal meta))
::
::    $transaction-tree: structured tree of transaction, input, and seed data
::
::  we use the axal structure to track the relationship between transactions,
::  inputs, and seeds. this allows us to navigate the tree and maintain
::  all the relationships without duplicating data.
::
::  paths in the transaction-tree follow these conventions:
::
::  /transaction/[transaction-name]                    :: transaction node
::  /transaction/[transaction-name]/input/[input-name] :: input in a transaction
::  /input/[input-name]                    :: input node
::  /input/[input-name]/seed/[seed-name]   :: seed in an input
::  /seed/[seed-name]                      :: seed node
::
::  +master: master key pair
++  master
  =<  form
  |%
    +$  form  (unit coil)
    ++  public
      |=  =form
      ?:  ?=(^ form)
        u.form
      ~|("master public key not found" !!)
    ::
    ++  to-b58
      |=  =form
      ^-  @t
      (crip (en:base58:wrap p.key:(public form)))
  --
::
+$  transaction-tree
  $+  wallet-transaction-tree
  (axal transaction-entity)
::    $transaction-entity: entities stored in the transaction tree
::
+$  transaction-entity
  $%  [%transaction =transaction-name =transaction]
      [%input =input-name =preinput]
      [%seed =seed-name =preseed]
  ==
::
::  $cc: chaincode
::
+$  cc  @ux
::  $balance: wallet balance
+$  balance
  $+  wallet-balance
  (z-map:zo nname:transact nnote:transact)
::
+$  ledger
  %-  list
  $:  name=nname:transact
      recipient=lock:transact
      gifts=coins:transact
      =timelock-intent:transact
  ==
  ::
  ::  $versioned-state: wallet state
  ::
  +$  versioned-state
    $%
      $:  %0
          =balance
          hash-to-name=(z-map:zo hash:transact nname:transact)  ::  hash of note -> name of note
          name-to-hash=(z-map:zo nname:transact hash:transact)  ::  name of note -> hash of note
          receive-address=lock:transact
          =master
          =keys
          transactions=$+(transactions (map * transaction))
          last-block=(unit block-id:transact)
          peek-requests=$+(peek-requests (map @ud ?(%balance %block)))
          active-transaction=(unit transaction-name)
          active-input=(unit input-name)
          active-seed=(unit seed-name)             ::  currently selected seed
          transaction-tree=transaction-tree        ::  structured tree of transactions, inputs, and seeds
          pending-commands=(z-map:zo @ud [phase=?(%block %balance %ready) wrapped=cause])  ::  commands waiting for sync
      ==
      ::
      $:  %1
          =balance
          =master
          =keys
          last-block=(unit block-id:transact)
          peek-requests=$+(peek-requests (map @ud ?(%balance %block)))
          active-transaction=(unit transaction-name)
          active-input=(unit input-name)
          active-seed=(unit seed-name)             ::  currently selected seed
          transaction-tree=transaction-tree                    ::  structured tree of transactions, inputs, and seeds
          pending-commands=(z-map:zo @ud [phase=?(%block %balance %ready) wrapped=cause])  ::  commands waiting for sync
      ==
    ==
  ::
  +$  state  $>(%1 versioned-state)
  ::
  +$  seed-name   $~('default-seed' @t)
  ::
  +$  transaction-name  $~('default-transaction' @t)
  ::
  +$  input-name  $~('default-input' @t)
  ::
  +$  order
    $%  [%multiple recipients=(list [m=@ pks=(list @t)]) gifts=(list coins:transact)]
        [%single recipient=[m=@ pks=(list @t)] gift=coins:transact]
    ==
  ::
  ::
  +$  grpc-bind-cause
    $%  [%grpc-bind result=*]
    ==
  ::
  +$  cause
    $%  [%keygen entropy=byts salt=byts]
        [%derive-child i=@ hardened=? label=(unit @tas)]
        [%import-keys keys=(list (pair trek meta))]
        [%import-extended extended-key=@t]                ::  extended key string
        [%export-keys ~]
        [%export-master-pubkey ~]
        [%import-master-pubkey =coil]                     ::  base58-encoded pubkey + chain code
        grpc-bind-cause
        [%send-tx dat=transaction]
        [%show-tx dat=transaction]
        [%sign-message msg=@ sign-key=(unit [child-index=@ud hardened=?])]
        [%verify-message msg=@ sig=@ pk-b58=@t]
        [%sign-hash hash-b58=@t sign-key=(unit [child-index=@ud hardened=?])]
        [%verify-hash hash-b58=@t sig=@ pk-b58=@t]
        [%list-notes-by-pubkey pubkey=@t]                 ::  base58-encoded pubkey
        [%list-notes-by-pubkey-csv pubkey=@t]             ::  base58-encoded pubkey, CSV format
        $:  %create-tx
            names=(list [first=@t last=@t])               ::  base58-encoded name hashes
            =order
            fee=coins:transact                            ::  fee
            sign-key=(unit [child-index=@ud hardened=?])  ::  child key information to sign from
            =timelock-intent:transact                     ::  timelock constraint
        ==
        [%sign-tx dat=transaction sign-key=(unit [child-index=@ud hardened=?]) entropy=@]
        [%list-pubkeys ~]
        [%list-notes ~]
        [%show-seedphrase ~]
        [%show-master-pubkey ~]
        [%show-master-privkey ~]
        [%show =path]
        [%gen-master-privkey seedphrase=@t]
        [%gen-master-pubkey privkey-b58=@t cc-b58=@t]
        [%update-balance-grpc balance=*]
        $:  %scan
            master-pubkey=@t              ::  base58 encoded master public key to scan for
            search-depth=$~(100 @ud)      ::  how many addresses to scan (default 100)
            include-timelocks=$~(%.n ?)   ::  include timelocked notes (default false)
            include-multisig=$~(%.n ?)    ::  include notes with multisigs (default false)
        ==
        [%advanced-spend advanced-spend]
        [%file %write path=@t contents=@t success=?]
    ==
  ::
  +$  advanced-spend
    $%  [%seed advanced-spend-seed]
        [%input advanced-spend-input]
        [%transaction advanced-spend-transaction]
    ==
  ::
  +$  advanced-spend-seed
    $%  [%new name=@t]                          ::  new empty seed in transaction
        $:  %set-name
            seed-name=@t
            new-name=@t
        ==
        $:  %set-source                                  ::  set .output-source
            seed-name=@t
            source=(unit [hash=@t is-coinbase=?])
        ==
        $:  %set-recipient                               ::  set .recipient
            seed-name=@t
            recipient=[m=@ pks=(list @t)]
        ==
        $:  %set-timelock                                ::  set .timelock-intent
            seed-name=@t
            absolute=timelock-range:transact
            relative=timelock-range:transact
        ==
        $:  %set-gift
            seed-name=@t
            gift=coins:transact
        ==
        $:  %set-parent-hash
            seed-name=@t
            parent-hash=@t
        ==
        $:  %set-parent-hash-from-name
            seed-name=@t
            name=[@t @t]
        ==
        $:  %print-status                                ::  do the needful
            seed-name=@t
        ==
    ==
  ::  $seed-mask: tracks which fields of a $seed:transact have been set
  ::
  ::    this might have been better as a "unitized seed" but would have been
  ::    much more annoying to read the code
  +$  seed-mask
    $~  [%.n %.n %.n %.n %.n]
    $:  output-source=?
        recipient=?
        timelock-intent=?
        gift=?
        parent-hash=?
    ==
  ::  $preseed: a $seed:transact in process of being built
  +$  preseed  [name=@t (pair seed:transact seed-mask)]
  ::
  ::  $spend-mask: tracks which field of a $spend:transact have been set
  +$  spend-mask
    $~  [%.n %.n %.n]
    $:  signature=?
        seeds=?
        fee=?
    ==
  ::
  +$  advanced-spend-input
    ::  there is only one right way to create an $input from a $spend, so we don't need
    ::  the mask or other commands.
    $%  [%new name=@t]                                   :: new empty input
        $:  %set-name
            input-name=@t
            new-name=@t
        ==
        $:  %add-seed
            input-name=@t
            seed-name=@t
        ==
        $:  %set-fee
            input-name=@t
            fee=coins:transact
        ==
        $:  %set-note-from-name                          ::  set .note using .name
            input-name=@t
            name=[@t @t]
        ==
        $:  %set-note-from-hash                          ::  set .note using hash
            input-name=@t
            hash=@t
        ==
        $:  %derive-note-from-seeds                      ::  derive note from seeds
            input-name=@t
        ==
        $:  %remove-seed
            input-name=@t
            seed-name=@t
        ==
        $:  %remove-seed-by-hash
            input-name=@t
            hash=@t
        ==
        $:  %print-status
            input-name=@t
        ==
    ==
  ::
  +$  input-mask
    $~  [%.n *spend-mask]
    $:  note=?
        spend=spend-mask
    ==
  ::
  +$  preinput  [name=@t (pair input:transact input-mask)]
  ::
  +$  transaction  [name=@t p=inputs:transact]
  ::
  +$  advanced-spend-transaction
    $%  [%new name=@t]                                    ::  new input transaction
        $:  %set-name
            transaction-name=@t
            new-name=@t
        ==
        $:  %add-input
            transaction-name=@t
            input-name=@t
        ==
        $:  %remove-input
            transaction-name=@t
            input-name=@t
        ==
        $:  %remove-input-by-name
            transaction-name=@t
            name=[first=@t last=@t]
        ==
        [%print-status =transaction-name]                            ::  print transaction status
    ==
  ::
  +$  effect
    $%  file-effect
        [%markdown @t]
        [%raw *]
        [%grpc grpc-effect]
        [%exit code=@]
    ==
  ::
  +$  file-effect
    $%
      [%file %read path=@t]
      [%file %write path=@t contents=@]
    ==
  ::
  +$  grpc-effect
    $%  [%poke pid=@ $>(%fact cause:dumb)]
        [%peek pid=@ typ=@tas path=path]
    ==
--
