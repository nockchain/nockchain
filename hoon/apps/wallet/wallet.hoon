::  /ker/wallet/wallet: nockchain wallet
/=  bip39  /common/bip39
/=  slip10  /common/slip10
/=  m  /common/markdown/types
/=  md  /common/markdown/markdown
/=  transact  /common/tx-engine
/=  z   /common/zeke
/=  zo  /common/zoon
/=  dumb  /apps/dumbnet/lib/types
/=  *   /common/zose
/=  *  /common/wrapper
/=  wt  /apps/wallet/lib/types
/=  wutils  /apps/wallet/lib/utils
/=  tx-builder  /apps/wallet/lib/tx-builder
=>
=|  bug=_&
|%
++  utils  ~(. wutils bug)
::
::  re-exporting names from wallet types while passing the bug flag
++  debug  debug:utils
++  warn  warn:utils
++  s10  s10:utils
++  moat  (keep state:wt)
--
::
%-  (moat &)
^-  fort:moat
|_  =state:wt
+*  v  ~(. vault:utils state)
    d  ~(. draw:utils state)
    e  ~(. edit:utils state)
    p  ~(. plan:utils transaction-tree.state)
::
++  load
  |=  old=versioned-state:wt
  ^-  state:wt
  |^
  ?-  -.old
    %0  state-0-1
    %1  old
  ==
  ::
  ++  state-0-1
    ^-  state:wt
    ?>  ?=(%0 -.old)
    :*  %1
        balance.old
        master.old
        keys.old
        last-block.old
        peek-requests.old
        active-transaction.old
        active-input.old
        active-seed.old
        transaction-tree.old
        pending-commands.old
    ==
  --
::
++  peek
  |=  =path
  ^-  (unit (unit *))
  %-  (debug "peek: {<state>}")
  ?+    path  ~
    ::
      [%balance ~]
    ``balance.state
    ::
      [%state ~]
    ``state
  ==
::
++  poke
  |=  =ovum:moat
  |^
  ^-  [(list effect:wt) state:wt]
  =/  cause=(unit cause:wt)
    %-  (soft cause:wt)
    cause.input.ovum
  =/  failure=effect:wt  [%markdown '## Poke failed']
  ?~  cause
    %-  (warn "input does not have a proper cause: {<cause.input.ovum>}")
    [~[failure] state]
  =/  =cause:wt  u.cause
  ::%-  (debug "cause: {<-.cause>}")
  =/  wir=(pole)  wire.ovum
  ?+    wir  ~|("unsupported wire: {<wire.ovum>}" !!)
      [%poke %grpc ver=@ pid=@ tag=@tas ~]
    ::
    ::  at the time of writing, there is only one poke that emits a %grpc
    ::  therefore, it is unnecessary at this point to manage pending requests.
    =^  effs  state
      (do-grpc-bind cause tag.wir)
    [effs state]
  ::
      [%poke ?(%one-punch %sys %wallet) ver=@ *]
    ?+    -.cause  ~|("unsupported cause: {<-.cause>}" !!)
        %show                  (show:utils state path.cause)
        %keygen                (do-keygen cause)
        %derive-child          (do-derive-child cause)
        %sign-tx               (do-sign-tx cause)
        %scan                  (do-scan cause)
        %list-notes            (do-list-notes cause)
        %list-notes-by-pubkey  (do-list-notes-by-pubkey cause)
        %list-notes-by-pubkey-csv  (do-list-notes-by-pubkey-csv cause)
        %create-tx             (do-create-tx cause)
        %update-balance-grpc   (do-update-balance-grpc cause)
        %sign-message          (do-sign-message cause)
        %verify-message        (do-verify-message cause)
        %sign-hash             (do-sign-hash cause)
        %verify-hash           (do-verify-hash cause)
        %import-keys           (do-import-keys cause)
        %import-extended       (do-import-extended cause)
        %export-keys           (do-export-keys cause)
        %export-master-pubkey  (do-export-master-pubkey cause)
        %import-master-pubkey  (do-import-master-pubkey cause)
        %gen-master-privkey    (do-gen-master-privkey cause)
        %gen-master-pubkey     (do-gen-master-pubkey cause)
        %send-tx               (do-send-tx cause)
        %show-tx               (do-show-tx cause)
        %list-pubkeys          (do-list-pubkeys cause)
        %show-seedphrase       (do-show-seedphrase cause)
        %show-master-pubkey    (do-show-master-pubkey cause)
        %show-master-privkey   (do-show-master-privkey cause)
    ::
        %advanced-spend
      ?-  +<.cause
        %seed   (do-advanced-spend-seed +>.cause)
        %input  (do-advanced-spend-input +>.cause)
        %transaction  (do-advanced-spend-transaction +>.cause)
      ==
    ::
        %file
      ?>  ?=(%write +<.cause)
      [[%exit 0]~ state]
    ==
  ==
  ::
  ++  do-grpc-bind
    |=  [=cause:wt typ=@tas]
    %-  (debug "grpc-bind")
    ?>  ?=(%grpc-bind -.cause)
    ?+    typ  !!
        %balance
      (do-update-balance-grpc [%update-balance-grpc result.cause])
    ==
  ::
  ++  do-update-balance-grpc
    |=  =cause:wt
    ?>  ?=(%update-balance-grpc -.cause)
    %-  (debug "update-balance-grpc")
    %-  (debug "last balance size: {<(lent ~(tap z-by:zo balance.state))>}")
    =/  softed=(unit (unit (unit (z-map:zo nname:transact nnote:transact))))
      %-  (soft (unit (unit (z-map:zo nname:transact nnote:transact))))
      balance.cause
    ?~  softed
      %-  (debug "do-update-balance-grpc: %balance: could not soft result")
      [~ state]
    =/  balance-result=(unit (unit _balance.state))  u.softed
    ?~  balance-result
      %-  (warn "%update-balance did not return a result: bad path")
      [~ state]
    ?~  u.balance-result
      %-  (warn "%update-balance did not return a result: nothing")
      [~ state]
    ?~  u.u.balance-result
      %-  (warn "%update-balance did not return a result: empty result")
      [~ state]
    =.  balance.state  u.u.balance-result
    %-  (debug "balance state updated!")
    [~ state]
  ::
  ++  do-import-keys
    |=  =cause:wt
    ?>  ?=(%import-keys -.cause)
    =/  new-keys=_keys.state
      %+  roll  keys.cause
      |=  [[=trek =meta:wt] acc=_keys.state]
      (~(put of acc) trek meta)
    =/  master-key=coil:wt
      %-  head
      %+  murn  ~(tap of new-keys)
      |=  [t=trek m=meta:wt]
      ^-  (unit coil:wt)
      ?:  ?&
            ?=(%coil -.m)
            =((slag 2 t) /pub/m)
          ==
        `m
      ~
    =/  key-list=(list tape)
      %+  murn  ~(tap of new-keys)
      |=  [t=trek m=meta:wt]
      ^-  (unit tape)
      ?.  ?=(%coil -.m)  ~
      =/  key-type=tape  ?:(?=(%pub -.key.m) "Public Key" "Private Key")
      =/  key=@t  (slav %t (snag 1 (pout t)))
      =+  (to-b58:coil:wt m)
      %-  some
      """
      - {key-type}: {(trip key-b58)}
      - Parent Key: {(trip key)}
      ---

      """
    =.  master.state  `master-key
    :_  state(keys new-keys)
    :~  :-  %markdown
        %-  crip
        """
        ## Imported Keys

        {(zing key-list)}
        """
        [%exit 0]
    ==
  ::
  ++  do-import-extended
    |=  =cause:wt
    ?>  ?=(%import-extended -.cause)
    %-  (debug "import-extended: {<extended-key.cause>}")
    =/  core  (from-extended-key:s10 extended-key.cause)
    =/  is-private=?  !=(0 prv:core)
    =/  key-type=?(%pub %prv)  ?:(is-private %prv %pub)
    =/  coil-key=key:wt
      ?:  is-private
        [%prv private-key:core]
      [%pub public-key:core]
    =/  imported-coil=coil:wt  [%coil coil-key chain-code:core]
    =/  public-coil=coil:wt  [%coil [%pub public-key] chain-code]:core
    =/  key-label=@t
      ?:  is-private
        (crip "imported-private-{<(end [3 4] public-key:core)>}")
      (crip "imported-public-{<(end [3 4] public-key:core)>}")
    ::  if this is a master key (no parent), set as master
    ?:  =(0 dep:core)
      =.  master.state  (some public-coil)

      =.  keys.state  (key:put:v imported-coil ~ `key-label)
      =.  keys.state  (key:put:v public-coil ~ `key-label)
      =/  extended-type=tape  ?:(is-private "private" "public")
      :_  state
      :~  :-  %markdown
          %-  crip
          """
          ## imported import {extended-type} key

          - import key: {(trip extended-key.cause)}
          - label: {(trip key-label)}
          - set as master key
          """
          [%exit 0]
      ==
    ::  otherwise, import as derived key
    ::  first validate that this key is actually a child of the current master
    ?~  master.state
      :_  state
      :~  :-  %markdown
          %-  crip
          """
          ## import failed

          cannot import derived key: no master key set
          """
          [%exit 1]
      ==
    =/  master-pubkey-coil=coil:wt  (public:master:wt master.state)
    =/  expected-children=(set coil:wt)
      (derive-child:v ind:core)
    =/  imported-pubkey=@  public-key:core
    ::  find the public key coil from the derived children set
    =/  expected-pubkey-coil=(unit coil:wt)
      %-  ~(rep in expected-children)
      |=  [=coil:wt acc=(unit coil:wt)]
      ?^  acc  acc
      ?:  ?=(%pub -.key.coil)
        `coil
      ~
    ?~  expected-pubkey-coil
      ~|("no public key found in derived children - this should not happen" !!)
    =/  expected-pubkey=@  p.key.u.expected-pubkey-coil
    ?.  =(imported-pubkey expected-pubkey)
      :_  state
      :~  :-  %markdown
          %-  crip
          """
          ## Import Failed

          Imported key at index {<ind:core>} does not match expected child of master key

          - Imported Public Key: {<imported-pubkey>}
          - Expected Public Key: {<expected-pubkey>}
          """
          [%exit 1]
      ==
    ::  key is valid, proceed with import
    =.  keys.state  (key:put:v imported-coil `ind:core `key-label)
    =/  extended-type=tape  ?:(is-private "private" "public")
    :_  state
    :~  :-  %markdown
        %-  crip
        """
        ## Imported {extended-type} Key

        - Import Key: {(trip extended-key.cause)}
        - Label: {(trip key-label)}
        - Index: {<ind:core>}
        - Verified as child of master key
        """
        [%exit 0]
    ==
  ::
  ++  do-export-keys
    |=  =cause:wt
    ?>  ?=(%export-keys -.cause)
    =/  keys-list=(list [trek meta:wt])
      ~(tap of keys.state)
    =/  dat-jam  (jam keys-list)
    =/  path=@t  'keys.export'
    =/  =effect:wt  [%file %write path dat-jam]
    :_  state
    :~  effect
        :-  %markdown
        %-  crip
        """
        ## Exported Keys

        - Path: {<path>}
        """
        [%exit 0]
    ==
  ::
  ++  do-export-master-pubkey
    |=  =cause:wt
    ?>  ?=(%export-master-pubkey -.cause)
    %-  (debug "export-master-pubkey")
    ?~  master.state
      %-  (warn "wallet: no master keys available for export")
      [[%exit 0]~ state]
    =/  master-coil=coil:wt  ~(master get:v %pub)
    ?.  ?=(%pub -.key.master-coil)
      %-  (warn "wallet: fatal: master pubkey malformed")
      [[%exit 0]~ state]
    =/  dat-jam=@  (jam master-coil)
    =/  key-b58=tape  (en:base58:wrap p.key.master-coil)
    =/  cc-b58=tape  (en:base58:wrap cc.master-coil)
    =/  extended-key=@t
      =/  core  (from-public:s10 [p.key cc]:master-coil)
      extended-public-key:core
    =/  file-path=@t  'master-pubkey.export'
    :_  state
    :~  :-  %markdown
        %-  crip
        """
        ## Exported Master Public Key

        - Import Key: {(trip extended-key)}
        - Public Key: {key-b58}
        - Chain Code: {cc-b58}
        - File: {(trip file-path)}
        """
        [%exit 0]
        [%file %write file-path dat-jam]
    ==
  ::
  ++  do-import-master-pubkey
    |=  =cause:wt
    ?>  ?=(%import-master-pubkey -.cause)
    %-  (debug "import-master-pubkey: {<coil.cause>}")
    =/  master-pubkey-coil=coil:wt  coil.cause
    =.  master.state  (some master-pubkey-coil)
    =/  label  `(crip "master-public-{<(end [3 4] p.key.master-pubkey-coil)>}")
    =.  keys.state  (key:put:v master-pubkey-coil ~ label)
    =/  key-b58=tape  (en:base58:wrap p.key.master-pubkey-coil)
    =/  cc-b58=tape  (en:base58:wrap cc.master-pubkey-coil)
    :_  state
    :~  :-  %markdown
        %-  crip
        """
        ## Imported Master Public Key

        - Public Key: {key-b58}
        - Chain Code: {cc-b58}
        """
        [%exit 0]
    ==
  ::
  ++  do-gen-master-privkey
    |=  =cause:wt
    ?>  ?=(%gen-master-privkey -.cause)
    ::  We do not need to reverse the endian-ness of the seedphrase
    ::  because the bip39 code expects a tape.
    =/  seed=byts  [64 (to-seed:bip39 (trip seedphrase.cause) "")]
    =/  cor  (from-seed:s10 seed)
    =/  master-pubkey-coil=coil:wt  [%coil [%pub public-key] chain-code]:cor
    =/  master-privkey-coil=coil:wt  [%coil [%prv private-key] chain-code]:cor
    =.  master.state  (some master-pubkey-coil)
    =/  public-label  `(crip "master-public-{<(end [3 4] public-key:cor)>}")
    =/  private-label  `(crip "master-private-{<(end [3 4] public-key:cor)>}")
    =.  keys.state  (key:put:v master-privkey-coil ~ private-label)
    =.  keys.state  (key:put:v master-pubkey-coil ~ public-label)
    =.  keys.state  (seed:put:v seedphrase.cause)
    %-  (debug "master.state: {<master.state>}")
    :_  state
    :~  :-  %markdown
        %-  crip
        """
        ## Master Key (Imported)

        - Seed Phrase: {<seedphrase.cause>}
        - Master Public Key: {(en:base58:wrap p.key.master-pubkey-coil)}
        - Master Private Key: {(en:base58:wrap p.key.master-privkey-coil)}
        - Chain Code: {(en:base58:wrap cc.master-privkey-coil)}
        """
        [%exit 0]
    ==
  ::
  ++  do-gen-master-pubkey
    |=  =cause:wt
    ?>  ?=(%gen-master-pubkey -.cause)
    =/  privkey-atom=@
      (de:base58:wrap (trip privkey-b58.cause))
    =/  chain-code-atom=@
      (de:base58:wrap (trip cc-b58.cause))
    =/  =keyc:slip10  [privkey-atom chain-code-atom]
    =/  cor  (from-private:s10 keyc)
    =/  master-pubkey-coil=coil:wt  [%coil [%pub public-key] chain-code]:cor
    =/  master-privkey-coil=coil:wt  [%coil [%prv private-key] chain-code]:cor
    %-  (debug "Generated master public key: {<public-key:cor>}")
    =/  public-label  `(crip "master-public-{<(end [3 4] public-key:cor)>}")
    =/  private-label  `(crip "master-private-{<(end [3 4] public-key:cor)>}")
    =.  master.state  (some master-pubkey-coil)

    =.  keys.state  (key:put:v master-privkey-coil ~ private-label)
    =.  keys.state  (key:put:v master-pubkey-coil ~ public-label)
    %-  (debug "master.state: {<master.state>}")
    =/  extended-key=@t  extended-public-key:cor
    :_  state
    :~  :-  %markdown
        %-  crip
        """
        ## Master Public Key (Imported)

        - Import Key: {(trip extended-key)}
        - Public Key: {(en:base58:wrap p.key.master-pubkey-coil)}
        - Private Key: {(en:base58:wrap p.key.master-privkey-coil)}
        - Chain Code: {(en:base58:wrap cc.master-pubkey-coil)}
        """
        [%exit 0]
    ==
  ::
  ++  do-send-tx
    |=  =cause:wt
    ?>  ?=(%send-tx -.cause)
    %-  (debug "send-tx: creating raw-tx")
    ::  note that new:raw-tx calls +validate already
    =/  raw=raw-tx:transact  (new:raw-tx:transact p.dat.cause)
    =/  tx-id  id.raw
    =/  nock-cause=$>(%fact cause:dumb)
      [%fact %0 %heard-tx raw]
    %-  (debug "send-tx: made raw-tx, sending poke request over grpc")
    =/  pid  generate-pid:v
    :_  state
    :~
      [%grpc %poke pid nock-cause]
      [%exit 0]
    ==
  ::
  ++  do-show-tx
    |=  =cause:wt
    ?>  ?=(%show-tx -.cause)
    %-  (debug "show-tx: displaying transaction")
    =/  =transaction:wt  dat.cause
    =/  transaction-name=@t  name.transaction
    =/  ins-transaction=inputs:transact  p.transaction
    =/  markdown-text=@t  (display-transaction-cord:utils transaction-name ins-transaction)
    :_  state
    :~
      [%markdown markdown-text]
      [%exit 0]
    ==
  ::
  ++  do-list-pubkeys
    |=  =cause:wt
    ?>  ?=(%list-pubkeys -.cause)
    =/  pubkeys  ~(coils get:v %pub)
    =/  base58-keys=(list tape)
      %+  turn  pubkeys
      |=  =coil:wt
      =/  [key-b58=@t cc-b58=@t]  (to-b58:coil:wt coil)
      """
      - Public Key: {<key-b58>}
      - Chain Code: {<cc-b58>}
      ---

      """
    :_  state
    :~  :-  %markdown
        %-  crip
        """
        ## Public Keys

        {?~(base58-keys "No pubkeys found" (zing base58-keys))}
        """
        [%exit 0]
    ==
  ::
  ++  do-show-seedphrase
    |=  =cause:wt
    ?>  ?=(%show-seedphrase -.cause)
    %-  (debug "show-seedphrase")
    =/  =meta:wt  seed:get:v
    =/  seedphrase=@t
      ?:  ?=(%seed -.meta)
        +.meta
      %-  crip
      "no seedphrase found"
    :_  state
    :~  :-  %markdown
        %-  crip
        """
        ## Seed Phrase

        {<seedphrase>}
        """
        [%exit 0]
    ==
  ::
  ++  do-show-master-pubkey
    |=  =cause:wt
    ?>  ?=(%show-master-pubkey -.cause)
    %-  (debug "show-master-pubkey")
    =/  =meta:wt  ~(master get:v %pub)
    ?>  ?=(%coil -.meta)
    =/  extended-key=@t
      =/  core  (from-public:s10 [p.key cc]:meta)
      extended-public-key:core
    :_  state
    :~  :-  %markdown
        %-  crip
        """
        ## Master Public Key

        - Import Key: {<extended-key>}
        - Public Key: {(en:base58:wrap p.key.meta)}
        - Chain Code: {(en:base58:wrap cc.meta)}
        """
        [%exit 0]
    ==
  ::
  ++  do-show-master-privkey
    |=  =cause:wt
    ?>  ?=(%show-master-privkey -.cause)
    %-  (debug "show-master-privkey")
    =/  =meta:wt  ~(master get:v %prv)
    ?>  ?=(%coil -.meta)
    =/  extended-key=@t
      =/  core  (from-private:s10 [p.key cc]:meta)
      extended-private-key:core
    =/  key-b58=tape  (en:base58:wrap p.key.meta)
    =/  cc-b58=tape  (en:base58:wrap cc.meta)
    :_  state
    :~  :-  %markdown
        %-  crip
        """
        ## Master Private Key

        - Import Key: {<extended-key>}
        - Private Key: {(en:base58:wrap p.key.meta)}
        - Chain Code: {(en:base58:wrap cc.meta)}
        """
        [%exit 0]
    ==
  ::
  ++  do-scan
    |=  =cause:wt
    ?>  ?=(%scan -.cause)
    %-  (debug "scan: scanning {<search-depth.cause>} addresses")
    ?>  ?=(^ master.state)
    ::  get all public keys up to search depth
    =/  index=@ud  search-depth.cause
    =/  coils=(list coil:wt)
      =/  keys=(list coil:wt)  [~(master get:v %pub)]~
      =|  done=_|
      |-  ^-  (list coil:wt)
      ?:  done  keys
      =?  done  =(0 index)  &
      =/  base=trek  /keys/pub/[ux/p.key:(public:master:wt master.state)]/[ud/index]
      =/  key=(unit coil:wt)
        ;;  (unit coil:wt)
        (~(get of keys.state) base)
      %=  $
        index  ?:(=(0 index) 0 (dec index))
        keys  ?^(key (snoc keys u.key) keys)
      ==
    ::  fail when no coils
    ?:  ?=(~ coils)
      ~|("no coils for master key" !!)
    ::  generate first names of notes owned by each pubkey
    =/  first-names=(list [hash:transact schnorr-pubkey:transact])
      %+  turn  coils
      |=  =coil:wt
      ::  create lock from public key
      =/  pubkey=schnorr-pubkey:transact  pub:(from-public:s10 [p.key cc]:coil)
      =/  =lock:transact  (new:lock:transact pubkey)
      ::  generate name and take first name
      =/  match-name=nname:transact
        %-  new:nname:transact
        :*  lock
            [*hash:transact %.n]  ::  empty source, not a coinbase
            *timelock:transact    ::  no timelock
        ==
      [-.match-name pubkey]
    ::  find notes with matching first names in balance
    =/  notes=(list nnote:transact)
      %+  murn
        ~(tap z-by:zo balance.state)
      |=  [name=nname:transact note=nnote:transact]
      ^-  (unit nnote:transact)
      ::  check if first name matches any in our list
      =/  matches
        %+  lien  first-names
        |=  [first-name=hash:transact pubkey=schnorr-pubkey:transact]
        =/  =lock:transact  (new:lock:transact pubkey)
        ::  update lock if include-multisig is true and pubkey is in
        ::  the multisig set in the note's lock
        =?  lock
          ?&  include-multisig.cause
              (~(has z-in:zo pubkeys.lock.note) pubkey)
          ==
        lock.note
        ::  update match-name if include-timelocks is set
        =?  first-name  include-timelocks.cause
          =<  -
          %-  new:nname:transact
          :*  lock
              [*hash:transact %.n]  ::  empty source, not a coinbase
              timelock.note         ::  include timelock
          ==
        =(-.name first-name)
      ?:(matches `note ~)
    %-  (debug "found matches: {<notes>}")
    =/  nodes=markdown:m
      :~  :-  %leaf
          :-  %heading
          :*  %atx  1
              :~  [%text 'Scan Result']
              ==
          ==
          :-  %container
          :-  %ul
          :*  0  '*'
             (turn notes display-note:utils)
          ==
      ==
    :_  state
    ~[(make-markdown-effect:utils nodes) [%exit 0]]
  ::
  ++  do-list-notes
    |=  =cause:wt
    ?>  ?=(%list-notes -.cause)
    %-  (debug "list-notes")
    :_  state
    :~  :-  %markdown
      %-  crip
      %+  welp
      """
      ## Wallet Notes

      """
      =-  ?:  =("" -)  "No notes found"  -
      %-  zing
      %+  turn  ~(val z-by:zo balance.state)
      |=  =nnote:transact
      %-  trip
      (display-note-cord:utils nnote)
      ::
      [%exit 0]
    ==
  ::
  ++  do-list-notes-by-pubkey
    |=  =cause:wt
    ?>  ?=(%list-notes-by-pubkey -.cause)
    =/  target-pubkey=schnorr-pubkey:transact
      (from-b58:schnorr-pubkey:transact pubkey.cause)
    =/  matching-notes=(list [name=nname:transact note=nnote:transact])
      %+  skim  ~(tap z-by:zo balance.state)
      |=  [name=nname:transact note=nnote:transact]
      (~(has z-in:zo pubkeys.lock.note) target-pubkey)
    :_  state
    :~  :-  %markdown
        %-  crip
        %+  welp
          """
          ## Wallet Notes for Public Key {<(to-b58:schnorr-pubkey:transact target-pubkey)>}

          """
        =-  ?:  =("" -)  "No notes found"  -
        %-  zing
        %+  turn  matching-notes
        |=  [* =nnote:transact]
        %-  trip
        (display-note-cord:utils nnote)
        ::
        [%exit 0]
    ==
  ::
  ++  do-list-notes-by-pubkey-csv
    |=  =cause:wt
    ?>  ?=(%list-notes-by-pubkey-csv -.cause)
    =/  target-pubkey=schnorr-pubkey:transact
      (from-b58:schnorr-pubkey:transact pubkey.cause)
    =/  matching-notes=(list [name=nname:transact note=nnote:transact])
      %+  skim  ~(tap z-by:zo balance.state)
      |=  [name=nname:transact note=nnote:transact]
      (~(has z-in:zo pubkeys.lock.note) target-pubkey)
    =/  csv-header=tape
      "name_first,name_last,assets,block_height,source_hash"
    =/  csv-rows=(list tape)
      %+  turn  matching-notes
      |=  [name=nname:transact note=nnote:transact]
      =/  name-b58=[first=@t last=@t]  (to-b58:nname:transact name)
      =/  source-hash-b58=@t  (to-b58:hash:transact p.source.note)
      """
      {(trip first.name-b58)},{(trip last.name-b58)},{(ui-to-tape:utils assets.note)},{(ui-to-tape:utils origin-page.note)},{(trip source-hash-b58)}
      """
    =/  csv-content=tape
      %+  welp  csv-header
      %+  welp  "\0a"
      %-  zing
      %+  turn  csv-rows
      |=  row=tape
      "{row}\0a"
    =/  filename=@t
      %-  crip
      "notes-{(trip (to-b58:schnorr-pubkey:transact target-pubkey))}.csv"
    :_  state
    :~  :-  %file
        :-  %write
        :-  filename
        %-  crip
        csv-content
        [%exit 0]
    ==
  ::
  ++  do-create-tx
    |=  =cause:wt
    ?>  ?=(%create-tx -.cause)
    |^
    %-  (debug "create-tx: {<names.cause>}")
    =/  names=(list nname:transact)  (parse-names names.cause)
    =/  sign-key  (sign-key:get:v sign-key.cause)
    =/  ins=inputs:transact  (tx-builder names order.cause fee.cause sign-key timelock-intent.cause get-note:v)
    (save-transaction ins)
    ::
    ++  parse-names
      |=  raw-names=(list [first=@t last=@t])
      ^-  (list nname:transact)
      %+  turn  raw-names
      |=  [first=@t last=@t]
      (from-b58:nname:transact [first last])
    ::
    ++  validate-inputs
      |=  =inputs:transact
      ^-  ?
      ?&  (validate:inputs:transact inputs)
          %+  levy  ~(tap z-by:zo inputs)
          |=  [name=nname:transact inp=input:transact]
          (spendable:lock:transact lock.note.inp)
      ==
    ::
    ++  save-transaction
      |=  ins=inputs:transact
      ^-  [(list effect:wt) state:wt]
      ~&  "Validating transaction before saving"
      ::  we fallback to the hash of the inputs as the transaction name
      ::  if the tx is invalid. this is just for display
      ::  in the error message, as an invalid tx is not saved.
      =/  transaction-name=@t
        %-  to-b58:hash:transact
        =-  %+  fall  -
            (hash:inputs:transact ins)
        %-  mole
        |.
        ::  TODO: this also calls validate:inputs, but we need it to
        ::  get the id of the transaction. we should deduplicate this.
        id:(new:raw-tx:transact ins)
      ::  jam inputs and save as transaction
      =/  =transaction:wt
        %*  .  *transaction:wt
          p  ins
          name  transaction-name
        ==
      ?.  (validate-inputs ins)
        =/  msg=@t
          %^  cat  3
            %-  crip
            """
            # TX Validation Failed

            Failed to validate the correctness of transaction {(trip transaction-name)}.

            Check that the note(s) you are spending from:

            1. Can be spent by the public key you are signing with.
            2. Have enough assets to cover the gift and fee.
            3. Have the correct timelock intent.

            ---


            """
          (display-transaction-cord:utils transaction-name ins)
        %-  (debug "{(trip msg)}")
        :_  state
        :~  [%markdown msg]
            [%exit 1]
        ==
      =/  transaction-jam  (jam transaction)
      =/  markdown-text=@t  (display-transaction-cord:utils transaction-name ins)
      =/  path=@t
        %-  crip
        "./txs/{(trip name.transaction)}.tx"
      %-  (debug "saving transaction to {<path>}")
      =/  =effect:wt  [%file %write path transaction-jam]
      :-  ~[effect [%markdown markdown-text] [%exit 0]]
      state
    --
  ::
  ++  do-keygen
    |=  =cause:wt
    ?>  ?=(%keygen -.cause)
    =+  [seed-phrase=@t cor]=(gen-master-key:s10 entropy.cause salt.cause)
    =/  master-public-coil  [%coil [%pub public-key] chain-code]:cor
    =/  master-private-coil  [%coil [%prv private-key] chain-code]:cor
    =.  master.state  (some master-public-coil)

    %-  (debug "keygen: public key: {<(en:base58:wrap public-key:cor)>}")
    %-  (debug "keygen: private key: {<(en:base58:wrap private-key:cor)>}")
    =/  pub-label  `(crip "master-public-{<(end [3 4] public-key:cor)>}")
    =/  prv-label  `(crip "master-public-{<(end [3 4] public-key:cor)>}")
    =.  keys.state  (key:put:v master-public-coil ~ pub-label)
    =.  keys.state  (key:put:v master-private-coil ~ prv-label)
    =.  keys.state  (seed:put:v seed-phrase)
    =/  extended-private=@t  extended-private-key:cor
    =/  extended-public=@t  extended-public-key:cor
    :_  state
    :~  :-  %markdown
        %-  crip
        """
        ## Keygen

        ### New Public Key
        {<(en:base58:wrap public-key:cor)>}

        ### New Private Key
        {<(en:base58:wrap private-key:cor)>}

        ### Chain Code
        {<(en:base58:wrap chain-code:cor)>}

        ### Import Private Key
        {(trip extended-private)}

        ### Import Public Key
        {(trip extended-public)}

        ### Seed Phrase
        {<seed-phrase>}
        """
        [%exit 0]
    ==
  ::
  ::  derives child keys of current master key
  ::  at index `i`. this will overwrite existing paths if
  ::  the master key changes
  ++  do-derive-child
    |=  =cause:wt
    ?>  ?=(%derive-child -.cause)
    =/  index
      ?:  hardened.cause
        (add i.cause (bex 31))
      i.cause
    =/  derived-keys=(set coil:wt)  (derive-child:v index)
    =.  keys.state
      %-  ~(rep in derived-keys)
      |=  [=coil:wt keys=_keys.state]
      =.  keys.state  keys
      (key:put:v coil `index label.cause)
    ::
    =/  key-text=tape
      %-  zing
      %+  turn  ~(tap in derived-keys)
      |=  =coil:wt
      =/  [key-b58=@t cc-b58=@t]  (to-b58:coil:wt coil)
      =/  key-type=tape
        ?:  ?=(%pub -.key.coil)
          "Public Key"
        "Private Key"
      """
      - {key-type}: {<key-b58>}
      - Chain Code: {<cc-b58>}
      """
    :_  state
    :~
      :-  %markdown
      %-  crip
      """
      ## Derive Child

      ### Derived Keys
      {key-text}
      """
      [%exit 0]
    ==
  ::
  ++  do-sign-tx
    |=  =cause:wt
    ?>  ?=(%sign-tx -.cause)
    %-  (debug "sign-tx: {<dat.cause>}")
    ::  get private key at child index, or master key if no index
    ::  add 2^31 to child index if hardened
    =/  sender-key=schnorr-seckey:transact
      (sign-key:get:v sign-key.cause)
    =/  signed-inputs=inputs:transact
      %-  ~(run z-by:zo p.dat.cause)
      |=  =input:transact
      %-  (debug "signing input: {<input>}")
      =.  spend.input
        %+  sign:spend:transact
          spend.input
        sender-key
      input
    =/  signed-transaction=transaction:wt
      %=  dat.cause
        p  signed-inputs
      ==
    =/  transaction-jam  (jam signed-transaction)
    =/  path=@t
      %-  crip
      "./txs/{(trip name.signed-transaction)}.tx"
    %-  (debug "saving input transaction to {<path>}")
    =/  =effect:wt  [%file %write path transaction-jam]
    :-  ~[effect [%exit 0]]
    state
  ::
  ++  do-sign-message
    |=  =cause:wt
    ?>  ?=(%sign-message -.cause)
    =/  sk=schnorr-seckey:transact  (sign-key:get:v sign-key.cause)
    =/  msg-belts=page-msg:transact  (new:page-msg:transact `cord`msg.cause)
    ?.  (validate:page-msg:transact msg-belts)
      :_  state
      :~  :-  %markdown
          %-  crip
          """
          # Message could not be converted to a list of based elements, cannot sign

          ### Message

          {(trip `@t`msg.cause)}

          """
          [%exit 1]
      ==
    =/  digest  (hash:page-msg:transact msg-belts)
    =/  sig=schnorr-signature:transact
      %+  sign:affine:belt-schnorr:cheetah:z
        sk
      digest
    =/  sig-hash  (hash:schnorr-signature:transact sig)
    =/  sig-jam=@  (jam sig)
    =/  path=@t  'message.sig'
    =/  markdown-text=@t
      %-  crip
      """
      # Message signed, signature saved to message.sig

      ### Message

      {(trip `@t`msg.cause)}

      ### Signature (Hashed)

      {(trip (to-b58:hash:transact sig-hash))}

      """
    :_  state
    :~  [%file %write path sig-jam]
        [%markdown markdown-text]
        [%exit 0]
    ==
  ::
  ++  do-verify-message
    |=  =cause:wt
    ?>  ?=(%verify-message -.cause)
    =/  sig=schnorr-signature:transact
      (need ((soft schnorr-signature:transact) (cue sig.cause)))
    =/  pk=schnorr-pubkey:transact
      (from-b58:schnorr-pubkey:transact pk-b58.cause)
    =/  msg-belts=page-msg:transact  (new:page-msg:transact `cord`msg.cause)
    ?.  (validate:page-msg:transact msg-belts)
      :_  state
      :~  :-  %markdown
          %-  crip
          """
          # Message could not be converted to a list of based elements, cannot verify signature

          ### Message

          {(trip `@t`msg.cause)}

          """
          [%exit 1]
      ==
    =/  digest  (hash:page-msg:transact msg-belts)
    =/  ok=?
      %:  verify:affine:belt-schnorr:cheetah:z
          pk
          digest
          sig
      ==
    :_  state
    :~  :-  %markdown
        ?:  ok  '# Valid signature, message verified'  '# Invalid signature, message not verified'
        [%exit ?:(ok 0 1)]
    ==
  ::
  ++  do-sign-hash
    |=  =cause:wt
    ?>  ?=(%sign-hash -.cause)
    =/  sk=schnorr-seckey:transact  (sign-key:get:v sign-key.cause)
    =/  digest=hash:transact  (from-b58:hash:transact hash-b58.cause)
    =/  sig=schnorr-signature:transact
      %+  sign:affine:belt-schnorr:cheetah:z
        sk
      digest
    =/  sig-jam=@  (jam sig)
    =/  path=@t  'hash.sig'
    :_  state
    :~  [%file %write path sig-jam]
        [%markdown '## Hash signed, signature saved to hash.sig']
        [%exit 0]
    ==
  ::
  ++  do-verify-hash
    |=  =cause:wt
    ?>  ?=(%verify-hash -.cause)
    =/  sig=schnorr-signature:transact
      (need ((soft schnorr-signature:transact) (cue sig.cause)))
    =/  pk=schnorr-pubkey:transact
      (from-b58:schnorr-pubkey:transact pk-b58.cause)
    =/  digest=hash:transact  (from-b58:hash:transact hash-b58.cause)
    =/  ok=?
      %:  verify:affine:belt-schnorr:cheetah:z
          pk
          digest
          sig
      ==
    :_  state
    :~  :-  %markdown
        ?:  ok  '# Valid signature, hash verified'  '# Invalid signature, hash not verified'
        [%exit ?:(ok 0 1)]
    ==
  ::
  ++  do-advanced-spend-seed
    |=  cause=advanced-spend-seed:wt
    ^-  [(list effect:wt) state:wt]
    |^
    =?  active-transaction.state  ?=(~ active-transaction.state)  `*transaction-name:wt
    =?  active-seed.state  ?=(~ active-seed.state)  `*seed-name:wt
    ?-  -.cause
      %new  do-new
      %set-name  do-set-name
      %set-source  do-set-source
      %set-recipient  do-set-recipient
      %set-timelock  do-set-timelock
      %set-gift  do-set-gift
      %set-parent-hash  do-set-parent-hash
      %set-parent-hash-from-name  do-set-parent-hash-from-name
      %print-status  do-print-status
    ==
    ::
    ++  do-new
      ?>  ?=([%new *] cause)
      =/  sed=preseed:wt
        %*  .  *preseed:wt
          name  name.cause
        ==
      (write-seed sed)
    ::
    ++  do-set-name
      ?>  ?=([%set-name *] cause)
      =/  pre=(unit preseed:wt)
        (get-seed:p seed-name.cause)
      ?>  ?=(^ pre)
      =.  name.u.pre  new-name.cause
      (write-seed u.pre)
    ::
    ++  do-set-source
      ?>  ?=([%set-source *] cause)
      =/  pre=(unit preseed:wt)
        (get-seed:p seed-name.cause)
      ?>  ?=(^ pre)
      =/  sed=preseed:wt
        ?~  source.cause
          %=  u.pre
            output-source.p  ~
            output-source.q  %.y
          ==
        %=  u.pre
          output-source.p  (some (from-b58:source:transact u.source.cause))
          output-source.q  %.y
        ==
      (write-seed sed)
    ::
    ++  do-set-recipient
      ?>  ?=([%set-recipient *] cause)
      =/  pre=(unit preseed:wt)
        (get-seed:p seed-name.cause)
      ?>  ?=(^ pre)
      =/  recipient=lock:transact
        %+  m-of-n:new:lock:transact  m.recipient.cause
        %-  ~(gas z-in:zo *(z-set:zo schnorr-pubkey:transact))
        (turn pks.recipient.cause from-b58:schnorr-pubkey:transact)
      =/  sed=preseed:wt
        %=  u.pre
          recipient.p  recipient
          recipient.q  %.y
        ==
      (write-seed sed)
    ::
    ++  do-set-timelock
      ?>  ?=([%set-timelock *] cause)
      ::TODO
      !!
    ::
    ++  do-set-gift
      ?>  ?=([%set-gift *] cause)
      =/  pre=(unit preseed:wt)
        (get-seed:p seed-name.cause)
      ?>  ?=(^ pre)
      =/  sed=preseed:wt
        %=  u.pre
          gift.p  gift.cause
          gift.q  %.y
        ==
      (write-seed sed)
    ::
    ++  do-set-parent-hash
      ?>  ?=([%set-parent-hash *] cause)
      =/  pre=(unit preseed:wt)
        (get-seed:p seed-name.cause)
      ?>  ?=(^ pre)
      =/  sed=preseed:wt
        %=  u.pre
          parent-hash.q  %.y
          parent-hash.p  (from-b58:hash:transact parent-hash.cause)
        ==
      (write-seed sed)
    ::
    ++  do-set-parent-hash-from-name
      ?>  ?=([%set-parent-hash-from-name *] cause)
      =/  pre=(unit preseed:wt)
        (get-seed:p seed-name.cause)
      ?>  ?=(^ pre)
      =/  name=nname:transact  (from-b58:nname:transact name.cause)
      =/  not=nnote:transact  (get-note:v name)
      =/  sed=preseed:wt
        %=  u.pre
          parent-hash.p  (hash:nnote:transact not)
          parent-hash.q  %.y
        ==
      (write-seed sed)
    ::
    ++  do-print-status
      ?>  ?=([%print-status *] cause)
      =/  pre=(unit preseed:wt)
        (get-seed:p seed-name.cause)
      ?>  ?=(^ pre)
      =/  output-source-text=tape
        ?:  !output-source.q.u.pre
          "Unset (any output source is OK)"
        <output-source.p.u.pre>
      =/  recipient-text=tape
        ?:  !recipient.q.u.pre
          "Unset"
        <recipient.p.u.pre>
      =/  timelock-text=tape
        ?:  !timelock-intent.q.u.pre
          "Unset (no intent)"
        <timelock-intent.p.u.pre>
      =/  gift-text=tape
        ?:  !gift.q.u.pre
          "Gift: unset (gift must be nonzero)"
        ?:  =(0 gift.p.u.pre)
          """
          Gift: 0 (must be nonzero)
          """
        """
        Gift: {<gift.p.u.pre>}
        """
      =/  status-text=tape
        """
        ## Seed Status

        ### Output Source
        {output-source-text}

        ### Recipient
        {recipient-text}

        ### Timelock Intent
        {timelock-text}

        ### Gift
        {gift-text}
        """
      :_  state
      (print:utils (need (de:md (crip status-text))))
    ::
    ++  write-seed
      |=  sed=preseed:wt
      ^-  [(list effect:wt) state:wt]
      =.  active-seed.state  (some name.sed)
      =.  transaction-tree.state  (add-seed:p name.sed sed)
      =^  writes  state  write-transaction:d
      [writes state]
    --  ::+do-advanced-spend-seed
  ::
  ++  do-advanced-spend-input
    |=  cause=advanced-spend-input:wt
    ^-  [(list effect:wt) state:wt]
    |^
    ?-  -.cause
      %new  do-new
      %set-name  do-set-name
      %add-seed  do-add-seed
      %set-fee   do-set-fee
      %set-note-from-name  do-set-note-from-name
      %set-note-from-hash  do-set-note-from-hash
      %derive-note-from-seeds  do-derive-note-from-seeds
      %remove-seed  do-remove-seed
      %remove-seed-by-hash  do-remove-seed-by-hash
      %print-status  do-print-status
    ==
    ::
    ++  do-new
      ?>  ?=([%new *] cause)
      =/  inp=preinput:wt
        %*  .  *preinput:wt
          name  name.cause
        ==
      =.  active-input.state  (some name.cause)
      (write-input inp)
    ::
    ++  do-set-name
      ?>  ?=([%set-name *] cause)
      =/  pre=(unit preinput:wt)
        (get-input:p input-name.cause)
      ?>  ?=(^ pre)
      =.  name.u.pre  new-name.cause
      =.  active-input.state  (some new-name.cause)
      (write-input u.pre)
    ::
    ++  do-add-seed
      ?>  ?=([%add-seed *] cause)
      ::
      =/  pre=(unit preinput:wt)
        (get-input:p input-name.cause)
      ?>  ?=(^ pre)
      =/  sed=(unit preseed:wt)
        (get-seed:p seed-name.cause)
      ?>  ?=(^ sed)
      ?:  (~(has z-in:zo seeds.spend.p.u.pre) p.u.sed)
        :_  state
        =/  nodes=markdown:m
          :~  :-  %leaf
              :-  %paragraph
              :~  [%text (crip "seed already exists in .spend, doing nothing.")]
              ==
          ==
        (print:utils nodes)
      =/  inp=preinput:wt
        %=  u.pre
          seeds.spend.p  (~(put z-in:zo seeds.spend.p.u.pre) p.u.sed)
          seeds.spend.q  %.y
        ==
      (write-input inp)
    ::
    ++  do-set-fee
      ?>  ?=([%set-fee *] cause)
      =/  pre=(unit preinput:wt)
        (get-input:p input-name.cause)
      ?>  ?=(^ pre)
      =.  fee.spend.p.u.pre  fee.cause
      =.  fee.spend.q.u.pre  %.y
      (write-input u.pre)
    ::
    ++  do-set-note-from-name
      ?>  ?=([%set-note-from-name *] cause)
      ::
      =/  pre=(unit preinput:wt)
        (get-input:p input-name.cause)
      ?>  ?=(^ pre)
      =/  name=nname:transact  (from-b58:nname:transact name.cause)
      =/  not=nnote:transact  (get-note:v name)
      =/  inp=preinput:wt
        %=  u.pre
          note.p  not
          note.q  %.y
        ==
      (write-input inp)
    ::
    ++  do-set-note-from-hash
      ?>  ?=([%set-note-from-hash *] cause)
      ::
      =/  pre=(unit preinput:wt)
        (get-input:p input-name.cause)
      ?>  ?=(^ pre)
      =/  =hash:transact  (from-b58:hash:transact hash.cause)
      =/  note=nnote:transact  (get-note-from-hash:v hash)
      =/  inp=preinput:wt
        %=  u.pre
          note.p  note
          note.q  %.y
        ==
      (write-input inp)
    ::
    ++  do-derive-note-from-seeds
      ?>  ?=([%derive-note-from-seeds *] cause)
      ::
      =/  pre=(unit preinput:wt)
        (get-input:p input-name.cause)
      ?>  ?=(^ pre)
      =/  seeds-list=(list seed:transact)
        ~(tap z-in:zo seeds.spend.p.u.pre)
      ?~  seeds-list
        :_  state
        =/  nodes=markdown:m
          :~  :-  %leaf
              :-  %paragraph
              :~  [%text (crip "no seeds exist in .spend, so note cannot be set.")]
              ==
          ==
        (print:utils nodes)
      =/  =hash:transact  parent-hash.i.seeds-list
      =/  note=nnote:transact  (get-note-from-hash:v hash)
      =/  inp=preinput:wt
        %=  u.pre
          note.p  note
          note.q  %.y
        ==
      (write-input inp)
    ::
    ++  do-remove-seed
      ?>  ?=([%remove-seed *] cause)
      ::
      =/  pre=(unit preinput:wt)
        (get-input:p input-name.cause)
      ?>  ?=(^ pre)
      =.  active-input.state  (some input-name.cause)
      =/  sed=(unit preseed:wt)
        (get-seed:p seed-name.cause)
      ?>  ?=(^ sed)
      ?:  !(~(has z-in:zo seeds.spend.p.u.pre) p.u.sed)
        :_  state
        =/  nodes=markdown:m
          :~  :-  %leaf
              :-  %paragraph
              :~  [%text (crip "seed does not exist in .spend, doing nothing")]
              ==
          ==
        (print:utils nodes)
      =/  inp=preinput:wt
        %=  u.pre
          seeds.spend.p  (~(del z-in:zo seeds.spend.p.u.pre) p.u.sed)
          seeds.spend.q  !=(*seeds:transact seeds.spend.p.u.pre)
        ==
      (write-input inp)
    ::
    ++  do-remove-seed-by-hash
      ?>  ?=([%remove-seed-by-hash *] cause)
      :: find seed with hash
      =/  pre=(unit preinput:wt)
        (get-input:p input-name.cause)
      ?>  ?=(^ pre)
      =.  active-input.state  (some input-name.cause)
      =/  seed-hashes=(z-map:zo hash:transact seed:transact)
        %-  ~(gas z-by:zo *(z-map:zo hash:transact seed:transact))
        %+  turn  ~(tap z-in:zo seeds.spend.p.u.pre)
        |=  sed=seed:transact
        [(hash:seed:transact sed) sed]
      =/  has=hash:transact  (from-b58:hash:transact hash.cause)
      ?.  (~(has z-by:zo seed-hashes) has)
        :_  state
        =/  nodes=markdown:m
          :~  :-  %leaf
              :-  %paragraph
              :~  [%text (crip "seed does not exist in .spend, doing nothing")]
              ==
          ==
        (print:utils nodes)
      =/  remove-seed=seed:transact
        (~(got z-by:zo seed-hashes) has)
      ::
      =/  inp=preinput:wt
        %=  u.pre
          seeds.spend.p  (~(del z-in:zo seeds.spend.p.u.pre) remove-seed)
          seeds.spend.q  !=(*seeds:transact seeds.spend.p.u.pre)
        ==
      (write-input inp)
    ::
    ++  do-print-status
      ?>  ?=([%print-status *] cause)
      =/  pre=(unit preinput:wt)
        (get-input:p input-name.cause)
      ?>  ?=(^ pre)
      =|  status-nodes=markdown:m
      =.  status-nodes
        %+  snoc  status-nodes
        :-  %leaf
        :-  %paragraph
        ?:  !signature.spend.q.u.pre
          ::  TODO we removed the ability to sign in this control flow
          [%text (crip ".signature: unset")]~
        ::  check the signature
        ::
        ::  get a .parent-hash of a seed. they have to all be the same, so which
        ::  one doesn't matter; if they're not all the same validation will fail.
        =/  seeds-list=(list seed:transact)
          ~(tap z-in:zo seeds.spend.p.u.pre)
        ?~  seeds-list
          [%text (crip "no seeds exist, so signature cannot be checked")]~
        =/  parent-note-hash=hash:transact  parent-hash.i.seeds-list
        =/  parent-note-hash-b58=tape
          (trip (to-b58:hash:transact parent-note-hash))
        =/  parent-note-name=(unit nname:transact)
          (find-name-by-hash:v parent-note-hash)
        ?~  parent-note-name
          :~
            :-  %text
            %-  crip
            """
            note with hash {parent-note-hash-b58} present in .spend but
            has no matching .name in wallet
            """
            :-  %text
            ::  TODO better, more succint error message.
            '''
            this implies that it is not in the balance unless there is a hash collision.
            please report this as a bug if you are sure you have the $note, as this
            situation is very unlkely. the spend ought to still be valid in that case
            and you can broadcast it anyway.
            '''
          ==
        =/  parent-note-name-b58=tape
          =;  [first=@t last=@t]
            "<(trip first)> <(trip last)>"
          (to-b58:nname:transact u.parent-note-name)
        =/  parent-note=(unit nnote:transact)
          (~(get z-by:zo balance.state) u.parent-note-name)
        ?~  parent-note
          :~
            :-  %text
            %-  crip
            """
            note with name {parent-note-name-b58} and hash {parent-note-hash-b58}
            present in .spend but not in balance
            """
          ==
        ?:  (verify:spend:transact spend.p.u.pre u.parent-note)
          [%text (crip "signature(s) on spend are valid.")]~
        ::  missing or invalid sigs
        =/  have-sigs
          %+  turn
            %~  tap  z-in:zo
            ^-  (z-set:zo schnorr-pubkey:transact)
            %~  key  z-by:zo  (need signature.spend.p.u.pre)
          |=  pk=schnorr-pubkey:transact
          [%text (to-b58:schnorr-pubkey:transact pk)]
        ?~  have-sigs
          [%text 'no signatures found!']~
        =/  lock-b58=[m=@ pks=(list @t)]
          (to-b58:lock:transact recipient.i.seeds-list)
        =/  need-sigs
          (turn pks.lock-b58 (lead %text))
        ?~  need-sigs
          [%text 'no recipients found!']~
        ;:  welp
          :~  [%text (crip "signature on spend did not validate.")]
              [%text (crip "signatures on spend:")]
          ==
          ::TODO check if any particular signature did not validate
          have-sigs
          :~  [%text (crip ".lock on parent note:")]
              [%text (crip "number of sigs required: {(scow %ud m.lock-b58)}")]
              [%text (crip "pubkeys of possible signers:")]
          ==
          need-sigs
        ==
      ::TODO  check individual seeds? this would require some refactoring and
      ::the happy path does not involve adding unfinished seeds to an input.
      :_  state
      (print:utils status-nodes)
    ::
    ++  write-input
      |=  inp=preinput:wt
      ^-  [(list effect:wt) state:wt]
      =.  active-input.state  (some name.inp)
      =.  transaction-tree.state  (add-input:p name.inp inp)
      =^  writes  state  write-transaction:d
      [writes state]
    --  ::+do-advanced-spend-input
  ::
  ++  do-advanced-spend-transaction
    |=  cause=advanced-spend-transaction:wt
    ^-  [(list effect:wt) state:wt]
    |^
    =?  active-transaction.state  ?=(~ active-transaction.state)  `*transaction-name:wt
    ?-  -.cause
      %new  do-new
      %set-name  do-set-name
      %add-input  do-add-input
      %remove-input  do-remove-input
      %remove-input-by-name  do-remove-input-by-name
      %print-status  do-print-status
    ==
    ::
    ++  do-new
      ?>  ?=([%new *] cause)
      =.  active-transaction.state  (some name.cause)
      =/  dat=transaction:wt
        %*  .  *transaction:wt
          name  name.cause
        ==
      (write-transaction dat)
    ::
    ++  do-set-name
      ?>  ?=([%set-name *] cause)
      =/  pre=(unit transaction:wt)
        (get-transaction:p transaction-name.cause)
      ?>  ?=(^ pre)
      =.  active-transaction.state  (some new-name.cause)
      =.  name.u.pre  new-name.cause
      (write-transaction u.pre)
    ::
    ++  do-add-input
      ?>  ?=([%add-input *] cause)
      =/  pre=(unit transaction:wt)
        (get-transaction:p transaction-name.cause)
      ?>  ?=(^ pre)
      =.  active-transaction.state  (some transaction-name.cause)
      =/  inp=(unit preinput:wt)
        (get-input:p input-name.cause)
      ?>  ?=(^ inp)
      ?:  (~(has z-by:zo p.u.pre) name.note.p.u.inp)
        :_  state
        %-  print:utils
        ^-  markdown:m
        :_  ~  :-  %leaf
        :-  %paragraph
        :_  ~  :-  %text
        %-  crip
        """
        transaction already has input with note name
        {<(to-b58:nname:transact name.note.p.u.inp)>}, doing nothing.
        """
      =.  p.u.pre
        (~(put z-by:zo p.u.pre) [name.note.p.u.inp p.u.inp])
      (write-transaction u.pre)
    ::
    ++  do-remove-input
      ?>  ?=([%remove-input *] cause)
      =/  pre=(unit transaction:wt)
        (get-transaction:p transaction-name.cause)
      ?>  ?=(^ pre)
      =.  active-transaction.state  (some transaction-name.cause)
      =/  inp=(unit preinput:wt)
        (get-input:p input-name.cause)
      ?>  ?=(^ inp)
      ?.  (~(has z-by:zo p.u.pre) name.note.p.u.inp)
        :_  state
        %-  print:utils
        :_  ~  :-  %leaf
        :-  %paragraph
        :_  ~  :-  %text
        %-  crip
        """
        transaction does not have input with note name
        {<(to-b58:nname:transact name.note.p.u.inp)>}, doing nothing.
        """
      ?.  =(u.inp (~(got z-by:zo p.u.pre) name.note.p.u.inp))
        :_  state
        %-  print:utils
        :_  ~  :-  %leaf
        :-  %paragraph
        :_  ~  :-  %text
        %-  crip
        """
        transaction has input with note name
        {<(to-b58:nname:transact name.note.p.u.inp)>}, but it is
        a different input. to remove this input, use %remove-input-by-name
        instead.
        """
      =.  p.u.pre
        (~(del z-by:zo p.u.pre) name.note.p.u.inp)
      (write-transaction u.pre)
    ::
    ++  do-remove-input-by-name
      ?>  ?=([%remove-input-by-name *] cause)
      =/  pre=(unit transaction:wt)
        (get-transaction:p transaction-name.cause)
      =.  active-transaction.state  (some transaction-name.cause)
      ?>  ?=(^ pre)
      =/  name=nname:transact  (from-b58:nname:transact name.cause)
      ?.  (~(has z-by:zo p.u.pre) name)
        :_  state
        %-  print:utils
        :_  ~  :-  %leaf
        :-  %paragraph
        :_  ~  :-  %text
        %-  crip
        """
        transaction does not have input with note name {(trip first.name.cause)}
        {(trip last.name.cause)}, doing nothing.
        """
      =.  p.u.pre  (~(del z-by:zo p.u.pre) name)
      (write-transaction u.pre)
    ::
    ++  do-print-status
      ?>  ?=([%print-status *] cause)
      =/  pre=(unit transaction:wt)
        (get-transaction:p transaction-name.cause)
      =.  active-transaction.state  (some transaction-name.cause)
      ?>  ?=(^ pre)
      =/  inputs=(list [name=nname:transact =input:transact])
        ~(tap z-by:zo p.u.pre)
      =/  input-texts=(list tape)
        %+  turn  inputs
        |=  [name=nname:transact =input:transact]
        =/  signature-text=tape  ?~(signature.spend.input "unset" "set")
        =/  name-text=tape  <(to-b58:nname:transact name)>
        =/  note-text=tape  <(to-b58:hash:transact (hash:nnote:transact note.input))>
        =/  seeds-text=tape
          %-  zing
          %+  turn  ~(tap z-in:zo seeds.spend.input)
          |=  =seed:transact
          """
          - recipient: {<(to-b58:lock:transact recipient.seed)>}
          - gift: {<gift.seed>}
          - parent hash: {<(to-b58:hash:transact parent-hash.seed)>}
          """
        """
        #### Input {name-text}:

        - Note hash: {note-text}
        - Fee: {<fee.spend.input>}
        - Signature: {signature-text}

        ##### Seeds

        {seeds-text}

        """
      =/  status-text=tape
        """
        ## Transaction Status

        Name: {(trip name.u.pre)}
        Number of inputs: {<(lent inputs)>}

        ### Inputs

        {(zing input-texts)}
        """
      :_  state
      (print:utils (need (de:md (crip status-text))))
    ::
    ++  write-transaction
      |=  dat=transaction:wt
      ^-  [(list effect:wt) state:wt]
      =.  active-transaction.state  (some name.dat)
      write-transaction:d
    --  ::+do-advanced-spend-transaction
  --  ::+poke
--
