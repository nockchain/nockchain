/=  *  /common/test
/=  *  /common/zeke
/=  zo  /common/zoon
/=  *  /common/zose
/=  dhel  /tests/dumb/helpers
/=  wt  /apps/wallet/lib/types
/=  hel  /tests/wallet/helpers
/=  t  /common/tx-engine
/=  wu  /apps/wallet/lib/utils
::
::  tiscom to avoid hoon-139 shadowing
|%
::
::  +test-fund-note-pull-resolves-multisig: the protocol-fund recovery on the
::    wallet side. A note carrying the protocol-fund first-name
::    (+fund-note-firstname) must resolve, via +pull:locks:utils, to the real
::    3-of-4 multisig spend-condition -- NOT the unsatisfiable wrapped %pkh the
::    normal path would reject. This is what lets the wallet require, and sign
::    with, the protocol-fund multisig keys. We do not hold the four production
::    seckeys, so this asserts the resolved required-key set IS exactly the four
::    participant pkhs at threshold 3 (so any participant key passes the wallet's
::    signable check and a non-participant fails), and that the resolved
::    spend-condition binds to +protocol-fund-address (so consensus +check-multisig-lock
::    accepts a spend revealing it). The schnorr signing itself is the generic,
::    already-tested multisig path.
++  test-fund-note-pull-resolves-multisig
  ^-  tang
  =/  fund-note-name=nname:t  [fund-note-firstname:t *hash:t ~]
  =/  resolved=(unit spend-condition:t)
    (pull:locks:wu [*note-data:v1:t fund-note-name `*hash:t])
  ?~  resolved
    (expect !>(?=(^ resolved)))
  =/  ml=spend-condition:t  fund-multisig-lock:t
  ?>  ?=(^ ml)
  ?>  ?=(%pkh -.i.ml)
  =/  pkh-payload  +.i.ml
  =/  a-participant=hash:t
    (from-b58:hash:t '7pGXggKU1AWk3d3wqX2kpKUatTqT68Cv8SQfGzGRQvJvYnQBvagSSjT')
  ;:  weld
    ::  pull resolves the fund note to the real multisig, not the wrapped pkh
    (expect-eq !>(fund-multisig-lock:t) !>(u.resolved))
    ::  the multisig binds to the consensus protocol-fund-address (so a spend is accepted)
    (expect-eq !>(protocol-fund-address:t) !>((hash:lock:t fund-multisig-lock:t)))
    ::  threshold 3, four signers
    (expect-eq !>(3) !>(m.pkh-payload))
    (expect-eq !>(4) !>(~(wyt z-in:zo h.pkh-payload)))
    ::  a participant key satisfies the wallet's signable membership check...
    (expect-eq !>(%.y) !>((~(has z-in:zo h.pkh-payload) a-participant)))
    ::  ...and a non-participant key does not
    (expect-eq !>(%.n) !>((~(has z-in:zo h.pkh-payload) *hash:t)))
  ==
::
++  test-multisig-1-of-1
  ::  single signer, single pubkey (degenerate case)
  ::  input lock = multisig lock, so refund goes to multisig (not custom)
  ::
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t signer-pubkey-2:hel)
  =/  gift=coins:t  500.000.000
  =/  fee=coins:t  50.000.000
  =/  refund=coins:t  100.000.000
  =/  note1-assets=coins:t  :(add gift fee refund)
  =/  [name1=nname:t note1=nnote:t]
    (build-v1-note:hel source1 note1-assets ~)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1]])
  =/  names=(list nname:t)  ~[name1]
  =/  pubkey-hashes=(z-set:zo hash:t)
    (build-pubkey-hashes-set:hel ~[signer-pubkey-1:hel])
  ::  custom refund pkh will be ignored since input=multisig lock
  =/  sign-keys=(list schnorr-seckey:t)
    ~[signer-seckey-1:hel]
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb-multisig:hel names gift fee 1 pubkey-hashes sign-keys ~ get-note %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  ~?  ?=(%.n -.validation)  "test-multisig-1-of-1 validation failed: {<p.validation>}"
  ::  both gift and refund go to multisig (input lock = multisig lock)
  =/  total-to-multisig=coins:t
    (sum-gifts-for-lock:hel spends (multisig-lock-root-from-set:hel 1 pubkey-hashes))
  =/  total-fee=coins:t  (roll-fees:spends:t spends)
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    (check-conservation:hel notes spends)
  ::
    %+  expect-eq
      !>(fee)
    !>(total-fee)
  ::
    ::  gift + refund both go to multisig
    %+  expect-eq
      !>((add gift refund))
    !>(total-to-multisig)
  ==
::
++  test-multisig-lock-data-without-include-data
  ::  even when include-data is disabled, multisig outputs must retain lock metadata
  ::
  ^-  tang
  =/  source=hash:t  (hash:schnorr-pubkey:t signer-pubkey-2:hel)
  =/  [gift=coins:t fee=coins:t]  [200.000.000 20.000.000]
  =/  note-assets=coins:t  (add gift fee)
  =/  [name=nname:t note=nnote:t]
    (build-v1-note:hel source note-assets `signer-pubkey-2:hel)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name note]])
  =/  names=(list nname:t)  ~[name]
  =/  pubkey-hashes=(z-set:zo hash:t)
    (build-pubkey-hashes-set:hel ~[signer-pubkey-1:hel signer-pubkey-2:hel])
  =/  participants=(list hash:t)  ~(tap z-in:zo pubkey-hashes)
  =/  order=order:wt
    [%multisig threshold=2 participants=participants gift=gift]
  =/  orders=(list order:wt)  ~[order]
  =/  get-note  (get-note-from:hel notes)
  =/  sign-keys=(list schnorr-seckey:t)
    ~[signer-seckey-2:hel]
  =/  =transaction:wt
    (build:txb-lib:hel names orders fee %.n sign-keys ~ get-note ~ %.n %desc 0)
  =/  spends=spends:t  (apply:witness-data:wt witness-data.transaction spends.transaction)
  =/  seeds=(list seed:v1:t)  (collect-seeds:hel spends)
  =/  multisig-lock=hash:t  (multisig-lock-root-from-set:hel 2 pubkey-hashes)
  =/  multisig-seed  (find-seed-by-lock:hel seeds multisig-lock)
  ?~  multisig-seed  (expect !>(?=(^ multisig-seed)))
  =/  stored-lock-hash=(unit hash:t)
    =+  nd=note-data.u.multisig-seed
    =+  lock-noun=(~(get z-by:zo nd) %lock)
    ?~  lock-noun  ~
    =/  parsed=(unit lock-data:wt)
      ((soft lock-data:wt) u.lock-noun)
    ?~  parsed  ~
    ?>  ?=(%0 -.u.parsed)
    `(hash:lock:t +.u.parsed)
  ?~  stored-lock-hash
    (expect !>(?=(^ stored-lock-hash)))
  (expect-eq !>(multisig-lock) !>(u.stored-lock-hash))
::
++  test-multisig-2-of-3
  ::  standard 2-of-3 multisig threshold
  ::
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t signer-pubkey-2:hel)
  =/  source2=hash:t  (hash:schnorr-pubkey:t signer-pubkey-3:hel)
  =/  gift=coins:t  700.000.000
  =/  fee=coins:t  75.000.000
  =/  note1-assets=coins:t  500.000.000
  =/  note2-assets=coins:t  350.000.000
  =/  [name1=nname:t note1=nnote:t]
    (build-v1-note:hel source1 note1-assets ~)
  =/  [name2=nname:t note2=nnote:t]
    (build-v1-note:hel source2 note2-assets ~)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1] [name2 note2]])
  =/  names=(list nname:t)  ~[name1 name2]
  =/  pubkey-hashes=(z-set:zo hash:t)
    (build-pubkey-hashes-set:hel ~[signer-pubkey-1:hel signer-pubkey-2:hel signer-pubkey-3:hel])
  ::  only sign with key matching input locks (1-of-1)
  =/  sign-keys=(list schnorr-seckey:t)
    ~[signer-seckey-1:hel]
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb-multisig:hel names gift fee 2 pubkey-hashes sign-keys default-refund-unit:hel get-note %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  =/  total-gift=coins:t
    (sum-gifts-for-lock:hel spends (multisig-lock-root-from-set:hel 2 pubkey-hashes))
  =/  total-fee=coins:t  (roll-fees:spends:t spends)
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    (check-conservation:hel notes spends)
  ::
    %+  expect-eq
      !>(gift)
    !>(total-gift)
  ::
    %+  expect-eq
      !>(fee)
    !>(total-fee)
  ==
::
++  test-multisig-3-of-5
  ::  higher threshold 3-of-5 multisig
  ::
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t signer-pubkey-2:hel)
  =/  gift=coins:t  800.000.000
  =/  fee=coins:t  100.000.000
  =/  note1-assets=coins:t  :(add gift fee 50.000.000)
  =/  [name1=nname:t note1=nnote:t]
    (build-v1-note:hel source1 note1-assets ~)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1]])
  =/  names=(list nname:t)  ~[name1]
  ::  create 5 pubkeys (reusing available ones)
  =/  pubkey-hashes=(z-set:zo hash:t)
    (build-pubkey-hashes-set:hel ~[signer-pubkey-1:hel signer-pubkey-2:hel signer-pubkey-3:hel signer-pubkey-1:hel signer-pubkey-2:hel])
  ::  only sign with key matching input locks (1-of-1)
  =/  sign-keys=(list schnorr-seckey:t)
    ~[signer-seckey-1:hel]
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb-multisig:hel names gift fee 3 pubkey-hashes sign-keys default-refund-unit:hel get-note %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  =/  total-gift=coins:t
    (sum-gifts-for-lock:hel spends (multisig-lock-root-from-set:hel 3 pubkey-hashes))
  =/  total-fee=coins:t  (roll-fees:spends:t spends)
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    (check-conservation:hel notes spends)
  ::
    %+  expect-eq
      !>(gift)
    !>(total-gift)
  ::
    %+  expect-eq
      !>(fee)
    !>(total-fee)
  ==
::
++  test-multisig-multiple-signers
  ::  test creating multisig output with multiple pubkeys
  ::
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t signer-pubkey-2:hel)
  =/  gift=coins:t  600.000.000
  =/  fee=coins:t  60.000.000
  =/  note1-assets=coins:t  :(add gift fee 100.000.000)
  =/  [name1=nname:t note1=nnote:t]
    (build-v1-note:hel source1 note1-assets ~)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1]])
  =/  names=(list nname:t)  ~[name1]
  =/  pubkey-hashes=(z-set:zo hash:t)
    (build-pubkey-hashes-set:hel ~[signer-pubkey-1:hel signer-pubkey-2:hel signer-pubkey-3:hel])
  ::  only sign with key matching input locks (1-of-1)
  =/  sign-keys=(list schnorr-seckey:t)
    ~[signer-seckey-1:hel]
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb-multisig:hel names gift fee 2 pubkey-hashes sign-keys default-refund-unit:hel get-note %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  =/  sp1=spend:v1:t  (~(got z-by:zo spends) name1)
  ::  verify all signers signed (check witness structure)
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    (check-conservation:hel notes spends)
  ==
::
++  test-multisig-conservation-basic
  ::  single note, gift + fee + refund
  ::
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t signer-pubkey-2:hel)
  =/  gift=coins:t  500.000.000
  =/  fee=coins:t  50.000.000
  =/  refund=coins:t  100.000.000
  =/  note1-assets=coins:t  :(add gift fee refund)
  =/  [name1=nname:t note1=nnote:t]
    (build-v1-note:hel source1 note1-assets ~)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1]])
  =/  names=(list nname:t)  ~[name1]
  =/  pubkey-hashes=(z-set:zo hash:t)
    (build-pubkey-hashes-set:hel ~[signer-pubkey-1:hel signer-pubkey-2:hel])
  ::  only sign with key matching input locks (1-of-1)
  =/  sign-keys=(list schnorr-seckey:t)
    ~[signer-seckey-1:hel]
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb-multisig:hel names gift fee 2 pubkey-hashes sign-keys default-refund-unit:hel get-note %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  =/  total-gift=coins:t
    (sum-gifts-for-lock:hel spends (multisig-lock-root-from-set:hel 2 pubkey-hashes))
  =/  total-refund=coins:t
    (sum-gifts-for-lock:hel spends (source-lock-root:hel signer-pubkey-1:hel))
  =/  total-fee=coins:t  (roll-fees:spends:t spends)
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    (check-conservation:hel notes spends)
  ::
    %+  expect-eq
      !>(gift)
    !>(total-gift)
  ::
    %+  expect-eq
      !>(fee)
    !>(total-fee)
  ::
    %+  expect-eq
      !>(refund)
    !>(total-refund)
  ==
::
++  test-multisig-from-v1-coinbase
  ::  ensure v1 coinbase input can fund multisig output
  ::
  ^-  tang
  =/  parent-hash=hash:t  (hash:schnorr-pubkey:t signer-pubkey-2:hel)
  =/  assets=coins:t  1.000.000.000
  =/  gift=coins:t  750.000.000
  =/  fee=coins:t  60.000.000
  =/  [name=nname:t note=nnote:t]
    (build-v1-coinbase-note:hel parent-hash assets)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name note]])
  =/  names=(list nname:t)  ~[name]
  =/  pubkey-hashes=(z-set:zo hash:t)
    (build-pubkey-hashes-set:hel ~[signer-pubkey-1:hel signer-pubkey-2:hel signer-pubkey-3:hel])
  ::  coinbase input still uses single-signer witness
  =/  sign-keys=(list schnorr-seckey:t)
    ~[signer-seckey-1:hel]
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb-multisig:hel names gift fee 2 pubkey-hashes sign-keys default-refund-unit:hel get-note %desc)
  =/  validation=(reason:t ~)
    ::  height 101 satisfies coinbase timelock
    %-  validate-with-notes:hel
  [notes spends 101]
  =/  spend=spend:v1:t  (~(got z-by:zo spends) name)
  ?>  ?=(%1 -.spend)
  =/  spend1=spend-1:v1:t  +.spend
  =/  seed-count=@  (lent ~(tap z-in:zo seeds.spend1))
  =/  total-gift=coins:t
    (sum-gifts-for-lock:hel spends (multisig-lock-root-from-set:hel 2 pubkey-hashes))
  =/  expected-refund=coins:t
    (sub assets (add gift fee))
  =/  total-refund=coins:t
    (sum-gifts-for-lock:hel spends (source-lock-root:hel signer-pubkey-1:hel))
  =/  total-fee=coins:t  (roll-fees:spends:t spends)
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    (check-conservation:hel notes spends)
  ::
    %+  expect-eq
      !>(gift)
    !>(total-gift)
  ::
    %+  expect-eq
      !>(expected-refund)
    !>(total-refund)
  ::
    %+  expect-eq
      !>(fee)
    !>(total-fee)
  ::
    %+  expect-eq
      !>(2)
    !>(seed-count)
  ==
::
++  test-multisig-conservation-multiple-notes
  ::  multiple notes across gift/fee/refund
  ::
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t signer-pubkey-2:hel)
  =/  source2=hash:t  (hash:schnorr-pubkey:t signer-pubkey-3:hel)
  =/  source3=hash:t  (hash:schnorr-pubkey:t signer-pubkey-1:hel)
  =/  gift=coins:t  1.000.000.000
  =/  fee=coins:t  150.000.000
  =/  [name1=nname:t note1=nnote:t]  (build-v1-note:hel source1 600.000.000 ~)
  =/  [name2=nname:t note2=nnote:t]  (build-v1-note:hel source2 500.000.000 ~)
  =/  [name3=nname:t note3=nnote:t]  (build-v1-note:hel source3 300.000.000 ~)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1] [name2 note2] [name3 note3]])
  =/  names=(list nname:t)  ~[name1 name2 name3]
  =/  pubkey-hashes=(z-set:zo hash:t)
    (build-pubkey-hashes-set:hel ~[signer-pubkey-1:hel signer-pubkey-2:hel signer-pubkey-3:hel])
  ::  only sign with key matching input locks (1-of-1)
  =/  sign-keys=(list schnorr-seckey:t)
    ~[signer-seckey-1:hel]
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb-multisig:hel names gift fee 2 pubkey-hashes sign-keys default-refund-unit:hel get-note %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  =/  total-assets=coins:t
    ;:  add
      assets.note1
      assets.note2
      assets.note3
    ==
  =/  expected-refund=coins:t  (sub total-assets (add gift fee))
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    (check-conservation:hel notes spends)
  ==
::
++  test-multisig-conservation-large-refund
  ::  most value becomes refund
  ::
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t signer-pubkey-2:hel)
  =/  source2=hash:t  (hash:schnorr-pubkey:t signer-pubkey-3:hel)
  =/  gift=coins:t  10.000.000
  =/  fee=coins:t  6.000.000
  =/  [name1=nname:t note1=nnote:t]  (build-v1-note:hel source1 300.000.000 ~)
  =/  [name2=nname:t note2=nnote:t]  (build-v1-note:hel source2 200.000.000 ~)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1] [name2 note2]])
  =/  names=(list nname:t)  ~[name1 name2]
  =/  pubkey-hashes=(z-set:zo hash:t)
    (build-pubkey-hashes-set:hel ~[signer-pubkey-1:hel signer-pubkey-2:hel])
  ::  only sign with key matching input locks (1-of-1)
  =/  sign-keys=(list schnorr-seckey:t)
    ~[signer-seckey-1:hel]
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb-multisig:hel names gift fee 2 pubkey-hashes sign-keys default-refund-unit:hel get-note %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  =/  total-refund=coins:t
    (sum-gifts-for-lock:hel spends (source-lock-root:hel signer-pubkey-1:hel))
  =/  expected-refund=coins:t
    (sub (add assets.note1 assets.note2) (add gift fee))
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    (check-conservation:hel notes spends)
  ::
    %+  expect-eq
      !>(expected-refund)
    !>(total-refund)
  ==
::
++  test-multisig-exact-match-no-refund
  ::  total assets exactly equal gift + fee, no refund
  ::
  ^-  tang
  ::  create different sources so the entry in the map will be under different keys
  =/  source1=hash:t  [0x0 0x0 0x0 0x0 0x0]
  =/  source2=hash:t  [0x0 0x0 0x0 0x0 0x1]
  =/  gift=coins:t  700.000.000
  =/  fee=coins:t  50.000.000
  =/  total-needed=coins:t  (add gift fee)
  =/  [name1=nname:t note1=nnote:t]  (build-v1-note:hel source1 400.000.000 `signer-pubkey-2:hel)
  =/  [name2=nname:t note2=nnote:t]  (build-v1-note:hel source2 350.000.000 `signer-pubkey-2:hel)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1] [name2 note2]])
  =/  names=(list nname:t)  ~[name1 name2]
  =/  pubkey-hashes=(z-set:zo hash:t)
    (build-pubkey-hashes-set:hel ~[signer-pubkey-1:hel signer-pubkey-2:hel])
  ::  only sign with key matching input locks (1-of-1)
  =/  sign-keys=(list schnorr-seckey:t)
    ~[signer-seckey-2:hel]
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb-multisig:hel names gift fee 2 pubkey-hashes sign-keys default-refund-unit:hel get-note %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  =/  total-gift=coins:t
    (sum-gifts-for-lock:hel spends (multisig-lock-root-from-set:hel 2 pubkey-hashes))
  =/  total-fee=coins:t  (roll-fees:spends:t spends)
  =/  all-seeds=(list seed-v1:t)
    %-  zing
    %+  turn  ~(tap z-by:zo spends)
    |=  [=nname:t sp=spend-v1:t]
    (gather-seeds:hel sp)
  =/  non-gift-seeds=(list seed-v1:t)
    %+  skim  all-seeds
    |=  sed=seed-v1:t
    !=(lock-root.sed (multisig-lock-root-from-set:hel 2 pubkey-hashes))
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    (check-conservation:hel notes spends)
  ::
    %+  expect-eq
      !>(gift)
    !>(total-gift)
  ::
    %+  expect-eq
      !>(fee)
    !>(total-fee)
  ::
    %+  expect-eq
      !>(~)
    !>(non-gift-seeds)
  ==
::
++  test-multisig-second-note-pure-refund
  ::  first note satisfies gift+fee, second is refund
  ::
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t signer-pubkey-2:hel)
  =/  source2=hash:t  (hash:schnorr-pubkey:t signer-pubkey-3:hel)
  =/  gift=coins:t  700.000.000
  =/  fee=coins:t  75.000.000
  =/  note1-assets=coins:t  (add gift fee)
  =/  note2-assets=coins:t  300.000.000
  =/  [name1=nname:t note1=nnote:t]
    (build-v1-note:hel source1 note1-assets ~)
  =/  [name2=nname:t note2=nnote:t]
    (build-v1-note:hel source2 note2-assets ~)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1] [name2 note2]])
  =/  names=(list nname:t)  ~[name1 name2]
  =/  pubkey-hashes=(z-set:zo hash:t)
    (build-pubkey-hashes-set:hel ~[signer-pubkey-1:hel signer-pubkey-2:hel])
  ::  only sign with key matching input locks (1-of-1)
  =/  sign-keys=(list schnorr-seckey:t)
    ~[signer-seckey-1:hel]
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb-multisig:hel names gift fee 2 pubkey-hashes sign-keys default-refund-unit:hel get-note %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  =/  total-gift=coins:t
    (sum-gifts-for-lock:hel spends (multisig-lock-root-from-set:hel 2 pubkey-hashes))
  =/  total-refund=coins:t
    (sum-gifts-for-lock:hel spends (source-lock-root:hel signer-pubkey-1:hel))
  =/  sp2=spend:v1:t  (~(got z-by:zo spends) name2)
  ?>  ?=(%1 -.sp2)
  =/  spend2=spend-1:v1:t  +.sp2
  =/  seeds2=(list seed-v1:t)  ~(tap z-in:zo seeds.spend2)
  =/  gift-seeds2=(list seed-v1:t)
    %+  skim  seeds2
    |=  sed=seed-v1:t
    =(lock-root.sed (multisig-lock-root-from-set:hel 2 pubkey-hashes))
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    %+  expect-eq
      !>(2)
    !>((lent ~(tap z-by:zo spends)))
  ::
    %+  expect-eq
      !>(gift)
    !>(total-gift)
  ::  note2 carries no recipient gift (its value is fee + change)
    %+  expect-eq
      !>(~)
    !>(gift-seeds2)
  ::
    %+  expect-eq
      !>(note2-assets)
    !>(total-refund)
  ::
    (check-conservation:hel notes spends)
  ::
    %+  expect-eq
      !>(fee)
    !>((roll-fees:spends:t spends))
  ==
::
++  test-multisig-second-note-fee-with-refund
  ::  first note satisfies gift, second pays fee + refund
  ::
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t signer-pubkey-2:hel)
  =/  source2=hash:t  (hash:schnorr-pubkey:t signer-pubkey-3:hel)
  =/  gift=coins:t  800.000.000
  =/  fee=coins:t  100.000.000
  =/  note1-assets=coins:t  gift
  =/  note2-assets=coins:t  (add fee 5.000.000)
  =/  [name1=nname:t note1=nnote:t]
    (build-v1-note:hel source1 note1-assets ~)
  =/  [name2=nname:t note2=nnote:t]
    (build-v1-note:hel source2 note2-assets ~)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1] [name2 note2]])
  =/  names=(list nname:t)  ~[name1 name2]
  =/  pubkey-hashes=(z-set:zo hash:t)
    (build-pubkey-hashes-set:hel ~[signer-pubkey-1:hel signer-pubkey-2:hel])
  ::  only sign with key matching input locks (1-of-1)
  =/  sign-keys=(list schnorr-seckey:t)
    ~[signer-seckey-1:hel]
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb-multisig:hel names gift fee 2 pubkey-hashes sign-keys default-refund-unit:hel get-note %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  =/  total-gift=coins:t
    (sum-gifts-for-lock:hel spends (multisig-lock-root-from-set:hel 2 pubkey-hashes))
  =/  total-fee=coins:t  (roll-fees:spends:t spends)
  =/  entry-count=@  (lent ~(tap z-by:zo spends))
  =/  sp2=spend:v1:t  (~(got z-by:zo spends) name2)
  ?>  ?=(%1 -.sp2)
  =/  spend2=spend-1:v1:t  +.sp2
  =/  seeds2-count=@  ~(wyt z-in:zo seeds.spend2)
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    %+  expect-eq
      !>(2)
    !>(entry-count)
  ::
    %+  expect-eq
      !>(gift)
    !>(total-gift)
  ::
    %+  expect-eq
      !>(fee)
    !>(total-fee)
  ::
    (check-conservation:hel notes spends)
  ==
::
++  test-multisig-three-notes-complex
  ::  gift spans multiple notes with refunds
  ::
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t signer-pubkey-2:hel)
  =/  source2=hash:t  (hash:schnorr-pubkey:t signer-pubkey-3:hel)
  =/  source3=hash:t  (hash:schnorr-pubkey:t signer-pubkey-1:hel)
  =/  gift=coins:t  1.000.000.000
  =/  fee=coins:t  150.000.000
  =/  [name1=nname:t note1=nnote:t]  (build-v1-note:hel source1 600.000.000 ~)
  =/  [name2=nname:t note2=nnote:t]  (build-v1-note:hel source2 500.000.000 ~)
  =/  [name3=nname:t note3=nnote:t]  (build-v1-note:hel source3 300.000.000 ~)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1] [name2 note2] [name3 note3]])
  =/  names=(list nname:t)  ~[name1 name2 name3]
  =/  pubkey-hashes=(z-set:zo hash:t)
    (build-pubkey-hashes-set:hel ~[signer-pubkey-1:hel signer-pubkey-2:hel signer-pubkey-3:hel])
  ::  only sign with key matching input locks (1-of-1)
  =/  sign-keys=(list schnorr-seckey:t)
    ~[signer-seckey-1:hel]
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb-multisig:hel names gift fee 2 pubkey-hashes sign-keys default-refund-unit:hel get-note %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  =/  total-gift=coins:t
    (sum-gifts-for-lock:hel spends (multisig-lock-root-from-set:hel 2 pubkey-hashes))
  =/  total-refund=coins:t
    (sum-gifts-for-lock:hel spends (source-lock-root:hel signer-pubkey-1:hel))
  =/  total-fee=coins:t  (roll-fees:spends:t spends)
  =/  total-assets=coins:t
    ;:  add
      assets.note1
      assets.note2
      assets.note3
    ==
  =/  expected-refund=coins:t  (sub total-assets (add gift fee))
  ;:  weld
    (check-conservation:hel notes spends)
  ::
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    %+  expect-eq
      !>(gift)
    !>(total-gift)
  ::
    %+  expect-eq
      !>(fee)
    !>(total-fee)
  ::
    %+  expect-eq
      !>(expected-refund)
    !>(total-refund)
  ==
::
++  test-multisig-refund-with-provided-pkh
  ::  custom refund PKH provided
  ::
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t signer-pubkey-2:hel)
  =/  gift=coins:t  500.000.000
  =/  fee=coins:t  50.000.000
  =/  refund-pkh=hash:t  (hash:schnorr-pubkey:t signer-pubkey-3:hel)
  =/  refund-unit=(unit hash:t)  [~ refund-pkh]
  =/  note1-assets=coins:t  :(add gift fee 100.000.000)
  =/  [name1=nname:t note1=nnote:t]
    (build-v1-note:hel source1 note1-assets ~)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1]])
  =/  names=(list nname:t)  ~[name1]
  =/  pubkey-hashes=(z-set:zo hash:t)
    (build-pubkey-hashes-set:hel ~[signer-pubkey-1:hel signer-pubkey-2:hel])
  ::  only sign with key matching input locks (1-of-1)
  =/  sign-keys=(list schnorr-seckey:t)
    ~[signer-seckey-1:hel]
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb-multisig:hel names gift fee 2 pubkey-hashes sign-keys refund-unit get-note %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  =/  custom-refund-lock-root=hash:t
    =/  lock=lock:t
      [%pkh [m=1 (z-silt:zo ~[refund-pkh])]]~
    (hash:lock:t lock)
  =/  total-refund=coins:t
    (sum-gifts-for-lock:hel spends custom-refund-lock-root)
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    (check-conservation:hel notes spends)
  ::
    %+  expect-eq
      !>(100.000.000)
    !>(total-refund)
  ==
::
++  test-multisig-refund-default-pkh
  ::  default refund uses first signer PKH
  ::
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t signer-pubkey-2:hel)
  =/  gift=coins:t  500.000.000
  =/  fee=coins:t  50.000.000
  =/  note1-assets=coins:t  :(add gift fee 100.000.000)
  =/  [name1=nname:t note1=nnote:t]
    (build-v1-note:hel source1 note1-assets ~)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1]])
  =/  names=(list nname:t)  ~[name1]
  =/  pubkey-hashes=(z-set:zo hash:t)
    (build-pubkey-hashes-set:hel ~[signer-pubkey-1:hel signer-pubkey-2:hel])
  ::  only sign with key matching input locks (1-of-1)
  =/  sign-keys=(list schnorr-seckey:t)
    ~[signer-seckey-1:hel]
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb-multisig:hel names gift fee 2 pubkey-hashes sign-keys ~ get-note %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  =/  total-refund=coins:t
    (sum-gifts-for-lock:hel spends (source-lock-root:hel signer-pubkey-1:hel))
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    (check-conservation:hel notes spends)
  ::
    %+  expect-eq
      !>(100.000.000)
    !>(total-refund)
  ==
::
++  test-multisig-spend-from-multisig-basic
  ::  spend from multisig, refund back to multisig
  ::
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t signer-pubkey-2:hel)
  =/  gift=coins:t  400.000.000
  =/  fee=coins:t  40.000.000
  =/  refund=coins:t  100.000.000
  =/  pubkey-hashes=(z-set:zo hash:t)
    (build-pubkey-hashes-set:hel ~[signer-pubkey-1:hel signer-pubkey-2:hel])
  =/  note1-assets=coins:t  :(add gift fee refund)
  ::  build note LOCKED BY multisig (not 1-of-1)
  =/  [name1=nname:t note1=nnote:t]
    (build-multisig-note:hel source1 note1-assets 2 pubkey-hashes)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1]])
  =/  names=(list nname:t)  ~[name1]
  ::  need 2 signatures to spend from 2-of-2 multisig
  =/  sign-keys=(list schnorr-seckey:t)
    ~[signer-seckey-1:hel signer-seckey-2:hel]
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb-multisig:hel names gift fee 2 pubkey-hashes sign-keys ~ get-note %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  ~?  ?=(%.n -.validation)  "test-multisig-spend-from-multisig-basic validation failed: {<p.validation>}"
  ::  when spending from multisig, both gift and refund go to multisig
  =/  total-to-multisig=coins:t
    (sum-gifts-for-lock:hel spends (multisig-lock-root-from-set:hel 2 pubkey-hashes))
  =/  total-fee=coins:t  (roll-fees:spends:t spends)
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    (check-conservation:hel notes spends)
  ::
    %+  expect-eq
      !>(fee)
    !>(total-fee)
  ::
    ::  gift + refund both go to multisig
    %+  expect-eq
      !>((add gift refund))
    !>(total-to-multisig)
  ==
::
++  test-multisig-spend-from-multisig-custom-refund
  ::  override default refund behavior when spending multisig inputs
  ::
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t signer-pubkey-2:hel)
  =/  gift=coins:t  400.000.000
  =/  fee=coins:t  40.000.000
  =/  refund=coins:t  100.000.000
  =/  refund-pkh=hash:t  (hash:schnorr-pubkey:t signer-pubkey-3:hel)
  =/  refund-unit=(unit hash:t)  [~ refund-pkh]
  =/  pubkey-hashes=(z-set:zo hash:t)
    (build-pubkey-hashes-set:hel ~[signer-pubkey-1:hel signer-pubkey-2:hel signer-pubkey-3:hel])
  =/  note1-assets=coins:t  :(add gift fee refund)
  =/  [name1=nname:t note1=nnote:t]
    (build-multisig-note:hel source1 note1-assets 3 pubkey-hashes)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1]])
  =/  names=(list nname:t)  ~[name1]
  =/  sign-keys=(list schnorr-seckey:t)
    ~[signer-seckey-1:hel signer-seckey-2:hel signer-seckey-3:hel]
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb-multisig:hel names gift fee 2 pubkey-hashes sign-keys refund-unit get-note %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  =/  custom-refund-lock-root=hash:t
    %-  hash:lock:t
    [%pkh [m=1 (z-silt:zo ~[refund-pkh])]]~
  =/  multisig-lock=hash:t  (multisig-lock-root-from-set:hel 2 pubkey-hashes)
  =/  total-to-multisig=coins:t
    (sum-gifts-for-lock:hel spends multisig-lock)
  =/  total-refund=coins:t
    (sum-gifts-for-lock:hel spends custom-refund-lock-root)
  =/  total-fees=coins:t  (roll-fees:spends:t spends)
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    (check-conservation:hel notes spends)
  ::
    %+  expect-eq
      !>(fee)
    !>(total-fees)
  ::
    %+  expect-eq
      !>(gift)
    !>(total-to-multisig)
  ::
    %+  expect-eq
      !>(refund)
    !>(total-refund)
  ==
::
++  test-multisig-spend-from-multisig-multiple-notes
  ::  spend from multiple multisig notes, refund to multisig
  ::
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t signer-pubkey-2:hel)
  =/  source2=hash:t  (hash:schnorr-pubkey:t signer-pubkey-3:hel)
  =/  gift=coins:t  600.000.000
  =/  fee=coins:t  80.000.000
  =/  pubkey-hashes=(z-set:zo hash:t)
    (build-pubkey-hashes-set:hel ~[signer-pubkey-1:hel signer-pubkey-2:hel signer-pubkey-3:hel])
  ::  both notes locked by multisig
  =/  [name1=nname:t note1=nnote:t]
    (build-multisig-note:hel source1 400.000.000 2 pubkey-hashes)
  =/  [name2=nname:t note2=nnote:t]
    (build-multisig-note:hel source2 500.000.000 2 pubkey-hashes)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1] [name2 note2]])
  =/  names=(list nname:t)  ~[name1 name2]
  ::  need 2 signatures to spend from 2-of-3 multisig
  =/  sign-keys=(list schnorr-seckey:t)
    ~[signer-seckey-1:hel signer-seckey-2:hel]
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb-multisig:hel names gift fee 2 pubkey-hashes sign-keys ~ get-note %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  ~?  ?=(%.n -.validation)  "test-multisig-spend-from-multisig-multiple-notes validation failed: {<p.validation>}"
  ::  when spending from multisig, both gift and refund go to multisig
  =/  total-to-multisig=coins:t
    (sum-gifts-for-lock:hel spends (multisig-lock-root-from-set:hel 2 pubkey-hashes))
  =/  total-assets=coins:t  (add assets.note1 assets.note2)
  =/  expected-to-multisig=coins:t  (sub total-assets fee)
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    (check-conservation:hel notes spends)
  ::
    ::  all non-fee value goes to multisig (gift + refund)
    %+  expect-eq
      !>(expected-to-multisig)
    !>(total-to-multisig)
  ==
::
++  test-multisig-input-lock-mismatch
  ::  multisig inputs require every note share the same lock
  ::
  ^-  tang
  =/  source-multi=hash:t  (hash:schnorr-pubkey:t signer-pubkey-2:hel)
  =/  source-single=hash:t  (hash:schnorr-pubkey:t signer-pubkey-3:hel)
  =/  gift=coins:t  400.000.000
  =/  fee=coins:t  40.000.000
  =/  pubkey-hashes=(z-set:zo hash:t)
    (build-pubkey-hashes-set:hel ~[signer-pubkey-1:hel signer-pubkey-2:hel])
  =/  note1-assets=coins:t  :(add gift fee 50.000.000)
  =/  [name1=nname:t note1=nnote:t]
    (build-multisig-note:hel source-multi note1-assets 2 pubkey-hashes)
  =/  [name2=nname:t note2=nnote:t]
    (build-v1-note:hel source-single 100.000.000 `signer-pubkey-3:hel)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1] [name2 note2]])
  =/  names=(list nname:t)  ~[name1 name2]
  =/  sign-keys=(list schnorr-seckey:t)
    ~[signer-seckey-1:hel signer-seckey-2:hel signer-seckey-3:hel]
  =/  get-note  (get-note-from:hel notes)
    %+  expect-fail
      |.((txb-multisig:hel names gift fee 2 pubkey-hashes sign-keys default-refund-unit:hel get-note %desc))
  `"Multisig detected in input. When a multisig is present, all inputs must share the same lock."
::
++  test-multisig-mixed-inputs
  ::  create multisig from multiple 1-of-1 notes, verify refunds to source
  ::
  ^-  tang
  =/  source1=hash:t  (hash:schnorr-pubkey:t signer-pubkey-2:hel)
  =/  source2=hash:t  (hash:schnorr-pubkey:t signer-pubkey-3:hel)
  =/  gift=coins:t  600.000.000
  =/  fee=coins:t  75.000.000
  =/  pubkey-hashes=(z-set:zo hash:t)
    (build-pubkey-hashes-set:hel ~[signer-pubkey-1:hel signer-pubkey-2:hel])
  ::  both notes locked by simple 1-of-1 with signer-pubkey-1:hel
  =/  [name1=nname:t note1=nnote:t]
    (build-v1-note:hel source1 400.000.000 ~)
  =/  [name2=nname:t note2=nnote:t]
    (build-v1-note:hel source2 500.000.000 ~)
  =/  notes=(z-map:zo nname:t nnote:t)
    (build-notes-map:hel ~[[name1 note1] [name2 note2]])
  =/  names=(list nname:t)  ~[name1 name2]
  ::  only sign with key matching input locks (1-of-1)
  =/  sign-keys=(list schnorr-seckey:t)
    ~[signer-seckey-1:hel]
  =/  get-note  (get-note-from:hel notes)
  =/  spends=spends:t
    (txb-multisig:hel names gift fee 2 pubkey-hashes sign-keys ~ get-note %desc)
  =/  validation=(reason:t ~)
    %-  validate-with-notes:hel
  [notes spends 10]
  ~?  ?=(%.n -.validation)  "test-multisig-mixed-inputs validation failed: {<p.validation>}"
  ::  gift goes to multisig
  =/  gift-to-multisig=coins:t
    (sum-gifts-for-lock:hel spends (multisig-lock-root-from-set:hel 2 pubkey-hashes))
  ::  refunds go back to source locks
  =/  refund-to-source=coins:t
    (sum-gifts-for-lock:hel spends (source-lock-root:hel signer-pubkey-1:hel))
  =/  total-assets=coins:t  (add assets.note1 assets.note2)
  =/  expected-refund=coins:t  (sub total-assets (add gift fee))
  ;:  weld
    %+  expect-eq
      !>(%.y)
    !>(-.validation)
  ::
    (check-conservation:hel notes spends)
  ::
    ::  verify gift went to multisig
    %+  expect-eq
      !>(gift)
    !>(gift-to-multisig)
  ::
    ::  verify refunds went back to source
    %+  expect-eq
      !>(expected-refund)
    !>(refund-to-source)
  ==
--
