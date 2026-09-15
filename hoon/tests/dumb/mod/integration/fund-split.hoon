::  tests/dumb/mod/integration/fund-split.hoon
::
::    integration tests for the post-activation fund-split consensus
::    rule (014-aletheia). Drives `validate-page-with-txs` directly
::    with hand-crafted post-activation coinbases and verifies that:
::      - honest 1-or-2-miner + fund splits accept,
::      - any deviation in the fund slot rejects with
::        %improper-fund-split (fund missing, wrong amount, wrong key),
::      - the entry-count cap (max-coinbase-split + 1 = 3) holds.
::    Pattern matches mod/integration/asert-activation.hoon: we use a
::    custom blockchain-constants with v1-phase=5 / phase.zk-asert=5 so a
::    handful of blocks reaches the post-activation regime.
/=  helpers  /tests/dumb/helpers
/=  dcon  /apps/dumbnet/lib/consensus
/=  txe   /common/tx-engine
/=  *  /apps/dumbnet/lib/types
/=  *  /common/zoon
/=  *  /common/zeke
/=  *  /common/test
::
=>
|%
::  bc-fund-split: low v1-phase / phase.zk-asert so we reach post-activation
::    in a few blocks. Heights 1..4 are v0; height 5+ is v1 post-activation.
++  bc-fund-split
  %*  .  default-bc:helpers
    blocks-per-epoch            1.000.000      :: avoid epoch boundary inside test
    v1-phase                    5
    phase.zk-asert                 5
    anchor-height.zk-asert         4
    anchor-target-atom.zk-asert    ^~((div max-tip5-atom:tip5 (bex 14)))
    ideal-block-time.zk-asert      150
    half-life.zk-asert             43.200
    anchor-min-timestamp.zk-asert  (add (time-in-secs:page:txe *@da) 1.200)
  ==
::
::  bc-pre-activation-v1: opens a real pre-asert-activation v1 window
::    by separating v1-phase from phase.zk-asert. Heights 1 are v0; heights
::    2..4 are v1 pre-asert-activation; height 5+ is v1 post-activation.
::    Used by test-fund-split-rejects-pre-activation-three-entries to
::    pin the consensus-layer height-aware count rule
::    (docs/2026-05-01-MR2545-EMISSIONS-REVIEW.md P1 #1).
++  bc-pre-activation-v1
  %*  .  default-bc:helpers
    blocks-per-epoch            1.000.000
    v1-phase                    2
    phase.zk-asert                 5
    anchor-height.zk-asert         4
    anchor-target-atom.zk-asert    ^~((div max-tip5-atom:tip5 (bex 14)))
    ideal-block-time.zk-asert      150
    half-life.zk-asert             43.200
    anchor-min-timestamp.zk-asert  (add (time-in-secs:page:txe *@da) 1.200)
  ==
::
--
::
|%
++  h  ~(. helpers bc-fund-split)
++  t  ~(. txe bc-fund-split)
++  h-pre  ~(. helpers bc-pre-activation-v1)
++  t-pre  ~(. txe bc-pre-activation-v1)
::  +der: pre-activation derived-state (read-only extra arg for consensus door)
++  der  ^-  derived-state  *derived-state
::
::  +with-coinbase-and-rehash: replace a v1 page's coinbase with the
::    given map and recompute the page digest. Mirrors the digest
::    mutation pattern in mod/integration/asert-activation.hoon (the
::    timestamp/digest tweak around B2).
++  with-coinbase-and-rehash
  |=  [pag=page:t cb=(z-map hash:t coins:t)]
  ^-  page:t
  =/  with-cb=page:t
    ?^  -.pag  pag                          :: v0 path: pass through (unused here)
    pag(coinbase cb)
  =/  d  (compute-digest:page:t with-cb)
  ?^  -.with-cb  with-cb(digest d)
  with-cb(digest d)
::
::  +second-pkh: a non-zero, non-protocol-fund-address hash for the second
::    miner-side recipient. Derived from default-a-pt-2 so it is
::    distinct from default-keys-1's pubkey hash.
++  second-pkh
  ^-  hash:t
  (hash:schnorr-pubkey:t default-a-pt-2:helpers)
::
::  +third-pkh: third arbitrary recipient hash for malformed coinbases.
++  third-pkh
  ^-  hash:t
  (hash:schnorr-pubkey:t default-a-pt-3:helpers)
::
::  +first-miner-pkh: the single miner pubkey hash that the helpers
::    seed every block with.
++  first-miner-pkh
  ^-  hash:t
  (hash:schnorr-pubkey:t default-a-pt-1:helpers)
::
::  +height-five-emission: emission scheduled for block height 5.
::    block 5 is well within eon 0 of the schedule (eon-0 ends at
::    13,150), so emission is the eon-0 reward of 65,536 NOCK.
++  height-five-emission
  ^-  coins:t
  (emission-calc:coinbase:t 5)
::
::  +height-three-emission: emission scheduled for block height 3.
::    Same eon-0 reward as height-five-emission (height 3 is also
::    deep within eon-0). Used by the pre-asert-activation v1 tests.
++  height-three-emission
  ^-  coins:t-pre
  (emission-calc:coinbase:t-pre 3)
::
::  +rejection-reason: extract the reject reason from a (reason tx-acc).
::    panics if the result is %.y, which would mean the test setup is
::    wrong (the malformed page somehow validated).
++  rejection-reason
  |=  r=(reason tx-acc:t)
  ^-  @tas
  ?:  ?=(%.y -.r)  ~|('expected rejection, got acceptance' !!)
  p.r
--
::
|%
::
::  +test-fund-split-accepts-honest-block: building 5 blocks with the
::    helpers reaches height 5, the first post-activation block. The
::    candidate is constructed by ++new-candidate which calls
::    ++new-with-fund-share, and add-n-pages runs validate-page-with-txs
::    against every block. Reaching height 5 without crash proves the
::    honest path validates.
++  test-fund-split-accepts-honest-block
  =/  con  (initial-consensus-state-custom:h bc-fund-split)
  =^  pag=page:t  con  (add-n-pages:h 5 con default-retain:h)
  (expect-eq !>(5) !>(~(height get:page:t pag)))
::
::  +test-fund-split-accepts-two-miners-plus-fund: post-activation
::    candidate with 3 entries — 2 miner PKHs + protocol-fund-address at
::    floor(emission/5). The total adds to emission, the fund slot
::    matches, so validate-page-with-txs accepts. Pins the multi-miner
::    semantic preserved by Fix 2.
++  test-fund-split-accepts-two-miners-plus-fund
  =/  con  (initial-consensus-state-custom:h bc-fund-split)
  =^  par=page:t  con  (add-n-pages:h 4 con default-retain:h)
  =/  base    (make-empty-page:h par)
  =/  emi     height-five-emission
  =/  fund    (div emi 5)
  =/  pool    (sub emi fund)
  =/  share-a  (div pool 2)
  =/  share-b  (sub pool share-a)
  =/  bad-cb=(z-map hash:t coins:t)
    %-  ~(gas z-by *(z-map hash:t coins:t))
    :~  [first-miner-pkh share-a]
        [second-pkh share-b]
        [protocol-fund-address:t fund]
    ==
  =/  pag  (with-coinbase-and-rehash base bad-cb)
  =/  r=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der bc-fund-split) pag)
  (expect-eq !>(%.y) !>(?=(%.y -.r)))
::
::  +test-fund-split-rejects-no-fund-slot: 2-entry post-activation
::    coinbase that totals to emission but neither key is the fund
::    address. Expect %improper-fund-split.
++  test-fund-split-rejects-no-fund-slot
  =/  con  (initial-consensus-state-custom:h bc-fund-split)
  =^  par=page:t  con  (add-n-pages:h 4 con default-retain:h)
  =/  base  (make-empty-page:h par)
  =/  emi   height-five-emission
  =/  half  (div emi 2)
  =/  bad-cb=(z-map hash:t coins:t)
    %-  ~(gas z-by *(z-map hash:t coins:t))
    :~  [first-miner-pkh half]
        [second-pkh (sub emi half)]
    ==
  =/  pag  (with-coinbase-and-rehash base bad-cb)
  =/  r=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der bc-fund-split) pag)
  (expect-eq !>(%improper-fund-split) !>((rejection-reason r)))
::
::  +test-fund-split-rejects-three-entries-no-fund-slot: 3-entry
::    post-activation coinbase that totals to emission but no key is
::    the fund address. Pre-Fix-2 this was rejected by the strict
::    `=(2 ~(wyt z-by +.cb))` pin; post-Fix-2 it must still reject
::    because the fund slot is missing.
++  test-fund-split-rejects-three-entries-no-fund-slot
  =/  con  (initial-consensus-state-custom:h bc-fund-split)
  =^  par=page:t  con  (add-n-pages:h 4 con default-retain:h)
  =/  base  (make-empty-page:h par)
  =/  emi   height-five-emission
  =/  third  (div emi 3)
  =/  rest   (sub emi third)
  =/  half   (div rest 2)
  =/  bad-cb=(z-map hash:t coins:t)
    %-  ~(gas z-by *(z-map hash:t coins:t))
    :~  [first-miner-pkh half]
        [second-pkh (sub rest half)]
        [third-pkh third]
    ==
  =/  pag  (with-coinbase-and-rehash base bad-cb)
  =/  r=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der bc-fund-split) pag)
  (expect-eq !>(%improper-fund-split) !>((rejection-reason r)))
::
::  +test-fund-split-rejects-wrong-fund-share: 2-entry post-activation
::    coinbase with the fund slot present but with a non-canonical
::    amount (off by one atom). Total-split still equals emission, so
::    the rejection comes from check-fund-split alone.
++  test-fund-split-rejects-wrong-fund-share
  =/  con  (initial-consensus-state-custom:h bc-fund-split)
  =^  par=page:t  con  (add-n-pages:h 4 con default-retain:h)
  =/  base  (make-empty-page:h par)
  =/  emi   height-five-emission
  =/  fund-correct   (div emi 5)
  =/  fund-bad       +(fund-correct)        :: off by 1 atom
  =/  miner-bad      (sub emi fund-bad)
  =/  bad-cb=(z-map hash:t coins:t)
    %-  ~(gas z-by *(z-map hash:t coins:t))
    :~  [first-miner-pkh miner-bad]
        [protocol-fund-address:t fund-bad]
    ==
  =/  pag  (with-coinbase-and-rehash base bad-cb)
  =/  r=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der bc-fund-split) pag)
  (expect-eq !>(%improper-fund-split) !>((rejection-reason r)))
::
::  +test-fund-split-rejects-wrong-protocol-fund-address: 2-entry post-activation
::    coinbase totalling emission, with the canonical fund amount but
::    that amount paid to a non-protocol-fund-address hash. The fund slot is
::    therefore absent — rejected.
++  test-fund-split-rejects-wrong-protocol-fund-address
  =/  con  (initial-consensus-state-custom:h bc-fund-split)
  =^  par=page:t  con  (add-n-pages:h 4 con default-retain:h)
  =/  base  (make-empty-page:h par)
  =/  emi   height-five-emission
  =/  fund   (div emi 5)
  =/  miner  (sub emi fund)
  =/  bad-cb=(z-map hash:t coins:t)
    %-  ~(gas z-by *(z-map hash:t coins:t))
    :~  [first-miner-pkh miner]
        [second-pkh fund]                    :: NOT protocol-fund-address
    ==
  =/  pag  (with-coinbase-and-rehash base bad-cb)
  =/  r=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der bc-fund-split) pag)
  (expect-eq !>(%improper-fund-split) !>((rejection-reason r)))
::
::  +test-fund-split-rejects-pre-activation-three-entries: closes the gap
::    flagged in docs/2026-05-01-MR2545-EMISSIONS-REVIEW.md P1 #1. The v1
::    parser-level shape check (++based:coinbase-split:v1) admits
::    `max-coinbase-split + 1` entries to leave room for the post-
::    activation fund slot, but pre-asert-activation v1 blocks must not
::    use that extra entry — they have no fund slot. With
::    bc-pre-activation-v1 (v1-phase=2, phase.zk-asert=5) heights 2..4 are
::    a real pre-asert-activation v1 window. Build to height 2 (so the
::    parent is v1) and hand-craft a 3-entry coinbase at height 3.
::    Without the consensus-layer phase-gated count check this block
::    would validate; with the fix it rejects as
::    %coinbase-split-pre-activation-too-many.
++  test-fund-split-rejects-pre-activation-three-entries
  =/  con  (initial-consensus-state-custom:h-pre bc-pre-activation-v1)
  =^  par=page:t-pre  con  (add-n-pages:h-pre 2 con default-retain:h-pre)
  =/  base  (make-empty-page:h-pre par)
  =/  emi   height-three-emission
  =/  third  (div emi 3)
  =/  rest   (sub emi third)
  =/  half   (div rest 2)
  =/  bad-cb=(z-map hash:t-pre coins:t-pre)
    %-  ~(gas z-by *(z-map hash:t-pre coins:t-pre))
    :~  [first-miner-pkh half]
        [second-pkh (sub rest half)]
        [third-pkh third]
    ==
  =/  pag  (with-coinbase-and-rehash base bad-cb)
  =/  r=(reason tx-acc:t-pre)
    (~(validate-page-with-txs dcon con der bc-pre-activation-v1) pag)
  (expect-eq !>(%coinbase-split-pre-activation-too-many) !>((rejection-reason r)))
::
::  +test-fund-split-accepts-pre-activation-two-entries: complement to the
::    rejection test above — confirms the new phase-gated count rule does
::    not over-reject pre-asert-activation v1 blocks. A 2-entry coinbase
::    (max-coinbase-split = 2, no fund slot) at height 3 is valid: it sits
::    inside the legacy `<= max-coinbase-split` budget that pre-activation
::    v1 must continue to honor.
++  test-fund-split-accepts-pre-activation-two-entries
  =/  con  (initial-consensus-state-custom:h-pre bc-pre-activation-v1)
  =^  par=page:t-pre  con  (add-n-pages:h-pre 2 con default-retain:h-pre)
  =/  base  (make-empty-page:h-pre par)
  =/  emi   height-three-emission
  =/  half  (div emi 2)
  =/  ok-cb=(z-map hash:t-pre coins:t-pre)
    %-  ~(gas z-by *(z-map hash:t-pre coins:t-pre))
    :~  [first-miner-pkh half]
        [second-pkh (sub emi half)]
    ==
  =/  pag  (with-coinbase-and-rehash base ok-cb)
  =/  r=(reason tx-acc:t-pre)
    (~(validate-page-with-txs dcon con der bc-pre-activation-v1) pag)
  (expect-eq !>(%.y) !>(?=(%.y -.r)))
::
::  +test-fund-split-post-cap-zero-emission-no-fund-slot: closes the gap
::    flagged in docs/2026-05-01-MR2545-EMISSIONS-REVIEW.md P1 #2. After
::    `tail-end` the schedule emits zero subsidy, so the expected fund
::    share is 0. ++based on v1 coinbase-split rejects zero-coin entries,
::    so the only way for a post-cap block to pay the fund is to omit the
::    slot entirely. Pre-fix, ++check-fund-split unconditionally required
::    the fund slot to exist; that made post-cap blocks unmineable. This
::    is a unit test of ++check-fund-split with hand-crafted inputs since
::    reaching height tail-end+1 in an integration test is impractical
::    (16,144,876 blocks).
++  test-fund-split-post-cap-zero-emission-no-fund-slot
  =/  con  (initial-consensus-state-custom:h bc-fund-split)
  =/  cb=coinbase-split:t
    :-  %1
    (~(put z-by *(z-map hash:t coins:t)) [first-miner-pkh 100])
  (expect-eq !>(%.y) !>((~(check-fund-split dcon con der bc-fund-split) cb 0)))
::
::  +test-fund-split-post-cap-zero-emission-empty-coinbase: post-cap with
::    zero fees and zero emission — the canonical empty coinbase must
::    pass ++check-fund-split. Mirrors the no-fund-slot case but with
::    no miner output either.
++  test-fund-split-post-cap-zero-emission-empty-coinbase
  =/  con  (initial-consensus-state-custom:h bc-fund-split)
  =/  cb=coinbase-split:t  [%1 *(z-map hash:t coins:t)]
  (expect-eq !>(%.y) !>((~(check-fund-split dcon con der bc-fund-split) cb 0)))
::
::  +test-fund-split-post-cap-zero-emission-rejects-fund-slot: the
::    inverse — a post-cap block that includes a protocol-fund-address slot
::    must reject. With expected fund == 0, any positive fund slot is
::    a fee diversion (since total-split has already been pinned to
::    fees). The rule still rejects that case so the post-cap path
::    cannot launder fees through the fund.
++  test-fund-split-post-cap-zero-emission-rejects-fund-slot
  =/  con  (initial-consensus-state-custom:h bc-fund-split)
  =/  cb=coinbase-split:t
    :-  %1
    %-  ~(gas z-by *(z-map hash:t coins:t))
    :~  [first-miner-pkh 50]
        [protocol-fund-address:t 50]
    ==
  (expect-eq !>(%.n) !>((~(check-fund-split dcon con der bc-fund-split) cb 0)))
::
--
