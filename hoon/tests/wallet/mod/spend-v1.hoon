/=  *  /common/test
/=  *  /common/zeke
/=  zo  /common/zoon
/=  *  /common/zose
/=  dhel  /tests/dumb/helpers
/=  wt  /apps/wallet/lib/types
/=  hel  /tests/wallet/helpers
/=  t  /common/tx-engine
|%
::
++  test-builds-spends-from-v0-notes
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t default-a-pt-2:dhel)
  =/  source2=hash:t  (hash:schnorr-pubkey:t default-a-pt-3:dhel)
  =/  [name1=nname:t note1=nnote:t]  (build-v0-note:hel source1 800.000.000)
  =/  [name2=nname:t note2=nnote:t]  (build-v0-note:hel source2 600.000.000)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1] [name2 note2]])
  =/  names=(list nname:t)  ~[name1 name2]
  =/  recipient-key=schnorr-pubkey:t  default-a-pt-2:dhel
  =/  recipient-hash=hash:t  (hash:schnorr-pubkey:t recipient-key)
  =/  order=order:wt  (pkh-order:hel recipient-hash 1.000.000.000)
  =/  orders=(list order:wt)  ~[order]
  =/  fee=coins:t  50.000.000
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb:hel names orders fee default-sign-keys:hel default-refund-unit:hel get-note %.y %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 5]
  =/  total-gift=coins:t
    (sum-gifts-for-lock:hel spends (recipient-lock-root:hel recipient-hash))
  =/  total-refund=coins:t
    (sum-gifts-for-lock:hel spends default-refund-lock-root:hel)
  =/  total-assets=coins:t  (add assets.note1 assets.note2)
  =/  expected-refund=coins:t
    (sub total-assets (add (gift:order:wt order) fee))
  =/  entry-count=@
    (lent ~(tap z-by:zo spends))
  ;:  weld
    %+  expect-eq
      !>(2)
    !>(entry-count)
  ::
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    %+  expect-eq
      !>((gift:order:wt order))
    !>(total-gift)
  ::
    %+  expect-eq
      !>(expected-refund)
    !>(total-refund)
  ==
++  test-rejects-mixed-note-versions
  ^-  tang
  =/  source0=hash:t  (hash:schnorr-pubkey:t default-a-pt-2:dhel)
  =/  source1=hash:t  (hash:schnorr-pubkey:t default-a-pt-3:dhel)
  =/  [name0=nname:t note0=nnote:t]  (build-v0-note:hel source0 500.000.000)
  =/  [name1=nname:t note1=nnote:t]  (build-v1-note:hel source1 500.000.000 ~)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name0 note0] [name1 note1]])
  =/  names=(list nname:t)  ~[name0 name1]
  =/  recipient-hash=hash:t  (hash:schnorr-pubkey:t default-a-pt-2:dhel)
  =/  order=order:wt  (pkh-order:hel recipient-hash 100.000.000)
  =/  orders=(list order:wt)  ~[order]
  =/  fee=coins:t  10.000.000
  =/  get-note  (get-note-from:hel notes)
    %+  expect-fail
      |.((txb:hel names orders fee default-sign-keys:hel default-refund-unit:hel get-note %.y %desc))
  ~
::
++  test-builds-spends-from-v1-notes
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t default-a-pt-2:dhel)
  =/  source2=hash:t  (hash:schnorr-pubkey:t default-a-pt-3:dhel)
  =/  [name1=nname:t note1=nnote:t]  (build-v1-note:hel source1 600.000.000 ~)
  =/  [name2=nname:t note2=nnote:t]  (build-v1-note:hel source2 500.000.000 ~)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1] [name2 note2]])
  =/  names=(list nname:t)  ~[name1 name2]
  =/  recipient-key=schnorr-pubkey:t  default-a-pt-2:dhel
  =/  recipient-hash=hash:t  (hash:schnorr-pubkey:t recipient-key)
  =/  order=order:wt  (pkh-order:hel recipient-hash 900.000.000)
  =/  orders=(list order:wt)  ~[order]
  =/  fee=coins:t  75.000.000
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb:hel names orders fee default-sign-keys:hel ~ get-note %.y %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  =/  total-gift=coins:t
    (sum-gifts-for-lock:hel spends (recipient-lock-root:hel recipient-hash))
  =/  entry-count=@
    (lent ~(tap z-by:zo spends))
  =/  all-v1=?
    %+  levy  ~(tap z-by:zo spends)
    |=  [=nname:t sp=spend-v1:t]
    ?=(%1 -.sp)
  ;:  weld
    %+  expect-eq
      !>(2)
    !>(entry-count)
  ::
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    %+  expect-eq
      !>((gift:order:wt order))
    !>(total-gift)
  ::
    %+  expect-eq
      !>(%.y)
    !>(all-v1)
  ==
::
++  test-v0-fee-from-second-note
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t default-a-pt-2:dhel)
  =/  source2=hash:t  (hash:schnorr-pubkey:t default-a-pt-3:dhel)
  =/  [name1=nname:t note1=nnote:t]  (build-v0-note:hel source1 500.000.000)
  =/  [name2=nname:t note2=nnote:t]  (build-v0-note:hel source2 200.000.000)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1] [name2 note2]])
  =/  names=(list nname:t)  ~[name1 name2]
  =/  recipient-key=schnorr-pubkey:t  default-a-pt-2:dhel
  =/  recipient-hash=hash:t  (hash:schnorr-pubkey:t recipient-key)
  =/  order=order:wt  (pkh-order:hel recipient-hash 500.000.000)
  =/  orders=(list order:wt)  ~[order]
  =/  fee=coins:t  50.000.000
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb:hel names orders fee default-sign-keys:hel default-refund-unit:hel get-note %.y %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 5]
  =/  sp1=spend:v1:t  (~(got z-by:zo spends) name1)
  ?>  ?=(%0 -.sp1)
  =/  spend1=spend-0:v1:t  +.sp1
  =/  seeds1=(list seed-v1:t)  ~(tap z-in:zo seeds.spend1)
  =/  recipient-root=hash:t  (recipient-lock-root:hel recipient-hash)
  =/  gift-total1=coins:t
    %+  roll
      %+  skim  seeds1
      |=  sed=seed-v1:t
      =(lock-root.sed recipient-root)
    |=  [sed=seed-v1:t acc=coins:t]
    (add acc gift.sed)
  =/  sp2=spend:v1:t  (~(got z-by:zo spends) name2)
  ?>  ?=(%0 -.sp2)
  =/  spend2=spend-0:v1:t  +.sp2
  =/  seeds2=(list seed-v1:t)  ~(tap z-in:zo seeds.spend2)
  =/  recipient-seeds2=(list seed-v1:t)
    %+  skim  seeds2
    |=  sed=seed-v1:t
    =(lock-root.sed recipient-root)
  =/  refund-root=hash:t  default-refund-lock-root:hel
  =/  refund-seeds2=(list seed-v1:t)
    %+  skim  seeds2
    |=  sed=seed-v1:t
    =(lock-root.sed refund-root)
  =/  refund-total2=coins:t
    %+  roll  refund-seeds2
    |=  [sed=seed-v1:t acc=coins:t]
    (add acc gift.sed)
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    %+  expect-eq
      !>(500.000.000)
    !>(gift-total1)
  ::
    %+  expect-eq
      !>(0)
    !>(fee.spend1)
  ::
    %+  expect-eq
      !>(~)
    !>(recipient-seeds2)
  ::
    %+  expect-eq
      !>(refund-seeds2)
    !>(seeds2)
  ::
    %+  expect-eq
      !>(50.000.000)
    !>(fee.spend2)
  ::
    %+  expect-eq
      !>(150.000.000)
    !>(refund-total2)
  ==
::
++  test-v0-insufficient-funds-for-fee
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t default-a-pt-2:dhel)
  =/  source2=hash:t  (hash:schnorr-pubkey:t default-a-pt-3:dhel)
  =/  [name1=nname:t note1=nnote:t]  (build-v0-note:hel source1 250.000.000)
  =/  [name2=nname:t note2=nnote:t]  (build-v0-note:hel source2 249.999.999)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1] [name2 note2]])
  =/  names=(list nname:t)  ~[name1 name2]
  =/  recipient-hash=hash:t  (hash:schnorr-pubkey:t default-a-pt-2:dhel)
  =/  order=order:wt  (pkh-order:hel recipient-hash 400.000.000)
  =/  orders=(list order:wt)  ~[order]
  =/  fee=coins:t  100.000.000
  =/  get-note  (get-note-from:hel notes)
    %+  expect-fail
      |.((txb:hel names orders fee default-sign-keys:hel default-refund-unit:hel get-note %.y %desc))
  ~
::
++  test-v1-multi-note-gift-split
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t default-a-pt-2:dhel)
  =/  source2=hash:t  (hash:schnorr-pubkey:t default-a-pt-3:dhel)
  =/  [name1=nname:t note1=nnote:t]  (build-v1-note:hel source1 600.000.000 ~)
  =/  [name2=nname:t note2=nnote:t]  (build-v1-note:hel source2 400.000.000 ~)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1] [name2 note2]])
  =/  names=(list nname:t)  ~[name1 name2]
  =/  recipient-key=schnorr-pubkey:t  default-a-pt-2:dhel
  =/  recipient-hash=hash:t  (hash:schnorr-pubkey:t recipient-key)
  =/  order=order:wt  (pkh-order:hel recipient-hash 900.000.000)
  =/  orders=(list order:wt)  ~[order]
  =/  fee=coins:t  60.000.000
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb:hel names orders fee default-sign-keys:hel ~ get-note %.y %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  =/  total-gift=coins:t
    (sum-gifts-for-lock:hel spends (recipient-lock-root:hel recipient-hash))
  =/  sp1=spend:v1:t  (~(got z-by:zo spends) name1)
  ?>  ?=(%1 -.sp1)
  =/  spend1=spend-1:v1:t  +.sp1
  =/  sp2=spend:v1:t  (~(got z-by:zo spends) name2)
  ?>  ?=(%1 -.sp2)
  =/  spend2=spend-1:v1:t  +.sp2
  =/  seeds2=(list seed-v1:t)  ~(tap z-in:zo seeds.spend2)
  =/  refund-total2=coins:t
    %+  roll  ~(tap z-in:zo seeds.spend2)
    |=  [sed=seed-v1:t acc=coins:t]
    ?:  =(lock-root.sed (recipient-lock-root:hel recipient-hash))
      acc
    (add acc gift.sed)
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    %+  expect-eq
      !>(900.000.000)
    !>(total-gift)
  ::  note2 still carries the change; the fee is now spread across both notes
    %+  expect-eq
      !>(40.000.000)
    !>(refund-total2)
  ::
    (check-conservation:hel notes spends)
  ::
    %+  expect-eq
      !>(fee)
    !>((roll-fees:spends:t spends))
  ==
::
++  test-fee-equals-minimum
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t default-a-pt-2:dhel)
  =/  source2=hash:t  (hash:schnorr-pubkey:t default-a-pt-3:dhel)
  =/  [name1=nname:t note1=nnote:t]  (build-v1-note:hel source1 800.000.000 ~)
  =/  [name2=nname:t note2=nnote:t]  (build-v1-note:hel source2 500.000.000 ~)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1] [name2 note2]])
  =/  names=(list nname:t)  ~[name1 name2]
  =/  recipient-key=schnorr-pubkey:t  default-a-pt-2:dhel
  =/  recipient-hash=hash:t  (hash:schnorr-pubkey:t recipient-key)
  =/  order=order:wt  (pkh-order:hel recipient-hash 900.000.000)
  =/  orders=(list order:wt)  ~[order]
  =/  generous-fee=coins:t  200.000.000
  =/  get-note  (get-note-from:hel notes)
  =/  spends-high=spends:t
    (txb:hel names orders generous-fee default-sign-keys:hel ~ get-note %.y %desc)
  =/  min-fee=coins:t  (calculate-min-fee:spends:t [spends-high 10])
  =/  spends=spends:t
    (txb:hel names orders min-fee default-sign-keys:hel ~ get-note %.y %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  =/  recomputed-min=coins:t  (calculate-min-fee:spends:t [spends 10])
  =/  total-fee=coins:t  (roll-fees:spends:t spends)
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    %+  expect-eq
      !>(min-fee)
    !>(recomputed-min)
  ::
    %+  expect-eq
      !>(min-fee)
    !>(total-fee)
  ==
::
++  test-fee-equals-minimum-pre-bythos
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t default-a-pt-2:dhel)
  =/  source2=hash:t  (hash:schnorr-pubkey:t default-a-pt-3:dhel)
  =/  [name1=nname:t note1=nnote:t]  (build-v1-note:hel source1 800.000.000 ~)
  =/  [name2=nname:t note2=nnote:t]  (build-v1-note:hel source2 500.000.000 ~)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1] [name2 note2]])
  =/  names=(list nname:t)  ~[name1 name2]
  =/  recipient-key=schnorr-pubkey:t  default-a-pt-2:dhel
  =/  recipient-hash=hash:t  (hash:schnorr-pubkey:t recipient-key)
  =/  order=order:wt  (pkh-order:hel recipient-hash 900.000.000)
  =/  orders=(list order:wt)  ~[order]
  =/  generous-fee=coins:t  200.000.000
  =/  get-note  (get-note-from:hel notes)
  =/  spends-high=spends:t
    (txb:hel names orders generous-fee default-sign-keys:hel ~ get-note %.y %desc)
  =/  min-fee=coins:t  (calculate-min-fee:spends:t [spends-high 9])
  =/  spends=spends:t
    (txb:hel names orders min-fee default-sign-keys:hel ~ get-note %.y %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 9]
  =/  recomputed-min=coins:t  (calculate-min-fee:spends:t [spends 9])
  =/  total-fee=coins:t  (roll-fees:spends:t spends)
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    %+  expect-eq
      !>(min-fee)
    !>(recomputed-min)
  ::
    %+  expect-eq
      !>(min-fee)
    !>(total-fee)
  ==
::
++  test-fee-below-minimum-fails
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t default-a-pt-2:dhel)
  =/  source2=hash:t  (hash:schnorr-pubkey:t default-a-pt-3:dhel)
  =/  [name1=nname:t note1=nnote:t]  (build-v1-note:hel source1 800.000.000 ~)
  =/  [name2=nname:t note2=nnote:t]  (build-v1-note:hel source2 500.000.000 ~)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1] [name2 note2]])
  =/  names=(list nname:t)  ~[name1 name2]
  =/  recipient-key=schnorr-pubkey:t  default-a-pt-2:dhel
  =/  recipient-hash=hash:t  (hash:schnorr-pubkey:t recipient-key)
  =/  order=order:wt  (pkh-order:hel recipient-hash 900.000.000)
  =/  orders=(list order:wt)  ~[order]
  =/  generous-fee=coins:t  200.000.000
  =/  get-note  (get-note-from:hel notes)
  =/  spends-high=spends:t
    (txb:hel names orders generous-fee default-sign-keys:hel ~ get-note %.y %desc)
  =/  min-fee=coins:t  (calculate-min-fee:spends:t [spends-high 10])
  ~&  min-fee-test+min-fee
  ?>  (gth min-fee 0)
  =/  below-min=coins:t  (dec min-fee)
  =/  min-fee-cord=@t  (rsh [3 2] (scot %ui min-fee))
    %+  expect-fail
      |.((txb:hel names orders below-min default-sign-keys:hel ~ get-note %.y %desc))
  %-  some
  "Min fee not met. This transaction requires at least: {(trip min-fee-cord)} nicks"
::
++  test-v1-omits-note-data-when-disabled
  ^-  tang
  =/  source=hash:t  (hash:schnorr-pubkey:t default-a-pt-2:dhel)
  =/  [name=nname:t note=nnote:t]
    (build-v1-note:hel source 550.000.000 ~)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name note]])
  =/  names=(list nname:t)  ~[name]
  =/  recipient-hash=hash:t  (hash:schnorr-pubkey:t default-a-pt-3:dhel)
  =/  order=order:wt  (pkh-order:hel recipient-hash 500.000.000)
  =/  orders=(list order:wt)  ~[order]
  =/  fee=coins:t  50.000.000
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb:hel names orders fee default-sign-keys:hel ~ get-note %.n %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  =/  all-seeds=(list seed-v1:t)
    %-  zing
    %+  turn  ~(tap z-by:zo spends)
    |=  [=nname:t sp=spend-v1:t]
    (gather-seeds:hel sp)
  =/  no-note-data-in-seeds=?
    %+  levy  all-seeds
    |=  sed=seed-v1:t
    ?=(~ note-data.sed)
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    %+  expect-eq
      !>(%.n)
    !>(?=(~ all-seeds))
  ::
    %+  expect-eq
      !>(%.y)
    !>(no-note-data-in-seeds)
  ==
::
++  test-v1-includes-note-data-when-enabled
  ^-  tang
  =/  source=hash:t  (hash:schnorr-pubkey:t default-a-pt-2:dhel)
  =/  [name=nname:t note=nnote:t]
    (build-v1-note:hel source 550.000.000 ~)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name note]])
  =/  names=(list nname:t)  ~[name]
  =/  recipient-hash=hash:t  (hash:schnorr-pubkey:t default-a-pt-3:dhel)
  =/  order=order:wt  (pkh-order:hel recipient-hash 500.000.000)
  =/  orders=(list order:wt)  ~[order]
  =/  fee=coins:t  50.000.000
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb:hel names orders fee default-sign-keys:hel ~ get-note %.y %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  =/  all-seeds=(list seed-v1:t)
    %-  zing
    %+  turn  ~(tap z-by:zo spends)
    |=  [=nname:t sp=spend-v1:t]
    (gather-seeds:hel sp)
  =/  expected-output-note-data
    =+  output-pkh=[%pkh [m=1 (z-silt:zo ~[recipient-hash])]]~
    %-  ~(put z-by:zo *note-data:v1:t)
    [%lock [%0 output-pkh]]
  ::  all seeds should have expected output note data because there is no refund.
  =/  seeds-with-note-data=(list seed-v1:t)
    %+  skim  all-seeds
    |=  sed=seed-v1:t
    =(note-data.sed expected-output-note-data)
  =/  first-seed=seed-v1:t
    ?~  seeds-with-note-data
      ~|('expected seeds with note-data' !!)
    i.seeds-with-note-data
  =/  first-note-data-not-null  !=(~ note-data.first-seed)
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    %+  expect-eq
      !>(all-seeds)
    !>(seeds-with-note-data)
  ::
    %+  expect-eq
      !>(%.y)
    !>(first-note-data-not-null)
  ==
::
++  test-count-seed-words-counts-output-note-data
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t default-a-pt-2:dhel)
  =/  source2=hash:t  (hash:schnorr-pubkey:t default-a-pt-3:dhel)
  =/  [name1=nname:t *]  (build-v1-note:hel source1 1.000.000 ~)
  =/  [name2=nname:t *]  (build-v1-note:hel source2 1.000.000 ~)
  =/  recipient-hash=hash:t  (hash:schnorr-pubkey:t default-a-pt-1:dhel)
  =/  lock-root=hash:t  (recipient-lock-root:hel recipient-hash)
  =/  parent-hash=hash:t  (hash:schnorr-pubkey:t default-a-pt-3:dhel)
  =/  note-data-1=note-data:v1:t  (~(put z-by:zo *note-data:v1:t) %foo 1)
  =/  note-data-2=note-data:v1:t  (~(put z-by:zo *note-data:v1:t) %foo 2)
  =/  seed1=seed-v1:t
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      lock-root
      note-data      note-data-1
      gift           1
      parent-hash    parent-hash
    ==
  =/  seed2=seed-v1:t
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      lock-root
      note-data      note-data-2
      gift           1
      parent-hash    parent-hash
    ==
  =/  seds1=seeds:v1:t  (~(put z-in:zo *seeds:v1:t) seed1)
  =/  seds2=seeds:v1:t  (~(put z-in:zo *seeds:v1:t) seed2)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds1
      fee      0
    ==
  =/  sp2=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds2
      fee      0
    ==
  =/  spends=spends:v1:t  (~(put z-by:zo *spends:v1:t) name1 [%1 sp1])
  =.  spends  (~(put z-by:zo spends) name2 [%1 sp2])
  =/  counted=@  (count-seed-words:spends:t [spends 999.999.999])
  =/  words-1=@
    %-  num-of-leaves:shape
    %-  ~(rep z-by:zo note-data-1)
    |=  [[k=@tas v=*] tree=*]
    [k v tree]
  =/  words-2=@
    %-  num-of-leaves:shape
    %-  ~(rep z-by:zo note-data-2)
    |=  [[k=@tas v=*] tree=*]
    [k v tree]
  =/  summed-by-seed=@  (add words-1 words-2)
  ;:  weld
    %+  expect-eq
      !>(words-1)
    !>(counted)
  ::
    %+  expect-eq
      !>(%.y)
    !>(!=(counted summed-by-seed))
  ==
::
++  test-output-notes-union-seed-note-data
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t default-a-pt-2:dhel)
  =/  source2=hash:t  (hash:schnorr-pubkey:t default-a-pt-3:dhel)
  =/  [name1=nname:t *]  (build-v1-note:hel source1 1.000.000 ~)
  =/  [name2=nname:t *]  (build-v1-note:hel source2 1.000.000 ~)
  =/  recipient-hash=hash:t  (hash:schnorr-pubkey:t default-a-pt-1:dhel)
  =/  lock-root=hash:t  (recipient-lock-root:hel recipient-hash)
  =/  parent-hash=hash:t  (hash:schnorr-pubkey:t default-a-pt-3:dhel)
  =/  note-data-a=note-data:v1:t  (~(put z-by:zo *note-data:v1:t) %a 1)
  =/  note-data-b=note-data:v1:t  (~(put z-by:zo *note-data:v1:t) %b 2)
  =/  expected-note-data=note-data:v1:t  (~(uni z-by:zo note-data-a) note-data-b)
  =/  seed1=seed-v1:t
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      lock-root
      note-data      note-data-a
      gift           1
      parent-hash    parent-hash
    ==
  =/  seed2=seed-v1:t
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      lock-root
      note-data      note-data-b
      gift           1
      parent-hash    parent-hash
    ==
  =/  seds1=seeds:v1:t  (~(put z-in:zo *seeds:v1:t) seed1)
  =/  seds2=seeds:v1:t  (~(put z-in:zo *seeds:v1:t) seed2)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds1
      fee      0
    ==
  =/  sp2=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds2
      fee      0
    ==
  =/  spends=spends:v1:t  (~(put z-by:zo *spends:v1:t) name1 [%1 sp1])
  =.  spends  (~(put z-by:zo spends) name2 [%1 sp2])
  =/  raw=raw-tx:t  (new:raw-tx:v1:t spends)
  =/  tx=tx:t  (new:tx:t raw 0)
  =/  outs=outputs:t  ~(outputs get:tx:t tx)
  ?>  ?=(%1 -.outs)
  =/  out-list=(list output:t)  ~(tap z-in:zo +.outs)
  =/  first-out=output:t
    ?~  out-list  ~|('expected at least one output' !!)
    i.out-list
  =/  out-note=nnote:t  ~(note get:output:t first-out)
  =/  note1=nnote-1:v1:t  ?>(?=(@ -.out-note) out-note)
  ;:  weld
    %+  expect-eq
      !>(1)
    !>((lent out-list))
  ::
    %+  expect-eq
      !>(expected-note-data)
    !>(note-data.note1)
  ==
::
++  test-v0-omits-note-data-when-disabled
  ^-  tang
  =/  source=hash:t  (hash:schnorr-pubkey:t default-a-pt-2:dhel)
  =/  [name=nname:t note=nnote:t]
    (build-v0-note:hel source 550.000.000)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name note]])
  =/  names=(list nname:t)  ~[name]
  =/  recipient-hash=hash:t  (hash:schnorr-pubkey:t default-a-pt-3:dhel)
  =/  order=order:wt  (pkh-order:hel recipient-hash 500.000.000)
  =/  orders=(list order:wt)  ~[order]
  =/  fee=coins:t  50.000.000
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb:hel names orders fee default-sign-keys:hel default-refund-unit:hel get-note %.n %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  =/  all-seeds=(list seed-v1:t)
    %-  zing
    %+  turn  ~(tap z-by:zo spends)
    |=  [=nname:t sp=spend-v1:t]
    (gather-seeds:hel sp)
  =/  no-note-data-in-seeds=?
    %+  levy  all-seeds
    |=  sed=seed-v1:t
    ?=(~ note-data.sed)
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    %+  expect-eq
      !>(%.n)
    !>(?=(~ all-seeds))
  ::
    %+  expect-eq
      !>(%.y)
    !>(no-note-data-in-seeds)
  ==
::
++  test-v0-includes-note-data-when-enabled
  ^-  tang
  =/  source=hash:t  (hash:schnorr-pubkey:t default-a-pt-2:dhel)
  =/  [name=nname:t note=nnote:t]
    (build-v0-note:hel source 550.000.000)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name note]])
  =/  names=(list nname:t)  ~[name]
  =/  recipient-hash=hash:t  (hash:schnorr-pubkey:t default-a-pt-3:dhel)
  =/  order=order:wt  (pkh-order:hel recipient-hash 500.000.000)
  =/  orders=(list order:wt)  ~[order]
  =/  fee=coins:t  50.000.000
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb:hel names orders fee default-sign-keys:hel default-refund-unit:hel get-note %.y %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  =/  all-seeds=(list seed-v1:t)
    %-  zing
    %+  turn  ~(tap z-by:zo spends)
    |=  [=nname:t sp=spend-v1:t]
    (gather-seeds:hel sp)
  =/  expected-output-note-data
    =+  output-pkh=[%pkh [m=1 (z-silt:zo ~[recipient-hash])]]~
    %-  ~(put z-by:zo *note-data:v1:t)
    [%lock [%0 output-pkh]]
  ::  all seeds should have expected output note data because there is no refund.
  =/  seeds-with-note-data=(list seed-v1:t)
    %+  skim  all-seeds
    |=  sed=seed-v1:t
    ~&  note-data+note-data.sed
    ~&  expected+expected-output-note-data
    =(note-data.sed expected-output-note-data)
  =/  first-seed=seed-v1:t
    ?~  seeds-with-note-data
      ~|('expected seeds with note-data' !!)
    i.seeds-with-note-data
  =/  first-note-data-not-null  !=(~ note-data.first-seed)
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    %+  expect-eq
      !>(all-seeds)
    !>(seeds-with-note-data)
  ::
    %+  expect-eq
      !>(%.y)
    !>(first-note-data-not-null)
  ==
::
++  test-spend-v0-coinbase-note
  ^-  tang
  =/  parent-hash=hash:t  (hash:schnorr-pubkey:t default-a-pt-3:dhel)
  =/  assets=coins:t  900.000.000
  =/  gift=coins:t  650.000.000
  =/  fee=coins:t  45.000.000
  =/  [name=nname:t note=nnote:t]
    (build-v0-coinbase-note:hel parent-hash assets)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name note]])
  =/  names=(list nname:t)  ~[name]
  =/  recipient-key=schnorr-pubkey:t  default-a-pt-2:dhel
  =/  recipient-hash=hash:t  (hash:schnorr-pubkey:t recipient-key)
  =/  order=order:wt  (pkh-order:hel recipient-hash gift)
  =/  orders=(list order:wt)  ~[order]
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb:hel names orders fee default-sign-keys:hel default-refund-unit:hel get-note %.y %desc)
  =/  validation=(reason:t ~)
    :: make sure to set height to 100 lbokcs in advance for coinbase timelock
    %-  validate-with-notes:hel
  [notes spends 101]
  =/  total-gift=coins:t
    (sum-gifts-for-lock:hel spends (recipient-lock-root:hel recipient-hash))
  =/  total-refund=coins:t
    (sum-gifts-for-lock:hel spends default-refund-lock-root:hel)
  =/  spend=spend:v1:t  (~(got z-by:zo spends) name)
  ?>  ?=(%0 -.spend)
  =/  spend0=spend-0:v1:t  +.spend
  =/  fee-paid=@  fee.spend0
  =/  refund-expected=coins:t  (sub assets (add gift fee))
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    %+  expect-eq
      !>(gift)
    !>(total-gift)
  ::
    %+  expect-eq
      !>(refund-expected)
    !>(total-refund)
  ::
    %+  expect-eq
      !>(fee)
    !>(fee-paid)
  ==
::
++  test-spend-v1-coinbase-note
  ^-  tang
  =/  parent-hash=hash:t  (hash:schnorr-pubkey:t default-a-pt-1:dhel)
  =/  assets=coins:t  1.000.000.000
  =/  gift=coins:t  750.000.000
  =/  fee=coins:t  60.000.000
  =/  [name=nname:t note=nnote:t]
    (build-v1-coinbase-note:hel parent-hash assets)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name note]])
  =/  names=(list nname:t)  ~[name]
  =/  recipient-key=schnorr-pubkey:t  default-a-pt-3:dhel
  =/  recipient-hash=hash:t  (hash:schnorr-pubkey:t recipient-key)
  =/  order=order:wt  (pkh-order:hel recipient-hash gift)
  =/  orders=(list order:wt)  ~[order]
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb:hel names orders fee default-sign-keys:hel default-refund-unit:hel get-note %.y %desc)
  =/  validation=(reason:t ~)
    :: make sure to set height to 100 lbokcs in advance for coinbase timelock
    %-  validate-with-notes:hel
  [notes spends 101]
  ~&  validation+validation
  =/  spend=spend:v1:t  (~(got z-by:zo spends) name)
  ?>  ?=(%1 -.spend)
  =/  spend1=spend-1:v1:t  +.spend
  =/  seeds=(list seed-v1:t)  ~(tap z-in:zo seeds.spend1)
  =/  total-gift=coins:t
    (sum-gifts-for-lock:hel spends (recipient-lock-root:hel recipient-hash))
  =/  total-refund=coins:t
    (sum-gifts-for-lock:hel spends default-refund-lock-root:hel)
  =/  refund-expected=coins:t  (sub assets (add gift fee))
  =/  fee-paid=@  fee.spend1
  =/  seed-count=@  (lent seeds)
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    %+  expect-eq
      !>(gift)
    !>(total-gift)
  ::
    %+  expect-eq
      !>(refund-expected)
    !>(total-refund)
  ::
    %+  expect-eq
      !>(fee)
    !>(fee-paid)
  ::
    %+  expect-eq
      !>(2)
    !>(seed-count)
  ==
::
++  test-v1-gift-consumes-note-no-refund
  ^-  tang
  =/  source=hash:t  (hash:schnorr-pubkey:t default-a-pt-2:dhel)
  =/  gift=coins:t  700.000.000
  =/  fee=coins:t  50.000.000
  =/  total=coins:t  (add gift fee)
  =/  [name=nname:t note=nnote:t]
    (build-v1-note:hel source total ~)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name note]])
  =/  names=(list nname:t)  ~[name]
  =/  recipient-key=schnorr-pubkey:t  default-a-pt-3:dhel
  =/  recipient-hash=hash:t  (hash:schnorr-pubkey:t recipient-key)
  =/  order=order:wt  (pkh-order:hel recipient-hash gift)
  =/  orders=(list order:wt)  ~[order]
  =/  spends=spends:t
    (txb:hel names orders fee default-sign-keys:hel ~ (get-note-from:hel notes) %.y %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  =/  spend=spend-v1:t  (~(got z-by:zo spends) name)
  ?>  ?=(%1 -.spend)
  =/  spend1=spend-1:v1:t  +.spend
  =/  seed-list=(list seed-v1:t)  ~(tap z-in:zo seeds.spend1)
  =/  seeds-count=@  (lent seed-list)
  =/  gift-total=coins:t
    %+  roll  seed-list
    |=  [sed=seed-v1:t acc=coins:t]
    (add acc gift.sed)
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    %+  expect-eq
      !>(1)
    !>(seeds-count)
  ::
    %+  expect-eq
      !>(gift)
    !>(gift-total)
  ::
    %+  expect-eq
      !>(fee)
    !>(fee.spend1)
  ==
::
++  test-zero-fee-zero-gift-refund-only
  ^-  tang
  =/  source=hash:t  (hash:schnorr-pubkey:t default-a-pt-2:dhel)
  =/  [name=nname:t note=nnote:t]  (build-v0-note:hel source 345.678.901)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name note]])
  =/  names=(list nname:t)  ~[name]
  =/  recipient-hash=hash:t  (hash:schnorr-pubkey:t default-a-pt-3:dhel)
  =/  order=order:wt  (pkh-order:hel recipient-hash 0)
  =/  orders=(list order:wt)  ~[order]
  =/  fee=coins:t  0
    %+  expect-fail
      |.((txb:hel names orders fee default-sign-keys:hel default-refund-unit:hel (get-note-from:hel notes) %.y %desc))
  ~
:::
++  prebroadcast-maturity-case
  |=  [tip=@ age=@]
  ^-  ?
  =/  pkh  (make-pkh-from-sig:spend-condition:t p:default-keys-1:dhel)
  =/  timing=lock-primitive:t  [%tim [[`100 ~] [~ ~]]]
  =/  timed=spend-condition:t  ~[timing]
  =/  sc=spend-condition:t  (combine:spend-condition:t form.pkh timed)
  =/  lk=lock:t  sc
  =/  root=hash:t  (hash:lock:t lk)
  =/  note=nnote-1:v1:t
    %*  .  *nnote-1:v1:t
      version  %1
      origin-page  (sub +(tip) age)
      name  (new-v1:nname:t root [*hash:t %.n])
      assets  1.000.000
    ==
  =/  sed=seed:v1:t
    (simple:seed-v1:t root 999.744 (hash:nnote:t note))
  =/  sp=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:t
      seeds  (~(put z-in:zo *seeds:v1:t) sed)
      fee  256
    ==
  =.  sp  sp(witness witness.sp(lmp (build-lock-merkle-proof-stub:lock:t lk 1)))
  =.  sp  (sign:spend-1:v1:t sp s:default-keys-1:dhel)
  =/  sps=spends:t  (~(put z-by:zo *spends:t) name.note [%1 sp])
  =/  dat=transaction:wt
    [%1 'candidate-maturity' sps *metadata:wt [%1 (~(put z-by:zo *(z-map:zo nname:t witness:t)) name.note witness.sp)]]
  =/  state=state:wt  gen-spend-test-state:hel
  =.  state
    state(balance [tip *hash:t (~(put z-by:zo *(z-map:zo nname:t nnote:t)) name.note note)])
  =/  loaded  (lod:hel state wal:hel)
  =/  [effs=(list effect:hel) sent=_wal:hel]
    (pok:hel 0 [%send-tx dat] loaded)
  %+  lien  effs
  |=  e=effect:wt
  ?=(%grpc -.e)
:::
++  test-send-tx-next-candidate-maturity
  =/  h=@  (need consensus-batch-phase:page:t)
  =/  results=(list ?)
    %+  turn  ~[(sub h 2) (dec h) h (sub h 2)]
    |=  tip=@
    ?&  !(prebroadcast-maturity-case tip 99)
        (prebroadcast-maturity-case tip 100)
    ==
  %+  expect-eq  !>((reap 4 %.y))
  !>  results
::::
++  fund-policy-scry-case
  |=  minimum=@
  ^-  tang
  =/  h=@  (need consensus-batch-phase:page:t)
  =/  state=state:wt  *state:wt
  =.  state  state(bc bc.state(coinbase-timelock-min minimum))
  =/  loaded  (lod:hel state wal:hel)
  =/  results=(list ?)
    %+  turn  ~[(dec h) h +(h) (dec h)]
    |=  height=@
    =/  decimal=@ta
      (crip (skip (scow %ud height) |=(c=@tD =(c '.'))))
    =/  effective=(unit @)  ?:((lth height h) ~ `minimum)
    =/  expected=(unit (unit *))
      ``[height protocol-fund-address:t fund-note-firstname:t effective]
    =(expected (peek:loaded [%fund-spend-policy decimal ~]))
  %+  expect-eq  !>((reap 4 %.y))
  !>  results
::::
++  test-fund-policy-scry-configured-and-backwards
  ;:  weld
    (fund-policy-scry-case 100)
    (fund-policy-scry-case 2)
    (fund-policy-scry-case 0)
  ==
::::
++  test-fund-policy-scry-malformed-and-nested-absence
  =/  loaded  wal:hel
  ;:  weld
    (expect-eq !>(~) !>((peek:loaded /fund-spend-policy/not-decimal)))
    (expect-eq !>(~) !>((peek:loaded [%fund-spend-policy '-1' ~])))
    (expect-eq !>(~) !>((peek:loaded [%fund-spend-policy '1.0' ~])))
    (expect-eq !>(~) !>((peek:loaded /fund-spend-policy)))
    (expect-eq !>(~) !>((peek:loaded /fund-spend-policy/0/extra)))
    (expect-eq !>(``[0 protocol-fund-address:t fund-note-firstname:t ~]) !>((peek:loaded /fund-spend-policy/0)))
  ==
--
