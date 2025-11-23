/=  transact  /common/tx-engine
/=  utils  /apps/wallet/lib/utils
/=  wt  /apps/wallet/lib/types
/=  zo  /common/zoon
/=  *  /common/zose
::
::  Builds a fan-in transaction that can emit both simple PKH and multisig locks.
|=  $:  names=(list nname:transact)
        orders=(list order:wt)
        fee=coins:transact
        sign-keys=(list schnorr-seckey:transact)
        refund-pkh=(unit hash:transact)
        get-note=$-(nname:transact nnote:transact)
        memo-data=(list @ux)
        include-data=?
    ==
|^
^-  $:  spends:v1:transact
        witness-data:wt
        display=transaction-display:wt
    ==
=+  orders-valid=(orders-valid orders)
?:  ?=(%.n -.orders-valid)
  ~|("One or more orders are invalid. Reason: {<p.orders-valid>}" !!)
=/  signer-pubkeys=(list schnorr-pubkey:transact)
  %+  turn  sign-keys
  |=  sk=schnorr-seckey:transact
  %-  from-sk:schnorr-pubkey:transact
  (to-atom:schnorr-seckey:transact sk)
?~  signer-pubkeys
  ~|("At least one signing key is required" !!)
=/  sender-pubkey=schnorr-pubkey:transact  i.signer-pubkeys
=/  sender-pkh=hash:transact  (hash:schnorr-pubkey:transact sender-pubkey)
=/  notes=(list nnote:transact)  (turn names get-note)
::  If all notes are v0
=/  [raw-spends=spends:v1:transact =witness-data:wt display=transaction-display:wt]
  ?:  (levy notes |=(=nnote:transact ?=(^ -.nnote)))
    ?~  refund-pkh
      ~|('Need to specify a refund address if spending from v0 notes. Use the `--refund-pkh` flag in the create-tx command' !!)
    =/  notes-v0=(list nnote:v0:transact)
      %+  turn  notes
      |=  =nnote:transact
      ?>  ?=(^ -.nnote)
      nnote
    =.  notes-v0
      %+  sort  notes-v0
      |=  [a=nnote:v0:transact b=nnote:v0:transact]
      (gth assets.a assets.b)
    =/  refund-lock=lock:transact  [%pkh [m=1 (z-silt:zo ~[u.refund-pkh])]]~
    (create-spends-0 notes-v0 orders fee sender-pubkey refund-lock)
  ::  If all notes are v1
  ?:  (levy notes |=(=nnote:transact ?=(@ -.nnote)))
    =/  notes-v1=(list nnote-1:v1:transact)
      %+  turn  notes
      |=  =nnote:transact
      ?>  ?=(@ -.nnote)
      nnote
    =.  notes-v1
      %+  sort  notes-v1
      |=  [a=nnote-1:v1:transact b=nnote-1:v1:transact]
      (gth assets.a assets.b)
    =/  multisig-lock=(unit lock:transact)
      ::
      ::  ensure that all multisig locks are the same in the input notes
      |-
      ?~  notes-v1  ~
      ?^  lok=(multisig-lock i.notes-v1)
        =+  ref-fn=(first:nname:v1:transact (hash:lock:transact u.lok))
        ?:  %+  levy  `(list nnote-1:v1:transact)`notes-v1
            |=  note=nnote-1:v1:transact
            =(ref-fn ~(first-name get:nnote:transact note))
          lok
        ~|('Multisig detected in input. When a multisig is present, all inputs must share the same lock.' !!)
      $(notes-v1 t.notes-v1)
    =/  refund-lock=lock:transact
      ?^  refund-pkh
        [%pkh [m=1 (z-silt:zo ~[u.refund-pkh])]]~
      %+  fall  multisig-lock
      [%pkh [m=1 (z-silt:zo ~[sender-pkh])]]~
    (create-spends-1 notes-v1 orders fee sender-pkh refund-lock)
::
~>  %slog.[0 'Notes must all be the same version!!!']  !!

:: add memo
=/  final-spends=spends:v1:transact
  ?~(memo-data raw-spends (apply-memo-to-spends raw-spends))

:: calculate fee based on final spends (with memo if present)
=+  min-fee=(spends:estimate-fee:utils final-spends inputs.display)
:: uncomment to debug out of band fee estimation
:: =+  min-fee-ref=(calculate-min-fee:spends:transact (apply:witness-data:wt witness-data raw-spends))
:: ~&  min-fee-est+min-fee
:: ~&  min-fee-ref+min-fee-ref
?:  (lth fee min-fee)
  ~|("Min fee not met. This transaction requires at least: {(trip (format-ui:common:display:utils min-fee))} nicks" !!)
  [final-spends witness-data display]
::
::  helpers for building display metadata
::
++  update-display-0
  |=  $:  note=nnote:v0:transact
          display=transaction-display:wt
          addition=output-lock-map:wt
      ==
  ^-  transaction-display:wt
  ?>  ?=(%0 -.inputs.display)
  %=    display
      outputs
    (~(uni z-by:zo outputs.display) addition)
  ::
      inputs
    :-  %0
    %-  ~(put z-by:zo p.inputs.display)
    [name.note sig.note]
  ==
::
++  update-display-1
  |=  $:  name=nname:transact
          display=transaction-display:wt
          addition=output-lock-map:wt
          =lock:transact
      ==
  ^-  transaction-display:wt
  ?>  ?=(%1 -.inputs.display)
  %=    display
      outputs
    (~(uni z-by:zo outputs.display) addition)
  ::
      inputs
    :-  %1
    %-  ~(put z-by:zo p.inputs.display)
    ::  assert that the lock is a spend-condition
    ?>  ?=(^ -.lock)
    [name lock]
  ==
::
++  create-spends-0
  |=  $:  notes=(list nnote:v0:transact)
          orders=(list order:wt)
          fee=@
          pubkey=schnorr-pubkey:transact
          refund-lock=lock:transact
      ==
  ^-  [=spends:v1:transact witness-data:wt transaction-display:wt]
  =/  initial-state=spend-build-state:wt
    %*  .  *spend-build-state:wt
      fee      fee
      orders   orders
      wd       [%0 ~]
      display  [[%0 ~] ~]
    ==
  =/  final-state
    (process-spends-0 notes initial-state pubkey refund-lock)
  =+  remaining-orders=orders.final-state
  =+  remaining-fee=fee.final-state
  ?.  ?&  =(~ remaining-orders)
          =(0 remaining-fee)
      ==
    ~|('Insufficient funds to pay fee and gift' !!)
  [spends.final-state wd.final-state display.final-state]
::
++  process-spends-0
  |=  $:  notes=(list nnote:v0:transact)
          state=spend-build-state:wt
          pubkey=schnorr-pubkey:transact
          refund-lock=lock:transact
      ==
  ^-  spend-build-state:wt
  ?~  notes
    state
  =/  note  i.notes
  ?.  ?&  =(1 m.sig.note)
          (~(has z-in:zo pubkeys.sig.note) pubkey)
      ==
    ~>  %slog.[0 'Note not spendable by signing key']  !!
  =/  [pending-orders=(list order:wt) specs=(list order:wt) remainder=@]
    (allocate-orders orders.state assets.note)
  =/  fee-portion=@  (min fee.state remainder)
  =/  new-fee=@  (sub fee.state fee-portion)
  =/  refund=@  (sub remainder fee-portion)
  =?  specs  !=(refund 0)
    [(build-refund-order refund refund-lock) specs]
  ?:  =(~ specs)
    %=  $
      notes   t.notes
    ==
  =/  [=seeds:v1:transact output-map=output-lock-map:wt]
    (seeds-from-specs specs note fee-portion)
  ?~  seeds
    ~|('No seeds were provided' !!)
  =/  spend=spend-0:v1:transact
    %*  .  *spend-0:v1:transact
      seeds  seeds
      fee    fee-portion
    ==
  %=  $
    notes          t.notes
    spends.state   (~(put z-by:zo spends.state) [name.note [%0 spend]])
    fee.state      new-fee
    orders.state   pending-orders
    display.state  (update-display-0 note display.state output-map)
    wd.state       (sign-spend name.note [%0 spend] wd.state)
  ==
::
++  create-spends-1
  |=  $:  notes=(list nnote-1:v1:transact)
          orders=(list order:wt)
          fee=@
          sender-pkh=hash:transact
          refund-lock=lock:transact
      ==
  ^-  [=spends:v1:transact witness-data:wt transaction-display:wt]
  =/  initial-state=spend-build-state:wt
    %*  .  *spend-build-state:wt
      fee      fee
      orders   orders
      wd       [%1 ~]
      display  [[%1 ~] ~]
    ==
  =/  final-state
    (process-spends-1 notes initial-state sender-pkh refund-lock)
  =+  remaining-orders=orders.final-state
  =+  remaining-fee=fee.final-state
  ?.  ?&  =(~ remaining-orders)
          =(0 remaining-fee)
      ==
    ~|('Insufficient funds to pay fee and gift' !!)
  [spends.final-state wd.final-state display.final-state]
::
++  apply-memo-to-spends
  |=  [=spends:v1:transact]
  %-  ~(gas z-by:zo *spends:v1:transact)
  %+  turn  ~(tap z-by:zo spends)
  |=  [name=nname:transact spend=spend:v1:transact]
  =/  updated-spend=spend:v1:transact
    ?-  -.spend
      %0  [%0 +.spend(seeds (apply-memo seeds.+.spend))]
      %1  [%1 +.spend(seeds (apply-memo seeds.+.spend))]
    ==
  [name updated-spend]
::
++  apply-memo
  |=  =seeds:v1:transact
  =/  seeds-list=(list seed:v1:transact)  ~(tap z-in:zo seeds)
  ::  Sort seeds by gift amount descending and pick the first (largest)
  =/  sorted-seeds=(list seed:v1:transact)
    %+  sort  seeds-list
    |=  [a=seed:v1:transact b=seed:v1:transact]
    (gth gift.a gift.b)
  =/  memo-seed=seed:v1:transact  (snag 0 sorted-seeds)
  =/  updated-note-data=note-data:v1:transact
    (~(put z-by:zo note-data.memo-seed) %memo ^-(memo-data:wt memo-data))
  =/  updated-seed=seed:v1:transact
    memo-seed(note-data updated-note-data)
  ::  Reconstruct the seeds set with the updated seed at the beginning (largest assets)
  =/  rest-seeds=(list seed:v1:transact)  (slag 1 sorted-seeds)
  (~(gas z-in:zo *(z-set:zo seed:v1:transact)) [updated-seed rest-seeds])
::
++  process-spends-1
  |=  $:  notes=(list nnote-1:v1:transact)
          state=spend-build-state:wt
          sender-pkh=hash:transact
          refund-lock=lock:transact
      ==
  ^-  spend-build-state:wt
  ?~  notes
    state
  =/  note  i.notes
  =/  nd=(unit note-data:v1:transact)
    ((soft note-data:v1:transact) note-data.note)
  ?~  nd
    ~>  %slog.[0 'error: note-data malformed in note!']  !!
  =+  pulled=(pull-lock:locks:utils [u.nd name.note (some sender-pkh)])
  ?~  pulled
    =+  name-cord=(name:v1:display:utils name.note)
    ~|  "Error processing note {<name-cord>}. Reason: first-name did not correspond to a supported lock."  !!
  =/  input-lock=lock:transact  u.pulled
  =/  allocation  (allocate-orders orders.state assets.note)
  =/  [pending-orders=(list order:wt) specs=(list order:wt) remainder=@]
    allocation
  =/  fee-portion=@  (min fee.state remainder)
  =/  new-fee=@  (sub fee.state fee-portion)
  =/  refund=@  (sub remainder fee-portion)
  =/  specs-with-refund=(list order:wt)
    ?:  =(refund 0)
      specs
    [(build-refund-order refund refund-lock) specs]
  ?:  =(~ specs-with-refund)
    $(notes t.notes)
  =/  [=seeds:v1:transact output-map=output-lock-map:wt]
    (seeds-from-specs specs-with-refund note fee-portion)
  ?~  seeds
    ~|('No seeds were provided' !!)
  =/  lmp=lock-merkle-proof:transact
    (build-lock-merkle-proof:lock:transact input-lock 1)
  =/  spend=spend-1:v1:transact
    %*  .  *spend-1:v1:transact
      seeds  seeds
      fee    fee-portion
    ==
  =.  witness.spend
    %*  .  *witness:transact
      lmp  lmp
    ==
  %=  $
    notes          t.notes
    spends.state   (~(put z-by:zo spends.state) [name.note [%1 spend]])
    fee.state      new-fee
    orders.state   pending-orders
    display.state  (update-display-1 name.note display.state output-map input-lock)
    wd.state       (sign-spend name.note [%1 spend] wd.state)
  ==
++  sign-spend
  |=  [name=nname:transact =spend:v1:transact wd=witness-data:wt]
  ^-  witness-data:wt
  ?-    -.spend
      %0
    ?>  ?=(%0 -.wd)
    :-  %0
    %+  ~(put z-by:zo p.wd)
      name
    =+  sig-hash=(sig-hash:spend:v1:transact spend)
    %+  roll  sign-keys
    |=  $:  sk=schnorr-seckey:transact
            acc=_signature.spend
        ==
    (sign:signature:transact acc sk sig-hash)
  ::
      %1
    ?>  ?=(%1 -.wd)
    :-  %1
    %+  ~(put z-by:zo p.wd)
      name
    =+  sig-hash=(sig-hash:spend:v1:transact spend)
    %+  roll  sign-keys
    |=  $:  sk=schnorr-seckey:transact
            acc=_witness.spend
        ==
    (sign:witness:transact acc sk sig-hash)
  ==
::
++  allocate-orders
  |=  [orders=(list order:wt) assets=@]
  ^-  [orders=(list order:wt) specs=(list order:wt) remainder=@]
  %+  roll  orders
  |=  $:  ord=order:wt
          next-orders=(list order:wt)
          out-orders=(list order:wt)
          rem=_assets
      ==
  ?:  =(0 rem)
    [[ord next-orders] out-orders rem]
  =/  gift-out  (order-gift ord)
  =/  take=@  (min gift-out rem)
  =.  rem  (sub rem take)
  =.  out-orders  [(with-gift ord take) out-orders]
  =?  next-orders  (lth take gift-out)
    [(with-gift ord (sub gift-out take)) next-orders]
  [next-orders out-orders rem]
::
++  seeds-from-specs
  |=  $:  specs=(list order:wt)
          note=nnote:transact
          fee-portion=@
      ==
  ^-  [seeds:v1:transact output-lock-map:wt]
  =;  [seeds=(list seed:v1:transact) total-gifts=@ =output-lock-map:wt]
    ~|  "assets in must equal gift + fee + refund"
    ?>  =(assets.note (add total-gifts fee-portion))
    [(z-silt:zo seeds) output-lock-map]
  %+  roll  specs
  |=  $:  spec=order:wt
          seeds=(list seed:v1:transact)
          gifts=@
          =output-lock-map:wt
      ==
  =/  output-lock=lock:transact  (order-lock spec)
  =?  include-data  ?=(%multisig -.spec)
    %.y
  =/  =note-data:v1:transact
    ?:  include-data
      (~(put z-by:zo *note-data:v1:transact) %lock ^-(lock-data:wt [%0 output-lock]))
    ~
  =/  seed=seed:v1:transact
    :*  output-source=~
        lock-root=(hash:lock:transact output-lock)
        note-data
        gift=(order-gift spec)
        parent-hash=(hash:nnote:transact note)
    ==
  =/  metadata=lock-metadata:wt
    [output-lock include-data]
  :*  [seed seeds]
      (add gifts (order-gift spec))
      %-  ~(put z-by:zo output-lock-map)
      [(first:nname:transact lock-root.seed) metadata]
  ==
::
++  orders-valid
  |=  orders=(list order:wt)
  ^-  (reason:transact ~)
  ?:  =(0 (lent orders))
    [%.n 'cannot create transaction with no orders']
  |-
  ?~  orders
    [%.y ~]
  =/  ord=order:wt  i.orders
  ?-    -.ord
      %pkh
    ?:  =(0 gift.ord)
      [%.n %gift-cannot-be-zero]
    $(orders t.orders)
  ::
      %multisig
    =/  participants=(list hash:transact)  participants.ord
    =/  unique=@ud
      ~(wyt z-in:zo (z-silt:zo participants))
    ?:  =(participants ~)
      [%.n 'Multisig order must include at least one participant']
    ?:  (lte threshold.ord 0)
      [%.n 'Multisig threshold must be greater than zero']
    ?:  (gth threshold.ord (lent participants))
      [%.n 'Multisig threshold cannot exceed number of participants']
    ?:  (lth unique (lent participants))
      [%.n 'Multisig participants must be unique']
    ?:  =(0 gift.ord)
      [%.n 'order must include a gift greater than 0']
    $(orders t.orders)
  ==
::
++  order-gift
  |=  ord=order:wt
  ^-  coins:transact
  ?-    -.ord
      %pkh       gift.ord
      %multisig  gift.ord
    ==
::
++  with-gift
  |=  [ord=order:wt gift=coins:transact]
  ^-  order:wt
  ?-    -.ord
      %pkh       [%pkh recipient=recipient.ord gift=gift]
      %multisig  [%multisig threshold=threshold.ord participants=participants.ord gift=gift]
    ==
::
++  order-lock
  |=  ord=order:wt
  ^-  lock:transact
  ?-    -.ord
      %pkh
        [%pkh [m=1 (z-silt:zo ~[recipient.ord])]]~
      %multisig
        =/  participants=(list hash:transact)  participants.ord
        =/  allowed=(z-set:zo hash:transact)  (z-silt:zo participants)
        [%pkh [m=threshold.ord allowed]]~
    ==
::
++  order-from-lock
  |=  [lok=lock:transact gift=@]
  ^-  (unit order:wt)
  ?@  -.lok  ~
  =/  primitive=lock-primitive:transact  i.lok
  ?.  ?=(%pkh -.primitive)
    ~
  =/  threshold=@  m.primitive
  =/  allowed=(z-set:zo hash:transact)  h.primitive
  =/  participants=(list hash:transact)  ~(tap z-in:zo allowed)
  ?~  participants
    ~|('Invalid lock, no participants specified.' !!)
  ?:  &(=(threshold 1) =(1 (lent participants)))
    (some [%pkh recipient=i.participants gift=gift])
  (some [%multisig threshold=threshold participants=participants gift=gift])
::
++  build-refund-order
  |=  [refund=@ refund-lock=lock:transact]
  ^-  order:wt
  ?~  parsed=(order-from-lock [refund-lock refund])
    ~|('Unsupported owner lock for refund; please specify --refund-pkh' !!)
  u.parsed
::
++  multisig-lock
 |=  note=nnote-1:v1:transact
 ^-  (unit lock:transact)
 ?~  lock-noun=(~(get z-by:zo note-data.note) %lock)
   ~
 ?~  soft-lock=((soft lock-data:wt) u.lock-noun)
   ~>  %slog.[0 'lock data in note is malformed']  ~
 =+  pulled=lock.u.soft-lock
 ?@  -.pulled
   ~
 ?:  !=(1 (lent pulled))
   ~
 =/  lp=lock-primitive:transact  -.pulled
 ?.  ?=(%pkh -.lp)
   ~
 ?:  =(1 ~(wyt z-in:zo h.lp))
   ~
  `pulled
::
--