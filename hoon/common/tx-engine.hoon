/=  v0  /common/tx-engine-0
/=  v1  /common/tx-engine-1
/=  *  /common/zeke
/=  *  /common/zoon
/=  *  /common/zose
=>  |%
    ++  blockchain-constants  blockchain-constants:v1
    --
|_  blockchain-constants
+*  v0  ~(. ^v0 +15:+<)
::  constants
++  quarter-ted  ^~((div target-epoch-duration 4))
++  quadruple-ted  ^~((mul target-epoch-duration 4))
++  genesis-target  ^~((chunk:bignum genesis-target-atom))
++  max-target  ^~((chunk:bignum max-target-atom))
::
++  bignum  bignum:v0
++  block-commitment  block-commitment:v0
++  block-id  block-id:v0
++  bn  bignum
++  btc-hash  btc-hash:v0
++  check-context  check-context:v1
++  coinbase-split
  =<  form
  |%
  +$  form
    $%  [%0 coinbase-split:v0]
        [%1 coinbase-split:v1]
    ==
  ++  v0
    =<  form
    =+  coinbase-split:^v0
    |%
    +$  form  $|(^form |=(* %&))
    ++  new  ^new
    --
  ++  v1
    =<  form
    =+  coinbase-split:^v1
    |%
    +$  form  $|(^form |=(* %&))
    ++  new  ^new
    --
  ++  based
    |=  =form
    ?-  -.form
      %0  (based:coinbase-split:v0 +.form)
      %1  (based:coinbase-split:v1 +.form)
    ==
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    ?-  -.form
      %0  [leaf+%0 (hashable:coinbase-split:v0 +.form)]
      %1  [leaf+%1 (hashable:coinbase-split:v1 +.form)]
    ==
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  --
++  coins  coins:v0
++  genesis-seal  genesis-seal:v0
++  genesis-template  genesis-template:v0
++  hash  hash:v0
++  local-page
  =<  form
  |%
  +$  form
    $+  local-page
    $^(local-page:v0 local-page:v1)
  ::
  ++  to-page
    |=  lp=form
    ^-  page
    ?^  -.lp  (to-page:local-page:v0 lp)
    (to-page:local-page:v1 lp)
  ::
  ++  get
    |_  =form
    ::
    ++  height
      ^-  page-number
      ?^  -.form  height.form
      height.form
    ::
    ++  digest
      ^-  block-id
      ?^  -.form  digest.form
      digest.form
    ::
    ++  accumulated-work
      ^-  bignum:bn
      ?^  -.form  accumulated-work.form
      accumulated-work.form
    ::
    ++  msg
      ^-  page-msg
      ?^  -.form  msg.form
      msg.form
    ::
    ++  parent
      ^-  block-id
      ?^  -.form  parent.form
      parent.form
    --
  --
++  lock  lock:v1
++  lock-primitive  lock-primitive:v1
++  nname  nname:v1
++  page-msg  page-msg:v0
++  page-number  page-number:v0
++  page-summary  page-summary:v0
++  pkh-signature  pkh-signature:v1
++  proof  proof:v0
++  reason
  |$  object
  (each object term)
++  schnorr-pubkey  schnorr-pubkey:v0
++  schnorr-seckey  schnorr-seckey:v0
++  schnorr-signature  schnorr-signature:v0
++  seed  seed:v0
++  seeds  seeds:v0
++  seed-v1
  =<  form
  =+  seed:v1
  |%
  +$  form  $|(^form |=(* %&))
  ::
  ::  +simple: construct seed from lock-root and gift
  ++  simple
    |=  [lock-root=hash gift=coins parent-hash=hash]
    ^-  form
    %*  .  *^form
      output-source  ~
      lock-root      lock-root
      note-data      *(z-map @tas *)
      gift           gift
      parent-hash    parent-hash
    ==
  --
++  shares  shares:v1
++  sig  sig:v0
++  signature  signature:v0
++  size  size:v0
++  source  source:v0
++  spend  spend:v0
++  timelock  timelock:v0
++  timelock-intent  timelock-intent:v0
++  timelock-range  timelock-range:v0
++  tx-id  tx-id:v0
++  first-name
  =<  form
  |%
  +$  form  $+(first-name hash)
  ++  v0
    |%
    ++  simple-sig-hash
      |=  key=schnorr-pubkey
      ^-  hash
      =/  =sig  [m=1 (z-silt ~[key])]
      (hash:^sig sig)
    ::
    ++  simple
      |=  key=schnorr-pubkey
      ^-  form
      =/  sig-hash  (simple-sig-hash key)
      (first-from-hash:new:nname:v0 sig-hash %.n)
    ::
    ++  coinbase
      |=  key=schnorr-pubkey
      ^-  form
      =/  sig-hash  (simple-sig-hash key)
      (first-from-hash:new:nname:v0 sig-hash %.y)
    --::v0
  ++  v1
    |%
    ++  simple-pkh-lp
      |=  key-hash=hash
      ^-  lock-primitive
      [%pkh [m=1 (z-silt ~[key-hash])]]
    ::
    ++  simple
      |=  key-hash=hash
      ^-  form
      =/  pkh-lock=spend-condition  [(simple-pkh-lp key-hash)]~
      =/  lock-hash  (hash:lock pkh-lock)
      (first:nname (hash:lock pkh-lock))
    ::
    ++  coinbase-pkh-sc
      |=  key-hash=hash
      ^-  spend-condition
      :~  ^-(lock-primitive (simple-pkh-lp key-hash))
          ^-(lock-primitive tim-lp:^coinbase)
      ==
    ::
    ++  coinbase
      |=  key-hash=hash
      ^-  form
      =/  coinbase-lock=spend-condition
        (coinbase-pkh-sc key-hash)
      (first:nname (hash:lock coinbase-lock))
    --::+v1
  --::+first-name
::
++  witness
  =<  form
  =+  witness:v1
  |%
  +$  form  $|(^form |=(* %&))
  ::
  ::  +make-pkh: build witness with pkh signatures for sig-hash
  ++  make-pkh
    |=  $:  root=^hash
            sc=spend-condition
            sig-hash=^hash
            keys=(list [schnorr-seckey schnorr-pubkey])
        ==
    ^-  form
    =/  pmap=pkh-signature:v1
      %+  roll  keys
      |=  $:  kp=[sk=schnorr-seckey pk=schnorr-pubkey]
              acc=pkh-signature:v1
          ==
      =/  sig=schnorr-signature
        %+  sign:affine:belt-schnorr:cheetah
          sk.kp
        sig-hash
      (~(put z-by acc) (hash:schnorr-pubkey pk.kp) [pk.kp sig])
    %*  .  *^form
      lmp  (build-lock-merkle-proof:lock sc 1)
      pkh  pmap
      hax  *(z-map ^hash *)
      tim  ~
    ==
  ::
  ::  +make-hax: build witness for %hax lock with preimage
  ++  make-hax
    |=  [root=^hash sc=spend-condition h=^hash pre=*]
    ^-  form
    %*  .  *^form
      lmp  (build-lock-merkle-proof:lock sc 1)
      pkh  *(z-map ^hash [pk=schnorr-pubkey sig=schnorr-signature])
      hax  (~(put z-by *(z-map ^hash *)) h pre)
      tim  ~
    ==
  ::
  ::  +make-hax-pkh: build witness for combined %hax AND %pkh
  ++  make-hax-pkh
    |=  $:  root=^hash
            sc=spend-condition
            sig-hash=^hash
            keys=(list [schnorr-seckey schnorr-pubkey])
            h=^hash
            pre=*
        ==
    ^-  form
    =/  pmap=pkh-signature:v1
      %+  roll  keys
      |=  $:  kp=[sk=schnorr-seckey pk=schnorr-pubkey]
              acc=pkh-signature:v1
          ==
      =/  sig=schnorr-signature
        %+  sign:affine:belt-schnorr:cheetah
          sk.kp
        sig-hash
      (~(put z-by acc) (hash:schnorr-pubkey pk.kp) [pk.kp sig])
    %*  .  *^form
      lmp  (build-lock-merkle-proof:lock sc 1)
      pkh  pmap
      hax  (~(put z-by *(z-map ^hash *)) h pre)
      tim  ~
    ==
  --::+witness
::
::  TODO: remove
++  lock-from-sig  from-sig:lock
::
++  coinbase
  =<  form
  |%
  +$  form   nnote
  ::
  ++  v0
    |%
    ++  new
      |=  [pag=page lok=sig]
      ^-  form
      ?^  -.pag  (new:coinbase:^v0 pag lok)
      ~|  %v0-coinbase-new-requires-v0-page  !!
    ::
    ++  validate
      |=  [pag=page cb=form]
      ^-  ?
      ?^  -.pag
        ?^  -.cb  (validate:coinbase:^v0 pag cb)
        %.n
      %.n
    ::
    ++  name-from-parent-hash  name-from-parent-hash:coinbase:^v0
    ++  first-month-coinbase-timelock  first-month-coinbase-timelock:coinbase:^v0
    --
  ::
  ++  validate
    |=  [=page =form]
    ^-  ?
    ::  v1 coinbase: check version, origin-page, and source hash
    ?.  ?=(@ -.form)  %|
    ?.  =(origin-page.form ~(height get:^page page))  %|
    =/  src=hash  (hash:source [~(parent get:^page page) %.y])
    =/  got=hash  (source-hash:nnote-1:v1 form)
    =(src got)
  ::
  ++  emission-calc  emission-calc:coinbase:^v0
  ++  tim-lp
    ^-  lock-primitive
    :-  %tim
    :*  rel=[min=`coinbase-timelock-min max=~]
        abs=[min=~ max=~]
    ==
  ::
  ::  +new: make v1 coinbase for page with pkh hashes
  ++  new
    |=  [=page pkh-hashes=(z-set hash)]
    ^-  form
    =/  cb-split=coinbase-split  ~(coinbase get:^page page)
    ?>  ?=(%1 -.cb-split)
    ::  sum rewards for all provided hashes
    =/  reward=coins
      %+  roll  ~(tap z-in pkh-hashes)
      |=  [h=hash acc=coins]
      (add acc (~(got z-by +.cb-split) h))
    %*  .  *nnote-1:v1
      version      %1
      origin-page  ~(height get:^page page)
      name         (make-name pkh-hashes ~(parent get:^page page))
      note-data    *(z-map @tas *)
      assets       reward
    ==
  ::
  ::
  ::  +make-name: build name from pubkey hashes with timelock
  ++  make-name
    |=  [pkh-hashes=(z-set hash) parent=block-id]
    ^-  nname
    ::  build %pkh lock from hashes, m=number of hashes
    =/  m=@  ~(wyt z-in pkh-hashes)
    =/  pkh=lock-primitive  [%pkh [m pkh-hashes]]
    =/  tim=lock-primitive  tim-lp
    =/  lk=lock  ~[pkh tim]
    =/  lmp=lock-merkle-proof  (build-lock-merkle-proof:lock lk 1)
    =/  root=hash  root.merk-proof.lmp
    (new-v1:nname [root [parent %.y]])
  --
::
::  $page: a nockchain block
++  page
  =<  form
  |%
  +$  form
    $+  page
    $^(page:v0 page:v1)
  ::
  ++  v0
    =<  form
    =+  page:^v0
    |%
    +$  form  $|(^form |=(* %&))
    ++  new-candidate  ^new-candidate
    --
  ::
  ::  +new-candidate: build candidate page for mining with v1 shares
  ::
  ::    creates a v1 page with hash-based coinbase-split.
  ++  new-candidate
    |=  [par=form now=@da target-bn=bignum:bn =shares]
    ^-  form
    (new-candidate:page:v1 par now target-bn shares)
  ::
  ++  get
    |_  =form
    ::
    ++  digest
      ^-  block-id
      ?^  -.form  digest.form
      digest.form
    ::
    ++  parent
      ^-  block-id
      ?^  -.form  parent.form
      parent.form
    ::
    ++  height
      ^-  page-number
      ?^  -.form  height.form
      height.form
    ::
    ++  coinbase
      ^-  coinbase-split
      ?^  -.form  [%0 coinbase.form]
      [%1 coinbase.form]
    ::
    ++  tx-ids
      ^-  (z-set tx-id)
      ?^  -.form  tx-ids.form
      tx-ids.form
    ::
    ++  timestamp
      ^-  @
      ?^  -.form  timestamp.form
      timestamp.form
    ::
    ++  epoch-counter
      ^-  @ud
      ?^  -.form  epoch-counter.form
      epoch-counter.form
    ::
    ++  target
      ^-  bignum:bn
      ?^  -.form  target.form
      target.form
    ::
    ++  accumulated-work
      ^-  bignum:bn
      ?^  -.form  accumulated-work.form
      accumulated-work.form
    ::
    ++  msg
      ^-  page-msg
      ?^  -.form  msg.form
      msg.form
    ::
    ++  pow
      ^-  (unit proof)
      ?^  -.form  pow.form
      pow.form
    --
  ::
  ++  txs-size-by-id
    |=  [=form got-raw-tx=$-(tx-id raw-tx)]
    %+  roll
      ~(tap z-in ~(tx-ids get form))
    |=  [=tx-id sum-sizes=size]
    %+  add  sum-sizes
    ~(size get:raw-tx (got-raw-tx tx-id))
  ::
  ++  to-local-page
    |=  pag=form
    ^-  local-page
    ?^  -.pag  (to-local-page:page:v0 pag)
    (to-local-page:page:v1 pag)
  ::
  ++  time-in-secs
    |=  now=@da
    ^-  @
    (time-in-secs:page:v0 now)
  ::
  ++  compute-work
    |=  target-bn=bignum:bn
    ^-  bignum:bn
    (compute-work:page:v0 target-bn)
  ::
  ++  compute-digest
    |=  pag=form
    ^-  block-id
    ?^  -.pag  (compute-digest:page:v0 pag)
    (compute-digest:page:v1 pag)
  ::
  ++  block-commitment
    |=  pag=form
    ^-  ^block-commitment
    ?^  -.pag  (block-commitment:page:v0 pag)
    (block-commitment:page:v1 pag)
  ::
  ++  new-genesis
    |=  [tem=genesis-template timestamp=@da]
    ^-  form
    (new-genesis:page:v0 tem timestamp)
  ::
  ++  to-page-summary
    |=  pag=form
    ^-  page-summary
    :*  ~(digest get pag)
        ~(timestamp get pag)
        ~(epoch-counter get pag)
        ~(target get pag)
        ~(accumulated-work get pag)
        ~(height get pag)
        ~(parent get pag)
    ==
  ::
  ++  check-digest
    |=  pag=form
    ^-  ?
    ?^  -.pag  (check-digest:page:v0 pag)
    (check-digest:page:v1 pag)
  ::
  ++  compute-size-without-txs
    |=  pag=form
    ^-  size
    ?^  -.pag  (compute-size-without-txs:page:v0 pag)
    (compute-size-without-txs:page:v1 pag)
  ::
  ++  compare-heaviness
    |=  [pag1=form pag2=local-page]
    ^-  ?
    %+  gth
      %-  merge:bn
      ~(accumulated-work get pag1)
    %-  merge:bn
    ~(accumulated-work get:local-page pag2)
  --
::
++  lock-merkle-proof
  =<  form
  =+  lock-merkle-proof:v1
  |%
  ++  form  $|(^form |=(* %&))
  ::
  --
::
::  $raw-tx: a raw transaction (v0 or v1)
++  raw-tx
  =<  form
  |%
  +$  form
    $+  raw-tx
    $^(raw-tx:v0 raw-tx:v1)
  ::
  ++  v0
    =<  form
    =+  raw-tx:^v0
    |%
    +$  form  $|(^form |=(* %&))
    ::
    ++  from-inputs
      |^  (corl new into)
      ++  into
        |=  ips=^inputs
        %-  ~(run z-by ips)
        |=  =^input
        ?>  ?=(%0 -.input)
        +.input
      --
    --
  ::
  ::  +simple-from-note: spend v1 note to single lock
  ::
  ::    creates a simple v1 raw-tx spending all assets from
  ::    note to recipient lock using %pkh witness
  ::
  ::    parent-lock: lock guarding the input note (witness proves we can unlock)
  ::    recipient-lock: lock that will guard the output note
  ::    note: the input note being spent
  ::    sk: secret key to sign the witness
  ++  simple-from-note
    |=  $:  parent=sig
            recipient=sig
            note=nnote
            sk=schnorr-seckey
        ==
    ^-  form
    ?>  ?=(@ -.note)
    ::  We only allow simple locks with 1 condition
    =/  parent-lock=lock  (from-sig:lock parent)
    =/  recipient-lock=lock  (from-sig:lock recipient)
    =/  parent-lmp=lock-merkle-proof
      (build-lock-merkle-proof:lock parent-lock 1)
    ::  build seeds for recipient lock
    =/  lock-root=hash  (hash:lock recipient-lock)
    =/  sed=seed:v1
      %*  .  *seed:v1
        lock-root    lock-root
        note-data    *(z-map @tas *)
        gift         assets.note
        parent-hash  (hash:nnote note)
      ==
    =/  seeds=(z-set seed:v1)  (~(put z-in *(z-set seed:v1)) sed)
    =/  sp=spend:v1
      :-  %1
      %*  .  *spend-1:v1
        seeds  seeds
        fee    0
      ==
    =/  pk=schnorr-pubkey
      %-  ch-scal:affine:curve:cheetah
      :*  (t8-to-atom:belt-schnorr:cheetah sk)
          a-gen:curve:cheetah
      ==
    =/  pk-hash=hash  (hash:schnorr-pubkey pk)
    ::  sign the spend
    =/  sig-hash=hash
      (sig-hash:spend:v1 sp)
    =/  sig=schnorr-signature
      %+  sign:affine:belt-schnorr:cheetah
        sk
      sig-hash
    =/  pkh-sig=pkh-signature
      %+  ~(put z-by *(z-map hash [schnorr-pubkey schnorr-signature]))
        pk-hash
      [pk sig]
    ?>  ?=(%1 -.sp)
    =.  witness.sp
      %*  .  *witness
        lmp  parent-lmp
        pkh  pkh-sig
      ==
    =/  sps=spends
      (~(put z-by *(z-map nname spend:v1)) name.note sp)
    (new:raw-tx:v1 sps)
  ::
  ++  get
    |_  =form
    ::
    ++  id
      ^-  tx-id
      ?^  -.form  id.form
      ~(id get:raw-tx:v1 form)
    ::
    ++  size
      ^-  ^size
      ?^  -.form  (compute-size-jam `*`form)
      ~(size get:raw-tx:v1 form)
    ::
    ++  input-names
      ^-  (z-set nname)
      ?^  -.form  (inputs-names:raw-tx:v0 form)
      ~(input-names get:raw-tx:v1 form)
    ::
    --
  ::
  ++  based
    |=  =form
    ^-  ?
    ?^  -.form  (based:raw-tx:v0 form)
    (based:raw-tx:v1 form)
  ::
  ++  compute-id
    |=  =form
    ^-  tx-id
    ?^  -.form  (compute-id:raw-tx:v0 form)
    (compute-id:raw-tx:v1 form)
  ::
  ++  validate
    |=  =form
    ^-  ?
    ?^  -.form  (validate:raw-tx:v0 form)
    (validate:raw-tx:v1 form)
  ::
  --
::
::  $nnote: a utxo
++  nnote
  =<  form
  =+  nnote:v1
  |%
  +$  form  $|(^form |=(* %&))
  ::
  ++  get
    |_  =form
    ++  name
      ^-  nname
      ?^  -.form  name.form
      name.form
    ++  first-name  ^-(hash -:name)
    ++  origin-page
      ^-  page-number
      ?^  -.form  origin-page.form
      origin-page.form
    ++  assets
      ^-  coins
      ?^  -.form  assets.form
      assets.form
    ::
    --
  --
::
::  $input: a note together with the seeds that spent into it
++  input
  =<  form
  |%
  +$  form
    $%  [%0 input:v0]
        [%1 input:v1]
    ==
  ++  v0
    =<  form
    =+  input:^v0
    |%
    +$  form  $|(^form |=(* %&))
    ++  new
      =<  ..default
      |%
      ++  $  (corl (lead %0) ^new)
      ++  default  ^new
      ++  simple-from-note
        %+  corl  (lead %0)
        simple-from-note:^new
      --
    ++  validate
      |=  in=^^form
      ?>  ?=(%0 -.in)
      (^validate +.in)
    --
  --
::
::  $inputs: a map of names to inputs
++  inputs
  =<  form
  |%
  +$  form  (z-map nname input)
  ::
  ++  v0
    =<  form
    =+  inputs:^v0
    |%
    +$  form  $|(^form |=(* %&))
    ++  new
      =<  default
      |%
      ++  default
        |=  =^input
        ^-  ^^form
        ?>  ?=(%0 -.input)
        (~(put z-by *^^form) [name.note.input input])
      ::
      ++  multi
        |=  ips=(list $>(%0 ^input))
        ^-  ^^form
        %-  ~(gas z-by *^^form)
        %+  turn  ips
        |=  =^input
        ?>  ?=(%0 -.input)
        [name.note.input input]
      --
    ::
    ++  into
      |=  ^^form
      ^-  inputs:^v0
      %-  ~(gas z-by *inputs:^v0)
      %+  turn  ~(tap z-by +<)
      |=  [=nname =^input]
      ?>  ?=(%0 -.input)
      [nname +.input]
    --
  ++  get
    |_  =form
    ++  names
      ^-  (z-set nname)
      ~(key z-by form)
    --
  --
::
::  $output: a note together with the seeds that spent into it
++  output
  =<  form
  |%
  +$  form
    $%  [%0 output:v0]
        [%1 output:v1]
    ==
  ++  get
  |_  =form
    ::
    ++  note
      ^-  nnote
      ?:  =(%0 -.form)  note.form
      note.form
    ::
    --
  --
::
::  $outputs: a set of outputs
++  outputs
  =<  form
  |%
  +$  form
    $%  [%0 (z-map sig output)]
        [%1 (z-set output)]
    ==
  ++  v0
    =<  form
    =+  outputs:^v0
    |%
    +$  form  $|(^form |=(* %&))
    ++  from
      |=  outputs:^v0
      ^-  ^^form
      :-  %0
      %-  ~(run z-by +<)
      (lead %0)
    ++  into
      |=  out=^^form
      ^-  outputs:^v0
      ?>  ?=(%0 -.out)
      %-  ~(gas z-by *outputs:^v0)
      %+  turn  ~(tap z-by +.out)
      :: TODO idk why but dry gate doesn't compile
      |*  [sig=* output=*]
      ^-  [^sig output:^v0]
      ?>  ?=(%0 -.output)
      [sig +.output]
    --
  ::
  ++  v1
    =<  form
    =+  outputs:^v1
    |%
    +$  form  $|(^form |=(* %&))
    ++  from
      |=  outputs:^v1
      ^-  ^^form
      :-  %1
      %-  ~(run z-in +<)
      (lead %1)
    --
  ::
  ++  validate
    |=  =form
    ^-  ?
    ?-  -.form
      %0  (validate:v0:outputs (into:v0 form))
      %1  %+  levy  ~(tap z-in +.form)
          |=  out=output
          ?.  ?=(%1 -.out)  %.n
          (validate:output:v1 +.out)
    ==
  --
::
::  $spend-condition: conditions under which a note can be spent
++  spend-condition
  =<  form
  =+  spend-condition:v1
  |%
  +$  form  $|(^form |=(* %&))
  ++  combine
    |=  [fst=form snd=form]
    ^-  form
    (welp fst snd)
  ::
  ::  +make-pkh: build %pkh spend-condition from m and pubkeys
  ::
  ::    returns:
  ::      root: the root of the lock-merkle-proof
  ::      form: the spend-condition
  ::      hs: the set of pubkey hashes associated with the pkh
  ++  make-pkh
    |=  [m=@ pks=(list schnorr-pubkey)]
    ^-  [root=^hash =form hs=(z-set ^hash)]
    ?>  &((gth m 0) (lte m (lent pks)))
    =/  hs=(z-set hash)
      %+  roll  pks
      |=  [pk=schnorr-pubkey acc=(z-set ^hash)]
      (~(put z-in acc) (hash:schnorr-pubkey pk))
    =/  prim=lock-primitive  [%pkh [m hs]]
    =/  root=hash  (hash:lock `spend-condition`~[prim])
    [root ~[prim] hs]
  ::
  ::  +make-pkh-from-sig: builds a %pkh from a sig
  ++  make-pkh-from-sig
    |=  =sig
    ^-  [root=^hash =form hs=(z-set ^hash)]
    (make-pkh m.sig ~(tap z-in pubkeys.sig))
  ::
  ::  +make-hax: build %hax spend-condition from preimage
  ::
  ::    returns:
  ::      root: the root of the lock-merkle-proof
  ::      form: the spend-condition
  ::      h: the hash of the preimage
  ++  make-hax
    |=  pre=*
    ^-  [root=^hash =form h=^hash]
    =/  h=^hash  (hash-noun:hax:v1 pre)
    =/  hs=(z-set ^hash)  (~(put z-in *(z-set ^hash)) h)
    =/  prim=lock-primitive  [%hax hs]
    =/  sc=form  ~[prim]
    =/  lmp=lock-merkle-proof  (build-lock-merkle-proof:lock sc 1)
    =/  root=hash  root.merk-proof.lmp
    [root sc h]
  --
++  spends
  =<  form
  =+  spends:v1
  |%
  +$  form  $|(^form |=(* %&))
  ++  calculate-min-fee
    |=  sps=form
    ^-  coins
    =/  word-count=@
      %+  roll  ~(tap z-by sps)
      |=  [[nam=nname sp=spend] acc=@]
      %+  add  acc
      %+  add
        (count-seed-words:spend-v1 sp)
      (count-witness-words:spend-v1 sp)
    =/  word-fee=coins  (mul word-count base-fee)
    (max word-fee min-fee.data)
  --
::
++  spend-v1
  =<  form
  =+  spend:v1
  |%
  +$  form  $|(^form |=(* %&))
  ++  new   (corl (lead %1) new:spend-1)
  ::
  ::  +simple-from-note: generates a $spend-v1 sending all assets to recipient from note
  ::
  ::    parent: lock guarding the input note (witness proves we can unlock)
  ::    recipient: lock that will guard the output note
  ::    note: the input note being spent
  ++  simple-from-note
    |=  [parent=sig recipient=sig note=nnote]
    ^-  form
    ::  build witness for parent lock
    =/  parent-lock=lock  (from-sig:lock parent)
    =/  parent-lmp=lock-merkle-proof  (build-lock-merkle-proof:lock parent-lock 1)
    =/  recipient-lock=lock  (from-sig:lock recipient)
    ::  build seeds for recipient lock
    =/  lock-root=hash  (hash:lock recipient-lock)
    =/  sed=seed-v1
      (simple:seed-v1 lock-root assets.note (hash:nnote note))
    =/  seeds=(z-set seed:v1)  (~(put z-in *(z-set seed:v1)) sed)
    =|  wit=witness
    =/  sp=spend-1
      %*  .  *spend-1
        witness  wit(lmp parent-lmp)
        seeds    seeds
        fee      0
      ==
    [%1 sp]
  ++  count-seed-words
    |=  sp=form
    ^-  @
    =/  seed-list=(list seed-v1)
      ?-  -.sp
        %0  ~(tap z-in seeds.+.sp)
        %1  ~(tap z-in seeds.+.sp)
      ==
    %+  roll  seed-list
    |=  [sed=seed-v1 acc=@]
    %+  add  acc
    %-  num-of-leaves:shape
    %-  ~(rep z-by note-data.sed)
    |=  [[k=@tas v=*] tree=*]
    [k v tree]
  ::
  ++  count-witness-words
    |=  sp=form
    ^-  @
    ?-  -.sp
      %0  (num-of-leaves:shape `*`signature.+.sp)
      %1  (num-of-leaves:shape `*`witness.+.sp)
    ==
  ::
  ++  sign
    |=  [=form sk=schnorr-seckey]
    ^-  ^form
    ?-  -.form
      %0  [%0 (sign:spend-0:v1 +.form sk)]
      %1  [%1 (sign:spend-1:v1 +.form sk)]
    ==
  ::
  ++  verify
    |=  [=form parent-note=nnote]
    ^-  ?
    ?-  -.form
      %0  (verify:spend-0:v1 +.form ?>(?=(^ -.parent-note) parent-note))
      %1  (verify:spend-1:v1 +.form ?>(?=(@ -.parent-note) parent-note))
    ==
  --
::
::  $tx: internally-validated transaction with external validation information
++  tx
  =<  form
  |%
  +$  form
    $%  [%0 =raw-tx:v0 total-size=size =outputs:v0]
        form:tx:v1
    ==
  ::
  ++  new
    |=  [=raw-tx =page-number]
    ^-  form
    ?^  -.raw-tx  [%0 (new:tx:v0 raw-tx page-number)]
    (new:tx:v1 raw-tx page-number)
  ::
  ++  validate
    |=  =form
    ?-  -.form
      %0  (validate:tx:v0 +.form *page-number)
      %1  (validate:tx:v1 form)
    ==
  ::
  ++  get
    |_  =form
    ::
    ++  outputs
      ^-  ^outputs
      ?-  -.form
        %0  (from:v0:^outputs outputs.form)
        %1  (from:v1:^outputs outputs.form)
      ==
    ++  id
      ^-  tx-id
      ?-  -.form
        %0  id.raw-tx.form
        %1  ~(id get:tx:v1 form)
      ==
    ::
    ++  total-fees
      ^-  coins
      ?-  -.form
        %0  total-fees.raw-tx.form
        %1  ~(total-fees get:tx:v1 form)
      ==
    ++  size
      ?-  -.form
        %0  total-size.form
        %1  ~(size get:tx:v1 form)
      ==
    --
  --
::
::  $txs: hash-addressed transactions
++  txs
  =<  form
  |%
  +$  form  (z-map tx-id tx)
  --
::
::  $tx-acc: accumulate transactions against a balance to create a new balance
++  tx-acc
  =<  form
  |%
  +$  form
    $:  balance=(z-map nname nnote)                     ::  current balance
        height=page-number                              ::  origin height
        fees=coins                                      ::  total fee
        =size                                           ::  total size
        =txs                                            ::  valid txs
    ==
  ++  new
    |=  $:  initial-balance=(unit (z-map nname nnote))
            initial-height=page-number
        ==
    ^-  form
    %*  .  *form
      balance  ?~  initial-balance  *(z-map nname nnote)
               u.initial-balance
      height   initial-height
    ==
  ::
  ++  txs-size-by-set
    |=  form
    %-  ~(rep z-by txs)
    |=  [[=tx-id =tx] sum-sizes=^size]
    %+  add  sum-sizes
    ~(size get:raw-tx raw-tx.tx)
  ::
  ::  fully validate a transaction and update the balance
  ++  process
    |=  [=form raw=raw-tx]
    ^-  (reason ^form)
    =+  mres=(mule |.((dispatch form raw)))
    =/  res=(reason ^form)
      ?-    -.mres
        ::
        %.y  p.mres
        ::
          %.n
        =,  format
        ~>  %slog.[2 (cat 3 'tx-acc: process crashed: ' (of-wain (to-wain p.mres)))]
        [%.n %process-crashed-non-deterministic]
      ==
    ?.  ?=(%.n -.res)  res
    ~>  %slog.[1 (cat 3 'tx-acc: process failed: ' +.res)]
    res
  ::
  ++  dispatch
    |=  [=form raw=raw-tx]
    ^-  (reason ^form)
    ?^  -.raw
      ::
      ::  v1 activation (see blockchain-constants: v1-phase)
      ::    v1-phase:
      ::      - allow v1 txs
      ::      - allow v1 coinbases
      ::      - prohibit v0 coinbases
      ::      - prohibit v0 txs
      ::  here we gate raw-tx type only (coinbase gating is page-level)
      ?:  (gte height.form v1-phase)
        [%.n %v0-tx-after-cutoff]
      (v0-to-v0 form raw)
    ::  allow v1 raw-tx only at or after v1-phase
    ?:  (lth height.form v1-phase)
      [%.n %v1-tx-before-activation]
    (v1-to-v1 form raw)
  ::
  ::  v0 raw-tx to v0 outputs
  ++  v0-to-v0
    |=  [=form raw0=raw-tx:v0]
    |^
    ^-  (reason ^form)
    =/  new-page-number=page-number  height.form
    =/  tx0=tx:v0  (new:tx:v0 raw0 new-page-number)
    ?.  (validate:tx:v0 tx0 new-page-number)
      [%.n %v0-tx-invalid]
    ::  add outputs to balance
    =/  add-result=(reason ^form)  (add-outputs outputs.tx0)
    ?.  ?=(%.y -.add-result)  add-result
    ~!  p.add-result
    =.  form  p.add-result
    ::  process inputs: remove from balance and accumulate timelocks
    =/  consume-result  (consume-inputs inputs.tx0 new-page-number)
    ?.  ?=(%.y -.consume-result)  consume-result
    =/  [tir=timelock-range new-form=^form]  p.consume-result
    =.  form  new-form
    ?.  (check:timelock-range tir new-page-number)
      [%.n %v0-timelock-failed]
    ::  construct final tx and update accumulator
    =/  computed-size  ~(size get:raw-tx raw0)
    =/  agg-tx=tx
      [%0 raw0 computed-size outputs.tx0]
    :-  %.y
    %_  form
      size  (add size.form computed-size)
      txs   (~(put z-by txs.form) id.tx0 agg-tx)
    ==
    ::
    ++  add-outputs
      |=  ops=(z-map sig output:v0)
      ^-  (reason _form)
      %+  roll  ~(val z-by ops)
      |:  [op=*output:v0 acc=`(reason _form)`[%.y form]]
      ?.  ?=(%.y -.acc)  acc
      =/  f=_form  p.acc
      ?:  (~(has z-by balance.f) name.note.op)
        [%.n %v0-output-already-exists]
      [%.y f(balance (~(put z-by balance.f) name.note.op note.op))]
    ::
    ++  consume-inputs
      |=  [ips=(z-map nname input:v0) page-num=page-number]
      ^-  (reason [timelock-range ^form])
      %+  roll  ~(val z-by ips)
      |:  :*  ip=*input:v0
              acc=`(reason [timelock-range ^form])`[%.y *timelock-range form]
          ==
      ?.  ?=(%.y -.acc)  acc
      =/  [tir=timelock-range f=^form]  p.acc
      ?.  =(`note.ip (~(get z-by balance.f) name.note.ip))
        [%.n %v0-input-missing]
      =/  new-tir=timelock-range
        %+  merge:timelock-range  tir
        %+  fix-absolute:timelock:v0
          timelock.note.ip
        origin-page.note.ip
      :-  %.y
      :-  new-tir
      %_  f
        balance  (~(del z-by balance.f) name.note.ip)
        fees     (add fees.f fee.spend.ip)
      ==
    --
  ::
  ::  v1 raw-tx to v1 outputs
  ++  v1-to-v1
    |=  [=form raw1=raw-tx:v1]
    |^
    ^-  (reason ^form)
    =/  tx1=tx:v1  (new:tx:v1 raw1 height.form)
    ?.  (validate:tx:v1 tx1)  [%.n %v1-tx-invalid]
    ::  validate all spends against their parent notes
    =/  validate-result
      %-  validate-with-context:spends
      [balance.form spends.raw1 height.form max-size.data]
    ?.  ?=(%.y -.validate-result)  validate-result
    ::  check fee covers word count
    =/  min-fee=coins  (calculate-min-fee:spends spends.raw1)
    =/  paid-fee=coins  (roll-fees:spends spends.raw1)
    ?.  (gte paid-fee min-fee)
      [%.n %v1-insufficient-fee]
    ::  add outputs to balance
    =/  add-result  (add-outputs outputs.tx1)
    ?.  ?=(%.y -.add-result)  add-result
    =.  form  p.add-result
    ::  remove inputs from balance and accumulate fees
    =/  consume-result  (consume-inputs spends.raw1)
    ?.  ?=(%.y -.consume-result)  consume-result
    =.  form  p.consume-result
    ::
    :-  %.y
    %_  form
      size  (add size.form ~(size get:raw-tx raw1))
      txs   (~(put z-by txs.form) (compute-id:raw-tx raw1) tx1)
    ==
    ::
    ++  add-outputs
      |=  outs=(z-set output:v1)
      ^-  (reason ^form)
      %+  roll  ~(tap z-in outs)
      |:  [op=*output:v1 acc=`(reason ^form)`[%.y form]]
      ?.  ?=(%.y -.acc)  acc
      =/  f=^form  p.acc
      =/  note=nnote  note.op
      ?.  ?=(@ -.note)  [%.n %v1-output-wrong-note-version]
      =/  nam=nname  name.note
      ?:  (~(has z-by balance.f) nam)
        [%.n %v1-output-already-exists]
      [%.y f(balance (~(put z-by balance.f) nam note))]
    ::
    ++  consume-inputs
      |=  sps=spends
      ^-  (reason ^form)
      =/  fees-add=coins  (roll-fees:spends sps)
      =/  remove-result=(reason ^form)
        %+  roll  ~(tap z-in ~(key z-by sps))
        |:  [nam=*nname acc=`(reason ^form)`[%.y form]]
        ?.  ?=(%.y -.acc)  acc
        =/  f=^form  p.acc
        ?.  (~(has z-by balance.f) nam)
          [%.n %v1-input-missing]
        [%.y f(balance (~(del z-by balance.f) nam))]
      ?.  ?=(%.y -.remove-result)  remove-result
      [%.y p.remove-result(fees (add fees.p.remove-result fees-add))]
    --
  --
::
--
