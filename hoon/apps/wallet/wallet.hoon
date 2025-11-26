::  /ker/wallet/wallet: nockchain wallet
/=  bip39  /common/bip39
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
/=  s10  /apps/wallet/lib/s10
=>
=|  bug=_&
|%
::
::  re-exporting names from wallet types while passing the bug flag
++  utils  ~(. wutils bug)
++  debug  debug:utils
++  warn  warn:utils
++  moat  (keep state:wt)
--
::
%-  (moat &)
^-  fort:moat
|_  =state:wt
+*  v  ~(. vault:utils state)
    d  ~(. draw:utils state)
    p  ~(. plan:utils transaction-tree.state)
::
++  load
  |=  old=versioned-state:wt
  ^-  state:wt
  |^
  |-
  ?:  ?=(%4 -.old)
    old
  ~>  %slog.[0 'load: State upgrade required']
  ?-  -.old
    %0  $(old state-0-1)
    %1  $(old state-1-2)
    %2  $(old state-2-3)
    %3  $(old state-3-4)
  ==
  ::
  ++  state-0-1
    ^-  state-1:wt
    ?>  ?=(%0 -.old)
    ~>  %slog.[0 'upgrade version 0 to 1']
    :*  %1
        balance.old
        active-master.old
        keys.old
        last-block.old
        peek-requests.old
        active-transaction.old
        active-input.old
        active-seed.old
        transaction-tree.old
        pending-commands.old
    ==
  ::
  ++  state-1-2
    ^-  state-2:wt
    ?>  ?=(%1 -.old)
    ~>  %slog.[0 'upgrade version 1 to 2']
    :*  %2
        balance=*balance-v2:wt
        active-master.old
        keys.old
        last-block.old
        peek-requests.old
        active-transaction.old
        active-input.old
        active-seed.old
        transaction-tree.old
        pending-commands.old
    ==
  ::
  ++  state-2-3
    ^-  state-3:wt
    ?>  ?=(%2 -.old)
    ~>  %slog.[0 'upgrade version 2 to 3']
    =/  new-keys=keys-v3:wt
      %+  roll  ~(tap of keys.old)
      |=  [[=trek m=meta-v2:wt] new=keys-v3:wt]
      %-  ~(put of new)
      :-  trek
      ^-  meta-v3:wt
      ?.  ?=(%coil -.m)
        m
      [%coil [%0 +.m]]
    =/  new-master=active-v3:wt
      ?~  active-master.old  ~
      `[%0 +.u.active-master.old]
    :*  %3
        balance.old
        new-master
        new-keys
        last-block.old
        peek-requests.old
        active-transaction.old
        active-input.old
        active-seed.old
        transaction-tree.old
        pending-commands.old
    ==
  ::
  ++  state-3-4
    ^-  state:wt
    ?>  ?=(%3 -.old)
    ~>  %slog.[0 'upgrade version 3 to 4']
    :*  %4
        balance.old
        :: delete active master
        active-master.old
        keys.old
    ==
  --
::
++  peek
  |=  arg=path
  ^-  (unit (unit *))
  %-  (debug "peek: {<arg>}")
  =/  =(pole)  arg
  ?+  pole  ~
    ::
      [%balance ~]
    ``balance.state
    ::
      [%state ~]
    ``state
    ::
    ::  returns a list of tracked first names
      [%tracked-names ~]
    :+  ~
      ~
    =/  signing-names=(list @t)
      %+  roll
        ~(coils get:v %pub)
      |=  [=coil:wt names=(list @t)]
      ::  exclude names for v0 keys because those are handled through tracked pubkeys
      ?:  ?=(%0 -.coil)
        names
      :+  (to-b58:hash:transact (simple-first-name:coil:wt coil))
        (to-b58:hash:transact (coinbase-first-name:coil:wt coil))
      names
    %~  tap  in
    %-  silt
    (weld signing-names watch-first-names:get:v)
    ::
    ::  returns a list of pubkeys
      [%tracked-pubkeys ~]
    :+  ~
      ~
    =;  signing-keys=(list @t)
      %+  weld  signing-keys
      %+  murn  watch-addrs:get:v
      |=  addr=@t
      ?:  (lth (met 3 addr) 132)
        ~
      `addr
    %+  murn
      ~(coils get:v %pub)
    |=  =coil:wt
    ?:  ?=(%1 -.coil)
      ~
    `~(address to-b58:coil:wt coil)
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
      [%poke ?(%one-punch %sys %wallet %file) ver=@ *]
    ?+    -.cause  ~|("unsupported cause: {<-.cause>}" !!)
        %show                  (show:utils state path.cause)
        %keygen                (do-keygen cause)
        %derive-child          (do-derive-child cause)
        %list-notes            (do-list-notes cause)
        %list-notes-by-address  (do-list-notes-by-address cause)
        %list-notes-by-address-csv  (do-list-notes-by-address-csv cause)
        %create-tx             (do-create-tx cause)
        %sign-multisig-tx      (do-sign-multisig-tx cause)
        %update-balance-grpc   (do-update-balance-grpc cause)
        %sign-message          (do-sign-message cause)
        %verify-message        (do-verify-message cause)
        %sign-hash             (do-sign-hash cause)
        %verify-hash           (do-verify-hash cause)
        %import-keys           (do-import-keys cause)
        %import-extended       (do-import-extended cause)
        %watch-address         (do-watch-address cause)
        ::%watch-first-name      (do-watch-first-name cause)
        %watch-address-multisig  (do-watch-address-multisig cause)
        %export-keys           (do-export-keys cause)
        %export-master-pubkey  (do-export-master-pubkey cause)
        %import-master-pubkey  (do-import-master-pubkey cause)
        %import-seed-phrase    (do-import-seed-phrase cause)
        %send-tx               (do-send-tx cause)
        %show-tx                (do-show-tx cause)
        %list-active-addresses  (do-list-active-addresses cause)
        %show-key-tree          (do-show-key-tree cause)
        %show-seed-phrase       (do-show-seed-phrase cause)
        %show-master-zpub    (do-show-master-zpub cause)
        %show-master-zprv  (do-show-master-zprv cause)
        %list-master-addresses  (do-list-master-addresses cause)
        %set-active-master-address  (do-set-active-master-address cause)
    ::
        %file
      ?-    +<.cause
          %write
        [[%exit 0]~ state]
      ::
          %batch-write
        [[%exit 0]~ state]
      ::
          %read
        ?^  contents.cause
          ~&  "success"
          ::  file read response with contents
          [[%exit 0]~ state]
        ::  file read error
        [[%exit 1]~ state]
      ==
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
    %-  (debug "last balance size: {<(lent ~(tap z-by:zo notes.balance.state))>}")
    =/  softed=(unit (unit (unit balance:wt)))
      %-  (soft (unit (unit balance:wt)))
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
    =/  update=balance:wt  u.u.balance-result
    =?  balance.state  (gte height.update height.balance.state)
      ?:  ?&  =(height.update height.balance.state)
              =(block-id.balance.state block-id.update)
          ==
          ~>  %slog.[0 'Received balance update from same block, adding update to current balance']
          ::  If it is duplicate balance update for the same address, union should have no impact
          update(notes (~(uni z-by:zo notes.balance.state) notes.update))
      ~>  %slog.[0 'Received balance update for new heaviest block, overwriting balance with update']
      update
    %-  (debug "balance state updated!")
    [~ state]
  ::
  ++  do-import-keys
    |=  =cause:wt
    ?>  ?=(%import-keys -.cause)
    =/  new-keys=_keys.state
      %+  roll  keys.cause
      |=  [[=trek raw-meta=*] acc=_keys.state]
      =/  converted-meta=meta:wt
        ;;  meta:wt
        ?.  ?=(%coil -.raw-meta)
          ::  non-coil meta (label, seed, watch-key) - unchanged
          raw-meta
        ::  it's a coil, check if it's already versioned
        ::  meta-v3 coil: [%coil [%0|%1 coil-data]]
        ::  meta-{v0,v1,v2} coil: [%coil coil-data]
        ::  we can check if +.raw-meta is itself a cell with %0 or %1 head
        =/  inner  +.raw-meta
        ?:  ?&  ?=(^ inner)
                ?|  ?=(%0 -.inner)
                    ?=(%1 -.inner)
                ==
            ==
          ::  already meta-v3 format
          raw-meta
        ::  old meta-v0 format, convert to meta-v3
        ::  inner is coil-data [=key =cc], wrap as [%0 coil-data]
        [%coil [%0 inner]]
      (~(put of acc) trek converted-meta)
    =/  master-key=coil:wt
      %-  head
      %+  murn  ~(tap of new-keys)
      |=  [t=trek m=meta:wt]
      ^-  (unit coil:wt)
      ?:  ?&
            ?=(%coil -.m)
            =((slag 2 t) /pub/m)
          ==
        `p.m
      ~
    =/  key-list=(list tape)
      %+  murn  ~(tap of new-keys)
      |=  [t=trek m=meta:wt]
      ^-  (unit tape)
      ?.  ?&  ?=(%coil -.m)
              (gte (lent t) 4)
          ==
        ~
      =/  =coil:wt  p.m
      =/  version=@  -.coil
      =/  parent=@t  (slav %t (snag 1 (pout t)))
      =/  key-or-address-b58=tape
        ?:  ?=(%prv -.key.coil)
          """
          - Type: Private
          - Private Key: {(trip ~(key to-b58:coil:wt coil))}
          """
        """
        - Type: Public
        - Address: {(trip ~(address to-b58:coil:wt coil))}
        """
      =/  info=tape
        =+  index-display=(snag 3 (pout t))
        ?:  =('m' index-display)
          "- Derivation Info: Master Key"
        =/  index=@  (slav %ud index-display)
        =?  index-display  (gte index (bex 31))
          =+  hardened-index=(mod index (bex 31))
          (cat 3 (scot %ud hardened-index) ' (hardened)')
        """
        - Derivation Info: Child Key
          - Index: {(trip index-display)}
          - Parent Address: {(trip parent)}
        """
      %-  some
      """
      {key-or-address-b58}
      {info}
      - Version: {<version>}
      ---

      """
    =.  active-master.state  `master-key
    =.  keys.state  new-keys
    :_  state
    :~  :-  %markdown
        %-  crip
        """
        ## Imported Keys

        {(zing key-list)}
        """
        [%exit 0]
    ==
  ::
  ++  do-watch-address
    |=  =cause:wt
    ?>  ?=(%watch-address -.cause)
    :_  state(keys (watch-addr:put:v address.cause))
    :~  :-  %markdown
        %-  crip
        """
        ## Imported watch-only address

        - Imported address: {<address.cause>}
        """
        [%exit 0]
    ==
  ::
  ::++  do-watch-first-name
  ::  |=  =cause:wt
  ::  ?>  ?=(%watch-first-name -.cause)
  ::  =/  first-name=hash:transact  (from-b58:hash:transact first-name.cause)
  ::  =/  maybe-lock=(unit lock:transact)  lock.cause
  ::  =/  first-name-b58=@t  (to-b58:hash:transact first-name)
  ::  :_  state(keys (watch-first-name:put:v first-name maybe-lock))
  ::  :~  :-  %markdown
  ::      %-  crip
  ::      """
  ::      ## Imported watch-only first name

  ::      - First name: {(trip first-name-b58)}
  ::      - Lock info: {?~(maybe-lock "N/A" (trip (lock:v1:display:utils u.maybe-lock)))}

  ::      """
  ::      [%exit 0]
  ::  ==
  ::
  ++  do-watch-address-multisig
    |=  =cause:wt
    ?>  ?=(%watch-address-multisig -.cause)
    =/  participant-count=@ud  (lent participants.cause)
    ?:  =(0 participant-count)
      :_  state
      :~  :-  %markdown
          %-  crip
          """
          No pubkeys were provided for the multisig watch request.
          """
          [%exit 0]
      ==
    ?:  ?|  (lte m.cause 0)
            (gth m.cause participant-count)
        ==
      :_  state
      :~  :-  %markdown
          %-  crip
          """
          Invalid m value: {<m.cause>}. Must be > 0 and <= number of participant addresses ({<participant-count>}).
          """
          [%exit 0]
      ==
    =/  address-hash-set=(z-set:zo hash:transact)
      %+  roll  participants.cause
      |=  [b58=@t acc=(z-set:zo hash:transact)]
      (~(put z-in:zo acc) (from-b58:hash:transact b58))
    =/  multisig=lock:transact
      [%pkh m=m.cause address-hash-set]~
    =/  first-name=hash:transact
      (first:nname:transact (hash:lock:transact multisig))
    =/  first-name-b58=@t  (to-b58:hash:transact first-name)
    =.  keys.state  (watch-first-name:put:v first-name `multisig)
    =/  keys=tape
      %-  zing
      %+  join  "\0a    "
      %+  turn
        participants.cause
      |=  k=@t
      """
          - {(trip k)}
      """
    =/  summary=@t
      %-  crip
      """
      ## Imported multisig watch

      - First name: {(trip first-name-b58)}
      - Required Signatures: {<m.cause>}
      - Signers:
          {keys}

      """
    :_  state
    :~  [%markdown summary]
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
    =/  protocol-version=@  protocol-version:core
    =/  [imported-coil=coil:wt public-coil=coil:wt]
      ?+    protocol-version  ~|('unsupported protocol version' !!)
           %0
        :-  [%0 coil-key chain-code:core]
        [%0 [%pub public-key] chain-code]:core
      ::
           %1
         :-  [%1 coil-key chain-code:core]
         [%1 [%pub public-key] chain-code]:core
      ==
    =/  key-label=@t
      ?:  is-private
        (crip "imported-private-{<(end [3 4] public-key:core)>}")
      (crip "imported-public-{<(end [3 4] public-key:core)>}")
    ::  if this is a master key (no parent), set as master
    ?:  =(0 dep:core)
      =.  active-master.state  (some public-coil)
      =.  keys.state  (key:put:v imported-coil ~ `key-label)
      =.  keys.state  (key:put:v public-coil ~ `key-label)
      =/  extended-type=tape  ?:(is-private "private" "public")
      :_  state
      :~  :-  %markdown
          %-  crip
          """
          ## Imported {extended-type} key

          - Imported Extended Key: {(trip extended-key.cause)}
          - Assigned Label: {(trip key-label)}
          - Set as active master key
          """
          [%exit 0]
      ==
    ::  otherwise, import as derived key
    ::  first validate that this key is actually a child of the current master
    ?~  active-master.state
      :_  state
      :~  :-  %markdown
          %-  crip
          """
          ## Import failed

          Cannot import derived key: no active master key set
          """
          [%exit 1]
      ==
    =/  master-pubkey-coil=coil:wt  (public:active:wt active-master.state)
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

        - Imported Extended Key: {(trip extended-key.cause)}
        - Assigned Label: {(trip key-label)}
        - Index: {<ind:core>}
        - Verified as child of active master key
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
    ?~  active-master.state
      %-  (warn "wallet: no active keys available for export")
      [[%exit 0]~ state]
    =/  master-coil=coil:wt  ~(master get:v %pub)
    ?.  ?=(%pub -.key.master-coil)
      %-  (warn "wallet: fatal: master pubkey malformed")
      [[%exit 0]~ state]
    =/  dat-jam=@  (jam master-coil)
    =/  addr-b58=@t  ~(address to-b58:coil:wt master-coil)
    =/  extended-key=@t
      =/  core  (from-public:s10 ~(keyc get:coil:wt master-coil))
      extended-public-key:core
    =/  file-path=@t  'master-pubkey.export'
    =/  version=@  -.master-coil
    :_  state
    :~  :-  %markdown
        %-  crip
        """
        ## Exported Master Public Key

        - Extended Key: {(trip extended-key)}
        - Address: {(trip addr-b58)}
        - Version: {<version>}
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
    =/  raw-coil=*  coil.cause
    =/  master-pubkey-coil=coil:wt
      ;;  coil:wt
      ?:  ?&  ?=(^ raw-coil)
              ?|  ?=(%0 -.raw-coil)
                  ?=(%1 -.raw-coil)
              ==
          ==
        ::  already coil-v3 format
        raw-coil
      ::  old coil-v0 format, convert to coil-v3
      ::  raw-coil is coil-data [=key =cc], wrap as [%0 coil-data]
      [%0 +.raw-coil]
    =.  active-master.state  (some master-pubkey-coil)
    =/  label  `(crip "master-public-{<(end [3 4] p.key.master-pubkey-coil)>}")
    =.  keys.state  (key:put:v master-pubkey-coil ~ label)
    =/  addr-b58=@t  ~(address to-b58:coil:wt master-pubkey-coil)
    =/  version=@  -.master-pubkey-coil
    :_  state
    :~  :-  %markdown
        %-  crip
        """
        ## Imported Master Public Key

        - Address: {(trip addr-b58)}
        - Version: {<version>}
        """
        [%exit 0]
    ==
  ::
  ++  do-import-seed-phrase
    |=  =cause:wt
    ?>  ?=(%import-seed-phrase -.cause)
    ::  We do not need to reverse the endian-ness of the seed phrase
    ::  because the bip39 code expects a tape.
    ::  TODO: move this conversion into s10
    =/  seed=byts  [64 (to-seed:bip39 (trip seed-phrase.cause) "")]
    =/  cor  (from-seed:s10 seed version.cause)
    =/  [master-pubkey-coil=coil:wt master-privkey-coil=coil:wt]
      ?-    version.cause
          %0
        :-  [%0 [%pub public-key] chain-code]:cor
        [%0 [%prv private-key] chain-code]:cor
      ::
          %1
        :-  [%1 [%pub public-key] chain-code]:cor
        [%1 [%prv private-key] chain-code]:cor
      ==
    =.  active-master.state  (some master-pubkey-coil)
    =/  public-label  `(crip "master-public-{<(end [3 4] public-key:cor)>}")
    =/  private-label  `(crip "master-private-{<(end [3 4] public-key:cor)>}")
    =.  keys.state  (key:put:v master-privkey-coil ~ private-label)
    =.  keys.state  (key:put:v master-pubkey-coil ~ public-label)
    =.  keys.state  (seed:put:v seed-phrase.cause)
    %-  (debug "active-master.state: {<active-master.state>}")
    =/  version=@  version.cause
    =/  address=@t  ~(address to-b58:coil:wt master-pubkey-coil)
    :_  state
    :~  :-  %markdown
        %-  crip
        """
        ## Master Key (Imported)

        - Address: {(trip address)}
        - Version: {<version>}
        """
        [%exit 0]
    ==
  ::
  ++  do-send-tx
    |=  =cause:wt
    ?>  ?=(%send-tx -.cause)
    %-  (debug "send-tx: creating raw-tx")
    =/  =transaction:wt  dat.cause
    =/  transaction-name=@t  name.transaction
    =/  =spends:transact  spends.transaction
    =/  display=transaction-display:wt  display.transaction
    =/  =witness-data:wt  witness-data.transaction
    =/  signed-spends=spends:v1:transact
      (apply:witness-data:wt witness-data spends)
    =/  raw=raw-tx:v1:transact  (new:raw-tx:v1:transact signed-spends)
    =/  =tx:v1:transact  (new:tx:v1:transact raw height.balance.state)
    =/  fees=@  (roll-fees:spends:v1:transact signed-spends)
    =/  tx-display=@t
      %:  transaction:v1:display:utils
          transaction-name
          outputs.tx
          fees
          display.transaction
          get-note:v
          `witness-data
      ==
    =+  data=data:*blockchain-constants:transact
    =/  valid=(reason:dumb ~)
      %-  validate-with-context:spends:transact
      [notes.balance.state signed-spends height.balance.state max-size.data]
    ?-    -.valid
        %.y
      =/  nock-cause=$>(%fact cause:dumb)
        [%fact %0 %heard-tx raw]
      %-  (debug "send-tx: made raw-tx, sending poke request over grpc")
      ::  we currently do not need to assign pids. shim is here in case
      =/  pid  *@
      =/  msg=@t
        %-  crip
        """
        ## Sent Tx
        - Validation for TX {(trip (to-b58:hash:transact id.raw))} passed. TX has been submitted to node.
        ---

        """
      :_  state
      :~  [%markdown msg]
          [%grpc %poke pid nock-cause]
          [%nockchain-grpc %send-tx raw]
          [%exit 0]
      ==
    ::
        %.n
      =/  msg=@t
          %-  crip
          """
          # TX Validation Failed

          Failed to validate the correctness of transaction {(trip transaction-name)}.
          Reason: {(trip p.valid)}

          {(trip tx-display)}
          ---

          """
      %-  (debug "{(trip msg)}")
      :_  state
      :~
        [%markdown msg]
        [%exit 1]
      ==
    ==
  ::
  ++  do-show-tx
    |=  =cause:wt
    ?>  ?=(%show-tx -.cause)
    %-  (debug "show-tx: displaying transaction")
    =/  =transaction:wt  dat.cause
    =/  transaction-name=@t  name.transaction
    =/  =spends:transact  spends.transaction
    =/  display=transaction-display:wt  display.transaction
    =/  fees=@  (roll-fees:spends:v1:transact spends)
    =/  =raw-tx:v1:transact  (new:raw-tx:v1:transact spends)
    =/  =tx:v1:transact  (new:tx:v1:transact raw-tx height.balance.state)
    =/  markdown-text=@t
      %:  transaction:v1:display:utils
          transaction-name
          outputs.tx
          fees
          display.transaction
          get-note:v
          `witness-data.transaction
      ==
    :_  state
    :~
      [%markdown markdown-text]
      [%exit 0]
    ==
  ::
  ++  do-list-active-addresses
    |=  =cause:wt
    ?>  ?=(%list-active-addresses -.cause)
    =/  base58-sign-keys=(list tape)
      %+  turn  ~(coils get:v %pub)
      |=  =coil:wt
      =/  version=@  -.coil
      =/  address=@t  ~(address to-b58:coil:wt coil)
      """
      - Address: {(trip address)}
      - Version: {<version>}
      ---

      """
    =/  base58-watch-addrs=(list tape)
      %+  turn  watch-addrs:get:v
      |=  key-b58=@t
      """
      - {<key-b58>}
      ---

      """
    =/  base58-watch-locks=(list tape)
      %+  turn  watch-locks:get:v
      |=  [name-b58=@t lock-b58=@t]
      """
      - First Name: {(trip name-b58)}
      {(trip lock-b58)}
      ---

      """
    :_  state
    :~  :-  %markdown
        %-  crip
        """
        ## Addresses -- Signing

        {?~(base58-sign-keys "No pubkeys found" (zing base58-sign-keys))}

        ## Addresses -- Watch Only

        {?~(base58-watch-addrs "No pubkeys found" (zing base58-watch-addrs))}

        ## Lock First Names -- Watch Only

        {?~(base58-watch-locks "No watch only locks found" (zing base58-watch-locks))}

        """
        [%exit 0]
    ==
  ::
  ++  do-show-key-tree
    |=  =cause:wt
    ?>  ?=(%show-key-tree -.cause)
    |^
    =/  include-values=?  include-values.cause
    =/  entries=(list [trek meta:wt])  ~(tap of keys.state)
    =/  root=trek  (pave /keys)
    =/  init=[seen=(set trek) ordered=(list trek)]
      [(~(put in *(set trek)) root) ~[root]]
    =/  final=[seen=(set trek) ordered=(list trek)]
      %+  roll  entries
      |=  [[path=trek meta=meta:wt] acc=[seen=(set trek) ordered=(list trek)]]
      ^-  [seen=(set trek) ordered=(list trek)]
      (add-prefixes (prefixes path) acc)
    =/  paths=(list trek)
      (flop ordered.final)
    =/  lines=(list tape)
      %+  turn  paths
      |=  path=trek
      (render-line path include-values)
    =/  tree-text=tape
      %-  zing
      %+  turn  lines
      |=  line=tape
      (weld line "\0a")
    :_  state
    :~
      :-  %markdown
      %-  crip
      """
      ## Key Tree

      ```
      {tree-text}
      ```

      """
      [%exit 0]
    ==
    ::
    ++  prefixes
      |=  path=trek
      ^-  (list trek)
      =|  acc=(list trek)
      =|  cur=trek
      |-  ^-  (list trek)
      ?~  path
        (flop acc)
      =.  cur  (snoc cur i.path)
      =.  acc  [cur acc]
      $(path t.path)
    ::
    ++  add-prefixes
      |=  [paths=(list trek) acc=[seen=(set trek) ordered=(list trek)]]
      ^-  [seen=(set trek) ordered=(list trek)]
      =/  seen=(set trek)  seen.acc
      =/  ordered=(list trek)  ordered.acc
      |-
      ?~  paths
        [seen ordered]
      =/  path=trek  i.paths
      ?:  (~(has in seen) path)
        $(paths t.paths)
      =.  seen  (~(put in seen) path)
      =.  ordered  [path ordered]
      $(paths t.paths)
    ::
    ++  render-line
      |=  [path=trek include-values=?]
      ^-  tape
      =/  len=@ud  (lent path)
      =/  depth=@ud  ?:(=(0 len) 0 (dec len))
      =/  indent=tape  (indent depth)
      =/  path-text=tape  (spud (pout path))
      =/  base=tape  (weld indent path-text)
      =/  value=(unit meta:wt)  (~(get of keys.state) path)
      ?~  value
        base
      ?:  include-values
        (weld base (weld "\0a {indent}  -> " (summarize-meta u.value indent)))
      base
    ::
    ++  indent
      |=  depth=@ud
      ^-  tape
      =|  i=@ud
      =|  acc=tape
      |-  ^-  tape
      ?:  =(i depth)
        acc
      $(i +(i), acc (weld acc "  "))
    ::
    ++  summarize-meta
      |=  [=meta:wt indent=tape]
      ^-  tape
      ?-  -.meta
          %coil  (summarize-coil p.meta)
          %label  "label {<p.meta>}"
          %seed  "seed {<p.meta>}"
          %watch-key  "watch-key {<p.meta>}"
          %first-name  (summarize-first-name indent +.meta)
      ==
    ::
    ++  summarize-first-name
      |=  [indent=tape first-name=hash:transact lock=(unit lock:transact)]
      ^-  tape
      """
      first-name {(trip (to-b58:hash:transact first-name))}
      """
    ::
    ++  summarize-coil
      |=  =coil:wt
      ^-  tape
      =/  version-text=tape  (scow %ud -.coil)
      =/  key-type=@tas
        =<  -
        ~(key get:coil:wt coil)
      =/  type-text=tape  (scow %tas key-type)
      "coil {version-text} {type-text}"
    ::
    ++  pout
      |=  pit=pith
      ^-  path
      %+  turn  pit
      |=  iot=iota
      ^-  @ta
      ?-  iot
        @tas  iot
        [%ub @]  (scot %ub +.iot)
        [%uc @]  (scot %uc +.iot)
        [%ud @]  (scot %ud +.iot)
        [%ui @]  (scot %ui +.iot)
        [%ux @]  (scot %ux +.iot)
        [%uv @]  (scot %uv +.iot)
        [%uw @]  (scot %uw +.iot)
        [%sb @]  (scot %sb +.iot)
        [%sc @]  (scot %sc +.iot)
        [%sd @]  (scot %sd +.iot)
        [%si @]  (scot %si +.iot)
        [%sx @]  (scot %sx +.iot)
        [%sv @]  (scot %sv +.iot)
        [%sw @]  (scot %sw +.iot)
        [%da @]  (scot %da +.iot)
        [%dr @]  (scot %dr +.iot)
        [%f ?]   (scot %f +.iot)
        [%n ~]   (scot %n ~)
        [%if @]  (scot %if +.iot)
        [%is @]  (scot %is +.iot)
        [%t @]   (scot %tas +.iot)
        [%ta @]  (scot %tas +.iot)
        [%p @]   (scot %p +.iot)
        [%q @]   (scot %q +.iot)
        [%rs @]  (scot %rs +.iot)
        [%rd @]  (scot %rd +.iot)
        [%rh @]  (scot %rh +.iot)
        [%rq @]  (scot %rq +.iot)
      ==
    --
  ::
  ++  do-show-seed-phrase
    |=  =cause:wt
    ?>  ?=(%show-seed-phrase -.cause)
    %-  (debug "show-seed-phrase")
    ?~  active-master.state
      :_  state
      :~  :-  %markdown
          %-  crip
          """
          Cannot show seed phrase without active master address set. Please import a master key / seed phrase or generate a new one.
          """
          [%exit 0]
      ==
    =/  =meta:wt  seed:get:v
    =/  version=@  -.u.active-master.state
    =/  seed-phrase=@t
      ?:  ?=(%seed -.meta)
        +.meta
      %-  crip
      "no seed-phrase found"
    :_  state
    :~  :-  %markdown
        %-  crip
        """
        ## Show Seed Phrase
        Store this seedphrase in a safe place. Keep note of the version
        - Seed Phrase: {<seed-phrase>}
        - Version: {<version>}
        """
        [%exit 0]
    ==
  ::
  ++  do-show-master-zpub
    |=  =cause:wt
    ?>  ?=(%show-master-zpub -.cause)
    %-  (debug "show-master-zpub")
    ?~  active-master.state
      :_  state
      :~  :-  %markdown
          %-  crip
          """
          Cannot show master pubkey without active master address set. Please import a master key / seed phrase or generate a new one.
          """
          [%exit 0]
      ==
    =/  =coil:wt  ~(master get:v %pub)
    =/  extended-key=@t  (extended-key:coil:wt coil)
    =/  version=@  -.coil
    =/  address=@t  ~(address to-b58:coil:wt coil)
    :_  state
    :~  :-  %markdown
        %-  crip
        """
        ## Show Master Extended Public Key

        - Extended Public Key: {(trip extended-key)} (save for import)
        - Corresponding Address: {(trip address)}
        - Version: {<version>}
        """
        [%exit 0]
    ==
  ::
  ++  do-show-master-zprv
    |=  =cause:wt
    ?>  ?=(%show-master-zprv -.cause)
    %-  (debug "show-master-zprv")
    ?~  active-master.state
      :_  state
      :~  :-  %markdown
          %-  crip
          """
          Cannot show master privkey without active master address set. Please import a master key / seed phrase or generate a new one.
          """
          [%exit 0]
      ==
    =/  [version=@ extended-key=@t]
      =/  =coil:wt  ~(master get:v %prv)
      [`@`-.coil (extended-key:coil:wt coil)]
    :_  state
    :~  :-  %markdown
        %-  crip
        """
        ## Master Extended Private Key (zprv)

        - Extended Private Key: {(trip extended-key)} (save for import)
        - Version: {<version>}
        """
        [%exit 0]
    ==
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
      - Height: {(trip (format-ui:common:display:utils height.balance.state))}
      - Block id: {(trip (to-b58:hash:transact block-id.balance.state))}

      """
      =-  ?:  =("" -)  "No notes found"  -
      %-  zing
      %+  turn  ~(val z-by:zo notes.balance.state)
      |=  note=nnote:transact
      ?^  -.note
        (trip (note:v0:display:utils note))
      (trip (note-from-balance:v1:display:utils note))
      ::
      [%exit 0]
    ==
  ::
  ++  do-list-notes-by-address
    |=  =cause:wt
    ?>  ?=(%list-notes-by-address -.cause)
    =/  [matching-notes=(list [name=nname:transact note=nnote:transact]) pkh=(unit hash:transact)]
      ::  v0 address case
      ?:  (gte (met 3 address.cause) 132)
        =/  target-pubkey=schnorr-pubkey:transact
          (from-b58:schnorr-pubkey:transact address.cause)
        =/  notes
        %+  skim  ~(tap z-by:zo notes.balance.state)
        |=  [name=nname:transact note=nnote:transact]
        ::  skip v1 notes
        ?@  -.note  %.n
        ::  this should cover all cases because we only
        ::  sync coinbase notes or non-coinbase notes with m=1 locks.
        (~(has z-in:zo pubkeys.sig.note) target-pubkey)
        [notes ~]
      ::  v1 address case
      =/  target-pkh=hash:transact
        (from-b58:hash:transact address.cause)
      =/  notes
      %+  skim  ~(tap z-by:zo notes.balance.state)
      |=  [name=nname:transact note=nnote:transact]
      ::  skip v0 notes
      ?^  -.note  %.n
      ::  look for coinbase notes with target-pkh
      ::  or notes with simple 1-of-1 lock containing
      =+  simple-fn=(simple:v1:first-name:transact target-pkh)
      =+  coinbase-fn=(coinbase:v1:first-name:transact target-pkh)
      ?|  =(simple-fn -.name.note)
          =(coinbase-fn -.name.note)
      ==
      [notes (some target-pkh)]
    :_  state
    :~  :-  %markdown
        %-  crip
        %+  welp
          """
          ## Wallet Notes for Address {(trip address.cause)}
          - Height: {(trip (format-ui:common:display:utils height.balance.state))}
          - Block id: {(trip (to-b58:hash:transact block-id.balance.state))}

          """
        =-  ?:  =("" -)  "No notes found"  -
        %-  zing
        %+  turn  matching-notes
        |=  [* =nnote:transact]
        %-  trip
        ?^  -.nnote
          (note:v0:display:utils nnote)
        (note-from-balance:v1:display:utils nnote)
        ::
        [%exit 0]
    ==
  ::
  ++  do-list-notes-by-address-csv
    |=  =cause:wt
    ?>  ?=(%list-notes-by-address-csv -.cause)
    =/  matching-notes=(list [name=nname:transact note=nnote:transact])
      ::  v0 address case
      ?:  (gte (met 3 address.cause) 132)
        =/  target-pubkey=schnorr-pubkey:transact
          (from-b58:schnorr-pubkey:transact address.cause)
        %+  skim  ~(tap z-by:zo notes.balance.state)
        |=  [name=nname:transact note=nnote:transact]
        ::  skip v1 notes
        ?@  -.note  %.n
        ::  this should cover all cases because we only
        ::  sync coinbase notes or non-coinbase notes with m=1 locks.
        (~(has z-in:zo pubkeys.sig.note) target-pubkey)
      ::  v1 address case
      =/  target-pkh=hash:transact
        (from-b58:hash:transact address.cause)
      %+  skim  ~(tap z-by:zo notes.balance.state)
      |=  [name=nname:transact note=nnote:transact]
      ::  skip v0 notes
      ?^  -.note  %.n
      ::  look for coinbase notes with target-pkh
      ::  or notes with simple 1-of-1 lock containing
      =+  simple-fn=(simple:v1:first-name:transact target-pkh)
      =+  coinbase-fn=(coinbase:v1:first-name:transact target-pkh)
      ?|  =(simple-fn -.name.note)
          =(coinbase-fn -.name.note)
      ==
    =/  csv-header=tape
      "version,name_first,name_last,assets,block_height,source_hash"
    =/  csv-rows=(list tape)
      %+  turn  matching-notes
      |=  [name=nname:transact note=nnote:transact]
      ?^  -.note
        ::  v0 note
        =+  version=0
        =/  name-b58=[first=@t last=@t]  (to-b58:nname:transact name)
        =/  source-hash-b58=@t  (to-b58:hash:transact p.source.note)
        """
        {(ui-to-tape:utils version)},{(trip first.name-b58)},{(trip last.name-b58)},{(ui-to-tape:utils assets.note)},{(ui-to-tape:utils origin-page.note)},{(trip source-hash-b58)}
        """
      ::  v1 note
      =+  version=1
      =/  name-b58=[first=@t last=@t]  (to-b58:nname:transact name)
      =/  source-hash-b58=@t  'N/A'
      """
      {(ui-to-tape:utils version)},{(trip first.name-b58)},{(trip last.name-b58)},{(ui-to-tape:utils assets.note)},{(ui-to-tape:utils origin-page.note)},{(trip source-hash-b58)}
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
      "notes-{(trip address.cause)}.csv"
    =/  markdown=tape
      """
      ## Result
      Output csv written to {(trip filename)} in current working directory
      """
    :_  state
    :~  [%file %write filename (crip csv-content)]
        [%markdown (crip markdown)]
        [%exit 0]
    ==
  ::
  ++  do-create-tx
    |=  =cause:wt
    ?>  ?=(%create-tx -.cause)
    |^
    %-  (debug "create-tx: {<names.cause>}")
    =/  names=(list nname:transact)  (parse-names names.cause)
    =/  orders=(list order:wt)  orders.cause
    ?~  active-master.state
      :_  state
      :~  :-  %markdown
          %-  crip
          """
          Cannot create a transaction without active master address set. Please import a master key / seed phrase or generate a new one.
          """
          [%exit 0]
      ==
    =/  sign-keys=(list schnorr-seckey:transact)
      ?~  sign-keys.cause
        ~[(sign-key:get:v ~)]
      %+  turn  u.sign-keys.cause
      |=  key-info=[child-index=@ud hardened=?]
      (sign-key:get:v [~ key-info])
    =/  [=spends:v1:transact =witness-data:wt display=transaction-display:wt]
      %:  tx-builder
        names
        orders
        fee.cause
        sign-keys
        refund-pkh.cause
        get-note:v
        include-data.cause
        selection-strategy.cause
      ==
    =/  multisig-recv-locks=(z-set:zo lock:transact)
      (gather-multisig-locks orders)
    =/  transaction-name=@t
      %-  to-b58:hash:transact
      id:(new:raw-tx:v1:transact spends)
    =/  =transaction:wt
      %*  .  *transaction:wt
        name     transaction-name
        spends   spends
        display  display
        witness-data  witness-data
      ==
    =/  res=effects=(list effect:wt)
      (save-transaction transaction)
    ?:  ?=(~ multisig-recv-locks)
      [effects.res state]
    :-  effects.res
    state(keys (watch-multisig-locks multisig-recv-locks))
    ::
    ++  parse-names
      |=  raw-names=(list [first=@t last=@t])
      ^-  (list nname:transact)
      %+  turn  raw-names
      |=  [first=@t last=@t]
      (from-b58:nname:transact [first last])
    ::
    ++  save-transaction
      |=  tx-ser=transaction:wt
      ^-  (list effect:wt)
      ::  we fallback to the hash of the spends as the transaction name
      ::  when generating filenames to ensure uniqueness.
      =/  =raw-tx:v1:transact  (new:raw-tx:v1:transact spends.tx-ser)
      =/  =tx:v1:transact  (new:tx:v1:transact raw-tx height.balance.state)
      =/  =witness-data:wt  witness-data.tx-ser
      =/  fees=@  (roll-fees:spends:v1:transact spends.tx-ser)
      =/  markdown-text=@t
        %:  transaction:v1:display:utils
            name.tx-ser
            outputs.tx
            fees
            display.tx-ser
            get-note:v
            `witness-data
        ==
      ::  jam inputs and save as transaction
      =/  transaction-jam  (jam tx-ser)
      =/  tx-path=@t
        (crip "./txs/{(trip name.tx-ser)}.tx")
      %-  (debug "saving transaction to {<path>}")
      =/  write-effect=effect:wt
        ?.  save-raw-tx.cause
          [%file %write tx-path transaction-jam]
        =/  hashable-path=@t
          %-  crip
          "./txs-debug/{(trip name.tx-ser)}-hashable.jam"
        =/  raw-tx-path=@t
          %-  crip
          "./txs-debug/{(trip name.tx-ser)}.jam"
        :*  %file
            %batch-write
            :~  [hashable-path (jam [leaf+%1 (hashable:spends:transact spends.tx-ser)])]
                [tx-path transaction-jam]
                [raw-tx-path (jam raw-tx)]
            ==
        ==
        =.  markdown-text
          ;:  (cury cat 3)
            '\0a## Create Tx'
            '\0a - Saved transaction to '
            tx-path
            '\0a '
            markdown-text
          ==
      ~[write-effect [%markdown markdown-text]]
    ::
    ++  gather-multisig-locks
      |=  orders=(list order:wt)
      ^-  (z-set:zo lock:transact)
      %-  z-silt:zo
      %+  murn  orders
      |=  ord=order:wt
      ?-    -.ord
          %pkh  ~
      ::
          %multisig
        =/  allowed=(z-set:zo hash:transact)  (z-silt:zo participants.ord)
        `[%pkh [m=threshold.ord allowed]]~
      ==
    ::
    ++  watch-multisig-locks
      |=  locks=(z-set:zo lock:transact)
      ^-  keys:wt
      %-  ~(rep z-in:zo locks)
      |=  [lock=lock:transact acc=_keys.state]
      %-  watch-first-name:put:v
      [(first:nname:transact (hash:lock:transact lock)) `lock]
    ::
    --
  ::
  ++  do-keygen
    |=  =cause:wt
    ?>  ?=(%keygen -.cause)
    =+  [seed-phrase=@t cor]=(gen-master-key:s10 entropy.cause salt.cause)
    =/  [master-public-coil=coil:wt master-private-coil=coil:wt]
      :-  [%1 [%pub public-key] chain-code]:cor
      [%1 [%prv private-key] chain-code]:cor
    =/  old-active  active-master.state
    =.  active-master.state  (some master-public-coil)
    %-  (debug "keygen: public key: {<(en:base58:wrap public-key:cor)>}")
    %-  (debug "keygen: private key: {<(en:base58:wrap private-key:cor)>}")
    =/  pub-label  `(crip "master-public-{<(end [3 4] public-key:cor)>}")
    =/  prv-label  `(crip "master-public-{<(end [3 4] public-key:cor)>}")
    =.  keys.state  (key:put:v master-public-coil ~ pub-label)
    =.  keys.state  (key:put:v master-private-coil ~ prv-label)
    =.  keys.state  (seed:put:v seed-phrase)
    =/  extended-private=@t  extended-private-key:cor
    =/  extended-public=@t  extended-public-key:cor
    =/  addr-b58=@t  ~(address to-b58:coil:wt master-public-coil)
    ::  If there was already an active master address, set it back to the old master address
    ::  The new keys generated are stored in the keys state and the user can manually
    ::  switch to them by running `set-active-master-address`
    =?  active-master.state  ?=(^ old-active)
      old-active
    =/  active-addr=@t  (to-b58:active:wt active-master.state)
    :_  state
    :~  :-  %markdown
        %-  crip
        """
        ## Generated New Master Key (version 1)
        - Added keys to wallet.
        - Active master key is set to {(trip active-addr)}.
          - To switch the active address, run `nockchain-wallet set-active-master-address <master-address>`.
          - To see the available master addresses, run `nockchain-wallet list-master-addresses`.
          - To see the current active address and its child keys, run `nockchain-wallet list-active-addresses`.

        ### Address
        {(trip addr-b58)}

        ### Extended Private Key (save this for import)
        {(trip extended-private)}

        ### Extended Public Key (save this for import)
        {(trip extended-public)}

        ### Seed Phrase (save this for import)
        {<seed-phrase>}

        ### Version (keep this for import with seed phrase)
        1

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
    =/  key-text=tape
      %-  zing
      %+  turn  ~(tap in derived-keys)
      |=  =coil:wt
      =/  version=@  -.coil
      =/  ext-key=@t  (extended-key:coil:wt coil)
      =/  address=@t
        ?:  ?=(%prv -.key.coil)
          'N/A (private key)'
        ~(address to-b58:coil:wt coil)
      =/  key-type=tape
        ?:  ?=(%pub -.key.coil)
          "Extended Public Key"
        "Extended Private Key"
      """
      - {key-type}: {(trip ext-key)}
      - Address: {(trip address)}
      - Version: {<version>}
      ---

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
  ++  do-sign-message
    |=  =cause:wt
    ?>  ?=(%sign-message -.cause)
    ?~  active-master.state
      :_  state
      :~  :-  %markdown
          %-  crip
          """
          Cannot sign a message without active master address set. Please import a master key / seed phrase or generate a new one.
          """
          [%exit 0]
      ==
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
  ++  do-list-master-addresses
    |=  =cause:wt
    ?>  ?=(%list-master-addresses -.cause)
    %-  (debug "list-master-addresses")
    =/  master-addrs=(list tape)
      %+  turn
        master-addresses:get:v
      |=  [version=@ addr=@t]
      =?  addr  =(addr (to-b58:active:wt active-master.state))
        (cat 3 addr ' **(active)**')
      """
      - Address: {(trip addr)}
      - Version: {<version>}
      ---

      """
    :_  state
    :~  :-  %markdown
        %-  crip
        """
        ## Master Address Information
        Note: Addresses are the same as pubkeys for v0 keys. For v1 keys, the address is the hash of the public key.

        {(zing master-addrs)}
        """
        [%exit 0]
    ==
  ::
  ++  do-set-active-master-address
    |=  =cause:wt
    ?>  ?=(%set-active-master-address -.cause)
    %-  (debug "set-active-master-address")
    =/  addr-b58=@t  address-b58.cause
    =/  =coil:wt  (master-by-addr:get:v addr-b58)
    :_  state(active-master `coil)
    :~  :-  %markdown
        %-  crip
        """
        ## Set Active Master Address To:

        - {(trip addr-b58)}
        """
        [%exit 0]
    ==
  ::
  ++  do-sign-multisig-tx
    |=  =cause:wt
    ?>  ?=(%sign-multisig-tx -.cause)
    |^
    %-  (debug "sign-multisig-tx: {<name.dat.cause>}")
    ?~  active-master.state
      :_  state
      :~  :-  %markdown
          %-  crip
          """
          Cannot sign without active master address. Please import a master key.
          """
          [%exit 1]
      ==
    =/  =transaction:wt  dat.cause
    =/  =witness-data:wt  witness-data.transaction
    ?>  ?&  ?=(%1 -.witness-data)
            ?=(%1 -.inputs.display.transaction)
        ==
    =/  =spends:v1:transact  spends.transaction
    ::  get sign-keys from wallet
    ::  if sign-keys is not provided, use master key
    =/  sign-keys=(list schnorr-seckey:transact)
      ?~  sign-keys.cause
        ~[(sign-key:get:v ~)]
      %+  turn  u.sign-keys.cause
      |=  key-info=[child-index=@ud hardened=?]
      (sign-key:get:v [~ key-info])
    =+  num-keys-provided=(lent sign-keys)
    =/  signer-pkhs=(z-set:zo hash:transact)
      %-  z-silt:zo
      %+  turn  sign-keys
      |=  sk=schnorr-seckey:transact
      %-  hash:schnorr-pubkey:transact
      %-  from-sk:schnorr-pubkey:transact
      (to-atom:schnorr-seckey:transact sk)
    ::
    ::  we assume that there is at most one pkh in a single-spend condition
    =/  pkh-lps=(z-map:zo nname:transact pkh:v1:transact)
      %-  ~(rep z-by:zo p.inputs.display.transaction)
      |=  $:  [k=nname:transact v=spend-condition:transact]
              acc=(z-map:zo nname:transact pkh:v1:transact)
          ==
      ?~  v
        acc
      ?:  ?=(%pkh -.i.v)
        (~(put z-by:zo acc) k +.i.v)
      $(v t.v)
    =/  not-required=(z-set:zo hash:transact)
      %-  ~(rep z-by:zo pkh-lps)
      |=  $:  [k=* =pkh:v1:transact]
              acc=(z-set:zo hash:transact)
          ==
      %-  ~(uni z-in:zo acc)
      (~(dif z-in:zo signer-pkhs) h.pkh)
    ?^  not-required
      =/  pkhs-list=@t
        %+  roll  ~(tap z-in:zo `(z-set:zo hash:transact)`not-required)
        |=  [=hash:transact acc=@t]
        ;:  (cury cat 3)
            acc
            '\0a  - '
            (to-b58:hash:transact hash)
        ==
      =/  markdown-text=@t
        ;:  (cury cat 3)
            '\0a## Error signing multisig'
            '\0a- Attempted to sign transaction with keys that are not required by inputs.'
            '\0a- PKHs that are not required: '
            pkhs-list
        ==
      :_  state
      :~  [%exit 0]
          [%markdown markdown-text]
      ==
    ::  sign all spends with all sign-keys
    =.  witness-data
      :-  %1
      %-  ~(rep z-by:zo spends)
      |=  $:  [name=nname:transact =spend:v1:transact]
              wd=(z-map:zo nname:transact witness:transact)
          ==
      ?>  ?=(%1 -.spend)
      =+  sig-hash=(sig-hash:spend-1:v1:transact +.spend)
      =+  curr-witness=(~(got z-by:zo p.witness-data) name)
      =+  curr-pkh=(~(got z-by:zo pkh-lps) name)
      =+  num-signed=~(wyt z-by:zo pkh.curr-witness)
      ?:  =(m.curr-pkh num-signed)
        ~|  ^-  @t
            ;:  (cury cat 3)
                'No more signatures are required to spend note: '
                (name:v1:display:utils name)
                '. Providing more signatures than required will result in an invalid transaction.'
            ==
        !!
      =+  num-needed=(sub m.curr-pkh num-signed)
      ?:  (gth num-keys-provided num-needed)
        ~|  ^-  @t
            ;:  (cury cat 3)
                'Number of sign keys exceeds the required remaining signatures. '
                'Needed: '
                (format-ui:common:display:utils num-needed)
                ', but provided: '
                (format-ui:common:display:utils num-keys-provided)
            ==
        !!
      %+  ~(put z-by:zo wd)  name
      %+  roll  sign-keys
      |=  [sk=schnorr-seckey:transact acc=_curr-witness]
      (sign:witness:transact acc sk sig-hash)
    (save-signed-transaction transaction(witness-data witness-data) sign-keys)
    ::
    ++  save-signed-transaction
      |=  [=transaction:wt sign-keys=(list schnorr-seckey:transact)]
      ^-  [(list effect:wt) state:wt]
      =/  transaction-jam  (jam transaction)
      =/  path=@t
        %-  crip
        "./txs/{(trip name.transaction)}.tx"
      %-  (debug "saving signed transaction to {<path>}")
      =/  =witness-data:wt  witness-data.transaction
      ?>  ?=(%1 -.witness-data)
      =/  sign-pkhs=@t
        %+  roll  sign-keys
        |=  [sk=schnorr-seckey:transact pkhs-list=@t]
        ;:  (cury cat 3)
            pkhs-list
            '\0a  - '
            %-  to-b58:hash:transact
            %-  hash:schnorr-pubkey:transact
            %-  from-seckey:schnorr-pubkey:transact
            sk
        ==
      =/  markdown-text=@t
        %-  crip
        """

        ### Transaction Signed

        - Transaction {(trip name.transaction)} has been signed with: {(trip sign-pkhs)}
        - Saved to: {(trip path)}

        ### Witness Data
        {(trip (witness-data:v1:display:utils witness-data))}

        """
      =/  =effect:wt  [%file %write path transaction-jam]
      :_  state
      :~  [%file %write path transaction-jam]
          [%markdown markdown-text]
          [%exit 0]
      ==
    --
  ::
  ::++  do-show-multisig-tx
  ::  |=  =cause:wt
  ::  ?>  ?=(%show-multisig-tx -.cause)
  ::  %-  (debug "show-multisig-tx: {<name.dat.cause>}")
  ::  =/  =transaction:wt  dat.cause
  ::  =/  =spends:transact  spends.transaction
  ::  =/  display=transaction-display:wt  display.transaction
  ::  =/  fees=@  (roll-fees:spends:v1:transact spends)
  ::  =/  =raw-tx:v1:transact  (new:raw-tx:v1:transact spends)
  ::  =/  =tx:v1:transact  (new:tx:v1:transact raw-tx height.balance.state)
  ::  ::  count signatures
  ::  =/  sig-count=@ud
  ::    %+  roll  ~(val z-by:zo spends)
  ::    |=  [spend=spend:v1:transact count=@ud]
  ::    ?-    -.spend
  ::        %0  count
  ::        %1
  ::      =/  sigs=(list *)  ~(tap z-in:zo pkh.witness.spend)
  ::      (add count (lent sigs))
  ::    ==
  ::  ::  extract required m from lock (if possible)
  ::  =/  required-m=(unit @ud)
  ::    =/  first-spend=(unit [name=nname:transact spend=spend:v1:transact])
  ::      %-  mole
  ::      |.((head ~(tap z-by:zo spends)))
  ::    ?~  first-spend  ~
  ::    ?-    -.spend.u.first-spend
  ::        %0  ~
  ::        %1
  ::      =/  first-seed=(unit seed:v1:transact)
  ::        %-  mole
  ::        |.
  ::        (head ~(tap z-in:zo seeds.spend.u.first-spend))
  ::      ?~  first-seed  ~
  ::      ::  would need to decode lock from lock-root to get m
  ::      ::  for now just show ~
  ::      ~
  ::    ==
  ::  =/  markdown-text=@t
  ::    %-  crip
  ::    """
  ::    # Multisig Transaction Details

  ::    Transaction: {(trip name.transaction)}
  ::    Total signatures: {<sig-count>}
  ::    {?~(required-m "" "Required signatures: {<u.required-m>}\0a")}
  ::    Fee: {(trip (format-ui:common:display:utils fees))} nicks

  ::    ## Outputs

  ::    {(trip (transaction:v1:display:utils name.transaction outputs.tx fees display.transaction))}
  ::    """
  ::  :_  state
  ::  :~  [%markdown markdown-text]
  ::      [%exit 0]
  ::  ==
  ::::
  --  ::+poke
--
