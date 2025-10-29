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
    ==
|^
^-  spends:v1:transact
=/  notes=(list nnote:transact)  (turn names get-note)
::  TODO: unify functions across versions. There's too much repetition
=/  =spends:v1:transact
  ?:  =(orders ~)
    ~|("Cannot create a transaction with empty order" !!)
  ?:  (lien orders |=([recipient=hash:transact gift=coins:transact] (lte gift 0)))
    ~|("Cannot create a transaction with zero gift" !!)
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
    (create-spends-0 notes)
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
    (create-spends-1 notes)
  ::
  ::  I don't want to do this, but the fact that we're constrained to a single master seckey
  ::  means no mixing versions in single spends.
  ::
  ~>  %slog.[0 'Notes must all be the same version!!!']  !!
=+  min-fee=(calculate-min-fee:spends:transact spends)
?:  (lth fee min-fee)
  ~|("Min fee not met. This transaction requires at least: {(trip (format-ui:common:display:utils min-fee))} nicks" !!)
spends
::
++  create-spends-0
  |=  notes=(list nnote:v0:transact)
  =;  [=spends:v1:transact remaining=[fee=@ orders=(list order:wt)]]
    ?.  ?&  =(~ orders.remaining)
            =(0 fee.remaining)
        ==
      ~>  %slog.[0 'Insufficient funds to pay fee and gift']  !!
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
  ?:  =(~ specs)
    [spends [fee=new-fee orders=new-orders]]
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
      ~>  %slog.[0 'Insufficient funds to pay fee and gift']  !!
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
  =+  simple-pkh=[%pkh [m=1 (z-silt:zo ~[pkh])]]
  =/  coinbase-lock=spend-condition:transact  ~[simple-pkh tim-lp:coinbase:transact]
  =/  input-lock=(reason:transact lock:transact)
    ::  if there is no lock noun, default to coinbase lock
    ?~  lok-noun=(~(get z-by:zo u.nd) %lock)
      [%.y coinbase-lock]
    ?~  parent-lock=((soft lock-data:wt) u.lok-noun)
      ~>  %slog.[0 'error: lock-data malformed in note!']  !!
    ::  more than one spend condition
    ?@  -.lock.u.parent-lock
      [%.n 'lock has multiple spend conditions, we are not supporting this at the moment']
    ?:  (gth (lent lock.u.parent-lock) 1)
      [%.n 'lock is a single spend-condition with more than one predicate']
    ::
    ::  Grab a single condition off of the lock,
    ::  check that it is a pkh condition that it is spendable
    =/  lp=lock-primitive:v1:transact  (snag 0 `spend-condition:transact`lock.u.parent-lock)
    ?.  ?=(%pkh -.lp)  [%.n 'lock is not a pkh lock']
    ?.  ?&  =(1 m.lp)
            (~(has z-in:zo h.lp) pkh)
        ==
      [%.n 'lock has a spend-condition for more than on predicate']
    [%.y lock.u.parent-lock]
  ?:  ?=(%.n -.input-lock)
    ~>  %slog.[0 "Error processing note {(trip (name:v1:display:utils name.note))}"]  !!
  :: fan-out gifts + fee for this v1 note (reuse shared gates)
  =/  res  (allocate-from-note orders.remaining note assets.note fee.remaining)
  =/  [new-orders=(list order:wt) specs=(list order:wt) new-fee=@]  res
  :: skip if no seeds would be created (protocol requires >=1 seed)
  ?:  =(~ specs)
    [spends [fee=new-fee orders=new-orders]]
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
    =/  pkh=hash:transact  (hash:schnorr-pubkey:transact pubkey)
    [%pkh [m=1 (z-silt:zo ~[pkh])]]
  =/  lok=lock:transact  ~[refund-lp]
  =/  =note-data:v1:transact
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
    =/  rem2=@  (sub rem.acc take)
    =/  out2=(list order:wt)
      ?:  =(0 take)  out.acc
      [[recipient=recipient.ord gift=take] out.acc]
    =/  ords2=(list order:wt)
      ?:  =(take gift.ord)
        remaining-orders.acc
      [[recipient=recipient.ord gift=(sub gift.ord take)] remaining-orders.acc]
    [remaining-orders=ords2 out=out2 rem=rem2]
  :: pay fee from remainder (post-gift), then refund any tail
  =/  fee-portion=@   (min fee rem)
  =/  fee2=@          (sub fee fee-portion)
  =/  rem2=@          (sub rem fee-portion)
  =?  out  (gth rem2 0)
    =/  refund-recipient=hash:transact
      ?^  refund-pkh
        u.refund-pkh
      (hash:schnorr-pubkey:transact pubkey)
    [[recipient=refund-recipient gift=rem2] out]
  [remaining-orders out fee2]
:: Build v1 seeds from (recipient,gift) specs for a given input note (any version).
++  seeds-from-specs
  |=  [specs=(list order:wt) note=nnote:transact fee-portion=@]
  ^-  seeds:v1:transact
  =/  [seeds=(list seed:v1:transact) gifts=@]
  %+  roll  specs
  |=  $:  spec=order:wt
          _acc=[seeds=`(list seed:v1:transact)`~ gifts=0]
      ==
  =/  output-lock=lock:transact
    [%pkh [m=1 (z-silt:zo ~[recipient.spec])]]~
  =/  =note-data:v1:transact
    %-  ~(put z-by:zo *note-data:v1:transact)
    [%lock ^-(lock-data:wt [%0 output-lock])]
  :_  (add gifts.acc gift.spec)
  :_  seeds.acc
  :*  output-source=~
      lock-root=(hash:lock:transact output-lock)
      note-data
      gift=gift.spec
      parent-hash=(hash:nnote:transact note)
  ==
  ~|  "assets in must equal gift + fee + refund"
  ?>  =(assets.note (add gifts fee-portion))
  %-  z-silt:zo
  seeds
--
