::  tests/dumb/mod/unit/coinbase-split.hoon
::
::    unit tests for the post-activation 80/20 coinbase split.
::    Verifies that:
::      - the pre-activation single-output path is unchanged,
::      - ++ new-with-fund-share produces fund = floor(emission/5) and
::        the miner pool = (emission - fund) + fees, distributed across
::        `shares` proportionally,
::      - fees never enter the fund slot,
::      - multi-miner partner mode keeps working (1-or-2 miner PKHs +
::        fund),
::      - the fund's lock-script hash equals the consensus-known
::        fund-address constant,
::      - the builder rejects shares that alias fund-address.
/=  tx-engine  /common/tx-engine
/=  emission  /common/schedule
/=  helpers   /tests/dumb/helpers
/=  *  /common/zoon
/=  *  /common/test
|_  constants=blockchain-constants:tx-engine
+*  t  ~(. tx-engine constants)
    h  ~(. helpers constants)
::
::  +test-split-pre-activation: with shares = {(miner, 1)} and ++ new
::    (the legacy proportional-allocation arm), the v1 coinbase-split
::    has exactly one output and gives 100% of the emission to the
::    miner.
++  test-split-pre-activation  ^-  tang
  =/  miner-shares  default-keys-1-share-v1:h
  =/  emission-atoms=coins:t  (mul 16.384 atoms-per-nock:emission)
  =/  cb=coinbase-split:v1:t
    (new:v1:coinbase-split:t emission-atoms miner-shares)
  =/  entries=(list [hash:t coins:t])  ~(tap z-by cb)
  =/  total=coins:t
    %+  roll  ~(val z-by cb)
    |=  [c=coins:t s=coins:t]  (add c s)
  ;:  weld
    (expect-eq !>(1) !>((lent entries)))
    (expect-eq !>(emission-atoms) !>(total))
  ==
::
::  +test-split-post-activation: with shares = {(miner, 1)} and zero
::    fees, the post-activation builder produces exactly two outputs.
::    fund gets floor(emission/5), miner gets the residual.
++  test-split-post-activation  ^-  tang
  =/  miner-shares  default-keys-1-share-v1:h
  =/  emission-atoms=coins:t  (mul 2.048 atoms-per-nock:emission)
  =/  cb=coinbase-split:v1:t
    (new-with-fund-share:v1:coinbase-split:t emission-atoms 0 miner-shares)
  =/  expected-fund=coins:t  (div emission-atoms 5)
  =/  expected-miner=coins:t  (sub emission-atoms expected-fund)
  =/  fund-entry=(unit coins:t)
    (~(get z-by cb) fund-address:t)
  =/  miner-pkh=hash:t
    (snag 0 ~(tap z-in ~(key z-by miner-shares)))
  =/  miner-entry=(unit coins:t)  (~(get z-by cb) miner-pkh)
  =/  total=coins:t
    %+  roll  ~(val z-by cb)
    |=  [c=coins:t s=coins:t]  (add c s)
  ;:  weld
    (expect-eq !>(2) !>(~(wyt z-by cb)))
    (expect-eq !>(`(unit coins:t)`[~ expected-fund]) !>(fund-entry))
    (expect-eq !>(`(unit coins:t)`[~ expected-miner]) !>(miner-entry))
    (expect-eq !>(emission-atoms) !>(total))
  ==
::
::  +test-split-post-activation-rounding: per-block atom remainders
::    accrue to the miner. With emission=2,048 NOCK = 134,217,728
::    atoms (= 5 * 26,843,545 + 3), the fund gets 26,843,545 atoms and
::    the miner gets 26,843,545 * 4 + 3 = 107,374,183 atoms.
++  test-split-post-activation-rounding  ^-  tang
  =/  miner-shares  default-keys-1-share-v1:h
  =/  emission-atoms=coins:t  134.217.728  :: 2,048 * 2^16
  =/  cb=coinbase-split:v1:t
    (new-with-fund-share:v1:coinbase-split:t emission-atoms 0 miner-shares)
  =/  fund-entry=(unit coins:t)
    (~(get z-by cb) fund-address:t)
  =/  miner-pkh=hash:t
    (snag 0 ~(tap z-in ~(key z-by miner-shares)))
  =/  miner-entry=(unit coins:t)  (~(get z-by cb) miner-pkh)
  ;:  weld
    (expect-eq !>(`(unit coins:t)`[~ 26.843.545]) !>(fund-entry))
    (expect-eq !>(`(unit coins:t)`[~ 107.374.183]) !>(miner-entry))
  ==
::
::  +test-split-post-activation-with-fees: fees flow entirely into the
::    miner pool; the fund slot stays at floor(emission/5) regardless
::    of fee total. Pin: emission=2,048 NOCK=134,217,728; fees=12,345.
::    Expect fund=26,843,545; miner=107,374,183 + 12,345 = 107,386,528;
::    total = 134,230,073 = emission + fees.
++  test-split-post-activation-with-fees  ^-  tang
  =/  miner-shares  default-keys-1-share-v1:h
  =/  emission-atoms=coins:t  134.217.728
  =/  fee-atoms=coins:t       12.345
  =/  cb=coinbase-split:v1:t
    %-  new-with-fund-share:v1:coinbase-split:t
    [emission-atoms fee-atoms miner-shares]
  =/  fund-entry=(unit coins:t)
    (~(get z-by cb) fund-address:t)
  =/  miner-pkh=hash:t
    (snag 0 ~(tap z-in ~(key z-by miner-shares)))
  =/  miner-entry=(unit coins:t)  (~(get z-by cb) miner-pkh)
  =/  total=coins:t
    %+  roll  ~(val z-by cb)
    |=  [c=coins:t s=coins:t]  (add c s)
  ;:  weld
    (expect-eq !>(2) !>(~(wyt z-by cb)))
    (expect-eq !>(`(unit coins:t)`[~ 26.843.545]) !>(fund-entry))
    (expect-eq !>(`(unit coins:t)`[~ 107.386.528]) !>(miner-entry))
    (expect-eq !>(134.230.073) !>(total))
  ==
::
::  +test-split-post-activation-two-miners: partner mode with two miner
::    PKHs at equal weight. Pin: emission=125,000,000 (chosen so the
::    miner pool divides evenly); fees=0; shares={(a,1), (b,1)}.
::    Expect 3 entries: fund=25,000,000; each miner=50,000,000.
::    Total = 125,000,000.
++  test-split-post-activation-two-miners  ^-  tang
  =/  miner-a-pkh=hash:t  (hash:schnorr-pubkey:t default-a-pt-1:h)
  =/  miner-b-pkh=hash:t  (hash:schnorr-pubkey:t default-a-pt-2:h)
  =/  miner-shares=shares:t
    %-  ~(gas z-by *shares:t)
    :~  [miner-a-pkh 1]  [miner-b-pkh 1]
    ==
  =/  emission-atoms=coins:t  125.000.000
  =/  cb=coinbase-split:v1:t
    %-  new-with-fund-share:v1:coinbase-split:t
    [emission-atoms 0 miner-shares]
  =/  fund-entry   (~(get z-by cb) fund-address:t)
  =/  miner-a-entry  (~(get z-by cb) miner-a-pkh)
  =/  miner-b-entry  (~(get z-by cb) miner-b-pkh)
  =/  total=coins:t
    %+  roll  ~(val z-by cb)
    |=  [c=coins:t s=coins:t]  (add c s)
  ;:  weld
    (expect-eq !>(3) !>(~(wyt z-by cb)))
    (expect-eq !>(`(unit coins:t)`[~ 25.000.000]) !>(fund-entry))
    (expect-eq !>(`(unit coins:t)`[~ 50.000.000]) !>(miner-a-entry))
    (expect-eq !>(`(unit coins:t)`[~ 50.000.000]) !>(miner-b-entry))
    (expect-eq !>(125.000.000) !>(total))
  ==
::
::  +test-split-post-activation-two-miners-with-fees: fees route to the
::    miner pool, never the fund, even with multi-miner shares.
::    Pin: emission=125,000,000; fees=1,000,000; shares={(a,1), (b,1)}.
::    Expect fund=25,000,000 (unchanged); each miner=(100M+1M)/2 =
::    50,500,000; total = 126,000,000 = emission + fees.
++  test-split-post-activation-two-miners-with-fees  ^-  tang
  =/  miner-a-pkh=hash:t  (hash:schnorr-pubkey:t default-a-pt-1:h)
  =/  miner-b-pkh=hash:t  (hash:schnorr-pubkey:t default-a-pt-2:h)
  =/  miner-shares=shares:t
    %-  ~(gas z-by *shares:t)
    :~  [miner-a-pkh 1]  [miner-b-pkh 1]
    ==
  =/  emission-atoms=coins:t  125.000.000
  =/  fee-atoms=coins:t        1.000.000
  =/  cb=coinbase-split:v1:t
    %-  new-with-fund-share:v1:coinbase-split:t
    [emission-atoms fee-atoms miner-shares]
  =/  fund-entry     (~(get z-by cb) fund-address:t)
  =/  miner-a-entry  (~(get z-by cb) miner-a-pkh)
  =/  miner-b-entry  (~(get z-by cb) miner-b-pkh)
  =/  total=coins:t
    %+  roll  ~(val z-by cb)
    |=  [c=coins:t s=coins:t]  (add c s)
  ;:  weld
    (expect-eq !>(3) !>(~(wyt z-by cb)))
    (expect-eq !>(`(unit coins:t)`[~ 25.000.000]) !>(fund-entry))
    (expect-eq !>(`(unit coins:t)`[~ 50.500.000]) !>(miner-a-entry))
    (expect-eq !>(`(unit coins:t)`[~ 50.500.000]) !>(miner-b-entry))
    (expect-eq !>(126.000.000) !>(total))
  ==
::
::  +test-split-rejects-fund-in-shares: the builder must crash when
::    `shares` includes the fund-address as a miner key — that would
::    let an honest miner accidentally pay the fund slot twice.
++  test-split-rejects-fund-in-shares  ^-  tang
  =/  bad-shares=shares:t
    (~(put z-by *shares:t) fund-address:t 1)
  %+  expect-fail
    |.  (new-with-fund-share:v1:coinbase-split:t 134.217.728 0 bad-shares)
  ~
::
::  +test-split-post-activation-residual-to-first-key: pin the
::    multi-miner per-block atom remainder to the first miner key in
::    z-map sort order. With `shares = {(a,1), (b,1)}` and emission =
::    11 atoms (fund=2, miner-pool=9, residual=1 atom after equal
::    split of 4+4), the legacy ++new arm awards the leftover atom to
::    whichever pkh sorts first via z-by. The test computes that
::    ordering at runtime — z-map order is deterministic but
::    implicit, so this test exists to lock in the current behaviour
::    and would catch any future change to the z-map sort or to
::    ++new's "give-to-first" branch. See
::    014-aletheia-emissions-audit.md finding #4.
++  test-split-post-activation-residual-to-first-key  ^-  tang
  =/  miner-a-pkh=hash:t  (hash:schnorr-pubkey:t default-a-pt-1:h)
  =/  miner-b-pkh=hash:t  (hash:schnorr-pubkey:t default-a-pt-2:h)
  =/  miner-shares=shares:t
    %-  ~(gas z-by *shares:t)
    :~  [miner-a-pkh 1]  [miner-b-pkh 1]
    ==
  =/  emission-atoms=coins:t  11
  =/  cb=coinbase-split:v1:t
    %-  new-with-fund-share:v1:coinbase-split:t
    [emission-atoms 0 miner-shares]
  ::  z-by sorts keys by hash atom; first-pkh receives the residual.
  =/  ordered-keys=(list hash:t)
    ~(tap z-in ~(key z-by miner-shares))
  =/  first-pkh   (snag 0 ordered-keys)
  =/  second-pkh  (snag 1 ordered-keys)
  =/  fund-entry    (~(get z-by cb) fund-address:t)
  =/  first-entry   (~(get z-by cb) first-pkh)
  =/  second-entry  (~(get z-by cb) second-pkh)
  =/  total=coins:t
    %+  roll  ~(val z-by cb)
    |=  [c=coins:t s=coins:t]  (add c s)
  ;:  weld
    (expect-eq !>(3) !>(~(wyt z-by cb)))
    (expect-eq !>(`(unit coins:t)`[~ 2]) !>(fund-entry))
    (expect-eq !>(`(unit coins:t)`[~ 5]) !>(first-entry))
    (expect-eq !>(`(unit coins:t)`[~ 4]) !>(second-entry))
    (expect-eq !>(11) !>(total))
  ==
::
::  +test-fund-address-is-3-of-4-multisig: pin fund-address:t to the
::    lock-root of a 3-of-4 multisig over the four pkhs listed in
::    /asert-protocol-lock-fund.txt. Reconstructs the multisig lock
::    independently here (mirroring /scripts/generate-fund-address.hoon)
::    and asserts the recomputed lock-root matches the literal in
::    +fund-address. Catches drift in any of:
::      - any of the four participant pkhs,
::      - the m=3 threshold,
::      - the lock-script structure (single ++pkh primitive in a single
::        spend-condition),
::      - the +hash:lock formula.
::    Also asserts fund-address is non-zero so we never ship the
::    legacy zero-hash placeholder by accident.
++  test-fund-address-is-3-of-4-multisig  ^-  tang
  =/  pkhs=(list hash:t)
    :~  (from-b58:hash:t '7pGXggKU1AWk3d3wqX2kpKUatTqT68Cv8SQfGzGRQvJvYnQBvagSSjT')
        (from-b58:hash:t '8Mc1U7kdujhPoEwog1BfNsFDtRp8St8UQCHk84iaLdhP4cX9a2CT1MU')
        (from-b58:hash:t 'DAvp9ffoyNTBqAudZN29qc6s8GZfvvvAGvAfEFrqQsCVgSSSkg1SaSm')
        (from-b58:hash:t '9LK7wEcQsmRpEot4qFaV9bwjSE9ZD6tB1kWbgNsFkDa2LEpvBV9WGY3')
    ==
  =/  participants=(z-set hash:t)  (z-silt pkhs)
  =/  multi-lock=lock:t  [%pkh [m=3 participants]]~
  =/  expected=hash:t  (hash:lock:t multi-lock)
  ;:  weld
    (expect-eq !>(expected) !>(fund-address:t))
    (expect-eq !>(%.y) !>(!=(*hash:t fund-address:t)))
  ==
::
::  +test-fund-note-firstname: pin fund-note-firstname:t to the on-chain
::    first-name (-.name) shared by every protocol-fund coinbase note.
::    Reconstructs the wrapped coinbase note lock exactly as
::    +make-name:coinbase does -- the single %pkh primitive over {fund-address}
::    plus the immutable fund-note maturity timelock -- takes
::    (first:nname (hash:lock ...)), and asserts it matches the pinned literal
::    in +fund-note-firstname. This is
::    the value +check:check-context special-cases to recover spendability;
::    drift in the participant set, the threshold, the lock structure, the
::    +hash:lock / +first:nname formulas, or fund-note-timelock-min would all
::    move it. Mirrors /scripts/generate-fund-note-name.hoon.
++  test-fund-note-firstname  ^-  tang
  =/  pkhs=(list hash:t)
    :~  (from-b58:hash:t '7pGXggKU1AWk3d3wqX2kpKUatTqT68Cv8SQfGzGRQvJvYnQBvagSSjT')
        (from-b58:hash:t '8Mc1U7kdujhPoEwog1BfNsFDtRp8St8UQCHk84iaLdhP4cX9a2CT1MU')
        (from-b58:hash:t 'DAvp9ffoyNTBqAudZN29qc6s8GZfvvvAGvAfEFrqQsCVgSSSkg1SaSm')
        (from-b58:hash:t '9LK7wEcQsmRpEot4qFaV9bwjSE9ZD6tB1kWbgNsFkDa2LEpvBV9WGY3')
    ==
  =/  participants=(z-set hash:t)  (z-silt pkhs)
  =/  fund-addr=hash:t  (hash:lock:t [%pkh [m=3 participants]]~)
  ::  reconstruct the wrapped coinbase note lock (mirrors +make-name:coinbase)
  =/  fund-pkh-set=(z-set hash:t)  (z-silt ~[fund-addr])
  =/  pkh-prim=lock-primitive:t  [%pkh [m=1 fund-pkh-set]]
  =/  tim-prim=lock-primitive:t
    [%tim [rel=[min=`fund-note-timelock-min:t max=~] abs=[min=~ max=~]]]
  =/  note-lock=lock:t  ~[pkh-prim tim-prim]
  =/  expected=hash:t  (first:nname:t (hash:lock:t note-lock))
  ;:  weld
    (expect-eq !>(100) !>(fund-note-timelock-min:t))
    (expect-eq !>(expected) !>(fund-note-firstname:t))
    (expect-eq !>(%.y) !>(!=(*hash:t fund-note-firstname:t)))
  ==
::
::  +test-fund-multisig-lock-binds-fund-address: the 3-of-4 multisig
::    spend-condition exposed for the wallet (+fund-multisig-lock) must hash to
::    +fund-address. This is the bind +check-multisig-lock enforces, so a fund
::    spend that reveals +fund-multisig-lock is accepted by consensus. Catches
::    drift between the participant pkhs / threshold listed in
::    +fund-multisig-lock and the pinned +fund-address.
++  test-fund-multisig-lock-binds-fund-address  ^-  tang
  (expect-eq !>(fund-address:t) !>((hash:lock:t fund-multisig-lock:t)))
::
--
