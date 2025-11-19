/=  transact  /common/tx-engine
/=  utils  /apps/wallet/lib/utils
/=  wt  /apps/wallet/lib/types
/=  zo  /common/zoon
::
::  Builds a simple fan-in transaction
|=  $:  names=(list nname:transact)
        orders=(list order:wt)
        fee=coins:transact
        sign-key=schnorr-seckey:transact
        pubkey=schnorr-pubkey:transact
        refund-pkh=(unit hash:transact)
        get-note=$-(nname:transact nnote:transact)
        include-data=?
    ==
|^
^-  [spends:v1:transact hash:transact]
?:  (lien orders |=(ord=order:wt =(0 gift.ord)))
  ~|('Cannot create a transaction with zero gift!' !!)
?:  =(orders ~)
  ~|("Cannot create a transaction with empty order" !!)
=/  sender-pkh=hash:transact  (hash:schnorr-pubkey:transact pubkey)
=/  notes=(list nnote:transact)  (turn names get-note)
::  TODO: unify functions across versions. There's too much repetition
=/  [=spends:v1:transact =hash:transact]
  ::  If all notes are v0
  ?:  (levy notes |=(=nnote:transact ?=(^ -.nnote)))
    ?~  refund-pkh
      ~|('Need to specify a refund address if spending from v0 notes. Use the `--refund-pkh` flag in the create-tx command' !!)
    =/  notes=(list nnote:v0:transact)
      %+  turn  notes
      |=  =nnote:transact
      ?>  ?=(^ -.nnote)
      nnote
    =.  notes
      %+  sort  notes
      |=  [a=nnote:v0:transact b=nnote:v0:transact]
      (gth assets.a assets.b)
    [(create-spends-0 notes) u.refund-pkh]
  ::  If all notes are v1
  ?:  (levy notes |=(=nnote:transact ?=(@ -.nnote)))
    =/  notes=(list nnote-1:v1:transact)
      %+  turn  notes
      |=  =nnote:transact
      ?>  ?=(@ -.nnote)
      nnote
    =.  notes
      %+  sort  notes
      |=  [a=nnote-1:v1:transact b=nnote-1:v1:transact]
      (gth assets.a assets.b)
    ::  If a refund-pkh is passed in, use that. Otherwise, default to pkh
    =/  refund-pkh=hash:transact  (fall refund-pkh sender-pkh)
    [(create-spends-1 notes) refund-pkh]
  ::
  ::  I don't want to do this, but the fact that we're constrained to a single master seckey
  ::  means no mixing versions in single spends.
  ::
  ~>  %slog.[0 'Notes must all be the same version!!!']  !!
=+  min-fee=(calculate-min-fee:spends:transact spends)
?:  (lth fee min-fee)
  ~|("Min fee not met. This transaction requires at least: {(trip (format-ui:common:display:utils min-fee))} nicks" !!)
[spends hash]
::
++  create-spends-0
  |=  notes=(list nnote:v0:transact)
  =;  [=spends:v1:transact remaining=[fee=@ orders=(list order:wt)]]
    ?.  ?&  =(~ orders.remaining)
            =(0 fee.remaining)
        ==
      ~|('Insufficient funds to pay fee and gift' !!)
    spends
  %+  roll  notes
  |=  $:  note=nnote:v0:transact
          =spends:v1:transact
          remaining=_[fee=fee orders=orders]
      ==
  ?.  ?&  =(1 m.sig.note)
          (~(has z-in:zo pubkeys.sig.note) pubkey)
      ==
      ~>  %slog.[0 'Note not spendable by signing key']  !!
  =/  res  (allocate-from-note orders.remaining note assets.note fee.remaining)
  =/  [new-orders=(list order:wt) specs=(list order:wt) new-fee=@]  res
  :: skip if no seeds would be created (protocol requires >=1 seed)
  :: do not update fees or orders
  ?:  =(~ specs)
    [spends remaining]
  =/  fee-portion=@  (sub fee.remaining new-fee)
  :: turn specs (recipient,gift) into v1 seeds
  =/  =seeds:v1:transact  (seeds-from-specs specs note fee-portion)
  ?~  seeds
    ~|('No seeds were provided' !!)
  =/  spend=spend-0:v1:transact
    %*  .  *spend-0:v1:transact
      seeds  seeds
      fee    fee-portion
    ==
  :_  [fee=new-fee orders=new-orders]
  %-  ~(put z-by:zo spends)
  [name.note (sign:spend-v1:transact [%0 spend] sign-key)]
::
++  create-spends-1
  |=  notes=(list nnote-1:v1:transact)
  =;  [=spends:v1:transact remaining=[fee=@ orders=(list order:wt)]]
    ?.  ?&  =(~ orders.remaining)
            =(0 fee.remaining)
        ==
      ~|('Insufficient funds to pay fee and gift' !!)
    spends
  =/  pkh=hash:transact  (hash:schnorr-pubkey:transact pubkey)
  %+  roll  notes
  |=  $:  note=nnote-1:v1:transact
          =spends:v1:transact
          remaining=_[fee=fee orders=orders]
      ==
  =/  nd=(unit note-data:v1:transact)  ((soft note-data:v1:transact) note-data.note)
  ?~  nd
    ~>  %slog.[0 'error: note-data malformed in note!']  !!
 =/  coinbase-lock=spend-condition:transact  (coinbase-pkh-sc:v1:first-name:transact pkh)
 =/  input-lock=(reason:transact lock:transact)
  ::  if there is no lock noun, default to coinbase lock
  ?~  parent-lock=(pull-lock:locks:utils [u.nd name.note (some pkh)])
    [%.n 'the first name of the note did not correspond to a simple-pkh or coinbase']
  [%.y u.parent-lock]
  ?:  ?=(%.n -.input-lock)
    =+  name-cord=(name:v1:display:utils name.note)
    ~&  "Error processing note {<name-cord>}. Reason: {(trip p.input-lock)}."  !!
  :: fan-out gifts + fee for this v1 note (reuse shared gates)
  =/  res  (allocate-from-note orders.remaining note assets.note fee.remaining)
  =/  [new-orders=(list order:wt) specs=(list order:wt) new-fee=@]  res
  :: skip if no seeds would be created (protocol requires >=1 seed)
  ?:  =(~ specs)
    [spends remaining]
  =/  fee-portion=@  (sub fee.remaining new-fee)
  :: build v1 seeds from specs (recipient,gift)
  =/  =seeds:v1:transact  (seeds-from-specs specs note fee-portion)
  ?~  seeds
    ~|('No seeds were provided' !!)
  :: prove input lock and emit spend-1 with fee-portion
  =/  lmp=lock-merkle-proof:transact
    (build-lock-merkle-proof:lock:transact p.input-lock 1)
  =/  spend=spend-1:v1:transact
    %*  .  *spend-1:v1:transact
      seeds  seeds
      fee    fee-portion
    ==
  =.  witness.spend
    %*  .  *witness:transact
      lmp  lmp
    ==
  :_  [fee=new-fee orders=new-orders]
  %-  ~(put z-by:zo spends)
  [name.note (sign:spend-v1:transact [%1 spend] sign-key)]
::
++  create-refund
  |=  [note=nnote:transact refund=@]
  ^-  seed:v1:transact
  =/  refund-lp=lock-primitive:transact
    ?^  refund-pkh
      [%pkh [m=1 (z-silt:zo ~[u.refund-pkh])]]
    =/  sender-pkh=hash:transact  (hash:schnorr-pubkey:transact pubkey)
    [%pkh [m=1 (z-silt:zo ~[sender-pkh])]]
  =/  lok=lock:transact  ~[refund-lp]
  =/  =note-data:v1:transact
    ?.  include-data
      ~
    %-  ~(put z-by:zo *note-data:v1:transact)
    [%lock ^-(lock-data:wt [%0 lok])]
  :*  output-source=~
      lock-root=(hash:lock:transact lok)
      note-data
      gift=refund
      parent-hash=(hash:nnote:transact note)
  ==
++  allocate-from-note
  |=  [orders=(list order:wt) note=nnote:transact assets=@ fee=@]
  ^-  [orders=(list order:wt) seeds=(list order:wt) fee=@]
  :: fill gifts greedily left-to-right
  =/  [remaining-orders=(list order:wt) out=(list order:wt) rem=@]
    %+  roll  orders
    |=  $:  ord=order:wt
            acc=_[remaining-orders=`(list order:wt)`~ out=`(list order:wt)`~ rem=`@`assets]
        ==
    ?:  =(0 rem.acc)
      [remaining-orders=[ord remaining-orders.acc] out=out.acc rem=rem.acc]
    =/  take=@  (min gift.ord rem.acc)
    =.  rem.acc  (sub rem.acc take)
    =.  out.acc  [[recipient=recipient.ord gift=take] out.acc]
    =?  remaining-orders.acc  (lth take gift.ord)
      [[recipient=recipient.ord gift=(sub gift.ord take)] remaining-orders.acc]
    acc
  :: pay fee from remainder (post-gift), then refund any tail
  =/  fee-portion=@   (min fee rem)
  =/  refund=@        (sub rem fee-portion)
  =?  out  (gth refund 0)
    =/  refund-recipient=hash:transact
      ?^  refund-pkh
        u.refund-pkh
      (hash:schnorr-pubkey:transact pubkey)
    [[recipient=refund-recipient gift=refund] out]
  ::  emit remaining orders, outputs which will translate into seeds, and remaining fee
  [remaining-orders out (sub fee fee-portion)]
:: Build v1 seeds from (recipient,gift) specs for a given input note (any version).
++  seeds-from-specs
  |=  $:  specs=(list order:wt)
          note=nnote:transact
          fee-portion=@
      ==
  ^-  seeds:v1:transact
  =/  [seeds=(list seed:v1:transact) gifts=@]
  %+  roll  specs
  |=  $:  spec=order:wt
          _acc=[seeds=`(list seed:v1:transact)`~ gifts=0]
      ==
  =/  output-lock=lock:transact
    [%pkh [m=1 (z-silt:zo ~[recipient.spec])]]~
  =/  nd=note-data:v1:transact
    ?.  include-data
      ~
    %-  ~(put z-by:zo *note-data:v1:transact)
    [%lock ^-(lock-data:wt [%0 output-lock])]
  :_  (add gifts.acc gift.spec)
  :_  seeds.acc
  :*  output-source=~
      lock-root=(hash:lock:transact output-lock)
      note-data=nd
      gift=gift.spec
      parent-hash=(hash:nnote:transact note)
  ==
  ~|  "assets in must equal gift + fee + refund"
  ?>  =(assets.note (add gifts fee-portion))
  %-  z-silt:zo
  seeds
--
