::  tests/dumb/helpers.hoon
::                                                                              ::
::                                                                              ::
::                      dumbnet helpers for constructing                        ::
::                            dummy states                                      ::
::                                                                              ::
::                                                                              ::
/=  dcon  /apps/dumbnet/lib/consensus
/=  dmin  /apps/dumbnet/lib/miner
/=  dder  /apps/dumbnet/lib/derived
/=  pow-lib  /common/pow
/=  *  /common/test
/=  *  /common/zeke
/=  *  /common/h-zoon
/=  txe  /common/tx-engine
/=  *  /apps/dumbnet/lib/types
/=  nock-ker  /apps/dumbnet/outer
=>
|%
::  default blockchain constants
::  note: input-fee-divisor=1 to avoid fee calculation issues with base-fee=0
++  default-bc  %*  .  *blockchain-constants:txe
          first-month-coinbase-min  0
          check-pow-flag  %.n
          base-fee  0
          input-fee-divisor  1
          v1-phase  10
          bythos-phase  1
        ==
++  bc-no-timelock
  %*  .  default-bc
    coinbase-timelock-min  0
  ==
::
++  bc-v0-phase
  %*  .  default-bc
    v1-phase  1.000.000
    ::  v0-only: v1 never activates, so the dual-puzzle regime must not either.
    ::  From +dual-puzzle-phase heaviness is priced per puzzle, and only the v1
    ::  candidate builder knows that -- a v0 page at that height would store an
    ::  accumulated-work validation rejects. +load asserts the ordering.
    phase.zk-asert-post-ai  2.000.000
    phase.ai-asert          2.000.000
    anchor-height.zk-asert-post-ai  1.999.999
    anchor-height.ai-asert          1.999.999
  ==
::
++  bc-v1-phase
  %*  .  default-bc
    coinbase-timelock-min  0
    v1-phase  2
  ==
++  bc-with-fees
  %*  .  default-bc
    coinbase-timelock-min  0
    base-fee  256
    input-fee-divisor  4  :: new fee rebalancing: inputs pay base-fee/4
    :: activate bythos so discounted witness fees are in effect for fee tests
    bythos-phase  10
  ==
::  bc-with-old-fees: pre-rebalancing fee behavior (inputs pay full base-fee)
++  bc-with-old-fees
  %*  .  default-bc
    coinbase-timelock-min  0
    base-fee  256
    input-fee-divisor  1  :: no discount (old behavior)
  ==
++  bc-without-fees
  %*  .  default-bc
    coinbase-timelock-min  0
    base-fee  0
    input-fee-divisor  1  :: explicit: no fees means divisor irrelevant
    data  [2.048 0]
  ==
++  bc-small-size
  %*  .  default-bc
    coinbase-timelock-min  0
    base-fee  0
    input-fee-divisor  1  :: explicit: no fees means divisor irrelevant
    data  [29 0]
  ==
::  for testing merged note-data size limits when seeds share lock-roots
++  bc-merged-note-data-test
  %*  .  default-bc
    coinbase-timelock-min  0
    base-fee  0
    input-fee-divisor  1
    data  [50 0]  :: max-size=50 for testing merged note-data
  ==
++  bc-epoch  %*  .  default-bc
                blocks-per-epoch  16
              ==
++  bc-max-block-one
  %*  .  default-bc
    max-block-size  `size:txe``@`1
  ==
::
::  10 kilobyte block size
++  bc-max-block-size-medium-v0
  %*  .  default-bc
    max-block-size  `size:txe``@`(add (mul 10 (mul 8 1.024)) max-size:proof:txe)
    coinbase-timelock-min  0
    v1-phase  1.000.000
    ::  v0-only: v1 never activates, so the dual-puzzle regime must not either.
    ::  From +dual-puzzle-phase heaviness is priced per puzzle, and only the v1
    ::  candidate builder knows that -- a v0 page at that height would store an
    ::  accumulated-work validation rejects. +load asserts the ordering.
    phase.zk-asert-post-ai  2.000.000
    phase.ai-asert          2.000.000
    anchor-height.zk-asert-post-ai  1.999.999
    anchor-height.ai-asert          1.999.999
  ==
++  bc-max-block-size-medium-v1
  %*  .  default-bc
    max-block-size  `size:txe``@`(add (mul 10 (mul 8 1.024)) max-size:proof:txe)
    coinbase-timelock-min  0
    v1-phase  1
  ==
::
++  bc-pending-integration-tests
  %*  .  *blockchain-constants:txe
    first-month-coinbase-min  0
    coinbase-timelock-min  0
    check-pow-flag  %.n
    max-future-timestamp  (bex 32)
  ==
::  provable variants for KERNEL integration tests (mempool/pending): on
::  this branch the kernel's +check-pow / +check-genesis do REAL STARK
::  verification (the check-pow-flag bypass was removed from consensus),
::  so blocks poked via %heard-block must carry a real, verifiable proof.
::  These variants set the difficulty target to max (any proof clears
::  +check-target, so no nonce search is required) and pow-len=1 (proving
::  a 1-item STARK is ~2s vs ~23s at the mainnet len=64), keeping the
::  suite fast. Pages are stamped with a real proof by +prove-page below.
++  bc-v1-phase-provable
  %*  .  bc-v1-phase
    genesis-target-atom  max-tip5-atom:tip5
    pow-len              1
  ==
++  bc-pending-provable
  %*  .  bc-pending-integration-tests
    genesis-target-atom  max-tip5-atom:tip5
    pow-len              1
  ==
::  Provable variant with AI-PoW active from genesis. The structured %ai-pow
::  artifact has discriminator %4, so the mandatory verifier jet runs without
::  changing the height-selected ZK proof version.
++  bc-ai-pow-provable
  %*  .  bc-pending-provable
    ai-pow-activation-height  0
  ==
::  Dual-puzzle test config: both Logos schedules activate at height 1 and
::  pin to genesis. Interleaved blocks must not change either puzzle's virtual
::  ASERT height.
++  bc-dual-puzzle
  %*  .  bc-pending-provable
    v1-phase                       1
    ai-pow-activation-height       1
    phase.zk-asert-post-ai           1
    anchor-height.zk-asert-post-ai   0
    phase.ai-asert                   1
    anchor-height.ai-asert         0
    ::  anchor timestamp at the genesis second (chain builds from
    ::  time-in-secs(*@da)); a tiny value here would make the ASERT exponent huge
    ::  and saturate the target to max, hiding the subchain-count behaviour.
    anchor-min-timestamp.ai-asert  (time-in-secs:page:txe *@da)
  ==
::  Like bc-dual-puzzle, but both pins recover their timestamp from the
::  puzzle-keyed cache rather than a constant.
++  bc-dual-repin-cache
  %*  .  bc-dual-puzzle
    anchor-min-timestamp.ai-asert  0
  ==
::  Tandem-retargeting config: BOTH puzzles' ASERT active + in their SUBCHAIN
::  regime at low heights, with hardcoded anchors so neither degenerates. Unlike
::  bc-dual-post (whose zk-asert-post-ai.phase stays at the 95000 default, so the
::  ZK ASERT runs in regime 1 / global-height), here zk-asert-post-ai.phase is low
::  so the ZK ASERT runs in regime 2 (ZK-subchain count), matching the AI ASERT.
::  Short half-life (600s) + 300s ideal amplify the retarget so a short test chain
::  shows a clear direction. The AI anchor is the ZK anchor shifted down by 64
::  bits, so normalized targets are directly comparable. Anchor timestamps equal
::  the genesis second, making block-distance times the controlled deltas.
++  bc-tandem
  %*  .  bc-pending-provable
    v1-phase                               1
    blocks-per-epoch                       1.000.000
    phase.zk-asert                         2
    anchor-height.zk-asert                 1
    ideal-block-time.zk-asert              300
    half-life.zk-asert                     600
    anchor-min-timestamp.zk-asert          (time-in-secs:page:txe *@da)
    phase.zk-asert-post-ai                 2
    ::  Simultaneous with the ZK re-pin; +load asserts it.
    phase.ai-asert                         2
    anchor-height.zk-asert-post-ai         1
    ::  Anchors are the ZK/AI calibration pair (zk == ai * 2^64), placed where
    ::  production places them: the AI anchor must stay at or below
    ::  +max-ai-target-atom or every AI block at it is rejected for
    ::  shape-scaling overflow. `div ... (bex 29)` puts ZK at ~2^291 and AI at
    ::  ~2^227, matching the mainnet defaults. The tandem assertions compare
    ::  RETARGET factors, so they do not depend on the absolute anchor.
    anchor-target-atom.zk-asert-post-ai    ^~((div max-tip5-atom:tip5 (bex 29)))
    ideal-block-time.zk-asert-post-ai      300
    half-life.zk-asert-post-ai             600
    anchor-min-timestamp.zk-asert-post-ai  (time-in-secs:page:txe *@da)
    ai-pow-activation-height               1
    anchor-height.ai-asert                 1
    anchor-target-atom.ai-asert            ^~((rsh [0 64] (div max-tip5-atom:tip5 (bex 29))))
    ideal-block-time.ai-asert              300
    half-life.ai-asert                     600
    anchor-min-timestamp.ai-asert          (time-in-secs:page:txe *@da)
  ==
::  Post-ASERT dual-puzzle config: low zk-asert phase (2) with a hardcoded ZK
::  anchor (so accept-page/validation can compute the ZK ASERT without a cache),
::  AI active from height 1 with a hardcoded AI anchor. Lets a low-height AI block
::  travel the full +validate-page-without-txs path in the post-asert regime.
++  bc-dual-post
  %*  .  bc-pending-provable
    v1-phase                       1
    blocks-per-epoch               1.000.000
    ::  All four dual-puzzle phases sit together: +dual-puzzle-asert-phase is the
    ::  LAST of them, so leaving the post-AI ZK / AI ASERT phases at their mainnet
    ::  defaults would put the per-puzzle-priced regime 126.000 blocks above any test
    ::  chain. Mainnet has all four at 126.000 for the same reason.
    phase.zk-asert-post-ai         2
    anchor-height.zk-asert-post-ai   1
    phase.ai-asert                 2
    phase.zk-asert                 2
    anchor-height.zk-asert         1
    anchor-target-atom.zk-asert    ^~((div max-tip5-atom:tip5 (bex 14)))
    ideal-block-time.zk-asert      150
    half-life.zk-asert             43.200
    anchor-min-timestamp.zk-asert  (add (time-in-secs:page:txe *@da) 1.200)
    ai-pow-activation-height       1
    anchor-height.ai-asert         1
    anchor-min-timestamp.ai-asert  (time-in-secs:page:txe *@da)
  ==
::  provable variant of +bc-max-block-size-medium-v0: same ~10 KB block-size
::  limit, but poke-able through the kernel. The oversize-tx mempool test
::  drives +init-nockchain / +add-n-pages-integration, both of which poke
::  %heard-block and assert acceptance, so the pages need real proofs.
::
::  max-future-timestamp is also lifted, as +bc-pending-integration-tests
::  does. +pok pokes with now=*@ (0), and +make-empty-page steps each page
::  600s past its parent, so consensus +check-timestamp
::  (lte timestamp (add now-secs max-future-timestamp)) caps the chain at
::  7.200/600 = 12 blocks under the 2-hour default. The oversize test needs
::  85, so without this the 13th page is rejected and
::  +add-n-pages-integration's acceptance assertion crashes.
++  bc-max-block-size-medium-v0-provable
  %*  .  bc-max-block-size-medium-v0
    genesis-target-atom  max-tip5-atom:tip5
    pow-len              1
    max-future-timestamp  (bex 32)
  ==
--
::
::  structs
::
::  helper functions
|_  bc=blockchain-constants:txe
+*  t  ~(. txe bc)
::
::  +der: pre-activation derived-state (read-only extra arg for consensus door)
++  der
  ^-  derived-state
  :*  highest-block-height=~
      heaviest-chain=~
      puzzle-asert-states=%hmap
  ==
::
::  +mock-pow: a minimal, well-typed version-%0 ZK proof artifact used as
::  the pow for library-level (non-kernel) test pages. On this branch the
::  `check-pow-flag` bypass was removed from consensus, so every block a
::  test validates via +validate-page-without-txs must carry a proof (the
::  arm does `(need ~(pow ...))`). That arm's only pow gate is
::  `check-target` — a difficulty check — NOT a full STARK verification
::  (real STARK verify lives in the KERNEL's +check-pow, not the lib).
::  proof-to-pow of this empty-objects %0 proof is a fixed tip5 hash that
::  is <= the default test genesis-target, so it passes check-target at
::  the standard test difficulty without any target override. All test
::  blocks sit far below proof-version-1-start (6.750), so %0 is the
::  height-correct version. This does NOT weaken any real check: the
::  kernel pow path (real verify:nv) is untouched and still rejects it.
++  mock-pow  ^-  (unit proof)  `[%0 objects=~ hashes=~ read-index=0]
::
::  +prove-page: stamp a page with a REAL, verifiable ZK proof for the
::  KERNEL block-acceptance path (+heard-block/+check-genesis run
::  +check-pow = check-pow-puzzle + verify:nv, which a mock cannot
::  satisfy). Requires `bc` to be a *-provable constant (target=max so
::  any proof clears +check-target; pow-len=1 so proving is fast). The
::  proof's %puzzle object commits to (block-commitment pag), matching
::  +check-pow-puzzle. Recomputes the digest AFTER attaching the proof
::  because the block-id hashes the pow (see +hashable-digest).
++  prove-page
  |=  pag=page:t
  ^-  page:t
  =/  header=noun-digest:tip5  (block-commitment:page:t pag)
  =/  res=[prf=proof dig=@]
    (prove-block-inner:pow-lib [%0 header *noun-digest:tip5 pow-len.bc])
  =.  pag
    ?^  -.pag
      pag(pow `prf.res)
    pag(pow `prf.res)
  =/  new-digest  (compute-digest:page:t pag)
  ?^  -.pag
    pag(digest new-digest)
  pag(digest new-digest)
::
::  +add-n-pages: add n empty pages and return the consensus  state
::
::    use with `=^`, returns the last page created and new con state
++  add-n-pages
  |=  [n=@ con=consensus-state retain=(unit @)]
  ^-  [page:t consensus-state]
  =/  last-page=page:t
    (to-page:local-page:t (~(got h-by blocks.con) (need heaviest-block.con)))
  =/  prev-page  last-page
  =|  k=@
  |-
  ?:  =(k n)
    [prev-page con]
  =/  new-page=page:t  (make-empty-page prev-page)
  =/  r=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der bc) new-page)
  ?>  ?=(%.y -.r)
  =/  acc=tx-acc:t  +.r
  =.  con  (~(accept-page dcon con der bc) new-page acc *@da)
  =.  con  (~(update-heaviest dcon con der bc) new-page)
  =.  con  (~(garbage-collect dcon con der bc) retain)
  $(k +(k), prev-page new-page)
::
::  +build-typed-chain: accept a chain of the given puzzle types on genesis via
::  the direct consensus path. ZK blocks carry a mock proof; AI blocks carry a
::  placeholder artifact. The helper updates branch-local puzzle state after
::  every accepted block so ASERT tests exercise the production lineage model.
++  build-typed-chain
  |=  types=(list ?(%zk %ai))
  ^-  [con=consensus-state der=derived-state tip=page:t]
  =/  con=consensus-state  initial-consensus-state
  =/  d=derived-state  der
  =/  parent=page:t  default-genesis-page
  |-
  ?~  types  [con d parent]
  =/  new-page=page:t
    ?:  =(%ai i.types)
      (make-ai-pow-page parent con d)
    (make-empty-page parent)
  =/  r=(reason tx-acc:t)  (~(validate-page-with-txs dcon con d bc) new-page)
  ~|  [%build-typed-chain-validation-failed r]
  ?>  ?=(%.y -.r)
  =.  con  (~(accept-page dcon con d bc) new-page +.r *@da)
  =.  con  (~(update-heaviest dcon con d bc) new-page)
  =.  d  (~(update dder d bc) con new-page)
  $(types t.types, parent new-page, con con, d d)
::  +build-typed-chain-timed: like +build-typed-chain but each entry carries an
::  absolute block timestamp (seconds), so a test can drive a controlled per-puzzle
::  cadence. Timestamps must be strictly increasing and above the genesis second.
++  build-typed-chain-timed
  |=  entries=(list [type=?(%zk %ai) ts=@])
  ^-  [con=consensus-state der=derived-state tip=page:t]
  =/  con=consensus-state  initial-consensus-state
  =/  d=derived-state  der
  =/  parent=page:t  default-genesis-page
  |-
  ?~  entries  [con d parent]
  =/  base=page:t
    ?:  =(%ai type.i.entries)
      (make-ai-pow-page parent con d)
    (make-empty-page parent)
  =/  new-page=page:t
    =.  base
      ?^  -.base  base(timestamp ts.i.entries)  base(timestamp ts.i.entries)
    ?^  -.base  base(digest (compute-digest:page:t base))  base(digest (compute-digest:page:t base))
  =/  r=(reason tx-acc:t)  (~(validate-page-with-txs dcon con d bc) new-page)
  ~|  [%build-typed-chain-timed-validation-failed r]
  ?>  ?=(%.y -.r)
  =.  con  (~(accept-page dcon con d bc) new-page +.r *@da)
  =.  con  (~(update-heaviest dcon con d bc) new-page)
  =.  d  (~(update dder d bc) con new-page)
  $(entries t.entries, parent new-page, con con, d d)
::
++  default-genesis-page
  ^-  page:t
  =/  genesis=page:t  (new-genesis:page:t default-genesis-template default-genesis-timestamp)
  =/  new-msg  (new:page-msg:t default-genesis-msg)
  =.  genesis
    ?^  -.genesis
      genesis(msg new-msg)
    genesis(msg new-msg)
  =/  new-digest  (compute-digest:page:t genesis)
  =.  genesis
    ?^  -.genesis
      genesis(digest new-digest)
    genesis(digest new-digest)
  genesis
::
++  default-genesis-msg  'HAIL ZORP'
++  default-genesis-timestamp  *@da
++  default-genesis-id  ~(digest get:page:t default-genesis-page)
++  default-genesis-template
  ^-  genesis-template:t
  :*  *btc-hash:t
      999.999
      (trip default-genesis-msg)
  ==
++  default-btc-hash  (hash:btc-hash:t *btc-hash:t)
++  default-genesis-seal  '6uuH7hHG1PDDxBrTHzMmVyU2dLDAdXJLPTqF2c3vKtkGkdbcyMxkxBM'
++  default-retain  (some 20)
++  default-tx-gc-retain  20
::
::  +initial-consensus-state: pass this into add-n-pages to get things going
++  initial-consensus-state
  ^-  consensus-state
  =/  con=consensus-state
    (~(add-btc-data dcon *consensus-state der bc) `*btc-hash:t)
  =.  con
    (~(set-genesis-seal dcon con der bc) height=0 msg-hash=default-genesis-seal)
  =.  con  (~(accept-page dcon con der bc) default-genesis-page *tx-acc:t *@da)
  =.  con  (~(update-heaviest dcon con der bc) default-genesis-page)
  con
::

++  initial-consensus-state-custom
  |=  cus=blockchain-constants:t
  ^-  consensus-state
  =/  con=consensus-state
    (~(add-btc-data dcon *consensus-state der cus) `*btc-hash:t)
  =.  con  (~(accept-page dcon con der cus) default-genesis-page *tx-acc:t *@da)
  =.  con  (~(update-heaviest dcon con der cus) default-genesis-page)
  con
::
++  initial-mining-state
  ^-  mining-state
  =/  pk-hash=hash:t  (hash:schnorr-pubkey:t default-a-pt-1)
  %*  .  *mining-state
    mining      %.y
    v0-shares   (~(put z-by *(z-map sig:t @)) [p:default-keys-1 1])
    shares      (~(put z-by *(z-map hash:t @)) [pk-hash 1])
  ==
::
::  v0 helpers
++  v0
  |%
  ::
  ++  make-raw-tx-from-coinbase
    |=  [recipient=sig:t cb-page=page:t]
    ^-  raw-tx:v0:t
    =/  =coinbase:t  (new:v0:coinbase:t cb-page p:default-keys-1)
    ?>  ?=(^ -.coinbase)
    (simple-from-note:new:raw-tx:v0:t recipient coinbase s:default-keys-1)
  ::
  ++  make-default-coinbase-raw-tx
    |=  recipient=sig:t
    ^-  raw-tx:t
    =/  =coinbase:t  make-default-coinbase
    ?>  ?=(^ -.coinbase)
    (simple-from-note:new:raw-tx:v0:t recipient coinbase s:default-keys-1)
  ::
  ::  +make-page-with-coinbase-spend
  ::
  ::    make block with parent $page, which spends from the coinbase in .cb-page
  ::    coinbase must be mined by pub key in .default-keys-1
  ++  make-page-with-coinbase-spend
    |=  [parent=page:t cb-page=page:t]
    ^-  page:t
    =/  page  (make-empty-page parent)
    =/  raw  (make-raw-tx-from-coinbase p:default-keys-2 cb-page)
    =/  new-tx-ids  (z-silt ~[~(id get:raw-tx:t raw)])
    =.  page
      ?^  -.page
        page(tx-ids new-tx-ids)
      page(tx-ids new-tx-ids)
    =/  new-digest  (compute-digest:page:t page)
    =.  page
      ?^  -.page
        page(digest new-digest)
      page(digest new-digest)
    page
  ::
  ++  make-page-with-txs
    |=  [parent=page:t txs=(list tx-id:t)]
    ^-  page:t
    =/  page  (make-empty-page parent)
    =/  new-tx-ids  (z-silt txs)
    =.  page
      ?^  -.page
        page(tx-ids new-tx-ids)
      page(tx-ids new-tx-ids)
    =/  new-digest  (compute-digest:page:t page)
    =.  page
      ?^  -.page
        page(digest new-digest)
      page(digest new-digest)
    page
  ::  +make-rogue-tx: tx using inputs not in balance
  ++  make-rogue-tx
    ^-  raw-tx:t
    ::  make a made-up note
    =/  =coinbase:t  make-default-coinbase
    ?>  ?=(^ -.coinbase)
    =.  sig.coinbase  p:default-keys-2
    =.  name.coinbase  (new:nname:t p:default-keys-2 [*block-id:t %.y] coinbase-timelock:coinbase:v0:t)
    (simple-from-note:new:raw-tx:v0:t p:default-keys-1 coinbase s:default-keys-2)
  --
::
::  v1 helpers
++  v1
  |%
    ::  +make-simple-note: create v1 note with simple %pkh lock
    ++  make-simple-note
      |=  [=sig:t assets=coins:t]
      ^-  nnote:t
      =/  lk=lock:t  (lock-from-sig:t sig)
      =/  lock-root=hash:t  (hash:lock:t lk)
      =/  source-hash=hash:t  *hash:t
      %*  .  *nnote-1:v1:t
        version      %1
        origin-page  0
        name         (new-v1:nname:t lock-root [source-hash %.n])
        note-data    *(z-map @tas *)
        assets       assets
      ==
  ::
  ++  make-hax-lock
    |=  pre=*
    ^-  [root=hash:t spend-condition:t h=hash:t]
    =/  [root0=hash:t sc=spend-condition:t h=hash:t]
      (make-hax:spend-condition:t pre)
    =/  root=hash:t  (hash:lock:t sc)
    [root sc h]
  ++  make-witness-hax
    |=  [root=hash:t sc=spend-condition:t h=hash:t pre=*]
    ^-  witness:t
    =/  lmp=lock-merkle-proof:t  (build-lock-merkle-proof:lock:t sc 1)
    %*  .  *witness:t
      lmp  lmp
      pkh  *(z-map hash:t [pk=schnorr-pubkey:t sig=schnorr-signature:t])
      hax  (~(put z-by *(z-map hash:t *)) h pre)
      tim  ~
    ==
  ++  make-seed
    |=  [lock-root=hash:t gift=coins:t parent-hash=hash:t]
    (simple:seed-v1:t lock-root gift parent-hash)
  ++  make-pkh-lock
    |=  [m=@ pks=(list schnorr-pubkey:t)]
    ^-  [root=hash:t spend-condition:t hs=(z-set hash:t)]
    =/  [root0=hash:t sc=spend-condition:t hs=(z-set hash:t)]
      (make-pkh:spend-condition:t m pks)
    =/  root=hash:t  (hash:lock:t sc)
    [root sc hs]
  ++  make-pkh-witness
    |=  $:  root=hash:t
            sc=spend-condition:t
            sig-hash=hash:t
            keys=(list [schnorr-seckey:t schnorr-pubkey:t])
        ==
    ^-  witness:t
    =/  pmap=pkh-signature:v1:t
      %+  roll  keys
      |=  $:  kp=[sk=schnorr-seckey:t pk=schnorr-pubkey:t]
              acc=pkh-signature:v1:t
          ==
      =/  sig=schnorr-signature:t
        %+  sign:affine:belt-schnorr:cheetah
          sk.kp
        sig-hash
      (~(put z-by acc) (hash:schnorr-pubkey:t pk.kp) [pk.kp sig])
    =/  lmp=lock-merkle-proof:t  (build-lock-merkle-proof:lock:t sc 1)
    %*  .  *witness:t
      lmp  lmp
      pkh  pmap
      hax  *(z-map hash:t *)
      tim  ~
    ==
  ::
  ::  +make-coinbase-lock: create v1 coinbase lock (pkh + tim)
  ++  make-coinbase-lock
    |=  [m=@ pks=(list schnorr-pubkey:t)]
    ^-  [root=hash:t spend-condition:t hs=(z-set hash:t)]
    =/  [root0=hash:t sc-pkh=spend-condition:t hs=(z-set hash:t)]
      (make-pkh:spend-condition:t m pks)
    =/  tim-prim=lock-primitive:v1:t
      [%tim [rel=[min=`coinbase-timelock-min.bc max=~] abs=[min=~ max=~]]]
    =/  sc=spend-condition:t  (combine:spend-condition:t sc-pkh ~[tim-prim])
    =/  root=hash:t  (hash:lock:t sc)
    [root sc hs]
  ++  make-witness-hax-pkh
    |=  $:  root=hash:t
            sc=spend-condition:t
            sig-hash=hash:t
            keys=(list [schnorr-seckey:t schnorr-pubkey:t])
            h=hash:t
            pre=*
        ==
    ^-  witness:t
    =/  pmap=pkh-signature:v1:t
      %+  roll  keys
      |=  $:  kp=[sk=schnorr-seckey:t pk=schnorr-pubkey:t]
              acc=pkh-signature:v1:t
          ==
      =/  sig=schnorr-signature:t
        %+  sign:affine:belt-schnorr:cheetah
          sk.kp
        sig-hash
      (~(put z-by acc) (hash:schnorr-pubkey:t pk.kp) [pk.kp sig])
    =/  lmp=lock-merkle-proof:t  (build-lock-merkle-proof:lock:t sc 1)
    %*  .  *witness:t
      lmp  lmp
      pkh  pmap
      hax  (~(put z-by *(z-map hash:t *)) h pre)
      tim  ~
    ==
  ++  make-default-coinbase
    ^-  coinbase:t
    ::  create a v1 page with hash-based shares for v1 coinbase
    =/  pk-hash=hash:t  (hash:schnorr-pubkey:t default-a-pt-1)
    =/  shares-v1=(z-map hash:t @)
      (~(put z-by *(z-map hash:t @)) pk-hash 1)
    =/  new-page=page:t
      %-  new-candidate:page:t
      :*  default-genesis-page
          *@da
          ~(target get:page:t default-genesis-page)
          shares-v1
          phase.zk-asert:default-bc
      ==
    =/  pkh-hashes=(z-set hash:t)  (~(put z-in *(z-set hash:t)) pk-hash)
    (new:coinbase:t new-page pkh-hashes)
  ::  +get-coinbase-from-balance: extract actual coinbase from balance
  ::
  ::    finds the coinbase note in a page's balance by checking the source
  ::    hash in the name. coinbases have source=[parent-hash %.y].
  ::
  ++  get-coinbase-from-balance
    |=  [pag=page:t bal=(h-map nname:t nnote:t)]
    ^-  nnote:t
    =/  entries=(list [nname:t nnote:t])  ~(tap h-by bal)
    |-
    ?~  entries  !!
    =/  [nam=nname:t note=nnote:t]  i.entries
    ::  skip v0 notes
    ?:  ?=(^ -.note)  $(entries t.entries)
    ::  for v1 notes, check if source matches coinbase pattern
    ?:  =(%1 version.note)
      =/  src-hash=hash:t  +<.nam
      =/  expected-src=hash:t  (last:nname:t [~(parent get:page:t pag) %.y])
      ?:  =(src-hash expected-src)
        note
      $(entries t.entries)
    $(entries t.entries)
  :: new tx from seeds and name
  ++  tx-from-seeds
    |=  [=seeds:v1:t =nname:t]
    ^-  raw-tx:t
    =/  =spend:v1:t  (new:spend-v1:t seeds 0)
    =/  =spends:v1:t  (~(put z-by *spends:v1:t) nname spend)
    (new:raw-tx:v1:t spends)
  ::  +sig-to-pkh-hashes: convert sig to set of pubkey hashes
  ++  sig-to-pkh-hashes
    |=  =sig:t
    ^-  (z-set hash:t)
    %+  roll  ~(tap z-in pubkeys.sig)
    |=  [pk=schnorr-pubkey:t acc=(z-set hash:t)]
    (~(put z-in acc) (hash:schnorr-pubkey:t pk))
  ::
  ++  sig-to-shares
    |=  [=sig:t share=@]
    ^-  shares:t
    %+  roll  ~(tap z-in pubkeys.sig)
    |=  [pk=schnorr-pubkey:t acc=shares:t]
    (~(put z-in acc) (hash:schnorr-pubkey:t pk) share)
  ::
  ::  +calculate-min-fee-for-witness: calculate minimum fee for a witness
  ++  calculate-min-fee-for-witness
    |=  wit=witness:t
    ^-  coins:t
    =/  wit-words=@  (num-of-leaves:shape `*`wit)
    (max 256 (mul wit-words 256))
  ::  +calculate-min-fee-for-witness-with-data: includes note-data words
  ++  calculate-min-fee-for-witness-with-data
    |=  [wit=witness:t note-data=(z-map @tas *)]
    ^-  coins:t
    =/  wit-words=@  (num-of-leaves:shape `*`wit)
    =/  data-words=@
      %-  num-of-leaves:shape
      %-  ~(rep z-by note-data)
      |=  [[k=@tas v=*] tree=*]
      [k v tree]
    =/  total-words=@  (add wit-words data-words)
    (max 256 (mul total-words 256))
  ::  +calculate-min-fee-for-spend-0: calculate minimum fee for spend-0
  ++  calculate-min-fee-for-spend-0
    |=  sig=signature:t
    ^-  coins:t
    =/  sig-words=@  (num-of-leaves:shape `*`sig)
    (max 256 (mul sig-words 256))
  ::  +make-rogue-tx: v1 tx using inputs not in balance
  ++  make-rogue-tx
    ^-  raw-tx:t
    ::  make a made-up v1 note with fake origin
    =/  =coinbase:t  make-default-coinbase
    ?>  ?=(@ -.coinbase)
    ::  mutate to have non-existent origin (note won't be in balance)
    =/  fake-origin=@  +(origin-page.coinbase)
    =/  fake-coin=coinbase:t  coinbase(origin-page fake-origin)
    ::  build spend-1 manually from fake note
    =/  nam=nname:t  ~(name get:nnote:t fake-coin)
    =/  recipient-lock=lock:t  (lock-from-sig:t p:default-keys-2)
    =/  sed=seed:v1:t
      %*  .  *seed:v1:t
        output-source  *(unit source:t)
        lock-root      (hash:lock:t recipient-lock)
        gift           assets.fake-coin
        parent-hash    (hash:nnote:t fake-coin)
      ==
    =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
    =/  pk=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1))
    =/  [root=hash:t sc=spend-condition:t *]
      (make-pkh-lock 1 ~[pk])
    =/  sp1=spend-1:v1:t
      %*  .  *spend-1:v1:t
        witness  *witness:v1:t
        seeds    seds
        fee  0
      ==
    =/  sig-h=hash:t  (sig-hash:spend-1:v1:t sp1)
    =/  wit=witness:t
      (make-pkh-witness root sc sig-h ~[[s:default-keys-1 pk]])
    =/  sp1=spend-1:v1:t  sp1(witness wit)
    =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
    (new:raw-tx:v1:t sps)
  --
::
::  add a raw-tx to a fresh page atop parent, validate, accept, update
++  add-raw-to-new-page
  |=  [par=page:t con=consensus-state raw=raw-tx:t]
  ^-  [page:t consensus-state tx-acc:t]
  =/  pag=page:t  (make-empty-page par)
  =/  height  ~(height get:page:t pag)
  =/  =tx:t  (new:tx:t raw height)
  =/  new-tx-ids  (~(put z-in *(z-set tx-id:t)) ~(id get:raw-tx:t raw))
  =.  pag
    ?^  -.pag
      pag(tx-ids new-tx-ids)
    pag(tx-ids new-tx-ids)
  =/  pag-emission=coins:t
    (emission-calc:coinbase:t ~(height get:page:t pag))
  =/  pag-fees=coins:t  ~(total-fees get:tx:t tx)
  =/  total-coinbase-assets=coins:t  (add pag-emission pag-fees)
  =.  pag
    ?^  -.pag
      =/  new-coinbase
        (new:v0:coinbase-split:t total-coinbase-assets default-keys-1-share)
      pag(coinbase new-coinbase)
    ::  v1: dispatch on activation height. Post-activation uses the
    ::  fee-aware 80/20 builder (014-aletheia); pre-activation keeps the
    ::  legacy 100%-to-miner shape via ++new.
    ?:  (pre-asert-activation:t ~(height get:page:t pag))
      =/  new-coinbase
        (new:v1:coinbase-split:t total-coinbase-assets default-keys-1-share-v1)
      pag(coinbase new-coinbase)
    =/  new-coinbase
      %-  new-with-fund-share:v1:coinbase-split:t
      [pag-emission pag-fees default-keys-1-share-v1]
    pag(coinbase new-coinbase)
  =/  new-digest  (compute-digest:page:t pag)
  =.  pag
    ?^  -.pag
      pag(digest new-digest)
    pag(digest new-digest)
  =^  ready  con  (~(add-raw-tx dcon con der bc) raw)
  ?>  -:(~(validate-page-without-txs dcon con der bc) pag ~(timestamp get:page:t pag))
  =/  r=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der bc) pag)
  ?:  ?=(%.n -.r)
    ~|  "failed-to-validate-page: {<p.r>}"
    !!
  =.  con  (~(accept-page dcon con der bc) pag +.r *@da)
  =.  con  (~(update-heaviest dcon con der bc) pag)
  [pag con +.r]
::
++  make-empty-page
  |=  parent=page:t
  ^-  page:t
  (make-empty-page-multisig parent p:default-keys-1)
::
++  make-empty-pages
  |=  [start=page:t num=@]
  ^-  (list page:t)
  =/  cur=page:t  start
  =|  pages-added=(list page:t)
  =-  (flop pages-added)
  %+  roll  (range:z num)
  |=  [i=@ cur=_cur pages-added=(list page:t)]
  =/  next=page:t  (make-empty-page cur)
  [next [next pages-added]]
::
++  make-empty-page-multisig
  |=  [parent=page:t s=sig:t]
  ^-  page:t
  =/  height  +(~(height get:page:t parent))
  =/  new-page
    ?:  &(?=(^ -.parent) (lth height v1-phase.bc))
      ::  v0 parent: use v0 new-candidate with sig-based shares
      =/  shares=(z-map sig:t @)
        (~(put z-by *(z-map sig:t @)) s 1)
      %-  new-candidate:v0:page:t
      :*  parent
          *@da
          ~(target get:page:t parent)
          shares
      ==
    ::  v1 parent: use v1 new-candidate with hash-based shares
    =/  =shares:t  (sig-to-shares:v1 s 1)
    %-  new-candidate:page:t
    :*  parent
        *@da
        ~(target get:page:t parent)
        shares
        phase.zk-asert.bc
    ==
  =/  new-timestamp  (add ~(timestamp get:page:t parent) 600)
  =.  new-page
    ?^  -.new-page
      new-page(timestamp new-timestamp)
    new-page(timestamp new-timestamp)
  =/  new-pow  mock-pow
  =.  new-page
    ?^  -.new-page
      new-page(pow new-pow)
    new-page(pow new-pow)
  =/  new-digest  (compute-digest:page:t new-page)
  =.  new-page
    ?^  -.new-page
      new-page(digest new-digest)
    new-page(digest new-digest)
  new-page
::
::  +make-ai-pow-garbage-page: like +make-empty-page but stamps a GARBAGE
::  version-%4 (%ai-pow) artifact instead of the mock ZK proof, so a poked
::  %heard-block reaches +check-pow's `%ai-pow` branch (the mandatory
::  ++ai-pow-verify jet). `page:t` is `$^(page:v0 page:v1)`, so mutation must
::  branch on `?^ -.new-page` (cell head ⇒ v0, atom head ⇒ v1). +new-candidate
::  builds a v1 page (atom head), so the v1 (FALSE) branch runs at runtime and
::  gets the `[%ai-pow 0 0]` pow — v1's pow is `(unit *)`, which accepts it. The
::  v0 (TRUE) branch is dead but must still type-check, and v0's pow is a
::  narrower `(unit proof:v0)` that `[%ai-pow ..]` cannot nest into, so it keeps
::  `mock-pow` there. The artifact is deliberately undecodable ⇒ jet returns %.n.
++  make-ai-pow-garbage-page
  |=  parent=page:t
  ^-  page:t
  =/  s=sig:t  p:default-keys-1
  =/  =shares:t  (sig-to-shares:v1 s 1)
  =/  new-page
    %-  new-candidate:page:t
    :*  parent
        *@da
        ~(target get:page:t parent)
        shares
        phase.zk-asert.bc
    ==
  =/  new-timestamp  (add ~(timestamp get:page:t parent) 600)
  =.  new-page
    ?^  -.new-page
      new-page(timestamp new-timestamp)
    new-page(timestamp new-timestamp)
  =.  new-page
    ?^  -.new-page
      new-page(pow mock-pow)
    new-page(pow `[%ai-pow 0 0])
  =.  new-page
    ?^  -.new-page
      new-page(digest (compute-digest:page:t new-page))
    new-page(digest (compute-digest:page:t new-page))
  new-page
:::
++  sample-ai-pow-cert
  |=  cert-len=@ud
  ^-  ai-pow-certificate:txe
  =/  cert-data=@
    ?:  =(0 cert-len)  0
    (bex (dec (mul 8 cert-len)))
  =/  cert=ai-pow-certificate:txe  *ai-pow-certificate:txe
  cert(version 1, certificate [%bytes cert-len cert-data])
:::
++  sample-ai-pow-artifact
  |=  cert-len=@ud
  ^-  pow-artifact:txe
  [%ai-pow [4 (bex 31)] (sample-ai-pow-cert cert-len)]
:::
::  +make-ai-pow-page: a post-ASERT AI header fixture for tests that exercise
::  target, heaviness, coinbase, and ASERT state without invoking the verifier
::  jet. It carries a small resource-bounded `%ai-pow` artifact so block-size
::  accounting follows the production AI path.
++  make-ai-pow-page
  |=  [parent=page:t con=consensus-state d=derived-state]
  ^-  page:t
  =/  zk-cand=page:t  (make-empty-page parent)
  ::  The AI candidate's coinbase is rebuilt from these shares — the same
  ::  single-miner split +make-empty-page seeds its ZK candidate with.
  =/  ai-cand=page:t
    (~(build-ai-candidate dcon con d bc) zk-cand (sig-to-shares:v1 p:default-keys-1 1))
  ::  v1 page (atom head) runs the FALSE branch and takes the %ai-pow artifact;
  ::  the v0 branch is dead but must type-check, and v0's narrower pow type cannot
  ::  hold [%ai-pow ..], so it keeps mock-pow (see +make-ai-pow-garbage-page).
  =.  ai-cand
    ?^  -.ai-cand  ai-cand(pow mock-pow)  ai-cand(pow `(sample-ai-pow-artifact 4))
  =.  ai-cand
    ?^  -.ai-cand  ai-cand(digest (compute-digest:page:t ai-cand))  ai-cand(digest (compute-digest:page:t ai-cand))
  ai-cand
::
++  make-default-coinbase
  ^-  coinbase:t
  =/  new-page=page:t  (make-empty-page default-genesis-page)
  (new:v0:coinbase:t new-page p:default-keys-1)
::
::  these keys were originally generated with
::  ^~((generate-keys:affine:belt-schnorr:cheetah:zeke foo))
::  but ^~ does not cache during compiation, so it made compilation
::  time unreasonable. so we write them out by hand.
::
++  default-a-pt-1
  ^-  a-pt:curve:cheetah:zeke
  :*  :*  a0=18.211.575.932.483.365.338
          a1=18.084.541.669.437.144.090
          a2=13.716.509.772.206.322.211
          a3=13.903.522.087.582.572.864
          a4=8.355.231.558.426.062.419
          a5=1.785.159.202.319.376.064
      ==
      :*  a0=6.862.026.292.343.821.647
          a1=3.472.296.653.227.435.007
          a2=14.264.708.252.454.369.462
          a3=12.448.508.371.644.144.441
          a4=10.672.764.440.307.750.310
          a5=7.731.969.120.172.842.597
      ==
      %.n
  ==
++  default-keys-1  :: (generate-keys:affine:belt-schnorr:cheetah:zeke 5)
  %*  .  *[s=sk:belt-schnorr:cheetah:zeke p=sig:t]
    s    :*  0xf4d7.8fd7
             0x2311.48c9
             0x2a9f.6a96
             0x7d7e.ae49
             0x54da.f4a5
             0xbd2.1e25
             0xdcbe.5587
             0x52f6.1d58
         ==
    ::
    p  (new:sig:t default-a-pt-1)
  ==
++  default-keys-1-share
  ^-  (z-map sig:t @)
  (~(put z-by *(z-map sig:t @)) p:default-keys-1 1)
::
++  default-keys-1-share-v1
  ^-  (z-map hash:t @)
  =/  pk-hash=hash:t  (hash:schnorr-pubkey:t default-a-pt-1)
  (~(put z-by *(z-map hash:t @)) pk-hash 1)
::
++  default-a-pt-2
  ^-  a-pt:curve:cheetah:zeke
  :*  :*  a0=8.112.277.360.505.529.168
          a1=13.815.204.321.620.730.735
          a2=4.609.498.217.171.624.338
          a3=17.544.458.397.231.430.472
          a4=1.434.491.911.615.350.717
          a5=13.302.043.377.247.567.356
      ==
      :*  a0=2.952.868.476.347.789.052
          a1=2.195.947.167.671.474.841
          a2=5.041.105.011.183.230.411
          a3=201.486.871.654.097.277
          a4=384.883.694.750.811.959
          a5=2.832.411.450.844.530.203
      ==
      %.n
  ==
::
++  default-keys-2  :: (generate-keys:affine:belt-schnorr:cheetah:zeke 23)
  %*  .  *[s=sk:belt-schnorr:cheetah:zeke p=sig:t]
    s    :*  0xdc5e.17b7
             0x5f26.324c
             0x8ed9.4f68
             0xb700.aa28
             0xb8c8.d96c
             0x7a22.545b
             0x2e98.4d9c
             0x3606.cc6f
         ==
    ::
    p  (new:sig:t default-a-pt-2)
  ==
++  default-keys-2-share
  ^-  (z-map sig:t @)
  (~(put z-by *(z-map sig:t @)) p:default-keys-2 1)
::
++  default-a-pt-3
  ^-  a-pt:curve:cheetah:zeke
  :*  :*  a0=9.328.090.969.704.952.711
          a1=6.464.356.965.168.029.919
          a2=15.057.652.275.350.316.414
          a3=16.160.083.394.729.116.439
          a4=16.516.510.259.443.376.809
          a5=15.842.189.185.823.855.279
      ==
      :*  a0=8.160.803.705.266.360.847
          a1=4.180.418.860.702.279.974
          a2=8.077.237.076.284.994.865
          a3=10.695.087.701.690.147.905
          a4=9.145.798.145.442.941.712
          a5=8.788.211.494.469.384.813
      ==
      %.n
  ==
::
++  default-keys-3  :: (generate-keys:affine:belt-schnorr:cheetah:zeke 28)
  %*  .  *[s=sk:belt-schnorr:cheetah:zeke p=sig:t]
    s    :*  0x7f62.f254
             0x1027.d16e
             0x979.58f8
             0xdc6c.736d
             0xaf71.62e2
             0xa9b9.b8e5
             0x98ed.52e5
             0x5c48.fed9
         ==
    ::
    p  (new:sig:t default-a-pt-3)
  ==
::
++  default-keys-3-share
  ^-  (z-map sig:t @)
  (~(put z-by *(z-map sig:t @)) p:default-keys-3 1)
::
++  initial-atom-targets
  ^-  (list @)
  %-  limo
  :~  ::genesis-target-atom
      max-target-atom:t  :: max target
      1  :: min target
      2  :: small targets
      4
      8
      (dec max-target-atom:t)
      (sub max-target-atom:t 4)   :: large target
      (sub max-target-atom:t 5)   :: large target
      (sub max-target-atom:t 100) :: large target
      (div max-target-atom:t 2)
      (div max-target-atom:t 3)
      (div max-target-atom:t 4)
      (div max-target-atom:t 8)
      (dec (div max-target-atom:t 8))
      +((div max-target-atom:t 8))
      (div max-target-atom:t 9)
      (dec (div max-target-atom:t 9))
      +((div max-target-atom:t 9))
  ==
::
++  initial-bignum-targets
  ^-  (list bignum:bignum:zeke)
  (turn initial-atom-targets chunk:bignum:zeke)
::
++  epoch-durs-list
  ^-  (list @)
  =/  epoch-dur-1  target-epoch-duration:t
  =/  epoch-dur-4  (mul epoch-dur-1 4)
  =/  epoch-dur-1d4  (div epoch-dur-1 4)
  %-  limo
  :~
    ::  no target adjustment
      epoch-dur-1
    ::  longer epoch -> larger target
      (mul epoch-dur-1 2)
      (mul epoch-dur-1 3)
      (mul epoch-dur-1 4)
    ::  edge cases
      +(epoch-dur-4)
      (dec epoch-dur-4)
    ::  larger cases
      (mul epoch-dur-1 5)
      (mul epoch-dur-1 10)
    ::  shorter epoch -> smaller target
      epoch-dur-1d4
      (div epoch-dur-1 2)
      (div epoch-dur-1 3)
      (div epoch-dur-1 4)
    ::  edge cases
      +(epoch-dur-1d4)
      (dec epoch-dur-1d4)
    ::  smaller cases
      (div epoch-dur-1 5)
      (div epoch-dur-1 10)
  ==
::
::  Helpers for integration tests
++  nockchain  (nock-ker *@uvI)
+$  effect  effect:^inner:nockchain
+$  cause  cause:^inner:nockchain
+$  input  input:nockchain
+$  ovum  ovum:nockchain
::
++  build-ovum
  |=  cau=cause
  ^-  ovum
  =/  =input  *input
  [/poke/sys/0 input(cause cau)]
::
++  build-ovum-on-wire
  |=  [wir=path cau=cause]
  ^-  ovum
  =/  =input  *input
  [wir input(cause cau)]
::
::  the wire the grpc driver stamps on a poke it makes for a local client,
::  e.g. `nockchain-wallet send-tx` (cf. +create-grpc-wire in the rust
::  nockapp-grpc crate, which yields source %grpc / version 1). +heard-tx
::  treats this as an operator submission and will re-gossip a tx it already
::  holds. cf. +local-tx-submission in apps/dumbnet/inner.hoon.
++  grpc-wire  `path`/poke/grpc/1
::
::  the wire the libp2p driver stamps on a tx a peer gossiped to us (cf.
::  +Libp2pWire in the rust nockchain-libp2p-io crate: source %libp2p,
::  version 1, tags [verb %peer-id <base58 peer id>]). +heard-tx must NOT
::  re-gossip a tx it already holds when it arrives on this wire.
++  libp2p-gossip-wire  `path`/poke/libp2p/1/gossip/peer-id/peer
::
++  pok
  |=  [cau=cause nockchain=_nockchain]
  =^  effs=(list *)  nockchain
  ::  line below is necessary due to type system nonsense.
  ::  basically, the result desk-hash is type [%~ @uvI], while
  ::  the wally desk-hash is type u(@uvI)
  =<  [- +(desk-hash.outer [~ *@uvI])]
  (poke:nockchain *@ (build-ovum cau))
  [;;((list effect) effs) nockchain]
::
::  +pok-on-wire: +pok, but stamping a caller-chosen wire on the ovum, so a
::  test can exercise the kernel's wire-dependent behaviour (which driver a
::  poke came from) rather than always speaking on the default %sys wire.
++  pok-on-wire
  |=  [wir=path cau=cause nockchain=_nockchain]
  =^  effs=(list *)  nockchain
  =<  [- +(desk-hash.outer [~ *@uvI])]
  (poke:nockchain *@ (build-ovum-on-wire wir cau))
  [;;((list effect) effs) nockchain]
::
++  init-nockchain
  ::  genesis must carry a real proof for the kernel's +check-genesis
  ::  (real +check-pow). Prove it, poke the proven page, and return the
  ::  proven genesis so callers build children on its (proven) digest.
  =/  genesis=page:t  (prove-page default-genesis-page)
  =/  [effs=(list effect) nockchain=_nockchain]
    (pok [%command %set-constants bc] nockchain)
  =/  [effs=(list effect) nockchain=_nockchain]
    (pok [%command %btc-data ~] nockchain)
  =/  [effs=(list effect) nockchain=_nockchain]
    (pok [%command %set-genesis-seal 0 default-genesis-seal] nockchain)
  =/  [effs=(list effect) nockchain=_nockchain]
    (pok [%command %born ~] nockchain)
  =^  effs=(list effect)  nockchain
    (pok [%fact %0 %heard-block genesis] nockchain)
  ?>  =((need heaviest-block.c.internal.outer.nockchain) ~(digest get:page:t genesis))
  [nockchain genesis]
::
++  add-n-pages-integration
  |=  [start=page:t num=@ nockchain=_nockchain]
  ^-  [(list page:t) _nockchain]
  =/  cur=page:t  start
  =|  pages-added=(list page:t)
  =|  i=@
  |-
  ?:  =(i num)
    [(flop pages-added) nockchain]
  ::  prove each block so the kernel's +heard-block (real +check-pow)
  ::  accepts it; thread the proven page as the parent for the next.
  =/  next=page:t  (prove-page (make-empty-page cur))
  =^  effs=(list effect)  nockchain
    (pok [%fact %0 %heard-block next] nockchain)
  ::  confirm block was added to consensus state
  ?>  =((need heaviest-block.c.internal.outer.nockchain) ~(digest get:page:t next))
  $(i +(i), cur next, pages-added [next pages-added])
::
++  filter-heard-tx-effects
  |=  effs=(list effect)
  ^-  (z-set raw-tx:t)
  %+  roll
    effs
  |=  [e=effect raw-txs=(z-set raw-tx:t)]
  ?.  ?=(%gossip -.e)
    raw-txs
  ?.  ?=(%heard-tx -.data.p.e)
    raw-txs
  (~(put z-in raw-txs) p.data.p.e)
::
++  filter-request-tx-effects
  |=  effs=(list effect)
  ^-  (z-set tx-id:t)
  %+  roll
    effs
  |=  [e=effect tx-ids=(z-set tx-id:t)]
  ?.  ?=(%request -.e)
    tx-ids
  ?.  ?=(%raw-tx -.p.e)
    tx-ids
  (~(put z-in tx-ids) p.p.e)
::
::  ker-by
++  k-by
  |_  nockchain=_nockchain
  ::
  ++  con  ~+  ;;(consensus-state c.internal.outer.nockchain)
  ::
  ::  the derived state, whose .heaviest-chain is the canonical
  ::  page-number -> block-id index used to tell an orphaned block from one on
  ::  the heaviest chain.
  ++  der  ~+  ;;(derived-state d.internal.outer.nockchain)
  ::
  ::  +strand-tx-on-block: put a tx back exactly the way the OLD kernel left it
  ::  -- still held in raw-txs, still claimed by .bid, absent from excluded-txs.
  ::
  ::    Reproduces the pre-fix stranded state, which the current kernel will no
  ::    longer produce on its own (+release-orphaned-branch frees the tx at reorg
  ::    time). Note the result still satisfies +apt: the tx IS in exactly one of
  ::    blocks-needed-by / excluded-txs. Nothing is structurally broken -- the
  ::    claim is simply never released -- which is why the bug went unnoticed.
  ++  strand-tx-on-block
    |=  [=tx-id:t =block-id:t]
    ^-  consensus-state
    =/  c=consensus-state  con
    =.  blocks-needed-by.c  (~(put h-ju blocks-needed-by.c) tx-id block-id)
    =.  excluded-txs.c  (~(del h-in excluded-txs.c) tx-id)
    c
  ::
  ::  +repair-orphans: the one-time repair +load runs on boot
  ++  repair-orphans
    |=  c=consensus-state
    ^-  consensus-state
    ~(repair-orphaned-claims dcon c der bc)
  ::
  ::  +boot-with: run the kernel's REAL +load over a given consensus state, the
  ::  way the runtime does when a new kernel is swapped in over existing state.
  ::
  ::    Calling +repair-orphaned-claims directly (via +repair-orphans above) only
  ::    proves the repair logic works. This proves +load actually RUNS it, which
  ::    is the part that can silently fail: if the wiring were wrong, or if
  ::    heaviest-chain.d were not populated at load time, the repair would find
  ::    no orphans, no-op, and leave every stranded tx stranded -- while the logs
  ::    looked perfectly healthy. That is the failure this exercises.
  ++  boot-with
    |=  c=consensus-state
    ^-  consensus-state
    =/  ks  internal.outer.nockchain
    =.  c.ks  c
    =/  booted  (load:nockchain [%0 desk-hash.outer.nockchain ks])
    ;;(consensus-state c.internal.outer.booted)
  ::
  ::  the page at the tip of the heaviest chain
  ++  tip-page
    ^-  page:t
    (to-page:local-page:t (~(got h-by blocks:con) (need heaviest-block:con)))
  ::
  ::  +with-heaviest-block: .c with its tip moved to .block-id. Pair with
  ::  +der-update to reach the state a reorg onto a shorter heavier chain
  ::  leaves: a tip below blocks that are still held.
  ++  with-heaviest-block
    |=  [c=consensus-state =block-id:t]
    ^-  consensus-state
    =.  heaviest-block.c  `block-id
    c
  ::
  ::  +der-update: the derived-state update, as +accept-block runs it
  ++  der-update
    |=  [d=derived-state c=consensus-state pag=page:t]
    ^-  derived-state
    (~(update dder d bc) c pag)
  ::
  ::  +release-branch: the live reorg release, as +accept-block runs it
  ++  release-branch
    |=  [c=consensus-state d=derived-state old-heavy=block-id:t hc=(z-map page-number:t block-id:t)]
    ^-  consensus-state
    (~(release-orphaned-branch dcon c d bc) old-heavy hc)
  ::
  ++  heaviest-chain-at
    |=  [d=derived-state =page-number:t]
    ^-  (unit block-id:t)
    (~(get z-by heaviest-chain.d) page-number)
  ::
  ++  put-heaviest-chain-at
    |=  [d=derived-state =page-number:t =block-id:t]
    ^-  derived-state
    =.  heaviest-chain.d  (~(put z-by heaviest-chain.d) page-number block-id)
    d
  ::
  ++  del-heaviest-chain-at
    |=  [d=derived-state =page-number:t]
    ^-  derived-state
    =.  heaviest-chain.d  (~(del z-by heaviest-chain.d) page-number)
    d
  ::
  ::  +with-con: this node, running on the given consensus state. Use to poke a
  ::  node with a state +boot-with produced: +load returns the wrapper's outer
  ::  core, which does not nest with the +k-by door's sample.
  ++  with-con
    |=  c=consensus-state
    ^-  _nockchain
    =/  n  nockchain
    =.  c.internal.outer.n  c
    n
  ::
  ::  membership probes against a bare consensus-state (rather than the kernel)
  ++  con-excluded
    |=  [c=consensus-state =tx-id:t]
    (~(has h-in excluded-txs.c) tx-id)
  ::
  ++  con-claimed
    |=  [c=consensus-state =tx-id:t]
    (~(has h-by blocks-needed-by.c) tx-id)
  ::
  ++  con-raw-tx
    |=  [c=consensus-state =tx-id:t]
    (~(has h-by raw-txs.c) tx-id)
  ::
  ++  con-invariants
    |=  c=consensus-state
    ^-  (unit @tas)
    ~(apt dcon c der bc)
  ::
  ::  +con-referential-integrity: every cross-map reference in .c resolves.
  ::  `~` means sound; each term names an invariant that broke.
  ::
  ::    Complements +apt, which checks only the raw-txs partition and nothing
  ::    about the block-keyed maps agreeing with each other. Most of these break
  ::    as a kernel crash on a `got`, not as a wrong answer.
  ++  con-referential-integrity
    |=  c=consensus-state
    ^-  (list @tas)
    =/  block-ids=(list block-id:t)  ~(tap h-in ~(key h-by blocks.c))
    =/  has-block  |=(=block-id:t (~(has h-by blocks.c) block-id))
    %+  murn
      ^-  (list [m=@tas ok=?])
      :~  ::  the tip itself must be a block we hold
          :-  %dangling-heaviest-block
          ?|  =(~ heaviest-block.c)
              (has-block (need heaviest-block.c))
          ==
          ::  every block we hold carries its own derived entries. Two entries
          ::  are legitimately absent and are NOT required here: .txs, which
          ::  only a block that actually carried txs has, and .balance for
          ::  GENESIS -- genesis has no coinbase, so +accept-page's fold over
          ::  coinbases puts nothing. +get-cur-balance encodes that same
          ::  exception: it returns an empty balance for a genesis tip and
          ::  crashes only for a non-genesis one.
          :-  %missing-balance
          %+  levy  block-ids
          |=  =block-id:t
          ?:  .=  *page-number:t
              ~(height get:local-page:t (~(got h-by blocks.c) block-id))
            %.y
          (~(has h-by balance.c) block-id)
          :-  %missing-min-timestamp
          (levy block-ids |=(=block-id:t (~(has h-by min-timestamps.c) block-id)))
          :-  %missing-epoch-start
          (levy block-ids |=(=block-id:t (~(has h-by epoch-start.c) block-id)))
          :-  %missing-target
          (levy block-ids |=(=block-id:t (~(has h-by targets.c) block-id)))
          ::  +update-min-timestamps walks parents 11 deep and +accept-page one
          ::  deep, both with `got`: a block whose parent was dropped crashes the
          ::  kernel the moment a child of it arrives.
          :-  %dangling-parent
          %+  levy  block-ids
          |=  =block-id:t
          =/  lp  (~(got h-by blocks.c) block-id)
          ?:  =(*page-number:t ~(height get:local-page:t lp))  %.y
          (has-block ~(parent get:local-page:t lp))
          ::  .epoch-start's VALUES are block-ids too, reaching up to
          ::  blocks-per-epoch back
          :-  %dangling-epoch-start
          %+  levy  block-ids
          |=  =block-id:t
          ?.  (~(has h-by epoch-start.c) block-id)  %.y
          (has-block (~(got h-by epoch-start.c) block-id))
          ::  ...and nothing may be keyed by a block we no longer hold
          :-  %orphaned-balance-key
          (levy ~(tap h-in ~(key h-by balance.c)) has-block)
          :-  %orphaned-txs-key
          (levy ~(tap h-in ~(key h-by txs.c)) has-block)
          :-  %orphaned-min-timestamps-key
          (levy ~(tap h-in ~(key h-by min-timestamps.c)) has-block)
          :-  %orphaned-epoch-start-key
          (levy ~(tap h-in ~(key h-by epoch-start.c)) has-block)
          :-  %orphaned-targets-key
          (levy ~(tap h-in ~(key h-by targets.c)) has-block)
      ==
    |=  [m=@tas ok=?]
    ^-  (unit @tas)
    ?:(ok ~ `m)
  ::
  ::  +con-block-residue: which block-keyed maps still hold .block-id, in a
  ::  fixed order. `~` means the block is gone from all of them.
  ::
  ::    Names every map rather than probing .blocks alone: deletion is
  ::    all-or-nothing across them, and a .blocks-only probe reports a dropped
  ::    .balance as clean.
  ::
  ::    A block that carried no txs legitimately has no .txs entry.
  ++  con-block-residue
    |=  [c=consensus-state =block-id:t]
    ^-  (list @tas)
    %+  murn
      ^-  (list [m=@tas present=?])
      :~  [%blocks (~(has h-by blocks.c) block-id)]
          [%balance (~(has h-by balance.c) block-id)]
          [%txs (~(has h-by txs.c) block-id)]
          [%min-timestamps (~(has h-by min-timestamps.c) block-id)]
          [%epoch-start (~(has h-by epoch-start.c) block-id)]
          [%targets (~(has h-by targets.c) block-id)]
      ==
    |=  [m=@tas present=?]
    ^-  (unit @tas)
    ?:(present `m ~)
  ::
  ::  +consensus-invariants: the consensus state's own +apt check, as an oracle.
  ::
  ::    ~ means every structural invariant holds; a term names the one that
  ::    broke. The interesting ones here are the raw-txs partition:
  ::      %txs-fell-through-cracks  a raw-tx in NEITHER blocks-needed-by nor
  ::                                excluded-txs -- stranded: unmineable,
  ::                                un-re-gossiped, un-droppable.
  ::      %excluded-txs-arent       a raw-tx in BOTH -- claimed by a block yet
  ::                                also offered to the miner.
  ::      %extra-excluded-txs       an excluded-tx we do not actually hold.
  ::    Any code that moves txs between those sets should be asserted against
  ::    this, not just against hand-picked membership checks.
  ++  consensus-invariants
    ^-  (unit @tas)
    ~(apt dcon con der bc)
  ::
  ++  has-raw-tx
    |=  =tx-id:t
    (~(has h-by raw-txs:con) tx-id)
  ::
  ++  has-excluded
    |=  =tx-id:t
    ^-  ?
    (~(has h-in excluded-txs:con) tx-id)
  ::
  ++  has-bnb-raw-tx
    |=  =tx-id:t
    (~(has h-by blocks-needed-by:con) tx-id)
  ::
  ++  has-bnb-block-id
    |=  [=tx-id:t =block-id:t]
    (~(has h-ju blocks-needed-by:con) tx-id block-id)
  ::
  ++  has-spent-by
    |=  [name=nname:t id=tx-id:t]
    ^-  ?
    =/  con  con
    ?.  (~(has h-ju spent-by.con) name id)
      ?<  (~(has h-by raw-txs.con) id)
      %.n
    (~(has h-by raw-txs.con) id)
  ::
  ++  has-spent-by-set
    |=  [names=(z-set nname:t) id=tx-id:t]
    ^-  ?
    %-  ~(all z-in names)
    (curr has-spent-by id)
  ::
  ++  get-excluded
    |=  =tx-id:t
    ?:  (has-excluded tx-id)
      (~(get-raw-tx dcon con der bc) tx-id)
    ~
  ::
  ++  get-bnb
    |=  =tx-id:t
    ^-  (unit (h-set block-id:t))
    (~(get h-by blocks-needed-by:con) tx-id)
  ::
  ++  get-bnb-raw-tx
    |=  id=tx-id:t
    ^-  (unit raw-tx:t)
    =/  con=consensus-state  con
    ?:  (~(has h-by blocks-needed-by.con) id)
      (~(get-raw-tx dcon con der bc) id)
    ~
  ::
  ++  heaviest-block
    ^-  block-id:t
    (need heaviest-block.c.internal.outer.nockchain)
  ::
  ++  heaviest-chain-height
    ^-  page-number:t
    (need highest-block-height.d.internal.outer.nockchain)
  ::
  ++  inputs-in-heaviest-balance
    |=  raw=raw-tx:t
   (~(inputs-in-heaviest-balance dcon con der bc) raw)
  ::
  ++  get-cur-balance
    ^-  (h-map nname:t nnote:t)
   ~(get-cur-balance dcon con der bc)
  ::
  ++  has-pending-block
    |=  id=block-id:t
    ^-  ?
    (~(has h-by pending-blocks:con) id)
  ::
  ++  check-excluded
    |=  =tx-id:t
    ^-  ?
    =/  has-excluded  (has-excluded tx-id)
    =/  has-bnb-raw-tx  (has-bnb-raw-tx tx-id)
    =/  has-raw-tx  (has-raw-tx tx-id)
    ::~&  :*  has-excluded+has-excluded
    ::        has-bnb-raw-tx+has-bnb-raw-tx
    ::        has-raw-tx+has-raw-tx
    ::    ==
    ?&  has-excluded
        !has-bnb-raw-tx
        has-raw-tx
    ==
  ::
  ++  check-bnb
    |=  [=tx-id:t =block-id:t]
    ^-  ?
    =/  has-excluded  (has-excluded tx-id)
    =/  has-bnb-block-id  (has-bnb-block-id tx-id block-id)
    =/  raw-tx-consistent  ?:((has-raw-tx tx-id) has-bnb-block-id %.y)
    ::~&  :*  has-excluded+has-excluded
    ::        has-bnb-block-id+has-bnb-block-id
    ::        raw-tx-consistent+raw-tx-consistent
    ::    ==
    ?&  !has-excluded
        has-bnb-block-id
        raw-tx-consistent
    ==
  ::
  ++  heard-tx
    |=  =raw-tx:t
    (poke [%fact %0 %heard-tx raw-tx])
  ::
  ++  heard-txs
    |=  txs=(list raw-tx:t)
    =|  effects=(list effect)
    |-
    ?:  ?=(~ txs)
      [effects nockchain]
    =^  effs=(list effect)  nockchain
     (heard-tx i.txs)
    $(effects (weld effects effs), nockchain nockchain, txs t.txs)
  ::
  ++  heard-block
    |=  =page:t
    (poke [%fact %0 %heard-block page])
  ::
  ++  heard-blocks
    |=  pages=(list page:t)
    =|  effects=(list effect)
    |-
    ?:  ?=(~ pages)
      [effects nockchain]
    =^  effs=(list effect)  nockchain
     (heard-block i.pages)
    $(effects (weld effects effs), nockchain nockchain, pages t.pages)
  ::
  ++  poke
    |=  =cause
    =^  effs=(list *)  nockchain
    ::  line below is necessary due to type system nonsense.
    ::  basically, the result desk-hash is type [%~ @uvI], while
    ::  the kernel desk-hash is type u(@uvI)
    =<  [- +(desk-hash.outer [~ *@uvI])]
    (poke:nockchain *@ (build-ovum cause))
    [;;((list effect) effs) nockchain]
  --
--
