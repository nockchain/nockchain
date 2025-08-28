/=  bip39  /common/bip39
/=  slip10  /common/slip10
/=  m  /common/markdown/types
/=  md  /common/markdown/markdown
/=  transact  /common/tx-engine
/=  zo  /common/zoon
/=  *   /common/zose
/=  wt  /apps/wallet/lib/types
|_  bug=?
::
::  print helpers
++  warn
  |*  meg=tape
  |*  *
  ?.  bug  +<
  ~>  %slog.[1 (cat 3 'wallet: warning: ' (crip meg))]
  +<
::
++  debug
  |*  meg=tape
  |*  *
  ?.  bug  +<
  ~>  %slog.[2 (cat 3 'wallet: debug: ' (crip meg))]
  +<
::
::  markdown rendering
++  print
  |=  nodes=markdown:m
  ^-  (list effect:wt)
  ~[(make-markdown-effect nodes)]
::
++  make-markdown-effect
  |=  nodes=markdown:m
  [%markdown (crip (en:md nodes))]
::
::
::  +timelock-helpers: helper functions for creating timelock-intents
::
++  timelock-helpers
  |%
  ::  +make-relative-timelock-intent: create relative timelock-intent
  ::
  ::    min-rel: minimum pages after note creation before spendable
  ::    max-rel: maximum pages after note creation when spendable
  ++  make-relative-timelock-intent
    |=  [min-rel=(unit @ud) max-rel=(unit @ud)]
    ^-  timelock-intent:transact
    `[*timelock-range:transact (new:timelock-range:transact min-rel max-rel)]
  ::
  ::  +make-absolute-timelock-intent: create absolute timelock-intent
  ::
  ::    min-abs: minimum absolute page number when spendable
  ::    max-abs: maximum absolute page number when spendable
  ++  make-absolute-timelock-intent
    |=  [min-abs=(unit @ud) max-abs=(unit @ud)]
    ^-  timelock-intent:transact
    `[(new:timelock-range:transact min-abs max-abs) *timelock-range:transact]
  ::
  ::  +make-combined-timelock-intent: create timelock-intent with both absolute and relative
  ++  make-combined-timelock-intent
    |=  $:  min-abs=(unit @ud)
            max-abs=(unit @ud)
            min-rel=(unit @ud)
            max-rel=(unit @ud)
        ==
    ^-  timelock-intent:transact
    `[(new:timelock-range:transact min-abs max-abs) (new:timelock-range:transact min-rel max-rel)]
  ::
  ::  +no-timelock: convenience function for no timelock constraint
  ++  no-timelock
    ^-  timelock-intent:transact
    *timelock-intent:transact
  --
::
::  +edit: modify inputs
++  edit
  |_  =state:wt
  ::
  +*  inp
    ^-  preinput:wt
    ?~  active-input.state
      %-  (debug "no active input set!")
      *preinput:wt
    =/  input-result  (~(get-input plan transaction-tree.state) u.active-input.state)
    ?~  input-result
      ~|("active input not found in transaction-tree" !!)
    u.input-result
  ::    +add-seed: add a seed to the input
  ::
  ++  add-seed
    |=  =seed:transact
    ^-  [(list effect:wt) state:wt]
    ?:  (~(has z-in:zo seeds.spend.p.inp) seed)
      :_  state
      %-  print
      %-  need
      %-  de:md
      %-  crip
      """
      ##  add-seed

      **seed already exists in .spend**
      """
    =/  pre=preinput:wt  inp
    =/  =preinput:wt
      %=    pre
          seeds.spend.p
        %.  seed
        ~(put z-in:zo seeds.spend.p.pre)
        ::
        seeds.spend.q  %.y
      ==
    =.  active-input.state  (some name.pre)
    ::
    =/  =input-name:wt  (need active-input.state)
    =.  transaction-tree.state
      (~(add-input plan transaction-tree.state) input-name preinput)
    ::  if active-seed is set, link it to this input
    =.  transaction-tree.state
      ?:  ?=(^ active-seed.state)
        (~(link-seed-to-input plan transaction-tree.state) input-name u.active-seed.state)
      transaction-tree.state
    `state
  ::
  ++  remove-seed
    |=  =seed:transact
    ^-  [(list effect:wt) state:wt]
    ?.  (~(has z-in:zo seeds.spend.p.inp) seed)
      :_  state
      %-  print
      %-  need
      %-  de:md
      %-  crip
      """
      ##  remove-seed

      **seed not found in .spend**
      """
    =/  pre=preinput:wt  inp
    =.  seeds.spend.p.pre
      %.  seed
      ~(del z-in:zo seeds.spend.p.pre)
    =.  transaction-tree.state
      =/  =input-name:wt  (need active-input.state)
      (~(add-input plan transaction-tree.state) input-name pre)
    `state
  --
::
::  +draw: modify transactions
++  draw
  |_  =state:wt
  +*  tx
    ^-  transaction:wt
    ?>  ?=(^ active-transaction.state)
    =/  transaction-result  (~(get-transaction plan transaction-tree.state) u.active-transaction.state)
    ?~  transaction-result
      *transaction:wt
    u.transaction-result
  ::    +add-input: add an input to the transaction
  ::
  ++  add-input
    |=  =input:transact
    ^-  [(list effect:wt) state:wt]
    =/  =transaction:wt  tx
    =/  =input-name:wt
      =+  (to-b58:nname:transact name.note.input)
      %-  crip
      "{<first>}-{<last>}"
    ?:  (~(has z-by:zo p.tx) name.note.input)
      :_  state
      %-  print
      %-  need
      %-  de:md
      %-  crip
      """
      ##  add-input

      **input already exists in .transaction**

      transaction already has input with note name: {<input-name>}
      """
    =/  active-transaction=transaction-name:wt  (need active-transaction.state)
    =.  p.transaction
      %-  ~(put z-by:zo p.transaction)
      :-  name.note.input
      input
    =.  transaction-tree.state
      %.  [active-transaction transaction]
      ~(add-transaction plan transaction-tree.state)
    =.  transaction-tree.state
      %.  [active-transaction input-name]
      ~(link-input-to-transaction plan transaction-tree.state)
    write-transaction
  ::
  ++  write-transaction
    ^-  [(list effect:wt) state:wt]
    =?  active-transaction.state  ?=(~ active-transaction.state)  (some *transaction-name:wt)
    ?>  ?=(^ active-transaction.state)
    =/  =transaction:wt  tx
    =.  transaction-tree.state  (~(add-transaction plan transaction-tree.state) u.active-transaction.state transaction)
    =/  dat-jam  (jam transaction)
    =/  path=@t  (crip "txs/{(trip u.active-transaction.state)}.tx")
    =/  effect  [%file %write path dat-jam]
    :_  state
    ~[effect [%exit 0]]
  --
::
::  Convenience wrapper door for slip10 library
::  ** Never use slip10 directly in the wallet **
++  s10
  |_  bas=base:slip10
  ++  gen-master-key
    |=  [entropy=byts salt=byts]
    =/  argon-byts=byts
      :-  32
      %+  argon2-nockchain:argon2:crypto
        entropy
      salt
    =/  memo=tape  (from-entropy:bip39 argon-byts)
    %-  (debug "memo: {memo}")
    :-  (crip memo)
    (from-seed:slip10 [64 (to-seed:bip39 memo "")])
  ++  from-seed
    |=  =byts
    (from-seed:slip10 byts)
::
  ++  from-private
    |=  =keyc:slip10
    (from-private:slip10 keyc)
::
  ++  from-public
    |=  =keyc:slip10
    (from-public:slip10 keyc)
  ::
  ::  derives the i-th child key(s) from a parent key.
  ::  index i can be any child index. returns the door
  ::  with the door sample modified with the values
  ::  corresponding to the key. the core sample can then
  ::  be harvested for keys.
  ::
  ++  derive
    |=  [parent=coil:wt i=@u]
    ?-    -.key.parent
        %pub
      =>  [cor=(from-public [p.key cc]:parent) i=i]
      (derive:cor i)
    ::
        %prv
      =>  [cor=(from-private [p.key cc]:parent) i=i]
      (derive:cor i)
    ==
  ::
  ++  from-extended-key
    |=  key=@t
    (from-extended-key:slip10 key)
  --
::
++  vault
  |_  =state:wt
  ++  base-path  ^-  trek
    ?~  master.state
      ~|("base path not accessible because master not set" !!)
    /keys/[t/(to-b58:master:wt master.state)]
  ::
  ++  seed-path  ^-  trek
    (welp base-path /seed)
  ::
  ++  has
    |_  key-type=?(%pub %prv)
    ++  key-path  ^-  trek
      (welp base-path ~[key-type])
    ::
    ++  master
      ^-  ?
      =/  =trek  (welp key-path /m)
      (~(has of keys.state) trek)
    --
  ++  get
    |_  key-type=?(%pub %prv)
    ::
    ++  key-path  ^-  trek
      (welp base-path ~[key-type])
    ::
    ::
    ++  master
      ^-  coil:wt
      =/  =trek  (welp key-path /m)
      =/  =meta:wt  (~(got of keys.state) trek)
      :: check if private key matches public key
      ?>  ?=(%coil -.meta)
      ?:  ?=(%prv key-type)
        =/  public-key=@
          public-key:(from-private:s10 [p.key cc]:meta)
        ?:  =(public-key p.key:(public:master:wt master.state))
          meta
        ~|("private key does not match public key" !!)
      meta
    ::
    ++  sign-key
      |=  key=(unit [child-index=@ hardened=?])
      ^-  schnorr-seckey:transact
      =.  key-type  %prv
      =/  sender=coil:wt
        ?~  key  master
        =/  [child-index=@ hardened=?]  u.key
        =/  absolute-index=@
          ?.(hardened child-index (add child-index (bex 31)))
        =/  key-at-index=meta:wt  (by-index absolute-index)
        ?>  ?=(%coil -.key-at-index)
        key-at-index
      (from-atom:schnorr-seckey:transact p.key.sender)
    ::
    ++  by-index
      |=  index=@ud
      ^-  coil:wt
      =/  =trek  (welp key-path /[ud/index])
      =/  =meta:wt  (~(got of keys.state) trek)
      ?>  ?=(%coil -.meta)
      meta
    ::
    ++  seed
      ^-  meta:wt
      (~(got of keys.state) seed-path)
    ::
    ++  by-label
      |=  label=@t
      %+  murn  keys
      |=  [t=trek =meta:wt]
      ?:(&(?=(%label -.meta) =(label +.meta)) `t ~)
    ::
    ++  keys
      ^-  (list [trek meta:wt])
      =/  subtree
        %-  ~(kids of keys.state)
        key-path
      ~(tap by kid.subtree)
    ::
    ++  coils
      ^-  (list coil:wt)
      %+  murn  keys
      |=  [t=trek =meta:wt]
      ^-  (unit coil:wt)
      ;;  (unit coil:wt)
      ?:(=(%coil -.meta) `meta ~)
    --
  ::
  ++  put
    |%
    ::
    ++  seed
      |=  seed-phrase=@t
      ^-  (axal meta:wt)
      %-  ~(put of keys.state)
      [seed-path [%seed seed-phrase]]
    ::
    ++  key
      |=  [=coil:wt index=(unit @) label=(unit @t)]
      ^-  (axal meta:wt)
      =/  key-type=@tas  -.key.coil
      =/  suffix=trek
        ?@  index
          /[key-type]/m
        /[key-type]/[ud/u.index]
      =/  key-path=trek  (welp base-path suffix)
      %-  (debug "adding key at {(en-tape:trek key-path)}")
      =.  keys.state  (~(put of keys.state) key-path coil)
      ?~  label
        keys.state
      %+  ~(put of keys.state)
        (welp key-path /label)
      label/u.label
    --
  ::
  ++  get-note
    |=  name=nname:transact
    ^-  nnote:transact
    ?:  (~(has z-by:zo balance.state) name)
      (~(got z-by:zo balance.state) name)
    ~|("note not found: {<name>}" !!)
  ::
  ::  TODO: way too slow, need a better way to do this or
  ::  remove entirely in favor of requiring note names in
  ::  the causes where necessary.
  ++  find-name-by-hash
    |=  has=hash:transact
    ^-  (unit nname:transact)
    =/  notes=(list [name=nname:transact note=nnote:transact])
      ~(tap z-by:zo balance.state)
    |-
    ?~  notes  ~
    ?:  =((hash:nnote:transact note.i.notes) has)
      `name.i.notes
    $(notes t.notes)
  ::
  ++  get-note-from-hash
    |=  has=hash:transact
    ^-  nnote:transact
    =/  name=(unit nname:transact)  (find-name-by-hash has)
    ?~  name
      ~|("note with hash {<(to-b58:hash:transact has)>} not found in balance" !!)
    (get-note u.name)
  ::
  ++  generate-pid
    ^-  @ud
    =/  used-pids=(list @ud)
      ~(tap in ~(key by peek-requests.state))
    =/  max-pid=@ud
      (roll used-pids max)
    =/  next-pid=@ud  +(max-pid)
    ?:  =(next-pid 0)  1  :: handle wraparound
    next-pid
  ::
  ::  +derive-child: derives the i-th hardened/unhardened child key(s)
  ::
  ::    derives the i-th child from the master key. for hardened keys,
  ::    (bex 31) should be already added to `i`.
  ::
  ++  derive-child
    |=  i=@u
    ^-  (set coil:wt)
    ?:  (gte i (bex 32))
      ~|("Child index {<i>} out of range. Child indices are capped to values between [0, 2^32)" !!)
    ?~  master.state
      ~|("No master keys available for derivation" !!)
    =;  coils=(list coil:wt)
      (silt coils)
    =/  hardened  (gte i (bex 31))
    ::
    ::  Grab the prv master key if it exists (cold wallet)
    ::  otherwise grab the pub master key (hot wallet).
    =/  parent=coil:wt
      ?:  ~(master has %prv)
        ~(master get %prv)
      ~(master get %pub)
    ?:  hardened
      ?>  ?=(%prv -.key.parent)
      ::
      =>  (derive:s10 parent i)
      :~  [%coil [%prv private-key] chain-code]
          [%coil [%pub public-key] chain-code]
      ==
    ::
    ::  if unhardened, we just assert that they are within the valid range
    ?:  (gte i (bex 31))
      ~|("Unhardened child index {<i>} out of range. Indices are capped to values between [0, 2^31)" !!)
    ?-    -.key.parent
     ::  if the parent is a private key, we can derive the unhardened prv and pub child
        %prv
      =>  (derive:s10 parent i)
      :~  [%coil [%prv private-key] chain-code]
          [%coil [%pub public-key] chain-code]
      ==
    ::
     ::  if the parent is a public key, we can only derive the unhardened pub child
        %pub
      =>  (derive:s10 parent i)
      ~[[%coil [%pub public-key] chain-code]]
    ==
  -- ::vault
  ::    +plan: core for managing transaction relationships
  ::
  ::  provides methods for adding, removing, and navigating the transaction tree.
  ::  uses the axal structure to maintain relationships between transactions, inputs,
  ::  and seeds.
  ::
  ++  plan
    |_  tree=transaction-tree:wt
    ::
    ::  +get-transaction: retrieve a transaction by name
    ::
    ++  get-transaction
      |=  name=transaction-name:wt
      ^-  (unit transaction:wt)
      =/  res  (~(get of tree) /transaction/[name])
      ?~  res  ~
      ?.  ?=(%transaction -.u.res)  ~
      `transaction.u.res
    ::    +get-input: retrieve an input by name
    ::
    ++  get-input
      |=  name=input-name:wt
      ^-  (unit preinput:wt)
      =/  res  (~(get of tree) /input/[name])
      ?~  res  ~
      ?.  ?=(%input -.u.res)  ~
      `preinput.u.res
    ::    +get-seed: retrieve a seed by name
    ::
    ++  get-seed
      |=  name=seed-name:wt
      ^-  (unit preseed:wt)
      =/  res  (~(get of tree) /seed/[name])
      ?~  res  ~
      ?.  ?=(%seed -.u.res)  ~
      `preseed.u.res
    ::    +add-transaction: add a new transaction
    ::
    ++  add-transaction
      |=  [name=transaction-name:wt =transaction:wt]
      ^-  transaction-tree:wt
      =/  entity  [%transaction name transaction]
      (~(put of tree) /transaction/[name] entity)
    ::    +add-input: add a new input
    ::
    ++  add-input
      |=  [name=input-name:wt =preinput:wt]
      ^-  transaction-tree:wt
      =/  entity  [%input name preinput]
      (~(put of tree) /input/[name] entity)
    ::    +add-seed: add a new seed
    ::
    ++  add-seed
      |=  [name=seed-name:wt =preseed:wt]
      ^-  transaction-tree:wt
      =/  entity  [%seed name preseed]
      (~(put of tree) /seed/[name] entity)
    ::    +link-input-to-transaction: link an input to a transaction
    ::
    ++  link-input-to-transaction
      |=  [=transaction-name:wt =input-name:wt]
      ^-  transaction-tree:wt
      =/  input-entity  (~(get of tree) /input/[input-name])
      ?~  input-entity  tree
      ?.  ?=(%input -.u.input-entity)  tree
      (~(put of tree) /transaction/[transaction-name]/input/[input-name] u.input-entity)
    ::    +link-seed-to-input: link a seed to an input
    ::
    ++  link-seed-to-input
      |=  [=input-name:wt seed-name=seed-name:wt]
      ^-  transaction-tree:wt
      =/  seed-entity  (~(get of tree) /seed/[seed-name])
      ?~  seed-entity  tree
      ?.  ?=(%seed -.u.seed-entity)  tree
      (~(put of tree) /input/[input-name]/seed/[seed-name] u.seed-entity)
    ::    +unlink-input-from-transaction: remove an input from a transaction
    ::
    ++  unlink-input-from-transaction
      |=  [=transaction-name:wt =input-name:wt]
      ^-  transaction-tree:wt
      (~(del of tree) /transaction/[transaction-name]/input/[input-name])
    ::    +unlink-seed-from-input: remove a seed from an input
    ::
    ++  unlink-seed-from-input
      |=  [=input-name:wt seed-name=seed-name:wt]
      ^-  transaction-tree:wt
      (~(del of tree) /input/[input-name]/seed/[seed-name])
    ::    +list-transaction-inputs: list all inputs in a transaction
    ::
    ++  list-transaction-inputs
      |=  name=transaction-name:wt
      ^-  (list input-name:wt)
      =/  kids  (~(kid of tree) /transaction/[name])
      %+  murn  ~(tap in ~(key by kids))
      |=  pax=pith
      ^-  (unit input-name:wt)
      =/  pax=path  (pout pax)
      ?>  ?=([%input *] pax)
      ?>  ?=(^ t.pax)
      `i.t.pax
    ::    +list-input-seeds: list all seeds in an input
    ::
    ++  list-input-seeds
      |=  name=input-name:wt
      ^-  (list seed-name:wt)
      =/  kids  (~(kid of tree) /input/[name])
      %+  murn  ~(tap in ~(key by kids))
      |=  pax=pith
      ^-  (unit seed-name:wt)
      =/  pax=path  (pout pax)
      ?:  &(?=([%seed *] pax) ?=(^ t.pax))
        `i.t.pax
      ~
    ::    +list-all-transactions: list all transaction names
    ::
    ++  list-all-transactions
      ^-  (list transaction-name:wt)
      =/  kids  (~(kid of tree) /transaction)
      %+  murn  ~(tap in ~(key by kids))
      |=  pax=pith
      ^-  (unit transaction-name:wt)
      =/  pax=path  (pout pax)
      ?:  ?=(^ pax)
        `i.pax
      ~
    ::    +list-all-inputs: list all input names
    ::
    ++  list-all-inputs
      ^-  (list input-name:wt)
      =/  kids  (~(kid of tree) /input)
      %+  murn  ~(tap in ~(key by kids))
      |=  pax=pith
      ^-  (unit input-name:wt)
      =/  pax=path  (pout pax)
      ?:  ?=(^ pax)
        `i.pax
      ~
    ::    +list-all-seeds: list all seed names
    ::
    ++  list-all-seeds
      ^-  (list seed-name:wt)
      =/  kids  (~(kid of tree) /seed)
      %+  murn  ~(tap in ~(key by kids))
      |=  pax=pith
      ^-  (unit seed-name:wt)
      =/  pax=path  (pout pax)
      ?:  &(?=([%seed *] pax) ?=(^ t.pax))
        `i.pax
      ~
    ::    +remove-transaction: completely remove a transaction and its associations
    ::
    ++  remove-transaction
      |=  name=transaction-name:wt
      ^-  transaction-tree:wt
      (~(lop of tree) /transaction/[name])
    ::    +remove-input: completely remove an input and its associations
    ::
    ++  remove-input
      |=  name=input-name:wt
      ^-  transaction-tree:wt
      (~(lop of tree) /input/[name])
    ::    +remove-seed: completely remove a seed and its associations
    ::
    ++  remove-seed
      |=  name=seed-name:wt
      ^-  transaction-tree:wt
      (~(lop of tree) /seed/[name])
    --
  ::
  ::  display functions
  ::  TODO: organize these in a core
  ::
  ++  display-poke
      |=  =cause:wt
      ^-  effect:wt
      =/  nodes=markdown:m
      %-  need
      %-  de:md
      %-  crip
      """
      ## poke
      {<cause>}
      """
      (make-markdown-effect nodes)
  ::
  ++  display-transaction-cord
      |=  [name=@t p=inputs:transact]
      ^-  @t
      =/  inputs  `(list [nname:transact input:transact])`~(tap z-by:zo p)
      =/  by-addrs
        %+  roll  inputs
        |=  [[name=nname:transact input=input:transact] acc=_`(z-map:zo lock:transact coins:transact)`~]
        =/  seeds  ~(tap z-in:zo seeds:spend:input)
        %+  roll  seeds
        |=  [seed=seed:transact acc=_acc]
        =/  lock  recipient:seed
        =/  cur  (~(gut z-by:zo acc) lock 0)
        =/  gift  gift:seed
        =/  new-bal  (add cur gift)
        (~(put z-by:zo acc) lock new-bal)
      %+  roll  ~(tap z-by:zo by-addrs)
      =/  acc=@t
        %-  crip
        """
        ## Transaction
        Name: {(trip name)}
        Outputs:
        """
      |=  [[recipient=lock:transact amt=coins:transact] acc=_acc]
      =/  r58  (to-b58:lock:transact recipient)
      =/  amtdiv  (dvr amt 65.536)
      %^  cat  3
        ;:  (cury cat 3)
          acc
          '\0a\0a- Assets: '
          (rsh [3 2] (scot %ui amt))
          '\0a  - Nocks: '
          (rsh [3 2] (scot %ui p.amtdiv))
          '\0a  - Nicks: '
          (rsh [3 2] (scot %ui q.amtdiv))
          '\0a- Required Signatures: '
          (rsh [3 2] (scot %ui m.recipient))
          '\0a- Signers: '
        ==
      %-  crip
      %+  join  ' '
      (serialize-lock recipient)
  ::
  ++  display-note-cord
      |=  note=nnote:transact
      ^-  @t
      %^  cat  3
       ;:  (cury cat 3)
          '''

          ---

          ## Details

          '''
          '- Name: '
          =+  (to-b58:nname:transact name.note)
          :((cury cat 3) '[' first ' ' last ']')
          '\0a- Assets: '
          (format-ui assets.note)
          '\0a- Block Height: '
          (format-ui origin-page.note)
          '\0a- Source: '
          (to-b58:hash:transact p.source.note)
          '\0a## Lock'
          '\0a- Required Signatures: '
          (format-ui m.lock.note)
          '\0a- Signers: '
        ==
      %-  crip
      %+  join  ' '
      (serialize-lock lock.note)
  ::
  ++  serialize-lock
      |=  =lock:transact
      ^-  (list @t)
      ~+
      pks:(to-b58:lock:transact lock)
  ::
  ++  display-note
      |=  note=nnote:transact
      ^-  markdown:m
      %-  need
      %-  de:md
      (display-note-cord note)
  ::
  ++  format-ui
      |=  @
      ^-  @t
      (rsh [3 2] (scot %ui +<))
  ::
  ++  show
      |=  [=state:wt =path]
      ^-  [(list effect:wt) state:wt]
      |^
      ?+    path  !!
          [%balance ~]
        :-  ~[(display-balance balance.state)]
        state
      ::
          [%state ~]
        :-  display-state
        state
      ==
      ++  display-balance
        |=  =balance:wt
        ^-  effect:wt
        =/  nodes=markdown:m
        %-  need
        %-  de:md
        %-  crip
        """
        ## balance
        {<balance>}
        """
        (make-markdown-effect nodes)
      ::
      ++  display-state
        ^-  (list effect:wt)
        =/  nodes=markdown:m
        %-  need
        %-  de:md
        %-  crip
        """
        ## state
        - last block: {<last-block.state>}
        """
        ~[(make-markdown-effect nodes)]
      --
  ::
  ++  ui-to-tape
      |=  @
      ^-  tape
      %-  trip
      (rsh [3 2] (scot %ui +<))
  --
