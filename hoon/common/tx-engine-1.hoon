/=  v0  /common/tx-engine-0
/=  *  /common/zeke
/=  *  /common/zoon
|%
::  import
++  hash  hash:v0
++  schnorr-pubkey  schnorr-pubkey:v0
++  sig  sig:v0
++  schnorr-signature  schnorr-signature:v0
++  schnorr-seckey  schnorr-seckey:v0
++  page-number  page-number:v0
++  coins  coins:v0
++  source  source:v0
++  tx-id  tx-id:v0
++  block-id  block-id:v0
++  bignum  bignum:v0
++  bn  bignum
++  page-msg  page-msg:v0
++  proof  proof:v0
++  reason
  |$  object
  (each object term)
::
::  $page: page with v1 coinbase-split
++  page
  =<  form
  |%
  +$  form
    $:  version=%1
        digest=block-id
        pow=$+(pow (unit proof))
        parent=block-id
        tx-ids=(z-set tx-id)
        coinbase=coinbase-split
        timestamp=@
        epoch-counter=@ud
        target=bignum:bn
        accumulated-work=bignum:bn
        height=page-number
        msg=page-msg
    ==
  ::
  ++  new-candidate
    |=  [par=$^(page:v0 form) now=@da target-bn=bignum:bn =shares]
    ^-  form
    ::  extract common fields from either v0 or v1 parent
    =/  [par-accumulated-work=bignum:bn par-digest=hash par-epoch-counter=@ par-height=@]
      ?^  -.par
        [accumulated-work.par digest.par epoch-counter.par height.par]
      [accumulated-work.par digest.par epoch-counter.par height.par]
    =/  accumulated-work
      %-  chunk:bn
      %+  add
        (merge:bn (compute-work:page:v0 target-bn))
      (merge:bn par-accumulated-work)
    =/  epoch-counter=@
      ?:  =(+(par-epoch-counter) blocks-per-epoch:v0)  0
      +(par-epoch-counter)
    =/  height=@  +(par-height)
    %*  .  *form
      height            height
      parent            par-digest
      timestamp         (time-in-secs:page:v0 now)
      epoch-counter     epoch-counter
      target            target-bn
      accumulated-work  accumulated-work
      coinbase          %+  new:coinbase-split
                          (emission-calc:coinbase:v0 height)
                        shares
    ==
  ::
  ++  to-local-page
    |=  pag=form
    ^-  local-page
    pag(pow (bind pow.pag |=(p=proof (jam p))))
  ::
  ++  hashable-block-commitment
    |=  =form
    ^-  hashable:tip5
    :*  hash+parent.form
        hash+(hash-hashable:tip5 (hashable-tx-ids tx-ids.form))
        hash+(hash:coinbase-split coinbase.form)
        leaf+timestamp.form
        leaf+epoch-counter.form
        leaf+target.form
        leaf+accumulated-work.form
        leaf+height.form
        leaf+msg.form
    ==
  ::
  ++  hashable-tx-ids
    |=  tx-ids=(z-set tx-id)
    ^-  hashable:tip5
    ?~  tx-ids  leaf+tx-ids
    :+  hash+n.tx-ids
      $(tx-ids l.tx-ids)
    $(tx-ids r.tx-ids)
  ::
  ++  hashable-digest
    |=  pag=form
    ^-  hashable:tip5
    :-  ?~  pow.pag  leaf+~
        [leaf+~ hash+(hash-proof:v0 u.pow.pag)]
    (hashable-block-commitment pag)
  ::
  ++  block-commitment
    |=  =form
    (hash-hashable:tip5 (hashable-block-commitment form))
  ::
  ++  compute-digest
    |=  pag=form
    ^-  block-id
    %-  hash-hashable:tip5
    (hashable-digest pag)
  ::
  ++  check-digest
    |=  pag=form
    ^-  ?
    ?&  (based:block-id digest.pag)
        =(digest.pag (compute-digest pag))
    ==
  ::
  ++  compute-size-without-txs
    |=  pag=form
    ^-  size
    ;:  add
        max-size:block-id:v0
        max-size:proof:v0
        (compute-size-jam `*`+>.pag)
    ==
  --
::
::  $local-page: v1 page with jammed pow for storage
++  local-page
  =<  form
  |%
  +$  form
    $+  local-page-v1
    $:  version=%1
        digest=block-id
        pow=$+(pow (unit @))
        parent=block-id
        tx-ids=(z-set tx-id)
        coinbase=coinbase-split
        timestamp=@
        epoch-counter=@ud
        target=bignum:bn
        accumulated-work=bignum:bn
        height=page-number
        msg=page-msg
    ==
  ::
  ++  to-page
    |=  lp=form
    ^-  page
    lp(pow (biff pow.lp |=(j=@ ((soft proof) (cue j)))))
  --
++  timelock-range  timelock-range:v0
++  size  size:v0
+$  blockchain-constants
  $+  blockchain-constants
  $~  :*
          v1-phase=39.000
          ::  note data field constraints
          ::    max-size: maximum number of leaves in the data field noun
          ::    min-fee:  minimum fee (in nicks)
          data=[max-size=2.048 min-fee=256]
          ::  base fee per word for witness and note-data storage
          base-fee=(bex 15)
          *blockchain-constants:v0
      ==
  $:  v1-phase=@
      data=[max-size=@ min-fee=@]
      base-fee=@
      blockchain-constants:v0
  ==
:: $nname
++  nname
  =<  form
  =+  nname:v0
  |%
  +$  form  $|(^form |=(* %&))
  ++  new-v1
    |=  [lock=hash =source]
    ^-  form
    [(first lock) (last source) ~]
  ::
  ++  first
    |=  lock=hash
    ^-  hash
    (hash-hashable:tip5 [leaf+& hash+lock])
  ::
  ++  last
    |=  =source
    ^-  hash
    %-  hash-hashable:tip5
    :*  leaf+&
        (hashable:^source source)
        leaf+~
    ==
  --
::
::  $nnote: a nockchain note (v0 or v1)
++  nnote
  =<  form
  |%
  +$  form  $^(nnote:v0 nnote-1)
  ++  based
    |=  =form
    ?^  -.form  (based:nnote:v0 form)
    (based:nnote-1 form)
  ++  hash
    |=  =form
    ?^  -.form  (hash:nnote:v0 form)
    (hash:nnote-1 form)
  ++  hashable
    |=  =form
    ?^  -.form  (hashable:nnote:v0 form)
    (hashable:nnote-1 form)
  --
::
:: $nnote. A Nockchain note. A UTXO. (Version 1)
++  nnote-1
  =<  form
  |%
  +$  form
    $:
      version=%1
      origin-page=page-number
      name=nname
      =note-data
      assets=coins
    ==
  ++  based
    |=  =form
    ?&  (based:nname name.form)
        (based:note-data note-data.form)
        (^based assets.form)
    ==
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    :*  leaf+version.form
        leaf+origin-page.form
        hash+(hash:nname name.form)
        hash+(hash:note-data note-data.form)
        leaf+assets.form
    ==
  ::
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  ++  lock-hash
    |=  =form
    -.name.form
  ++  source-hash
    |=  =form
    +<.name.form
  --  :: nnote
::
::  $note-data: data associated with a note
++  note-data
  =<  form
  |%
  +$  form  (z-map @tas *)
  ++  based
    |=  =form
    |^
      ^-  ?
      %-  ~(rep by form)
      |=  [[k=@tas v=*] a=?]
      ?&(a (^based k) (based-noun v))
    ++  based-noun
      |=  n=*
      ?^  n  ?&($(n -.n) $(n +.n))
      (^based n)
    --  ::  based:note-data
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    |^
      ?~  form  leaf+~
      :+  [leaf+p.n.form (hashable-noun q.n.form)]
        $(form l.form)
      $(form r.form)
    ::
    ++  hashable-noun
      |=  n=*
      ?^  n  [$(n -.n) $(n +.n)]
      leaf+n
    --  ::  $hashable:note-data
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  --  ::  $note-data
::
::  $seed: carrier of value from input to output (v1)
++  seed
  =<  form
  |%
  +$  form
    $:  ::  if non-null, enforces that output note must have precisely this source
        output-source=(unit source)
        ::  merkle root of lock script
        lock-root=^hash
        ::  data to store with note
        =note-data
        ::  asset quantity
        gift=coins
        ::  check that parent hash of every seed is the hash of the parent note
        parent-hash=^hash
    ==
  ::
  ++  new
    |=  $:  output-source=(unit source)
            =lock
            gift=coins
            parent-hash=^hash
        ==
    %*  .  *form
      output-source  output-source
      lock-root  (hash:^lock lock)
      gift  gift
      parent-hash  parent-hash
    ==
  ::
  ++  based
    |=  =form
    ^-  ?
    =/  based-output-source
      ?~  output-source.form  %&
      (based:^hash p.u.output-source.form)
    ?&  based-output-source
        (based:^hash lock-root.form)
        (^based gift.form)
        (based:^hash parent-hash.form)
    ==
  ::
  ++  hashable
    |=  sed=form
    ^-  hashable:tip5
    :^    hash+lock-root.sed
        hash+(hash:note-data note-data.sed)
      leaf+gift.sed
    hash+parent-hash.sed
  ::
  ++  sig-hashable
    |=  sed=form
    ^-  hashable:tip5
    :*  (hashable-unit:source output-source.sed)
        hash+lock-root.sed
        hash+(hash:note-data note-data.sed)
        leaf+gift.sed
        hash+parent-hash.sed
    ==
  ::
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  --  ::  seed
::
::  $seeds: Collection of seeds used in a $spend
++  seeds
  =<  form
  |%
  +$  form  (z-set seed)
  ::
  ++  new
    |=  seds=(list seed)
    ^-  form
    (~(gas z-in *form) seds)
  ::
  ++  based
    |=  =form
    ^-  ?
    %-  ~(rep z-in form)
    |=  [s=seed a=?]
    ?&(a (based:seed s))
  ::
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    ?~  form  leaf+~
    :+  (hashable:seed n.form)
      $(form l.form)
    $(form r.form)
  ::
  ++  sig-hashable
    |=  =form
    ^-  hashable:tip5
    ?~  form  leaf+form
    :+  (sig-hashable:seed n.form)
      $(form l.form)
    $(form r.form)
  ::
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  --  ::  seeds
::
::  $spend: Spend a note into v1 notes
++  spend
  =<  form
  |%
  +$  form  $+(spend-v1 $%([%0 spend-0] [%1 spend-1]))
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    ?-  -.form
      %0  [leaf+%0 (hashable:spend-0 +.form)]
      %1  [leaf+%1 (hashable:spend-1 +.form)]
    ==
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  ++  sig-hash
    |=  =form
    ^-  ^hash
    ?-  -.form
      %0  (sig-hash:spend-0 +.form)
      %1  (sig-hash:spend-1 +.form)
    ==
  ++  validate-without-signatures
    |=  =form
    ^-  ?
    ?-  -.form
      %0  (validate-basic:spend-0 +.form)
      %1  (validate-basic:spend-1 +.form)
    ==
  ::
  ++  validate-with-parent-note
    |=  [=form parent=nnote]
    ^-  ?
    ?&  (validate-without-signatures form)
        (check-gifts-and-fee form parent)
    ==
  ::
  ++  check-gifts-and-fee
    |=  [=form parent=nnote]
    ^-  ?
    =/  gifts-and-fee=coins
      %+  add  fee.form
      %+  roll  ~(tap z-in seeds.form)
      |=  [=seed acc=coins]
      (add acc gift.seed)
    =(gifts-and-fee assets.parent)
  --
::
++  shares
  =<  form
  |%
  +$  form  $+(shares (z-map hash @))
  ::
  ++  validate
    |=  =form
    ?&  (lte ~(wyt z-by form) max-coinbase-split:v0)
    ::
        %+  levy  ~(tap z-by form)
        |=  [h=hash s=@]
        !=(s 0)
    ==
  --
::
::  $coinbase-split: v1 coinbase split using lock hashes
++  coinbase-split
  =<  form
  |%
  +$  form  (z-map ^hash coins)
  ::
  ++  new
    |=  [assets=coins =shares]
    ^-  form
    =/  hashes=(list ^hash)  ~(tap z-in ~(key z-by shares))
    ?:  =(1 (lent hashes))
      ::  if only one lock hash, give all assets to it
      (~(put z-by *form) (snag 0 hashes) assets)
    ::
    =/  split=(list [h=^hash share=@ =coins])
      %+  turn  ~(tap z-by shares)
      |=([h=^hash s=@] [h s 0])
    ::
    =|  recursion-depth=@
    =/  remaining-coins=coins  assets
    =/  total-shares=@
      %+  roll  split
      |=  [[h=^hash share=@ =coins] sum=@]
      (add share sum)
    |-
    ?:  =(0 remaining-coins)
      (~(gas z-by *form) (turn split |=([h=^hash s=@ c=coins] [h c])))
    ?:  (gth recursion-depth 2)
      ::  if any coins left after 2 rounds, give to first hash
      =/  final-split=(list [^hash coins])
        (turn split |=([h=^hash s=@ c=coins] [h c]))
      =/  first=[h=^hash c=coins]  (snag 0 final-split)
      =.  c.first  (add c.first remaining-coins)
      =.  final-split  [first (slag 1 final-split)]
      (~(gas z-by *form) final-split)
    ::  distribute proportionally
    =/  new-split=(list [h=^hash share=@ total=coins this=coins])
      %+  turn  split
      |=  [h=^hash share=@ current-coins=coins]
      =/  coins-for-share=coins
        (div (mul share remaining-coins) total-shares)
      [h share (add current-coins coins-for-share) coins-for-share]
    ::  calculate distributed amount
    =/  distributed=coins
      %+  roll  new-split
      |=  [[h=^hash s=@ c=coins this=coins] sum=coins]
      (add this sum)
    ?:  =(0 distributed)
      ::  no coins distributed, give remainder to first
      =/  final-split=(list [^hash coins])
        (turn new-split |=([h=^hash s=@ t=coins d=coins] [h t]))
      =/  first=[h=^hash c=coins]  (snag 0 final-split)
      =.  c.first  (add c.first remaining-coins)
      =.  final-split  [first (slag 1 final-split)]
      (~(gas z-by *form) final-split)
    =/  still-remaining=@  (sub remaining-coins distributed)
    %=  $
      split            (turn new-split |=([h=^hash s=@ t=coins d=coins] [h s t]))
      remaining-coins  still-remaining
      recursion-depth  +(recursion-depth)
    ==
  ::
  ++  based
    |=  =form
    ?.  (lte ~(wyt z-by form) max-coinbase-split:v0)
      %|
    %+  levy  ~(tap z-by form)
    |=  [h=^hash =coins]
    ?&  !=(0 coins)
        (^based coins)
        (based:^hash h)
    ==
  ::
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    ?~  form  leaf+form
    :+  [hash+p.n.form leaf+q.n.form]
      $(form l.form)
    $(form r.form)
  ::
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  --
::  $spend-0: Spend a v0 note into v1 notes
++  spend-0
  =<  form
  |%
  +$  form
    $+  spend-0
    $:  signature=signature:v0
        =seeds
        fee=coins
    ==
  ++  new
    |=  [=seeds fees=coins]
    %*  .  *form
      seeds  seeds
      fee  fees
    ==
  ::  sign with v0 schnorr, identical hash as v0 spends
  ++  sign
    |=  [sen=form sk=schnorr-seckey]
    ^+  sen
    =/  pk=schnorr-pubkey
      %-  ch-scal:affine:curve:cheetah
      :*  (t8-to-atom:belt-schnorr:cheetah sk)
          a-gen:curve:cheetah
      ==
    =/  sig=schnorr-signature
      %+  sign:affine:belt-schnorr:cheetah
        sk
      (sig-hash sen)
    %_  sen
      signature  (~(put z-by signature.sen) pk sig)
    ==
  ::  batch verification helpers
  ++  signatures
    |=  sen=form
    ^-  (list [schnorr-pubkey ^hash schnorr-signature])
    %+  turn  ~(tap z-by signature.sen)
    |=  [pk=schnorr-pubkey sg=schnorr-signature]
    :*  pk
        (sig-hash sen)
        sg
    ==
  ++  verify
    |=  [sen=form parent-note=nnote:v0]
    ^-  ?
    ?&  (verify-without-signatures sen parent-note)
        (verify-signatures sen)
    ==
  ++  verify-signatures
    |=  sen=form
    ^-  ?
    (batch-verify:affine:belt-schnorr:cheetah (signatures sen))
  ++  verify-without-signatures
    |=  [sen=form parent-note=nnote:v0]
    ^-  ?
    =/  parent-hash=hash  (hash:nnote:v0 parent-note)
    ::  parent-hash commit
    ?.  (~(all z-in seeds.sen) |=(sed=seed =(parent-hash.sed parent-hash)))
      %.n
    ::  m-of-n against parent lock
    =/  have-pks=(z-set schnorr-pubkey)  ~(key z-by signature.sen)
    ?:  (lth ~(wyt z-in have-pks) m.sig.parent-note)
      ::  Even if the # of have-pks is less than m, accept the
      ::  v0 note if # of pubkeys in sig is equal to the number
      ::  of signatures provided and the pubkeys provided with
      ::  the signature are equal to the set of required pubkeys.
      ::  This does not guarentee that the note can be spent: the
      ::  signatures might be invalid.
      =(pubkeys.sig.parent-note have-pks)
    ?.  =((~(int z-in pubkeys.sig.parent-note) have-pks) have-pks)
      %.n
    %.y
  ::
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    [(hashable:signature:v0 signature.form) (hashable:seeds seeds.form) leaf+fee.form]
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  ++  based  based:spend:v0
  ++  sig-hash
    |=  =form
    ^-  ^hash
    %-  hash-hashable:tip5
    [(sig-hashable:seeds seeds.form) leaf+fee.form]
  ::
  ++  validate-basic
    |=  =form
    ^-  ?
    ?&  (based:seeds seeds.form)
        (^based fee.form)
        ?.  =(seeds.form *seeds)  %.y  %.n  :: must have at least one seed
    ==
  --
::
::  $spend-1: Spend a v1 note
++  spend-1
  =<  form
  |%
  +$  form
    $+  spend-1
    $:  =witness
        =seeds
        fee=coins
    ==
  ::
  ++  new
    |=  [=seeds fee=coins]
    %*  .  *form
      seeds  seeds
      fee  fee
    ==
  ::
  ++  signatures
    |=  sen=form
    ^-  (list [schnorr-pubkey ^hash schnorr-signature])
    (signatures:pkh-signature pkh.witness.sen (sig-hash sen))
  ::  +sign: add a single signature to the witness
  ++  sign
    |=  [sen=form sk=schnorr-seckey]
    ^+  sen
    ::  we must derive the pubkey from the seckey
    =/  pk=schnorr-pubkey
      %-  ch-scal:affine:curve:cheetah
      :*  (t8-to-atom:belt-schnorr:cheetah sk)
          a-gen:curve:cheetah
      ==
    =/  sog=schnorr-signature
      %+  sign:affine:belt-schnorr:cheetah
        sk
      (sig-hash sen)
    =.  witness.sen
      %_  witness.sen
        pkh  (~(put z-by pkh.witness.sen) (hash:schnorr-pubkey pk) [pk sog])
      ==
    sen
  ::
  ::  +verify: verify the witness and each seed has correct parent-hash
  ++  verify
    |=  [sen=form parent-note=nnote]
    ^-  ?
    ?&
    ::  check without signatures
      (verify-without-signatures sen parent-note)
    ::  check signatures
      (verify-signatures sen)
    ::  check hashes
      (verify-hashes sen)
    ==
  ::
  ++  verify-signatures
    |=  sen=form
    ^-  ?
    (batch-verify:affine:belt-schnorr:cheetah (signatures sen))
  ::
  ::  +verify-hashes: verify hash locks in witness
  ++  verify-hashes
    |=  sen=form
    ^-  ?
    %+  levy  ~(tap z-by hax.witness.sen)
    |=  [h=^hash pre=*]
    =(h (hash-noun:hax pre))
  ::
  ::  +verify-without-signatures: verify without checking signatures
  ++  verify-without-signatures
    |=  [sen=form parent-note=nnote]
    ^-  ?
    ?>  ?=(@ -.parent-note)
    =/  parent-hash=hash  (hash:nnote parent-note)
    ::  check that parent hash of each seed matches the note's hash
    ?.  (~(all z-in seeds.sen) |=(sed=seed =(parent-hash.sed parent-hash)))
      %.n
    ::  check that witness is valid
    ?.  (based:witness witness.sen)
      %.n
    ::  check that lock-merkle-proof is valid
    =/  lock-hash=hash  (lock-hash:nnote-1 parent-note)
    (check:lock-merkle-proof lmp.witness.sen lock-hash)
  ::
  ::  +sig-hash: the hash used for signing and verifying
  ++  sig-hash
    |=  sen=form
    ^-  ^hash
    %-  hash-hashable:tip5
    [(sig-hashable:seeds seeds.sen) leaf+fee.sen]
  ::
  ++  based
    |=  sen=form
    ?&  (based:witness witness.sen)
        (based:seeds seeds.sen)
        (^based fee.sen)
    ==
  ::
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  ::
  ++  hashable
    |=  sen=form
    ^-  hashable:tip5
    [(hashable:witness witness.sen) (hashable:seeds seeds.sen) leaf+fee.sen]
  ::
  ++  validate-basic
    |=  =form
    ^-  ?
    ?&  (based:witness witness.form)
        (based:seeds seeds.form)
        (^based fee.form)
        ?.  =(seeds.form *seeds)  %.y  %.n  :: must have at least one seed
    ==
  ::
  --  ::  spend
::
::  $spends: associate spends with their input note names
++  spends
  =<  form
  |%
  +$  form  (z-map nname spend)
  ++  based
    |=  =form
    ^-  ?
    %-  ~(rep z-by form)
    |=  [[nam=nname sp=spend] ok=?]
    ?&  ok
        (based:nname nam)
        ?-    -.sp
            %0
          ?&  (based:signature:v0 signature.+.sp)
              (based:seeds seeds.+.sp)
              (^based fee.+.sp)
          ==
            %1
          ?&  (based:witness witness.+.sp)
              (based:seeds seeds.+.sp)
              (^based fee.+.sp)
          ==
        ==
    ==
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    |-
    ?~  form  leaf+form
    :+  [(hashable:nname p.n.form) (hashable:spend q.n.form)]
      $(form l.form)
    $(form r.form)
  ::
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  ::
  ++  validate-with-context
    |=  [balance=(z-map nname nnote) sps=form page-num=page-number max-size=@]
    ^-  (reason ~)
    %+  roll  ~(tap z-by sps)
    |=  [[nam=nname sp=spend] acc=(reason ~)]
    ?.  ?=(%.y -.acc)  acc
    ::  check note-data size limits
    =/  seed-list=(list seed)
      ?-  -.sp
        %0  ~(tap z-in seeds.+.sp)
        %1  ~(tap z-in seeds.+.sp)
      ==
    =/  exceeds-size=?
      %+  lien  seed-list
      |=  sed=seed
      =/  data-size=@
        %-  num-of-leaves:shape
        %-  ~(rep z-by note-data.sed)
        |=  [[k=@tas v=*] tree=*]
        [k v tree]
      (gth data-size max-size)
    ?:  exceeds-size  [%.n %v1-note-data-exceeds-max-size]
    =/  mnote=(unit nnote)  (~(get z-by balance) nam)
    ?~  mnote  [%.n %v1-input-missing]
    =/  note=nnote  u.mnote
    ?-    -.sp
      ::
        %0
      ::  v0 note must back a %0 spend
      ?:  ?=(@ -.note)  [%.n %v1-spend-version-mismatch]
      =/  verified=?  (verify:spend-0 +.sp note)
      ?.  verified
        [%.n %v1-spend-0-verify-failed]
      ?.  (check-gifts-and-fee:spend sp note)
        [%.n %v1-spend-0-gifts-failed]
      [%.y ~]
      ::
        %1
      ::  v1 note must back a %1 spend
      ?:  ?=(^ -.note)  [%.n %v1-spend-version-mismatch]
      ?:  !=(%1 version.note)  [%.n %v1-note-version-mismatch]
      =/  ctx=check-context
        :*  page-num
            origin-page.note
            (sig-hash:spend-1 +.sp)
            witness.+.sp
        ==
      ?.  %+  check:check-context  ctx
          (lock-hash:nnote-1 note)
        [%.n %v1-spend-1-lock-failed]
      ?.  (check-gifts-and-fee:spend sp note)
        [%.n %v1-spend-1-gifts-failed]
      [%.y ~]
    ==
  ++  validate
    |=  =form
    ^-  ?
    ?:  =(form *^form)  %.n
    ?.  (verify-signatures form)
      ~>  %slog.[1 'spends: validate: Invalid spends. There is a spend with an invalid signature']
      %.n
    %+  levy  ~(tap z-by form)
    |=  [=nname =spend]
    (validate-without-signatures:^spend spend)
  ::
  ++  verify-signatures
    |=  =form
    ^-  ?
    %+  levy  ~(tap z-by form)
    |=  [=nname =spend]
    ?-  -.spend
      %0  (verify-signatures:spend-0 +.spend)
      %1  (verify-signatures:spend-1 +.spend)
    ==
  ::
  ++  roll-fees
    |=  =form
    ^-  coins
    %+  roll  ~(val z-by form)
    |=  [sp=spend acc=coins]
    (add acc fee.+.sp)
  --
::
::  $input: inputs to a v1 transaction
::
::  Note that .note can be a v0 or v1 note,
::  and that witness.spend can be a v0 witness (just signatures)
::  or a v1 witness (a segwit witness), so validity checking must first ensure
::  matching versions between note and witness before checking the witness itself
++  input
  =<  form
  |%
  +$  form  [note=nnote =spend]
  ++  new
    |=  $:  note=nnote
            =seeds
            fee=coins
            sk=schnorr-seckey
        ==
    ^-  form
    :-  note
    ?^  -.note
      :-  %0
      (new:spend-0 seeds fee)
    :-  %1
    (new:spend-1 seeds fee)
  ++  based
    |=  inp=form
    ?&  (based:nnote note.inp)
        ?-    -.spend.inp
            %0
          ?&  (based:signature:v0 signature.+.spend.inp)
              (based:seeds seeds.+.spend.inp)
              (^based fee.+.spend.inp)
          ==
            %1
          ?&  (based:witness witness.+.spend.inp)
              (based:seeds seeds.+.spend.inp)
              (^based fee.+.spend.inp)
          ==
        ==
    ==
  ++  hashable
    |=  inp=form
    ^-  hashable:tip5
    :-  (hashable:nnote note.inp)
    (hashable:spend spend.inp)
  ::
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  --
::
::  $inputs: map of names to inputs (version 1)
++  inputs
  =<  form
  |%
  +$  form  (z-map nname input)
  ++  new
    =<  from-spends
    |%
    ++  from-spends
      |=  [=spends notes=(z-map nname nnote)]
      ^-  (unit form)
      %-  ~(rep z-by spends)
      |=  [[=nname =spend] i=(unit form)]
      ^-  (unit form)
      ?~  i  ~
      =/  note  (~(get z-by notes) nname)
      ?~  note  ~
      `(~(put z-by u.i) nname [u.note spend])
    --
  ++  based
    |=  =form
    ^-  ?
    %-  ~(rep z-by form)
    |=  [[nam=nname inp=input] ok=?]
    ?&  ok
        (based:nname nam)
        (based:input inp)
    ==
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    ?~  form  leaf+form
    :+  [(hashable:nname p.n.form) (hashable:input q.n.form)]
      $(form l.form)
    $(form r.form)
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  --
::
++  output
  =<  form
  |%
  +$  form  [note=nnote =seeds]
  ::
  ++  validate
    |=  out=form
    ^-  ?
    ::  extract source from note's name
    ?>  ?=(@ -.note.out)
    =/  source-hash=hash  (source-hash:nnote-1 note.out)
    =/  source-check=?
      %+  levy  ~(tap z-in seeds.out)
      |=  =seed
      ?~  output-source.seed  %.y
      .=   %-  hash-hashable:tip5
           :*  leaf+&
               (hashable:source u.output-source.seed)
               leaf+~
            ==
      source-hash
    =/  assets-check=?
      =/  calc-assets=coins
        %+  roll  ~(tap z-in seeds.out)
        |=  [=seed acc=coins]
        (add gift.seed acc)
      =(calc-assets assets.note.out)
    &(source-check assets-check)
  --
::
::  $raw-tx: version 1 transaction
::
::  Transactions were not initially versioned which was a mistake.
::  Fortunately we can disambiguate carefully.
::  The head of a v0 transaction will be a cell (the tx-id)
::  The head of a v >0 transaction will be the version atom
++  raw-tx
  =<  form
  |%
  +$  form
    $+  raw-tx-v1
    $:  version=%1
        id=tx-id
        =spends
    ==
  ++  new
    |=  sps=spends
    ^-  form
    =/  raw=form
      %*  .  *form
        version  %1
        spends   sps
      ==
    raw(id (compute-id raw))
  ++  compute-id
    |=  raw=form
    ^-  tx-id
    %-  hash-hashable:tip5
    [leaf+%1 (hashable:spends spends.raw)]
  ::
  ++  based
    |=  raw=form
    ^-  ?
    |^
    ?&  (based:hash id.raw)
        (based-spends spends.raw)
    ==
    ::
    ++  based-spends
      |=  sps=spends
      ^-  ?
      %-  ~(rep z-by sps)
      |=  [[nam=nname sp=spend] ok=?]
      ?&  ok
          (based:nname nam)
          (based-spend sp)
      ==
    ::
    ++  based-spend
      |=  sp=spend
      ^-  ?
      ?-  -.sp
        %0
          ?&  (based:signature:v0 signature.+.sp)
              (based:seeds seeds.+.sp)
              (^based fee.+.sp)
          ==
        %1
          ?&  (based:witness witness.+.sp)
              (based:seeds seeds.+.sp)
              (^based fee.+.sp)
          ==
      ==
    --
  ++  validate
    |=  =form
    ^-  ?
    ?&  (based form)
        =(id.form (compute-id form))
        (validate:spends spends.form)
    ==
  ::
  ++  get
    |_  =form
    ::
    ++  id
      ^-  tx-id  id.form
    ::
    ++  size
      ^-  ^size  (compute-size-jam `*`form)
    ++  input-names
      ^-  (z-set nname)
      ~(key z-by spends.form)
    --
--
::
::  $lock-primitive: lock script primitive
++  lock-primitive
  =<  form
  |%
  +$  form
    $+  lock-primitive
    $%  [%pkh pkh]
        [%tim tim]
        [%hax hax]
    ::  it's important that this be the default to break a type loop in the compiler
        [%brn ~]
    ==
  ++  based
    |=  =form
    ?-  -.form
        %tim  (based:tim +.form)
        %hax  (based:hax +.form)
        %pkh  (based:pkh +.form)
        %brn  %&
    ==
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    ?-  -.form
        %tim  [leaf+%tim (hashable:tim +.form)]
        %hax  [leaf+%hax (hashable:hax +.form)]
        %pkh  [leaf+%pkh (hashable:pkh +.form)]
        %brn  [leaf+%brn leaf+~]
    ==
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  --
::
::  $spend-condition: AND-list of lock-primitives: all must be satisfied to spend
++  spend-condition
  =<  form
  |%
  +$  form
    $+  spend-condition
    (list lock-primitive)
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    ?~  form  leaf+~
    :-  (hashable:lock-primitive i.form)
    $(form t.form)
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  ::
  ++  based
    |=  =form
    ^-  ?
    (levy form based:lock-primitive)
  --  ::  spend-condition
::
::  $witness: version 1 witness for spend conditions, includes static data
++  witness
  =<  form
  |%
  +$  form
    $:  lmp=lock-merkle-proof
        pkh=pkh-signature
        hax=(z-map ^hash *)
        ::  timelock is dynamic
        tim=~
    ==
  ++  based
    |=  =form
    ^-  ?
    ?&  (based:lock-merkle-proof lmp.form)
        (based:pkh-signature pkh.form)
        %-  ~(rep z-by hax.form)
        |=  [[k=^hash v=*] a=?]
        &(a (based:^hash k))
    ==
  ::
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    :*  hash+(hash:lock-merkle-proof lmp.form)
        hash+(hash:pkh-signature pkh.form)
        hash+(hash-hashable:tip5 (hashable-hax hax.form))
        leaf+tim.form
    ==
  ::
  ++  hashable-hax
    |=  m=(z-map ^hash *)
    ^-  hashable:tip5
    ?~  m  leaf+m
    :+  [hash+p.n.m (hashable-noun q.n.m)]
        $(m l.m)
    $(m r.m)
  ::
  ++  hashable-noun
    |=  n=*
    ^-  hashable:tip5
    ?^  n  [$(n -.n) $(n +.n)]
    leaf+n
  ::
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  --
::
::  +lock: an OR of all conditions in tree
++  lock
  =<  form
  |%
  +$  form
    $+  lock
    $^  spend-condition
    $%  [%2 v2]
        [%4 v4]
        [%8 v8]
        [%16 v16]
    ==
  +$  v2    [p=spend-condition q=spend-condition]
  +$  v4    [p=v2 q=v2]
  +$  v8    [p=v4 q=v4]
  +$  v16   [p=v8 q=v8]
  ::
  ++  from-list
    |=  scs=(list spend-condition)
    ^-  form
    ?~  scs
      ~|  'error: provided empty list of spend-condition'  !!
    |^
    =/  filler=spend-condition  ~[[%brn ~]]
    =/  len=@  (lent scs)
    ::  check if len is a power of 2
    =/  nearest-power-of-two  (bex (xeb (dec len)))
    ?>  (lte nearest-power-of-two 16)
    =/  padded=(list spend-condition)
      ?:  =(len nearest-power-of-two)
        scs
      (weld scs (reap (sub nearest-power-of-two len) filler))
    (build padded nearest-power-of-two)
  ::
  ++  build
    |=  [leaves=(list spend-condition) size=@]
    ^-  form
    ?~  leaves
      ~|  'error: build called with empty leaves'  !!
    ?:  =(size 1)
      i.leaves
    =/  [half=@ rem=@]  (dvr size 2)
    ?>  =(rem 0)
    =/  left-leaves
      (scag half `(list spend-condition)`leaves)
    =/  right-leaves
      (slag half `(list spend-condition)`leaves)
    =/  left=form
      $(leaves left-leaves, size half)
    =/  right=form
      $(leaves right-leaves, size half)
    ?^  -.left
      ?>  ?=(^ -.right)
      [%2 left right]
    ?+    -.left  ~|('from-list:lock: length of leaves is not a power of 2' !!)
        %2
      ?>  ?=(%2 -.right)
      [%4 +.left +.right]
    ::
        %4
      ?>  ?=(%4 -.right)
      [%8 +.left +.right]
    ::
        %8
      ?>  ?=(%8 -.right)
      [%16 +.left +.right]
    ==
  --
  ::
  ++  hash
    |=  =form
    ^-  ^hash
    (hash-hashable:tip5 (hashable form))
  ::
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    |^
    ?^  -.form
      hash+(hash:spend-condition form)
    ?-  -.form
      %2   [leaf+%2 (hashable-v2 +.form)]
      %4   [leaf+%4 (hashable-v4 +.form)]
      %8   [leaf+%8 (hashable-v8 +.form)]
      %16  [leaf+%16 (hashable-v16 +.form)]
    ==
    ::
    ++  hashable-v2
      |=  =v2
      ^-  hashable:tip5
      :-  hash+(hash:spend-condition p.v2)
      hash+(hash:spend-condition q.v2)
    ::
    ++  hashable-v4
      |=  =v4
      ^-  hashable:tip5
      :-  (hashable-v2 p.v4)
      (hashable-v2 q.v4)
    ::
    ++  hashable-v8
      |=  =v8
      ^-  hashable:tip5
      :-  (hashable-v4 p.v8)
      (hashable-v4 q.v8)
    ::
    ++  hashable-v16
      |=  =v16
      ^-  hashable:tip5
      :-  (hashable-v8 p.v16)
      (hashable-v8 q.v16)
    --
  ::
  ++  build-lock-merkle-proof
    |=  [=form leaf-number=@]
    ^-  lock-merkle-proof
    |^
    ?>  !=(leaf-number 0)
    ::
    ::  adjust the leaf number by 1 if the lock has a head tag
    =/  hashable-index=@
      ?^  -.form
        leaf-number
      +(leaf-number)
    =/  [axis=@ =merk-proof:merkle]
      (prove-hashable-by-index:merkle (hashable form) hashable-index)
    =/  =spend-condition
      (traverse-lock form)
    [spend-condition axis merk-proof]
    ::
    ++  traverse-lock
      |=  =^form
      ^-  spend-condition
      ?^  -.form
        ?>  =(leaf-number 1)
        form
      ?-  -.form
          %2
        ?:  =(leaf-number 1)  p.form
        ?:  =(leaf-number 2)  q.form
        !!
          %4
        ?:  (lte leaf-number 2)
          $(form 2+p.form, leaf-number leaf-number)
        $(form 2+q.form, leaf-number (sub leaf-number 2))
      ::
          %8
        ?:  (lte leaf-number 4)
          $(form 4+p.form, leaf-number leaf-number)
        $(form 4+q.form, leaf-number (sub leaf-number 4))
      ::
          %16
        ?:  (lte leaf-number 8)
          $(form 8+p.form, leaf-number leaf-number)
        $(form 8+q.form, leaf-number (sub leaf-number 8))
      ==
    --  ::+build-lock-merkle-proof
    ++  from-sig
      |=  =sig
      ^-  form
      =/  hs=(z-set ^hash)
        %+  roll  ~(tap z-in pubkeys.sig)
        |=  [pk=schnorr-pubkey acc=(z-set ^hash)]
        (~(put z-in acc) (hash:schnorr-pubkey pk))
      [%pkh [m.sig hs]]~
  --
::
::  $lock-merkle-proof: merkle proof for a branch of a lock script
++  lock-merkle-proof
  =<  form
  |%
  +$  form  [=spend-condition axis=@ =merk-proof:merkle]
  ++  based
    |=  =form
    ^-  ?
    ?&  (based:spend-condition spend-condition.form)
        (^based axis.form)
        (based:^hash root.merk-proof.form)
        (levy path.merk-proof.form based:^hash)
    ==
  ::
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    |^
    :+  hash+(hash:spend-condition spend-condition.form)
      hash+(from-b58:^hash '6mhCSwJQDvbkbiPAUNjetJtVoo1VLtEhmEYoU4hmdGd6ep1F6ayaV4A')
    (hashable-merk-proof merk-proof.form)
    ::
    ++  hashable-merk-proof
      |=  =merk-proof:merkle
      ^-  hashable:tip5
      :-  hash+root.merk-proof
      |-  ^-  hashable:tip5
      ?~  path.merk-proof
        leaf+~
      :-  hash+i.path.merk-proof
      $(path.merk-proof t.path.merk-proof)
    --
  ::
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  ::  note that the hash comes from the nname and thus must be
  ::  the hash of the merkle proof, paired with & (see v1-name:nname)
  ::
  ++  check
    |=  [=form parent-firstname=^hash]
    ^-  ?
    ?.  =(1 axis.form)
      ~>  %slog.[0 'axis must be 1 until a protocol upgrade that properly commits to lmp in witness']
      %.n
    =/  spend-firstname
      (hash-hashable:tip5 [leaf+& hash+root.merk-proof.form])
    ?.  =(spend-firstname parent-firstname)
      ~>  %slog.[0 'spend first name does not match parent note first name']
      %.n
    =/  leaf-hash  (hash:spend-condition spend-condition.form)
    =/  merk-verified
      (verify-merk-proof:merkle leaf-hash axis.form merk-proof.form)
    merk-verified
  --
::
::  $pkh: pay to public key hash
++  pkh
  =<  form
  |%
  +$  form  [m=@ h=(z-set ^hash)]
  ++  based
    |=  =form
    ^-  ?
    ?&  (^based m.form)
        %-  ~(all z-in h.form)
        based:^hash
    ==
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    |^
    [leaf+m.form (hashable-hashes h.form)]
    ::
    ++  hashable-hashes
      |=  hs=(z-set ^hash)
      ^-  hashable:tip5
      ?~  hs  leaf+hs
      :+  hash+n.hs
        $(hs l.hs)
      $(hs r.hs)
    --
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  ++  check
    |=  [=form ctx=check-context]
    ^-  ?
    ?&
    ::  correct number of signatures
      =(m.form ~(wyt z-by pkh.witness.ctx))
    ::  permissible public key hashes
      =(~ (~(dif z-in ~(key z-by pkh.witness.ctx)) h.form))
    ::  hashes match
      %-  ~(rep z-by pkh.witness.ctx)
      |=  [[h=^hash pk=schnorr-pubkey sig=schnorr-signature] a=?]
      ?&  a
          =(h (hash:schnorr-pubkey pk))
      ==
    ::  signatures valid
      %-  batch-verify:affine:belt-schnorr:cheetah
      (signatures:pkh-signature pkh.witness.ctx sig-hash.ctx)
    ==
  ::
  --
::
::  $hax: Hashlock
++  hax
  =<  form
  |%
  +$  form  (z-set ^hash)
  ++  based
    |=  =form
    %-  ~(all z-in form)
    based:^hash
  ++  hashable
    |=  =form
    ?~  form  leaf+~
    :*  hash+n.form
        $(form l.form)
        $(form r.form)
    ==
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  ++  hash-noun
    |=  n=*
    ^-  ^hash
    %-  hash-hashable:tip5
    |-  ^-  hashable:tip5
    ?^  n  [$(n -.n) $(n +.n)]
    leaf+n
  ++  check
    |=  [=form ctx=check-context]
    ^-  ?
    %-  ~(all z-in form)
    |=  =^hash
    =/  preimage  (~(get z-by hax.witness.ctx) hash)
    ?~  preimage  %|
    =(hash (hash-noun u.preimage))
  --  :: hax
::
::  $tim: timelock for lockscripts
++  tim
  =<  form
  |%
  +$  form
    $:  rel=[min=(unit page-number) max=(unit page-number)]
        abs=[min=(unit page-number) max=(unit page-number)]
    ==
  ++  based
    |=  =form
    ^-  ?
    ?&  ?~(min.rel.form %& (^based u.min.rel.form))
        ?~(max.rel.form %& (^based u.max.rel.form))
        ?~(min.abs.form %& (^based u.min.abs.form))
        ?~(max.abs.form %& (^based u.max.abs.form))
    ==
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    :-  :-  ?~(min.rel.form %leaf^~ [%leaf^~ leaf+u.min.rel.form])
            ?~(max.rel.form %leaf^~ [%leaf^~ leaf+u.max.rel.form])
        :-  ?~(min.abs.form %leaf^~ [%leaf^~ leaf+u.min.abs.form])
            ?~(max.abs.form %leaf^~ [%leaf^~ leaf+u.max.abs.form])
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  ++  check
    |=  [=form ctx=check-context]
    ^-  ?
    =/  rmin-ok=?
      ?~  min.rel.form  %.y
      (gte now.ctx (add since.ctx u.min.rel.form))
    =/  rmax-ok=?
      ?~  max.rel.form  %.y
      (lte now.ctx (add since.ctx u.max.rel.form))
    =/  amin-ok=?
      ?~  min.abs.form  %.y
      (gte now.ctx u.min.abs.form)
    =/  amax-ok=?
      ?~  max.abs.form  %.y
      (lte now.ctx u.max.abs.form)
    &(rmin-ok rmax-ok amin-ok amax-ok)
  --
::
::  $pkh-signature: pubkeys and signatures witnessing a spend of a %pkh
++  pkh-signature
  =<  form
  |%
  +$  form  (z-map ^hash [pk=schnorr-pubkey sig=schnorr-signature])
  ++  based
    |=  =form
    ^-  ?
    %-  ~(rep z-by form)
    |=  [[h=^hash val=[pk=schnorr-pubkey sig=schnorr-signature]] a=?]
    ?&  a
        (based:^hash h)
        (based:schnorr-pubkey pk.val)
        (based:schnorr-signature sig.val)
    ==
  ++  hashable
    |=  =form
    ^-  hashable:tip5
    |^
    ?~  form  leaf+form
    :+  [hash+p.n.form (hashable-val q.n.form)]
      $(form l.form)
    $(form r.form)
    ::
    ++  hashable-val
      |=  [pk=schnorr-pubkey sig=schnorr-signature]
      ^-  hashable:tip5
      [hash+(hash:schnorr-pubkey pk) (hashable:schnorr-signature sig)]
    --
  ++  hash
    |=  =form
    %-  hash-hashable:tip5
    (hashable form)
  ::
  ::  all the signatures in a form suitable for batch verification
  ++  signatures
    |=  [=form sig-hash=^hash]
    ^-  (list [schnorr-pubkey ^hash schnorr-signature])
    %-  ~(rep z-by form)
    |=  $:  [* pk=schnorr-pubkey sig=schnorr-signature]
            sigs=(list [schnorr-pubkey ^hash schnorr-signature])
        ==
    ^-  (list [schnorr-pubkey ^hash schnorr-signature])
    :_  sigs
    [pk sig-hash sig]
  --
::
::  $check-context: Context provided for validating locks
::
::    .now: current page height
::    .since: page height of the note
::    .sig-hash: signature to be hashed for a spend
::    .witness: witness to spend conditions
++  check-context
  =<  form
  |%
  +$  form
    $:  now=page-number
        since=page-number
        sig-hash=hash
        =witness
    ==
  ::
  ++  check
    |=  [=form lock=hash]
    ^-  ?
    ?&
    ::  check the merkle proof for the lock script
      (check:lock-merkle-proof lmp.witness.form lock)
    ::  check each primitive
      %+  levy  spend-condition.lmp.witness.form
      |=  p=lock-primitive
      ^-  ?
      ?-  -.p
        %tim  (check:tim +.p form)
        %hax  (check:hax +.p form)
        %pkh  (check:pkh +.p form)
        %brn  %|
      ==
    ==
  ::
  --
::
++  outputs
  =<  form
  |%
  +$  form  (z-set output)
  ::
  ++  validate
    |=  =form
    ^-  ?
    %+  levy  ~(tap z-in form)
    |=  out=output
    (validate:output out)
  --
::
++  tx
  =<  form
  |%
  +$  form  [%1 =raw-tx total-size=size =outputs]
  ::
  ++  validate
    |=  =form
    ^-  ?
    ?&  (validate:raw-tx raw-tx.form)
        (validate:outputs outputs.form)
        =(total-size.form ~(size get form))
    ==
  ::
  ++  new
    |=  [=raw-tx =page-number]
    ^-  form
    |^
      =/  =outputs  (build-outputs raw-tx)
      %*  .  *form
        raw-tx      raw-tx
        total-size  ~(size get:^raw-tx raw-tx)
        outputs     outputs
      ==
    ::
    ++  build-outputs
      |=  =^raw-tx
      ^-  outputs
      =/  spends-list=(list [nname spend])
        ~(tap z-by spends.raw-tx)
      =|  children=(z-map hash output)
      |-  ^-  outputs
      ?~  spends-list
        %-  ~(gas z-in *(z-set output))
        ~(val z-by children)
      =/  sp=spend  +.i.spends-list
      =/  sed-list=(list seed)
        ?-  -.sp
          %0  ~(tap z-in seeds.+.sp)
          %1  ~(tap z-in seeds.+.sp)
        ==
      =.  children
        %+  roll  sed-list
        |=  [sed=seed acc=_children]
        =/  key=hash  lock-root.sed
        =/  mchild=(unit output)  (~(get z-by acc) key)
        ?^  mchild
          =*  child  u.mchild
          =/  new-seeds=seeds  (~(put z-in seeds.child) sed)
          =/  new-assets=coins  (add assets.note.child gift.sed)
          ::  normalize: strip output-source before hashing to ensure consistent
          ::  tree structure
          =/  normalized-seeds=seeds
            %-  ~(gas z-in *seeds)
            %+  turn  ~(tap z-in new-seeds)
            |=(s=seed s(output-source ~))
          =/  src-hash=hash  (hash:seeds normalized-seeds)
          =/  src=source  [src-hash %.n]
          ~|  "build-outputs: v0 note detected"
          ?>  ?=(@ -.note.child)
          =/  updated-child=output
            :_  new-seeds
            %=  note.child
              assets  new-assets
              name    (new-v1:nname [lock-root.sed src])
            ==
          (~(put z-by acc) key updated-child)
        =/  single=seeds  (~(put z-in *seeds) sed)
        ::  normalize: strip output-source before hashing to ensure consistent
        ::  tree structure
        =/  normalized-single=seeds
          (~(put z-in *seeds) sed(output-source ~))
        =/  sh=hash  (hash:seeds normalized-single)
        =/  src=source  [sh %.n]
        =/  note1=nnote-1
          %*  .  *nnote-1
            version      %1
            origin-page  page-number
            name         (new-v1:nname [lock-root.sed src])
            note-data    note-data.sed
            assets       gift.sed
          ==
        =/  out=output  [note1 single]
        (~(put z-by acc) key out)
      $(spends-list t.spends-list)
    --
  ::
  ++  get
    |_  =form
    ++  id
      ^-  tx-id
      id.raw-tx.form
    ::
    ++  total-fees
      ^-  coins
      (roll-fees:spends spends.raw-tx.form)
    ::
    ++  size  ~(size get:raw-tx raw-tx.form)
    --
  --
--
