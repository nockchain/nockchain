/=  s10  /apps/wallet/lib/s10
/=  m  /common/markdown/types
/=  md  /common/markdown/markdown
/=  transact  /common/tx-engine
/=  zo  /common/zoon
/=  *   /common/zose
/=  *  /common/zeke
/=  wt  /apps/wallet/lib/types
|_  [bug=? bc=blockchain-constants:transact]
+*  t  ~(. transact bc)
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
++  estimate-fee
  |%
  ::  Commonly used constants
  ::  - 13 leaves for the key which is a schnorr-pubkey:
  ::    - 6 for x, 6 for y, 1 for inf flag
  ::  - 16 leaves for the signature
  ++  pubkey-count  13
  ++  signature-count  16
  ++  hash-count  5
  ++  pkh-spend
    |=  $:  =note-data:t
            num-input-notes=@
            m=@
            signers=(z-set:zo schnorr-pubkey:t)
        ==
    =/  seed-count=@
      =/  data-size=@
        %-  num-of-leaves:shape
        %-  ~(rep z-by:zo note-data)
        |=  [[k=@tas v=*] tree=*]
        [k v tree]
      ::  each seed is a spend from an input note into output note(s).
      ::  note-data is attached to the output of each seed.
      ::  note-data size is calculated from the inputs even though it should not be.
      (mul data-size num-input-notes)
    =/  witness-count=@
      =/  pkh-count
        =/  num-sigs-required  (mul m num-input-notes)
        (map-words num-sigs-required hash-count (add signature-count pubkey-count))
      =/  lmp-count
        ::  1 for the sig at the end of the list, 1 for the number of signers (m),
        ::  added to the number of leaves in the set of valid signers
        =/  pkh-spend-condition  :(add 1 1 (set-words hash-count pubkey-count))
        =/  merk-proof  (add hash-count 1)
        =/  axis-count  1
        :(add pkh-spend-condition merk-proof axis-count)
      :(add hax-count=1 tim-count=1 pkh-count lmp-count)
    =/  fee  (mul base-fee.bc (add witness-count seed-count))
    (max fee min-fee.data.bc)
  ::
  ++  transaction
    |=  [=transaction:wt page-num=page-number:transact]
    (spends spends.transaction inputs.metadata.transaction page-num)
  ++  spends
    |=  [raw-spends=spends:v1:transact =input-metadata:wt page-num=page-number:transact]
    =/  bythos-active=?  (gte page-num bythos-phase.bc)
    =/  seeds-count=@
      :: count seeds against the tx-engine instance already bound to this wallet's constants
      (count-seed-words:spends:t [raw-spends page-num])
    =/  witness-count=@
      (count-witness-words [raw-spends input-metadata page-num])
    ::  match consensus formula:
    ::    - pre-bythos: legacy base-fee (2x current base-fee), no input discount
    ::    - post-bythos: configured base-fee with discounted input fees
    =/  effective-base-fee=@  ?:(bythos-active base-fee.bc (mul 2 base-fee.bc))
    =/  witness-divisor=@  ?:(bythos-active input-fee-divisor.bc 1)
    =/  seed-fee=@  (mul seeds-count effective-base-fee)
    =/  witness-fee=@  (div (mul witness-count effective-base-fee) witness-divisor)
    =/  word-fee=@  (add seed-fee witness-fee)
    (max word-fee min-fee.data.bc)
  ::
  ++  count-witness-words-raw
    |=  [raw-spends=spends:v1:transact =input-metadata:wt]
    ^-  @
    %-  ~(rep z-by:zo raw-spends)
    |=  [[nam=nname:transact sp=spend:v1:transact] acc=@]
    %+  add  acc
    (witness-words sp nam input-metadata)
  ::
  ++  count-witness-words
    |=  [raw-spends=spends:v1:transact =input-metadata:wt page-num=page-number:transact]
    ?:  (gte page-num bythos-phase.bc)
      (count-witness-words-raw [raw-spends input-metadata])
    (count-witness-words-raw [raw-spends input-metadata])
  ::
  ::  +witness-words: estimate the number of words in a witness
  ++  witness-words
    |=  [=spend:v1:transact nam=nname:transact =input-metadata:wt]
    ?-    -.spend
        %0
      ?>  ?=(%0 -.input-metadata)
      =/  signature-leaves=@
        =/  num-sigs-required=@
          =/  =sig:transact  (~(got z-by:zo p.input-metadata) nam)
          m.sig
        (map-words num-sigs-required signature-count pubkey-count)
      signature-leaves
    ::
        %1
      ?>  ?=(%1 -.input-metadata)
      =/  =witness:transact  witness.+.spend
      ?>  ?&  !=(*lock-merkle-proof:v1:transact lmp.witness)
              =(~ tim.witness)
              =(~ hax.witness)
          ==
      =/  lmp-count=@  (num-of-leaves:shape lmp.witness)
      =/  tim-count=@  (num-of-leaves:shape tim.witness)
      =/  hax-count=@  (num-of-leaves:shape hax.witness)
      =/  pkh-count=@
        =/  num-sigs-required=@
          =/  sc=spend-condition:transact  (~(got z-by:zo p.input-metadata) nam)
          %+  roll  sc
          |=  [lp=lock-primitive:transact acc=@]
          ::  TODO handle hax lock primitives size contribution. for now we will just do pkhs
          ?.  ?=(%pkh -.lp)
            acc
          (add acc m.lp)
        (map-words num-sigs-required hash-count (add pubkey-count signature-count))
      :(add lmp-count pkh-count tim-count hax-count)
    ==
  ::
  ::  +map-words:  estimate number of leaves in a map given node size and number of entries
  ::  A map (binary tree) with n entries has n + 1 null branches, independent of shape/balance.
  ::  To calculate the size of a map, we take the number of leaves contributed by each node by
  ::  multiplying the per-node-leaf count by the number of entries. We leaf contribution of the
  ::  nodes and add them to the number of null branches, each of which contributes 1 to the size.
  ::
  ::  If either key-leaves or val-leaves are variable size, their maximum theoretical size should
  ::  be provided.
  ++  map-words
    |=  $:  entries=@
            key-leaves=@
            val-leaves=@
        ==
    ^-  @
    =+  per-node-count=(add key-leaves val-leaves)
    %+  add  (mul entries per-node-count)
    (add entries 1)
  ::
  ++  set-words
    |=  $:  entries=@
            val-leaves=@
        ==
    ^-  @
    (map-words entries 0 val-leaves)
  --
++  locks
  |%
  ++  pull-inner
    |=  [nd=note-data:v1:transact nn=nname:transact pkh=(unit hash:transact)]
    ^-  (unit spend-condition:transact)
    ?~  lock-noun=(~(get z-by:zo nd) %lock)
      ?~  pkh
        ~
      :: There's no stored lock. Attempt rebuilding from name
      =/  simple-lock  [(simple-pkh-lp:v1:first-name:transact u.pkh)]~
      ?:  =((first:nname:transact (hash:lock:transact simple-lock)) -.nn)
        (some simple-lock)
      =/  coinbase-lock  (coinbase-pkh-sc:v1:first-name:t u.pkh)
      ?:  =((first:nname:transact (hash:lock:transact coinbase-lock)) -.nn)
        (some coinbase-lock)
      ~>  %slog.[2 'unsupported lock type']
      ~
    ?~  soft-lock=((soft lock-data:wt) u.lock-noun)
      ~>  %slog.[0 'lock data in note is malformed']  ~
    ?@  -.lock.u.soft-lock
      ~>  %slog.[0 'lock data in note is not a single spend condition']  ~
    (some lock.u.soft-lock)
  ++  pull
    |=  [nd=note-data:v1:transact nn=nname:transact pkh=(unit hash:transact)]
    ^-  (unit spend-condition:transact)
    ?~  lok=(pull-inner [nd nn pkh])
      ~
    ?:  =((first:nname:transact (hash:lock:transact u.lok)) -.nn)
      lok
    ~>  %slog.[0 'first-name does not match the pulled lock']  ~
  --
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
++  vault
  |_  =state:wt
  ::
  ++  base-path  ^-  trek
    ?~  active-master.state
      ~|('base path not accessible because master not set' !!)
    /keys/[t/(to-b58:active:wt active-master.state)]
  ::
  ++  watch-path  ^-  trek
    /keys/watch
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
    ++  master-addresses
      ^-  (list [version=@ addr=@t])
      =/  subtree  (~(kids of keys.state) /keys)
      %~  tap  in
      %-  silt
      ^-  (list [version=@ addr=@t])
      %+  murn  ~(tap by kid.subtree)
      |=  [pax=trek =meta:wt]
      ^-  (unit [version=@ addr=@t])
      =/  version=(unit @)
        ?.  ?=(%coil -.meta)
          ~
        (some `@`-.p.meta)
      =/  addr=(unit @t)
        ?~  pax  ~
        =/  segment  i.pax
        ?.  ?=([%t @t] segment)
          ~
        `+.segment
      (both version addr)
    ::
    ::  Grab other master addr
    ++  master-by-addr
      |=  master-b58=@t
      ^-  coil:wt
      =/  root-path=trek  /keys/[t/master-b58]/pub/m
      =/  meta=(unit meta:wt)  (~(get of keys.state) root-path)
      ?~  meta
        ~|("Requested master addr not found" !!)
      ?>  ?=(%coil -.u.meta)
      p.u.meta
    ::
    ++  master
      ^-  coil:wt
      =/  =trek  (welp key-path /m)
      =/  =meta:wt  (~(got of keys.state) trek)
      :: check if private key matches public key
      ?>  ?=(%coil -.meta)
      =/  =coil:wt  p.meta
      ?:  ?=(%prv key-type)
        =/  keyc=keyc:s10  ~(keyc get:coil:wt coil)
        =/  public-key=@  public-key:(from-private:s10 keyc)
        ?:  =(public-key p.key:(public:active:wt active-master.state))
          coil
        ~|("private key does not match public key" !!)
      coil
    ::
    ++  sign-key
      |=  key=(unit [child-index=@ hardened=?])
      ^-  schnorr-seckey:transact
      =.  key-type  %prv
      =/  =coil:wt
        ?~  key  master
        =/  [child-index=@ hardened=?]  u.key
        =/  absolute-index=@
          ?.(hardened child-index (add child-index (bex 31)))
        (by-index absolute-index)
      =/  keyc=keyc:s10  ~(keyc get:coil:wt coil)
      (from-atom:schnorr-seckey:transact p:~(key get:coil:wt coil))
    ::
    ++  by-index
      |=  index=@ud
      ^-  coil:wt
      =/  =trek  (welp key-path /[ud/index])
      =/  =meta:wt  (~(got of keys.state) trek)
      ?>  ?=(%coil -.meta)
      p.meta
    ::
    ++  seed
      ^-  meta:wt
      (~(got of keys.state) seed-path)
    ::
    ++  watch-addrs
      ^-  (list @t)
      =+  subtree=(~(kids of keys.state) watch-path)
      %+  murn
        ~(tap by kid.subtree)
      |=  [=trek =meta:wt]
      ?:  ?=(%watch-key -.meta)
        `p.meta
      ~
    ::
    ++  watch-locks
      ^-  (list [name=@t lock=@t])
      =+  subtree=(~(kids of keys.state) watch-path)
      %+  murn
        ~(tap by kid.subtree)
      |=  [=trek =meta:wt]
      ?.  ?=(%first-name -.meta)
        ~
      %-  some
      :-  (to-b58:hash:transact name.meta)
      ?~  lock.meta
        'N/A'
      (lock:v1:display u.lock.meta)
    ::
    ++  watch-first-name-locks
      ^-  (list [name=hash:transact lock=(unit lock:transact)])
      =+  subtree=(~(kids of keys.state) watch-path)
      %+  murn
        ~(tap by kid.subtree)
      |=  [=trek =meta:wt]
      ?.  ?=(%first-name -.meta)
        ~
      %-  some
      [name.meta lock.meta]
    ::
    ++  watch-first-names
      ^-  (list @t)
      =+  subtree=(~(kids of keys.state) watch-path)
      %+  roll
        ~(tap by kid.subtree)
      |=  [[=trek =meta:wt] acc=(list @t)]
      ?+    -.meta  acc
          %watch-key
        =+  addr=p.meta
        ?:  (gte (met 3 addr) 132)
          acc
        =+  pubkey-hash=(from-b58:hash:transact addr)
        =+  simple-name=(simple:v1:first-name:t pubkey-hash)
        =+  coinbase-name=(coinbase:v1:first-name:t pubkey-hash)
        :+  (to-b58:hash:transact simple-name)
          (to-b58:hash:transact coinbase-name)
        acc
      ::
          %first-name
        [(to-b58:hash:transact name.meta) acc]
      ==
    ::
    ++  keys
      ^-  (list [trek coil:wt])
      ?~  active-master.state
        ~
      =/  subtree
        %-  ~(kids of keys.state)
        key-path
      %+  murn  ~(tap by kid.subtree)
      |=  [pax=trek =meta:wt]
      ^-  (unit [trek coil:wt])
      ?:(?=(%coil -.meta) `[pax p.meta] ~)
    ::
    ++  coils
      ^-  (list coil:wt)
      %+  turn  keys
      |=  [=trek =coil:wt]
      coil
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
      =.  keys.state  (~(put of keys.state) key-path [%coil coil])
      ?~  label
        keys.state
      %+  ~(put of keys.state)
        (welp key-path /label)
      label/u.label
    ::
    ++  watch-addr
      |=  b58-addr=@t
      %+  ~(put of keys.state)
        (welp watch-path ~[t/b58-addr])
      [%watch-key b58-addr]
    ::
    ++  watch-first-name
      |=  [name=hash:transact lock=(unit lock:transact)]
      =/  name-b58=@t  (to-b58:hash:transact name)
      %+  ~(put of keys.state)
        (welp watch-path ~[t/name-b58])
      [%first-name name lock]
    --
  ::
  ++  get-note
    |=  name=nname:transact
    ^-  nnote:transact
    ?:  (~(has z-by:zo notes.balance.state) name)
      (~(got z-by:zo notes.balance.state) name)
    ~|  "note not found: ".
        "{(trip (name:v1:display name))}"
    !!
  ::
  ++  get-note-v0
    |=  name=nname:transact
    ^-  nnote:v0:transact
    ?:  (~(has z-by:zo notes.balance.state) name)
      =/  note=nnote:transact  (~(got z-by:zo notes.balance.state) name)
      ::  v0 note
      ?>  ?=(^ -.note)
      note
    ~|  "note not found: ".
        "{(trip (name:v1:display name))}"
    !!
  ::
  ::  TODO: way too slow, need a better way to do this or
  ::  remove entirely in favor of requiring note names in
  ::  the causes where necessary.
  ++  find-name-by-hash
    |=  has=hash:transact
    ^-  (unit nname:transact)
    =/  notes=(list [name=nname:transact note=nnote:transact])
      ~(tap z-by:zo notes.balance.state)
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
    ?~  active-master.state
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
    =/  keyc=keyc:s10  ~(keyc get:coil:wt parent)
    ?:  hardened
      ?>  ?=(%prv -.key.parent)
      ::
      =>  (derive:s10 keyc %prv i)
      ?:  =(%1 +..)
        :~  [%1 [%prv private-key] `@ux`chain-code]
            [%1 [%pub public-key] `@ux`chain-code]
        ==
      :~  [%0 [%prv private-key] `@ux`chain-code]
          [%0 [%pub public-key] `@ux`chain-code]
      ==
    ::
    ::  if unhardened, we just assert that they are within the valid range
    ?:  (gte i (bex 31))
      ~|("Unhardened child index {<i>} out of range. Indices are capped to values between [0, 2^31)" !!)
    ?-    -.key.parent
     ::  if the parent is a private key, we can derive the unhardened prv and pub child
        %prv
      =>  [(derive:s10 keyc %prv i) version=ver.keyc]
      ?:  =(%1 version)
        :~  [%1 [%prv private-key] `@ux`chain-code]
            [%1 [%pub public-key] `@ux`chain-code]
        ==
      :~  [%0 [%prv private-key] `@ux`chain-code]
          [%0 [%pub public-key] `@ux`chain-code]
      ==
    ::
     ::  if the parent is a public key, we can only derive the unhardened pub child
        %pub
      =>  [(derive:s10 keyc %pub i) version=ver.keyc]
      ?:  =(%1 version)
        ~[[%1 [%pub public-key] `@ux`chain-code]]
      ~[[%0 [%pub public-key] `@ux`chain-code]]
    ==
  -- ::vault
  ::
  ++  display
    |%
    ++  common
      |%
        ++  format-ui
          |=  @
          ^-  @t
          (rsh [3 2] (scot %ui +<))
        ++  format-ux
          |=  @ux
          ^-  @t
          %-  crip
          (z-co:co +<)
        ::
        ++  poke
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
      --  ::  +common
    ++  v0
      |%
      ::
      ++  transaction
        |=  [name=@t p=inputs:v0:transact]
        ^-  @t
        =/  inputs  `(list [nname:transact input:v0:transact])`~(tap z-by:zo p)
        =/  by-addrs
          %+  roll  inputs
          |=  [[name=nname:transact input=input:v0:transact] acc=_`(z-map:zo sig:transact coins:transact)`~]
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
        |=  [[recipient=sig:transact amt=coins:transact] acc=_acc]
        =/  r58  (to-b58:sig:transact recipient)
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
      ++  note-md
        |=  =nnote:transact
        ^-  markdown:m
        %-  need
        %-  de:md
        (note nnote)
      ::
      ++  note
          |=  note=nnote:transact
          ^-  @t
          ?>  ?=(^ -.note)
          ^-  cord
          ;:  (cury cat 3)
           ;:  (cury cat 3)
              '''

              ---

              ## Details

              '''
              '- Name: '
              =+  (to-b58:nname:transact name.note)
              :((cury cat 3) '[' first ' ' last ']')
              '\0a- Version: '
              (format-ui:common 0)
              '\0a- Assets: '
              (format-ui:common assets.note)
              '\0a- Block Height: '
              (format-ui:common origin-page.note)
              '\0a- Source: '
              (to-b58:hash:transact p.source.note)
              '\0a## Lock'
              '\0a  - Required Signatures: '
              (format-ui:common m.sig.note)
              '\0a  - Signers: '
            ==
          ::
            %+  roll  (serialize-lock sig.note)
            |=  [lock=@t acc=@t]
            ;:  (cury cat 3)
                acc
                '\0a        - '
                lock
            ==
          ::
            '\0a---'
          ==
      ::
      ++  serialize-lock
        |=  =sig:transact
        ^-  (list @t)
        ~+
        pks:(to-b58:sig:transact sig)
      ::
      --  ::  +v0
    ++  v1
      |%
      ++  name
        |=  name=nname:transact
        ^-  @t
        =+  (to-b58:nname:transact name)
        :((cury cat 3) '[' first ' ' last ']')
      ::
      ++  lock
        |=  lk=lock:transact
        ^-  @t
        =/  cond=(unit spend-condition:transact)
          ((soft spend-condition:transact) lk)
        ?~  cond
          'Lock data not displayable'
        (spend-condition u.cond)
      ::
      ++  lock-primitive
        |=  prim=lock-primitive:transact
        ^-  cord
        ?-    -.prim
            %pkh
          =/  participants=(list hash:transact)  ~(tap z-in:zo h.prim)
          %^  cat  3
            '\0a  - PKH Lock (m-of-n)'
          (render-lock-signers m.prim participants)
        ::
            %hax
          =/  hashes=(list hash:transact)  ~(tap z-in:zo +.prim)
          ;:  (cury cat 3)
              '\0a  - Hash Lock'
              '\0a    - Preimage Hashes:'
              %+  roll  hashes
              |=  [h=hash:transact hash-lines=@t]
              ;:  (cury cat 3)
                  hash-lines
                  '\0a      - '
                  (to-b58:hash:transact h)
              ==
          ==
        ::
            %tim
          =/  rel-min=@t
            %^  cat  3
              '\0a      - Min Relative Height: '
            ?~  min.rel.prim  'N/A'
            (format-ui:common u.min.rel.prim)
          =/  rel-max=@t
            %^  cat  3
              '\0a      - Max Relative Height: '
            ?~  max.rel.prim  'N/A'
            (format-ui:common u.max.rel.prim)
          =/  abs-min=@t
            %^  cat  3
              '\0a      - Min Absolute Height: '
            ?~  min.abs.prim  'N/A'
            (format-ui:common u.min.abs.prim)
          =/  abs-max=@t
            %^  cat  3
              '\0a      - Max Absolute Height: '
            ?~  max.abs.prim  'N/A'
            (format-ui:common u.max.abs.prim)
          ;:  (cury cat 3)
              '\0a    - Time Lock'
              rel-min
              rel-max
              abs-min
              abs-max
          ==
        ::
            %brn
          '\0a  - Unspendable (burn) condition'
        ==
      ::
      ++  spend-condition
        |=  cond=spend-condition:transact
        ^-  @t
        %+  roll  cond
        |=  [lp=lock-primitive:transact lines=@t]
        ;:  (cury cat 3)
            lines
            (lock-primitive lp)
        ==
      ::
      ++  lock-data
        |=  data=note-data:v1:transact
        ^-  @t
        ?~  lock-data=(~(get z-by:zo data) %lock)
          ~>  %slog.[2 'lock data in note is missing']  'N/A'
        ?~  soft-lock=((soft lock-data:wt) u.lock-data)
          ~>  %slog.[2 'lock data in note is malformed']  'N/A'
        (lock lock.u.soft-lock)
      ::
      ++  bool-text
        |=  flag=?
        ^-  @t
        ?:  flag  'yes'  'no'
      ::
      ++  render-lock-signers
        |=  [required=@ participants=(list hash:transact)]
        ^-  @t
        =/  signer-text=@t
          %+  roll  participants
          |=  [hash=hash:transact acc=@t]
          ;:  (cury cat 3)
              acc
              '\0a            - '
              (to-b58:hash:transact hash)
          ==
        ;:  (cury cat 3)
            '\0a        - Required Signatures: '
            (format-ui:common required)
            '\0a        - Signers:'
            signer-text
        ==
      ::
      ++  lock-metadata
        |=  data=lock-metadata:wt
        ^-  @t
        =?  data  ?=(^ -.data)
          [%1 %lock data]
        ?>  ?=(@ -.data)
        ?-    -.+.data
            %lock
          =/  cond=(unit spend-condition:transact)
            ((soft spend-condition:transact) lock.data)
          ?~  cond
            '\0a  - Lock data not displayable'
          ;:  (cury cat 3)
            '\0a  - Lock data included in note: '
            (bool-text include-data.data)
            (spend-condition u.cond)
          ==
        ::
            %lock-root
          ;:  (cury cat 3)
            '\0a  - Lock data included in note: '
            (bool-text %.n)
            '\0a  - Assets should go to lock script root: '
            (to-b58:hash:transact root.data)
          ==
        ::
            %bridge-deposit
          ;:  (cury cat 3)
            '\0a  - Lock data included in note: '
            (bool-text %.n)
            '\0a  - Bridge Deposit: '
            '\0a          - Assets should go to lock script root (this should be bridge operator lock root): '
            (to-b58:hash:transact root.data)
            '\0a          - EVM recipient address (tokens should mint to this address): '
            (format-ux:common evm-addr.data)
          ==
        ::
            %bridge-withdrawal
          ;:  (cury cat 3)
            '\0a  - Lock data included in note: '
            (bool-text %.n)
            '\0a  - Bridge Withdrawal: '
            '\0a          - Assets should go to lock script root: '
            (to-b58:hash:transact root.data)
            '\0a          - Base batch end: '
            (format-ui:common base-batch-end.data)
          ==
        ==
    ::
      ++  note-from-balance
        |=  note=nnote-1:v1:transact
        (^note note (lock-data note-data.note) %.n)
    ::
      ++  note-from-output
        |=  $:  note=nnote-1:v1:transact
                metadata=(unit lock-metadata:wt)
            ==
        ?>  ?=(@ -.note)
        =/  lock-info=@t
          ?~  metadata
            (lock-data note-data.note)
          (lock-metadata u.metadata)
        (^note note lock-info %.y)
    ::
      ++  note-from-input
        |=  $:  note=nnote-1:v1:transact
                sc=spend-condition:transact
            ==
        =/  lock-info=@t  (spend-condition sc)
        (^note note lock-info %.n)
    ::
      ::
      ::  +note: display note. Sometimes lock data is not included in note, it can be passed in
      ::    separately in the output-lock-map which is accumulated in the tx-builder.
      ++  note
        |=  $:  note=nnote-1:v1:transact
                lock-info=@t
                output=?
            ==
        ^-  @t
        ;:  (cury cat 3)
           '''

           ## Note Information

           '''
           '- Name: '
           (name name.note)
           '\0a- Version: '
           (format-ui:common 1)
           '\0a- Assets (nicks): '
           (format-ui:common assets.note)
           '\0a- Block Height: '
           ?:  output
             'N/A (output note has not been submitted yet)'
           (format-ui:common origin-page.note)
           '\0a- Lock Information: '
           lock-info
         ==
    ::
      ++  witness-data
        |=  wd=witness-data:wt
        ^-  @t
        =;  signers=(set @t)
          =/  signers-text=@t
            %+  roll  ~(tap in signers)
            |=  [signer=@t text=@t]
            ;:  (cury cat 3)
                text
                '\0a        - '
                signer
            ==
          ;:  (cury cat 3)
              '\0a  - Number of Unique Signers So Far: '
              (format-ui:common ~(wyt in signers))
              '\0a  - Signers So Far: '
              signers-text
          ==
        ?-    -.wd
            %0
          %-  ~(rep z-by:zo p.wd)
          |=  $:  [=nname:transact =signature:transact]
                  signers=(set @t)
              ==
          %-  ~(gas in signers)
          %+  turn  ~(tap z-in:zo ~(key z-by:zo signature))
          to-b58:schnorr-pubkey:transact
        ::
            %1
          %-  ~(rep z-by:zo p.wd)
          |=  $:  [=nname:transact =witness:transact]
                  signers=(set @t)
              ==
          %-  ~(gas in signers)
          %+  turn  ~(tap z-in:zo ~(key z-by:zo pkh.witness))
          to-b58:hash:transact
        ==
      ::
      ++  timelock-range
        |=  [label=@t range=timelock-range:transact]
        ^-  @t
        =/  min-text=@t
            ?~  min.range  'N/A'
            (format-ui:common u.min.range)
        =/  max-text=@t
            ?~  max.range  'N/A'
            (format-ui:common u.max.range)
        ;:  (cury cat 3)
            label
            ' min: '
            min-text  ', max: '  max-text
        ==
    ::
    ::  show-tx should require sync now
      ++  transaction
        |=  $:  name=@t
                outs=outputs:v1:transact
                fees=@
                metadata=metadata:wt
                get-note=$-(nname:transact nnote:transact)
                wd=(unit witness-data:wt)
            ==
        ^-  @t
        =/  input-notes=tape
          ?:  ?=(%0 -.inputs.metadata)
            %-  zing
            %+  turn
            ~(tap z-in:zo ~(key z-by:zo p.inputs.metadata))
            |=  =nname:transact
            =+  note=(get-note nname)
            ?@  -.note
              ~|  %expected-v0-note-but-got-v1-note  !!
            "\0a{(trip (note:v0 note))}"
          %-  zing
          %+  turn
            ~(tap z-by:zo p.inputs.metadata)
          |=  [name=nname:transact sc=spend-condition:transact]
          =/  out-note=nnote:transact  (get-note name)
          ?^  -.out-note
            ~|  %expected-v1-note-but-got-v0-note  !!
          "\0a{(trip (note-from-input out-note sc))}"
        =/  output-notes=tape
          %-  zing
          %+  turn
            ~(tap z-in:zo outs)
          |=  out=output:v1:transact
          =/  out-note=nnote:v1:transact  note.out
          =+  fn=~(first-name get:nnote:transact out-note)
          =+  out-metadata=(~(get z-by:zo outputs.metadata) fn)
          ?^  -.out-note
            "\0a{(trip (note:v0 out-note))}"
          "\0a{(trip (note-from-output out-note out-metadata))}"
        %-  crip
        """

        ### Transaction Information
        - Name: {(trip name)}
        - Fee: {(trip (format-ui:common fees))}

        ### Input Notes
        {input-notes}

        ### Output Notes
        {output-notes}

        ### Witness Data
        {(trip ?~(wd 'N/A' (witness-data u.wd)))}
        ---

        """
      --  ::  +v1
    --  ::  +display
  ::
  ++  show
      |=  [=state:wt =path]
      ^-  [(list effect:wt) state:wt]
      |^
      ?+    path  !!
          [%balance ~]
        :-  ~[[%exit 0] (display-balance balance.state)]
        state
      ::
      ==
      ++  display-balance
        |=  =balance:wt
        ^-  effect:wt
        =/  notes=(list nnote:transact)
          ~(val z-by:zo notes.balance)
        ::  shows the sum of assets included in balance, making sure to exclude watch-only pubkeys
        =/  owned-names=(set hash:transact)
          %-  silt
          %+  roll
            ~(coils ~(get vault state) %pub)
          |=  [=coil:wt first-names=(list hash:transact)]
          ^-  (list hash:transact)
          :*  (simple-first-name:coil:wt coil)
              (coinbase-first-name:coil:wt coil)
              first-names
          ==
        =/  [total-notes=@ total-nicks=coins:transact]
          %+  roll
            ::  all notes owned by keys in wallet, excluding watch-only pubkeys
            %+  skim  notes
            |=  note=nnote:transact
            %-  ~(has in owned-names)
            ~(first-name get:nnote:transact note)
          |=  [note=nnote:transact [len=@ acc=coins:transact]]
          :-  +(len)
          (add acc assets.note)
        =/  nodes=markdown:m
          =+  block-b58=(to-b58:hash:transact block-id.balance.state)
          %-  need
          %-  de:md
          %-  crip
          """
          ## Wallet Balance
          Wallet balance from block {(trip block-b58)} at height {<height.balance.state>}
          - Wallet Version: {<-.state>}
          - Number of Notes: {(trip (format-ui:common:display total-notes))}
          - Balance: {(trip (format-ui:common:display total-nicks))} nicks
          """
        (make-markdown-effect nodes)
      ::
      ::++  display-state
      ::  ^-  (list effect:wt)
      ::  =/  nodes=markdown:m
      ::  %-  need
      ::  %-  de:md
      ::  %-  crip
      ::  """
      ::  ## Wallet State
      ::  - Wallet Version: -.state
      ::  - Last Block: {<block-id.balance.state>}
      ::  - Height: {<height.balance.state>}
      ::  """
      ::  ~[(make-markdown-effect nodes)]
      --
  ::
  ++  ui-to-tape
      |=  @
      ^-  tape
      %-  trip
      (rsh [3 2] (scot %ui +<))
  ::
  ++  ux-to-tape
      |=  @
      ^-  tape
      %-  trip
      (rsh [3 2] (scot %ux +<))
  --
