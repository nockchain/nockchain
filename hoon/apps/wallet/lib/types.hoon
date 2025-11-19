/=  transact  /common/tx-engine
/=  zo  /common/zoon
/=  *  /common/zose
/=  dumb  /apps/dumbnet/lib/types
/=  s10  /apps/wallet/lib/s10
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
+$  coil-data  $+(coil-data [=key =cc])
::
::
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
+$  coil-v0  $+(coil-v0 [%coil coil-data])
+$  coil-v1  $+(coil-v1 coil-v0)
+$  coil-v2  $+(coil-v2 coil-v1)
++  coil-v3
  =<  form
  |%
    +$  form
      $+  coil-v3
      $%  [%0 coil-data]
          [%1 coil-data]
      ==
    ++  get
      |_  =form
      ++  key
        ^-  ^key
        ?-    -.form
            %0  key.form
            %1  key.form
        ==
      ++  cc
        ^-  ^cc
        ?-    -.form
            %0  cc.form
            %1  cc.form
        ==
      ++  keyc
        ^-  keyc:s10
        ::  [key chaincode version]
        [p.key cc -]:form
      --:: +get
    ::
    ++  extended-key
      |=  =form
      ^-  @t
      =/  =keyc:s10  ~(keyc get form)
      ?:  ?=(%pub -.key.form)
        extended-public-key:(from-public:s10 keyc)
      extended-private-key:(from-private:s10 keyc)
    ::
    ++  coinbase-first-name
      |=  =form
      ^-  hash:transact
      ?>  ?=(%pub -.key.form)
      ?-    -.form
          %0
        =/  key=schnorr-pubkey:transact
          (from-ser:schnorr-pubkey:transact p.key.form)
        (coinbase:v0:first-name:transact key)
      ::
          %1
        =/  key-hash=hash:transact
          %-  hash:schnorr-pubkey:transact
          (from-ser:schnorr-pubkey:transact p.key.form)
        (coinbase:v1:first-name:transact key-hash)
      ==
    ::
    ++  simple-first-name
      |=  =form
      ^-  hash:transact
      ?>  ?=(%pub -.key.form)
      ?-    -.form
          %0
        =/  key=schnorr-pubkey:transact
          (from-ser:schnorr-pubkey:transact p.key.form)
        (simple:v0:first-name:transact key)
      ::
          %1
        =/  key-hash=hash:transact
          %-  hash:schnorr-pubkey:transact
          (from-ser:schnorr-pubkey:transact p.key.form)
        (simple:v1:first-name:transact key-hash)
      ==
    ::
    ++  to-b58
      |_  =form
      ++  key
        ^-  @t
        =/  key=@ux  p.key.form
        (crip (en:base58:wrap key))
      ::
      ++  address
        ^-  @t
        ?>  ?=(%pub -.key.form)
        ?-    -.form
            %0  ::  return b58 pubkey
          (crip (en:base58:wrap p.key.form))
        ::
            %1    ::  return b58 pkh address
          %-  to-b58:hash:transact
          %-  hash:schnorr-pubkey:transact
          (from-ser:schnorr-pubkey:transact p.key.form)
        ==
      --
  --
++  coil  coil-v3
::
::    $meta: stored metadata for a key
::
+$  meta-v0
  $%  coil-v0
      [%label p=@t]
      [%seed p=@t]
      [%watch-key p=@t]
  ==
+$  meta-v1  $+(meta-v1 meta-v0)
+$  meta-v2  $+(meta-v2 meta-v1)
+$  meta-v3
  $+  meta-v3
  $%  [%coil p=coil-v3]
      [%label p=@t]
      [%seed p=@t]
      [%watch-key p=@t]
  ==
+$  meta  meta-v3
::
::    $keys: path indexed map for keys
::
::  path -> value format for keys state:
::
::  /keys                                                           ::  root path (holds nothing in its fil)
::  /keys/watch/[t/watch-key] -> [coil/watch-key]                   ::  watch key
::  /keys/[t/master]/[key-type]/m -> [coil/key]                     ::  master key
::  /keys/[t/master]/[key-type]/[ud/index] -> [coil/key]            ::  derived key
::  /keys/[t/master]/[key-type]/[ud/index]/label -> [label/label]   ::  key label for derived key
::  /keys/[t/master]/[key-type]/m/label -> [label/label]            ::  key label for master key
::  /keys/[t/master]/seed -> [seed/seed-phrase]                     ::  seed-phrase
::
::  where:
::  - the entry after -> is the non-unit .fil (see the $axal definition in zose)
::  - [t/master] is the base58 encoded address corresponding to the master pubkey.
::     -  for v0, this is the base58 encoded master public key as @t.
::     -  for v1 or higher, this is the base58 encoded hash of the master public key.
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
+$  keys-v0  $+(keys-axal (axal meta-v0))
+$  keys-v1  $+(keys-axal keys-v0)
+$  keys-v2  $+(keys-axal keys-v1)
+$  keys-v3  $+(keys-axal (axal meta-v3))
+$  keys-v4  $+(keys-axal keys-v3)
+$  keys  keys-v4
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
::  +active: active master public address
++  active-v0  $+(active-v0 (unit coil-v0))
++  active-v1  $+(active-v1 active-v0)
++  active-v2  $+(active-v2 active-v1)
::
++  active-v3
  =<  form
  |%
    +$  form  $+(active-v3 (unit coil))
    ++  public
      |=  =form
      ?:  ?=(^ form)
        ?.  ?=(%pub -.key.u.form)
          ~|('fatal: active master public key set to a private key' !!)
        u.form
      ~|("active master public key not found" !!)
    ::
    ::  returns active master address
    ++  to-b58
      |=  =form
      ^-  @t
      =/  pubcoil=coil  (public form)
      ~(address to-b58:coil pubcoil)
  --
++  active-v4  active-v3
++  active  active-v4
::
::
::  The format of the lock-data that is written into the
::  $note-data field of seeds generated by the wallet.
::  This data is stored under the %lock key in $note-data.
+$  lock-data
  $%  [%0 =lock:transact]
  ==
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
::
+$  balance-v0  $+(balance-v0 (z-map:zo nname:transact nnote:transact))
+$  balance-v1  $+(balance-v1 balance-v0)
+$  balance-v2
  $:  height=page-number:transact
      block-id=hash:transact   :: block hash of balance
      notes=(z-map:zo nname:transact nnote:v0:transact)  :: notes
  ==
+$  balance-v3
  $:  height=page-number:transact
      block-id=hash:transact   :: block hash of balance
      notes=(z-map:zo nname:transact nnote:transact)  :: notes
  ==
+$  balance-v4  $+(balance-v4 balance-v3)
+$  balance  balance-v4
::
+$  ledger-v0-engine
  %-  list
  $:  name=nname:transact
      recipient=sig:transact
      gifts=coins:transact
      =timelock-intent:transact
  ==
::
  +$  state-0
    $:  %0
        balance=balance-v0
        hash-to-name=(z-map:zo hash:transact nname:transact)  ::  hash of note -> name of note
        name-to-hash=(z-map:zo nname:transact hash:transact)  ::  name of note -> hash of note
        receive-address=lock:transact
        active-master=active-v0
        keys=keys-v0
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
  +$  state-1
    $:  %1
        balance=balance-v1
        active-master=active-v1
        keys=keys-v1
        last-block=(unit block-id:transact)
        peek-requests=$+(peek-requests (map @ud ?(%balance %block)))
        active-transaction=(unit transaction-name)
        active-input=(unit input-name)
        active-seed=(unit seed-name)             ::  currently selected seed
        transaction-tree=transaction-tree                    ::  structured tree of transactions, inputs, and seeds
        pending-commands=(z-map:zo @ud [phase=?(%block %balance %ready) wrapped=cause])  ::  commands waiting for sync
    ==
  ::
  +$  state-2
    $:  %2
        balance=balance-v2
        active-master=active-v2
        keys=keys-v2
        last-block=(unit block-id:transact)
        peek-requests=$+(peek-requests (map @ud ?(%balance %block)))
        active-transaction=(unit transaction-name)
        active-input=(unit input-name)
        active-seed=(unit seed-name)             ::  currently selected seed
        transaction-tree=transaction-tree                    ::  structured tree of transactions, inputs, and seeds
        pending-commands=(z-map:zo @ud [phase=?(%block %balance %ready) wrapped=cause])  ::  commands waiting for sync
    ==
  ::
  +$  state-3
    $:  %3
        balance=balance-v3
        active-master=active-v3
        keys=keys-v3
        last-block=(unit block-id:transact)
        peek-requests=$+(peek-requests (map @ud ?(%balance %block)))
        active-transaction=(unit transaction-name)
        active-input=(unit input-name)
        active-seed=(unit seed-name)             ::  currently selected seed
        transaction-tree=transaction-tree                    ::  structured tree of transactions, inputs, and seeds
        pending-commands=(z-map:zo @ud [phase=?(%block %balance %ready) wrapped=cause])  ::  commands waiting for sync
    ==
  ::
  +$  state-4
    $:  %4
        balance=balance-v4
        active-master=active-v4
        keys=keys-v4
    ==
  ::
  ::  $versioned-state: wallet state
  ::
  +$  versioned-state
    $%  state-0
        state-1
        state-2
        state-3
        state-4
    ==
  ::
  +$  state  $>(%4 versioned-state)
  ::
  +$  seed-name   $~('default-seed' @t)
  ::
  +$  transaction-name  $~('default-transaction' @t)
  ::
  +$  input-name  $~('default-input' @t)
  ::
  +$  order  [recipient=hash:transact gift=coins:transact]
  ::
  ::
  +$  grpc-bind-cause
    $%  [%grpc-bind result=*]
    ==
  ::
  ++  key-version  ?(%0 %1)
  +$  cause
    $%  [%keygen entropy=byts salt=byts]
        [%derive-child i=@ hardened=? label=(unit @tas)]
        [%import-keys keys=(list (pair trek *))]
        [%import-extended extended-key=@t]                ::  extended key string
        [%watch-address address=@t]                ::  imports base58-encoded pubkey
        [%export-keys ~]
        [%export-master-pubkey ~]
        [%import-master-pubkey coil=*]                    ::  base58-encoded pubkey + chain code
        grpc-bind-cause
        [%send-tx dat=transaction]
        [%show-tx dat=transaction]
        [%sign-message msg=@ sign-key=(unit [child-index=@ud hardened=?])]
        [%verify-message msg=@ sig=@ pk-b58=@t]
        [%sign-hash hash-b58=@t sign-key=(unit [child-index=@ud hardened=?])]
        [%verify-hash hash-b58=@t sig=@ pk-b58=@t]
        [%list-notes-by-address address=@t]                 ::  base58-encoded address
        [%list-notes-by-address-csv address=@t]             ::  base58-encoded address, CSV format
        $:  %create-tx
            names=(list [first=@t last=@t])               ::  base58-encoded name hashes
            orders=(list order)
            fee=coins:transact                            ::  fee
            sign-key=(unit [child-index=@ud hardened=?])  ::  child key information to sign from
            refund-pkh=(unit hash:transact)               ::  refund pkh for spends over v0 notes
            include-data=?                                ::  whether or not we should include note-data. defaults
                                                          ::  to yes in cli. not including note-data is a power-user option because
                                                          ::  if the lock is not a standard 1-of-1 pkh or coinbase, the wallet won't
                                                          ::  be able to guess it, so the funds could be lost forever if the user.
                                                          ::  doesn't keep track of the lock.
            save-raw-tx=?                                 ::  if %.y, saves jams of the raw-tx and its hashable into a txs-debug folder
                                                          ::  in the current working directory
        ==
        [%list-active-addresses ~]
        [%list-notes ~]
        [%show-seed-phrase ~]
        [%show-master-zpub ~]
        [%show-master-zprv ~]
        [%show =path]
        [%import-seed-phrase seed-phrase=@t version=key-version]
        [%update-balance-grpc balance=*]
        [%set-active-master-address address-b58=@t]
        [%list-master-addresses ~]
        [%file file-cause]
    ==
  +$  file-cause
    $%  [%write path=@t contents=@t success=?]
        [%batch-write result=(list [path=@t contents=@t success=?])]
    ==
  ::
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
  +$  input-mask
    $~  [%.n *spend-mask]
    $:  note=?
        spend=spend-mask
    ==
  ::
  +$  preinput  [name=@t (pair input:transact input-mask)]
  ::
  +$  transaction  [name=@t p=spends:transact]
  ::
  ::
  +$  nockchain-grpc-effect
    $%  [%send-tx tx=raw-tx:v1:transact]
    ==
  ::
  +$  effect
    $%  file-effect
        [%markdown @t]
        [%raw *]
        [%grpc grpc-effect]
        [%nockchain-grpc nockchain-grpc-effect]
        [%exit code=@]
        grpc-effect
    ==
  ::
  +$  file-effect
    $%  [%file %read path=@t]
        [%file %write path=@t contents=@]
        [%file %batch-write files=(list [path=@t contents=@])]
    ==
  ::
  +$  grpc-effect
    $%  [%poke pid=@ $>(%fact cause:dumb)]
        [%peek pid=@ typ=@tas path=path]
    ==
  ::
  +$  upgrade-effect
    $%  [%upgrade-1-to-2 ~]
    ==
--
