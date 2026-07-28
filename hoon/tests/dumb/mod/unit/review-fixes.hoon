::  tests/dumb/mod/unit/review-fixes.hoon
::
::    unit tests for three bugs fixed in PR review:
::      fix-1: miner emits candidate block's own target, not targets[parent]
::      fix-2: height check runs before ASERT walk in validate-page-without-txs
::      fix-3a: kernel-state-6 decodes with pre-ASERT constants shape
/=  helpers  /tests/dumb/helpers
/=  dcon     /apps/dumbnet/lib/consensus
/=  dmin     /apps/dumbnet/lib/miner
/=  txe      /common/tx-engine
/=  *        /apps/dumbnet/lib/types
/=  *        /common/zeke
/=  *        /common/h-zoon
/=  *        /common/test
::
=>
|%
::  bc-fix: low phase.zk-asert so tests cross activation without building
::  thousands of blocks. mirrors bc-asert in asert-activation.hoon.
::  v1-phase must be <= phase.zk-asert (enforced by inner.hoon load arm),
::  and the height-5 candidate built by add-n-pages must be v1 so it
::  carries the post-activation fund slot — otherwise consensus rejects
::  the block via +check-fund-split.
++  bc-fix
  %*  .  default-bc:helpers
    blocks-per-epoch            1.000.000
    v1-phase                    5
    phase.zk-asert                 5
    anchor-height.zk-asert         4
    anchor-target-atom.zk-asert    ^~((div max-tip5-atom:tip5 (bex 14)))
    ideal-block-time.zk-asert      150
    half-life.zk-asert             43.200
    anchor-min-timestamp.zk-asert  (add (time-in-secs:page:txe *@da) 1.200)
  ==
--
::
|%
++  h  ~(. helpers bc-fix)
++  t  ~(. txe bc-fix)
::  +der: pre-activation derived-state (read-only extra arg for consensus/miner doors)
++  der  ^-  derived-state  *derived-state
::
::  fix-1 (a): post-ASERT, the candidate's stored target must differ from
::  targets[parent]. targets[parent.digest] holds the *parent's own* ASERT
::  target (computed at parent's height); the candidate's target is computed
::  at candidate height = parent height + 1. with 600 s block spacing vs
::  150 s ideal the chain runs slow, so each successive height gets a
::  strictly larger target — the two are never equal.
++  test-fix1-candidate-target-differs-from-targets-parent
  =/  bc   bc-fix
  =/  con  (initial-consensus-state-custom:h bc)
  ::  build 5 blocks (heights 1-5); height 5 is the first post-ASERT block
  =^  _=page:t  con  (add-n-pages:h 5 con default-retain:h)
  =/  m=mining-state  initial-mining-state:h
  =/  new-m=mining-state
    (~(heard-new-block dmin m der bc) con *@da)
  =/  cand=page:t  candidate-block.new-m
  ::  old buggy lookup: targets[parent-digest] = parent's own ASERT target
  =/  parent-digest  ~(parent get:page:t cand)
  =/  stale-target   (~(got h-by targets.con) parent-digest)
  ::  correct value: candidate's own stored target field
  =/  cand-target    ~(target get:page:t cand)
  %+  expect-eq  !>(%.y)
  !>(!=(stale-target cand-target))
::
::  fix-1 (b): the candidate's target field equals compute-target-asert at
::  candidate height. this pins that the miner stores the right value and
::  that reading it directly (the fix) gives the correct target.
++  test-fix1-candidate-target-matches-compute-target-asert
  =/  bc   bc-fix
  =/  con  (initial-consensus-state-custom:h bc)
  =^  _=page:t  con  (add-n-pages:h 5 con default-retain:h)
  =/  m=mining-state  initial-mining-state:h
  =/  new-m=mining-state
    (~(heard-new-block dmin m der bc) con *@da)
  =/  cand=page:t   candidate-block.new-m
  =/  cand-height   ~(height get:page:t cand)
  =/  parent-digest  ~(parent get:page:t cand)
  =/  expected  (~(compute-target-asert dcon con der bc) %zk cand-height parent-digest)
  =/  got       ~(target get:page:t cand)
  %+  expect-eq  !>(expected)  !>(got)
::
::  fix-2: a block whose parent is a pre-anchor block (e.g., genesis) but
::  that falsely claims a height >= phase.zk-asert must return %page-height-invalid
::  rather than crashing inside find-anchor-min-ts. before the fix, the ASERT
::  target computation ran first and walked ancestry looking for the anchor
::  height, which doesn't exist in a pre-anchor chain, crashing on a missing
::  got key.
++  test-fix2-fake-post-asert-height-returns-height-invalid
  =/  bc   bc-fix
  =/  con  (initial-consensus-state-custom:h bc)
  ::  genesis is the only block in state; it is the known parent.
  =/  genesis=page:t
    (to-page:local-page:t (~(got h-by blocks.con) (need heaviest-block.con)))
  ::  honest child of genesis (height 1, epoch-counter 1, timestamp 600)
  =/  honest=page:t  (make-empty-page:h genesis)
  ::  overwrite height with a fake post-ASERT value and recompute the digest
  =/  fake-height  phase.zk-asert.bc
  =/  faked=page:t
    =/  p
      ?^  -.honest
        honest(height fake-height)
      honest(height fake-height)
    =/  d  (compute-digest:page:t p)
    ?^  -.p  p(digest d)
    p(digest d)
  ::  validate: must return [%.n %page-height-invalid], not crash
  =/  now-secs  ~(timestamp get:page:t faked)
  =/  result
    (~(validate-page-without-txs dcon con der bc) faked now-secs)
  ;:  weld
    %+  expect-eq  !>(%.n)               !>(-.result)
    %+  expect-eq  !>(%page-height-invalid)  !>(+.result)
  ==
::
::  fix-3a: kernel-state-6 uses blockchain-constants-v1-pre-asert (the
::  frozen pre-ASERT shape, no asert-* fields). kernel-state-7 uses the
::  frozen phase-1 shape (five asert-* fields, no
::  anchor-min-timestamp.zk-asert). their default constants nouns must
::  differ — the pre-ASERT type has five fewer trailing fields,
::  producing a shorter noun.
++  test-fix3a-kernel-state-6-constants-shape-pre-asert
  =/  k6=kernel-state-6  *kernel-state-6
  =/  k7=kernel-state-7  *kernel-state-7
  %+  expect-eq  !>(%.y)
  !>(!=(constants.k6 constants.k7))
::
::  phase-2: kernel-state-7 uses the frozen phase-1 shape (five asert-*
::  fields). kernel-state-8 uses the full post-phase-2 shape (six asert-*
::  fields including anchor-min-timestamp.zk-asert). their default constants
::  nouns must differ.
++  test-phase-2-kernel-state-7-constants-shape-phase-1
  =/  k7=kernel-state-7  *kernel-state-7
  =/  k8=kernel-state-8  *kernel-state-8
  %+  expect-eq  !>(%.y)
  !>(!=(constants.k7 constants.k8))
::
::  fix-3 (finding 3): compute-work clamps to minimum 1 so blocks at
::  max-target still contribute non-zero accumulated work. before the fix,
::  max-target-atom / (max-target-atom + 1) integer-divides to 0, making
::  a max-target block unable to advance the heaviest chain.
::
::  the late-upgrade guard (finding 4) fires on mainnet only — it compares
::  the genesis message hash against realnet-genesis-msg:dk, which never
::  matches in fakenet tests. no test is written for it.
++  test-fix3-max-target-work-nonzero
  =/  t    ~(. txe bc-fix)
  =/  work=bignum:bignum  (compute-work:page:t max-target:t)
  %+  expect-eq  !>(%.y)
  !>((gth (merge:bignum work) 0))
::
::  phase-2 cutover: pin both checkpoint entries at the ASERT anchor and
::  the first ASERT block. paired with the validate-page-without-txs
::  checkpoint check, any competing block at either height is rejected
::  network-wide once a node treats its genesis as realnet.
::
::  reproduce the canonical mainnet block digests at heights 65,499 and
::  65,500 (observed from the canonical chain at phase-2 cutover time).
++  test-phase-2-checkpoint-65499-pinned
  =/  cd  ~(checkpointed-digests dcon *consensus-state der bc-fix)
  =/  expected
    (from-b58:hash:txe 'vYekzUpi6o95oA6qHfvcq9kVRzFMZLuUw33YxXQRqNCvBHwU7wys73')
  ?>  (~(has z-by cd) %65.499)
  %+  expect-eq  !>(expected)
  !>((~(got z-by cd) %65.499))
::
++  test-phase-2-checkpoint-65500-pinned
  =/  cd  ~(checkpointed-digests dcon *consensus-state der bc-fix)
  =/  expected
    (from-b58:hash:txe '4dr8f3hWcQfgSMUrKRcNb1Z4nwzECbbUuqDYUp8G4WF6G5ocFXzPp2')
  ?>  (~(has z-by cd) %65.500)
  %+  expect-eq  !>(expected)
  !>((~(got z-by cd) %65.500))
::
::  the anchor and activation digests are distinct hashes — sanity that
::  the cutover values weren't transcribed identically.
++  test-phase-2-checkpoint-anchor-and-activation-differ
  =/  cd  ~(checkpointed-digests dcon *consensus-state der bc-fix)
  =/  anchor      (~(got z-by cd) %65.499)
  =/  activation  (~(got z-by cd) %65.500)
  %+  expect-eq  !>(%.y)
  !>(!=(anchor activation))
::
::  phase-2 cutover: anchor-min-timestamp.zk-asert is now a load-bearing
::  blockchain-constants field. the realnet bunt must pin the canonical
::  mainnet median-of-11 at the anchor block (height 65,499), observed
::  from the live chain via min-timestamps[asert-anchor-digest]. shipping
::  with 0 (the type-bunt default) would compute every post-activation
::  target against a zero anchor min-ts, corrupting consensus.
++  test-phase-2-asert-anchor-min-timestamp-pinned
  =/  default-bc=blockchain-constants:txe  *blockchain-constants:txe
  %+  expect-eq  !>(9.223.372.093.639.027.842)
  !>(anchor-min-timestamp.zk-asert.default-bc)
--
