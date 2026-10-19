::  tests/dumb/mod/unit/h-zoon-consensus.hoon
::
::    hand-computed consensus flows for h-backed state.
::
/=  dcon  /apps/dumbnet/lib/consensus
/=  dder  /apps/dumbnet/lib/derived
/=  dmin  /apps/dumbnet/lib/miner
/=  dumb  /apps/dumbnet/inner
/=  helpers  /tests/dumb/helpers
/=  fixtures  /tests/dumb/mod/unit/consensus
/=  tx-engine  /common/tx-engine
/=  *  /apps/dumbnet/lib/types
/=  *  /common/h-zoon
/=  *  /common/zeke
/=  *  /common/test
::
|_  constants=_bc-no-timelock:helpers
+*  t  ~(. tx-engine constants)
    h  ~(. helpers constants)
::
++  snip-pre-ai-constants
  |=  bc=blockchain-constants:t
  ^-  blockchain-constants-v1-pre-ai
  :*  v1-phase.bc
      bythos-phase.bc
      data.bc
      base-fee.bc
      input-fee-divisor.bc
      :*  max-block-size.bc
          blocks-per-epoch.bc
          target-epoch-duration.bc
          update-candidate-interval.bc
          max-future-timestamp.bc
          min-past-blocks.bc
          genesis-target-atom.bc
          max-target-atom.bc
          check-pow-flag.bc
          coinbase-timelock-min.bc
          pow-len.bc
          max-coinbase-split.bc
          first-month-coinbase-min.bc
      ==
      phase.zk-asert.bc
      anchor-height.zk-asert.bc
      anchor-target-atom.zk-asert.bc
      ideal-block-time.zk-asert.bc
      half-life.zk-asert.bc
      anchor-min-timestamp.zk-asert.bc
  ==
::
++  pre-ai-state
  |=  current=kernel-state
  ^-  kernel-state-10
  =/  con=consensus-state-10
    %*  .  *consensus-state-10
      blocks-needed-by             blocks-needed-by.c.current
      excluded-txs                 excluded-txs.c.current
      spent-by                     spent-by.c.current
      pending-blocks               pending-blocks.c.current
      balance                      balance.c.current
      txs                          txs.c.current
      raw-txs                      raw-txs.c.current
      blocks                       blocks.c.current
      heaviest-block               heaviest-block.c.current
      min-timestamps               min-timestamps.c.current
      asert-anchor-min-timestamps   asert-anchor-min-timestamps.c.current
      epoch-start                  epoch-start.c.current
      targets                      targets.c.current
      btc-data                     btc-data.c.current
      genesis-seal                 genesis-seal.c.current
    ==
  :*  %10
      c=con
      a=a.current
      m=m.current
      d=[highest-block-height.d.current heaviest-chain.d.current]
      constants=(snip-pre-ai-constants constants.current)
  ==
::
++  test-zoe-proof-version-3-activation-boundary
  =/  con  ~(. dcon initial-consensus-state:h der constants)
  %+  expect-eq
    !>(~[%0 %2 %3 %3])
  !>  :~  (height-to-proof-version:con 0)
          (height-to-proof-version:con (dec proof-version-3-start:dcon))
          (height-to-proof-version:con proof-version-3-start:dcon)
          (height-to-proof-version:con +(proof-version-3-start:dcon))
      ==
::
++  test-zoe-proof-version-gate-includes-genesis
  =/  con  ~(. dcon initial-consensus-state:h der constants)
  =/  base=proof:t  *proof:t
  =/  v0=proof:t  [%0 objects.base hashes.base read-index.base]
  =/  v3=proof:t  [%3 objects.base hashes.base read-index.base]
  %+  expect-eq
    !>([%.y %.n %.y])
  !>  :*  (proof-version-valid:con [0 v0])
          (proof-version-valid:con [0 v3])
          (proof-version-valid:con [proof-version-3-start:dcon v3])
      ==
:::
++  test-zoe-and-logos-proof-discriminators-coexist
  =/  con  ~(. dcon initial-consensus-state:h der constants)
  %+  expect-eq
    !>([%.y %.y %.n])
  !>  :*  (proof-version-valid-at-height:con %3 proof-version-3-start:dcon)
          (proof-version-valid-at-height:con %4 ai-pow-activation-height.constants)
          (proof-version-valid-at-height:con %4 (dec ai-pow-activation-height.constants))
      ==
::
++  test-checkpoint-load-discards-prepolicy-pending-header
  =/  state=kernel-state  complete-fakenet-genesis-state
  =/  pending=page:v1:t  *page:v1:t
  =.  pending
    pending(height 140.000, tx-ids (~(put z-in *(z-set tx-id:t)) *tx-id:t))
  =.  pending  pending(digest (compute-digest:page:t pending))
  =^  missing=(list tx-id:t)  c.state
    (~(add-pending-block dcon c.state d.state constants.state) pending)
  =/  loaded=kernel-state  (load:inner:dumb state)
  ;:  weld
    (expect-eq !>(1) !>((lent missing)))
    (expect-eq !>(1) !>(~(wyt h-by pending-blocks.c.state)))
    (expect-eq !>(0) !>(~(wyt h-by pending-blocks.c.loaded)))
    (expect-eq !>(0) !>(~(wyt h-by blocks-needed-by.c.loaded)))
    (expect-eq !>(blocks.c.state) !>(blocks.c.loaded))
    (expect-eq !>(heaviest-block.c.state) !>(heaviest-block.c.loaded))
  ==
::
++  test-zoe-load-preserves-born-realnet-waiting-for-genesis
  =/  boot-btc-data=(unit (unit btc-hash:t))  ``*btc-hash:t
  =/  realnet-seal=genesis-seal:t  `[0 realnet-genesis-msg]
  =/  waiting-consensus=consensus-state
    %*  .  *consensus-state
      btc-data      boot-btc-data
      genesis-seal  realnet-seal
    ==
  =/  post-born-admin=admin-state  *admin-state
  =.  post-born-admin  post-born-admin(init %.n)
  =/  waiting-state=kernel-state
    %*  .  *kernel-state
      c          waiting-consensus
      a          post-born-admin
      constants  constants
    ==
  =/  loaded=kernel-state  (load:inner:dumb waiting-state)
  %+  expect-eq
    !>([%.n %.y %.y %.y 0])
  !>  :*  init.a.loaded
          =(boot-btc-data btc-data.c.loaded)
          =(realnet-seal genesis-seal.c.loaded)
          ?=(~ heaviest-block.c.loaded)
          ~(wyt h-by blocks.c.loaded)
      ==
::
++  test-zoe-load-rejects-partially-stored-genesis
  =/  con  ~(. dcon initial-consensus-state:h der constants)
  =/  canonical=block-id:t  (~(got z-by checkpointed-digests:con) 0)
  =/  boot-btc-data=(unit (unit btc-hash:t))  ``*btc-hash:t
  =/  realnet-seal=genesis-seal:t  `[0 realnet-genesis-msg]
  =/  stray-page=page:v1:t  *page:v1:t
  =/  partial-consensus=consensus-state
    %*  .  *consensus-state
      blocks
        (~(put h-by *(h-map block-id:t local-page:t)) canonical (to-local-page:page:t stray-page))
      btc-data      boot-btc-data
      genesis-seal  realnet-seal
    ==
  =/  partial-state=kernel-state
    %*  .  *kernel-state
      c          partial-consensus
      constants  constants
    ==
  %+  expect-fail
    |.((load:inner:dumb partial-state))
  ~
::
++  complete-fakenet-genesis-state
  =/  con=consensus-state  initial-consensus-state:h
  =/  d=derived-state
    (~(update dder *derived-state constants) con default-genesis-page:h)
  %*  .  *kernel-state
    c          con
    d          d
    constants  constants(check-pow-flag %.n)
  ==
::
++  extend-fakenet-chain
  |=  [k=kernel-state page-pow=(unit proof:t)]
  ^-  kernel-state
  =/  con  ~(. dcon c.k d.k constants.k)
  =/  parent=page:t
    (to-page:local-page:t (~(got h-by blocks.c.k) (need heaviest-block.c.k)))
  =/  pag=page:t  (make-empty-page:h parent)
  =/  height=page-number:t  ~(height get:page:t pag)
  =/  target=bignum:bignum:t
    ?:  (post-asert-activation:t height)
      (compute-target-zk-asert:con height ~(digest get:page:t parent))
    (~(got h-by targets.c.k) ~(digest get:page:t parent))
  =.  pag
    ?^  -.pag
      pag(target target, pow ?:(?=(~ page-pow) ~ mock-pow:h))
    pag(target target, pow page-pow)
  =/  work=bignum:bignum:t
    %-  chunk:bignum:t
    (add (merge:bignum:t ~(accumulated-work get:page:t parent)) (merge:bignum:t (block-compute-work:con pag)))
  =.  pag
    ?^  -.pag  pag(accumulated-work work)
    pag(accumulated-work work)
  =/  digest=block-id:t  (compute-digest:page:t pag)
  =.  pag
    ?^  -.pag  pag(digest digest)
    pag(digest digest)
  =/  r=(reason tx-acc:t)  (validate-page-with-txs:con pag)
  ~|  [%fakenet-chain-validation-failed height r]
  ?>  ?=(%.y -.r)
  =.  c.k  (accept-page:con pag +.r *@da)
  =.  c.k  (~(update-heaviest dcon c.k d.k constants.k) pag)
  =.  d.k  (~(update dder d.k constants.k) c.k pag)
  k
::
++  complete-zoe-boundary-states
  ~+
  ^-  [before=kernel-state after=kernel-state]
  =/  k=kernel-state  complete-fakenet-genesis-state
  =/  base=proof:t  *proof:t
  |-
  =/  height=page-number:t
    +(~(get-cur-height dcon c.k d.k constants.k))
  =/  version=proof-version:sp
    (~(height-to-proof-version dcon c.k d.k constants.k) height)
  ?>  ?=(?(%0 %1 %2 %3) version)
  =/  prf=proof:t
    ?-  version
      %0  [%0 objects.base hashes.base read-index.base]
      %1  [%1 objects.base hashes.base read-index.base]
      %2  [%2 objects.base hashes.base read-index.base]
      %3  [%3 objects.base hashes.base read-index.base]
    ==
  =/  next=kernel-state  (extend-fakenet-chain k `prf)
  ?:  =(height proof-version-3-start:dcon)
    [k next]
  $(k next)
::
++  fakenet-tip-state-at
  |=  [tip-height=page-number:t page-pow=(unit proof:t)]
  ^-  kernel-state
  =/  tip-page=page:v1:t
    %*  .  *page:v1:t
      height  tip-height
      pow     page-pow
    ==
  =.  tip-page  tip-page(digest (compute-digest:page:t tip-page))
  =/  tip-id=block-id:t  ~(digest get:page:t tip-page)
  =/  genesis-id=block-id:t  *block-id:t
  =/  fakenet-seal=genesis-seal:t  `[0 *hash:t]
  =/  fakenet-consensus=consensus-state
    %*  .  *consensus-state
      blocks
        (~(put h-by *(h-map block-id:t local-page:t)) tip-id (to-local-page:page:t tip-page))
      heaviest-block  `tip-id
      genesis-seal    fakenet-seal
    ==
  =/  fakenet-derived=derived-state
    %*  .  *derived-state
      highest-block-height  `tip-height
      heaviest-chain
        %+  ~(put z-by (~(put z-by *(z-map page-number:t block-id:t)) 0 genesis-id))
          tip-height
        tip-id
    ==
  %*  .  *kernel-state
    c          fakenet-consensus
    d          fakenet-derived
    constants  constants(check-pow-flag %.n)
  ==
::
++  zoe-fakenet-tip-state
  |=  page-pow=(unit proof:t)
  (fakenet-tip-state-at proof-version-3-start:dcon page-pow)
::
++  test-zoe-load-preserves-postactivation-v3-fakenet-tip
  =/  states=[before=kernel-state after=kernel-state]
    complete-zoe-boundary-states
  =/  valid-state=kernel-state  after.states
  =/  loaded=kernel-state  (load:inner:dumb valid-state)
  %+  expect-eq
    !>([%.y %.y %.y %.y])
  !>  :*  =(heaviest-block.c.valid-state heaviest-block.c.loaded)
          =(~(wyt h-by blocks.c.valid-state) ~(wyt h-by blocks.c.loaded))
          =(balance.c.valid-state balance.c.loaded)
          =(genesis-seal.c.valid-state genesis-seal.c.loaded)
      ==
::
++  test-zoe-state-11-preserves-preactivation-chain-and-rebuilds-mining-work
  =/  base=proof:t  *proof:t
  =/  v3=proof:t  [%3 objects.base hashes.base read-index.base]
  =/  states=[before=kernel-state after=kernel-state]
    complete-zoe-boundary-states
  =/  current=kernel-state  before.states
  =/  tip-id=block-id:t  (need heaviest-block.c.current)
  =/  pending=page:v1:t
    %*  .  *page:v1:t
      height  proof-version-3-start:dcon
      parent  tip-id
      pow     (some v3)
    ==
  =.  pending  pending(digest (compute-digest:page:t pending))
  =/  pending-id=block-id:t  ~(digest get:page:t pending)
  =.  pending-blocks.c.current
    (~(put h-by pending-blocks.c.current) pending-id [pending 0])
  ::  A real pending header raises this monotone marker before its transactions
  ::  are complete.  Migration must not mistake that for an accepted tip.
  =.  highest-block-height.d.current  `proof-version-3-start:dcon
  =.  m.current
    %*  .  m.current
      mining          %.y
      shares          (~(put z-by *(z-map hash:t @)) *hash:t 1)
      candidate-block  pending
    ==
  =/  legacy=kernel-state-10  (pre-ai-state current)
  =/  loaded=kernel-state  (load:inner:dumb legacy)
  ::  The Zoe anchor carries the height-112500 target forward rather than
  ::  restoring 2^291, so the rebuilt candidate is built against that target.
  ::  Derive it from the same helper consensus uses instead of restating the
  ::  literal, so the assertion tracks the schedule.
  =/  zcon  ~(. dcon c.current d.current constants.current)
  =/  expected-anchor-target=@
    (asert-target-for-rate:zcon 3.000.000 ideal-block-time.zk-asert.constants.current)
  %+  expect-eq
    !>  [%.y %.y %.y 0 %.y proof-version-3-start:dcon %.y %.y %.y]
  !>  :*
          =(heaviest-block.c.current heaviest-block.c.loaded)
          =(~(wyt h-by blocks.c.current) ~(wyt h-by blocks.c.loaded))
          =(balance.c.current balance.c.loaded)
          ~(wyt h-by pending-blocks.c.loaded)
          !=(*page:t candidate-block.m.loaded)
          ~(height get:page:t candidate-block.m.loaded)
          =(tip-id ~(parent get:page:t candidate-block.m.loaded))
          =(expected-anchor-target (merge:bignum:t ~(target get:page:t candidate-block.m.loaded)))
          =(genesis-seal.c.current genesis-seal.c.loaded)
      ==
::
++  test-zoe-state-11-rejects-cache-starved-state-9-late-upgrade
  =/  current=kernel-state  (fakenet-tip-state-at 112.500 ~)
  =/  legacy-c=consensus-state-9
    %*  .  *consensus-state-9
      blocks-needed-by  blocks-needed-by.c.current
      excluded-txs      excluded-txs.c.current
      spent-by          spent-by.c.current
      pending-blocks    pending-blocks.c.current
      balance           balance.c.current
      txs               txs.c.current
      raw-txs           raw-txs.c.current
      blocks            blocks.c.current
      heaviest-block    heaviest-block.c.current
      min-timestamps    min-timestamps.c.current
      epoch-start       epoch-start.c.current
      targets           targets.c.current
      btc-data          btc-data.c.current
      genesis-seal      genesis-seal.c.current
    ==
  =/  legacy=kernel-state-9
    :*  %9
        c=legacy-c
        a=a.current
        m=m.current
        d=[highest-block-height.d.current heaviest-chain.d.current]
        constants=constants.current
    ==
  %+  expect-fail
    |.((load:inner:dumb legacy))
  `"Version-10 state cannot be migrated safely"
::
++  test-state-10-load-reconstructs-accepted-puzzle-metadata
  =/  f  ~(. fixtures *blockchain-constants:t)
  =/  current=kernel-state  (consensus-batch-load-state:f %.y 123.456)
  =.  current  (hardened-load-extend:f current %.y)
  =.  current  (hardened-load-extend:f current %.n)
  =/  legacy=kernel-state-10  (pre-ai-state current)
  =.  asert-anchor-min-timestamps.c.legacy
    *(map @tas (h-map block-id:t @))
  =/  loaded=kernel-state  (load:inner:dumb legacy)
  =/  ai-id=block-id:t  (~(got z-by heaviest-chain.d.loaded) 170.000)
  =/  zk-id=block-id:t  (need heaviest-block.c.loaded)
  =/  con  ~(. dcon c.loaded d.loaded constants.loaded)
  ;:  weld
    (expect-eq !>(%12) !>(-.loaded))
    (expect-eq !>(blocks.c.current) !>(blocks.c.loaded))
    (expect-eq !>(heaviest-block.c.current) !>(heaviest-block.c.loaded))
    (expect-eq !>(heaviest-chain.d.current) !>(heaviest-chain.d.loaded))
    (expect-eq !>(puzzle-asert-states.d.current) !>(puzzle-asert-states.d.loaded))
    (expect-eq !>((hardened-next-targets:f current)) !>((hardened-next-targets:f loaded)))
    (expect-eq !>([%4 %5]) !>([(~(got h-by block-versions.c.loaded) ai-id) (~(got h-by block-versions.c.loaded) zk-id)]))
    (expect-eq !>([123.456 123.456]) !>([(get-asert-anchor-min-timestamp:con %zk 169.999 zk-id) (get-asert-anchor-min-timestamp:con %ai 169.999 zk-id)]))
    (expect-eq !>(%.y) !>(check-pow-flag.constants.loaded))
    (expect-eq !>(loaded) !>((load:inner:dumb loaded)))
  ==
::
++  test-zoe-state-11-rejects-missing-canonical-min-timestamp
  =/  current=kernel-state  (fakenet-tip-state-at 112.500 ~)
  =/  tip-id=block-id:t  (need heaviest-block.c.current)
  =/  exact-cache=(h-map block-id:t @)
    (~(put h-by *(h-map block-id:t @)) tip-id 1)
  =.  asert-anchor-min-timestamps.c.current
    (~(put by asert-anchor-min-timestamps.c.current) %zk exact-cache)
  =/  legacy=kernel-state-10  (pre-ai-state current)
  %+  expect-fail
    |.((load:inner:dumb legacy))
  `"Version-10 state cannot be migrated safely"
::
++  test-zoe-state-11-deletes-preactivation-v3-side-work-before-events
  =/  base=proof:t  *proof:t
  =/  v3=proof:t  [%3 objects.base hashes.base read-index.base]
  =/  current=kernel-state  (fakenet-tip-state-at 0 ~)
  =/  tip-id=block-id:t  (need heaviest-block.c.current)
  =.  min-timestamps.c.current
    (~(put h-by min-timestamps.c.current) tip-id 1)
  =/  side-page=page:v1:t
    %*  .  *page:v1:t
      height  proof-version-3-start:dcon
      target  (chunk:bignum:t 42)
      pow     (some v3)
    ==
  =.  side-page  side-page(digest (compute-digest:page:t side-page))
  =/  side-id=block-id:t  ~(digest get:page:t side-page)
  =.  blocks.c.current
    (~(put h-by blocks.c.current) side-id (to-local-page:page:t side-page))
  =.  highest-block-height.d.current  `proof-version-3-start:dcon
  =/  legacy=kernel-state-10  (pre-ai-state current)
  =/  loaded=kernel-state  (load:inner:dumb legacy)
  %+  expect-eq
    !>([%.y 1 %.n %.y])
  !>  :*
          =(heaviest-block.c.current heaviest-block.c.loaded)
          ~(wyt h-by blocks.c.loaded)
          (~(has h-by blocks.c.loaded) side-id)
          =(genesis-seal.c.current genesis-seal.c.loaded)
      ==
::
++  test-zoe-load-preserves-proofless-check-disabled-fakenet-tip
  =/  valid-state=kernel-state
    (extend-fakenet-chain complete-fakenet-genesis-state ~)
  =/  loaded=kernel-state  (load:inner:dumb valid-state)
  %+  expect-eq
    !>([%.y %.y %.y %.y])
  !>  :*  =(heaviest-block.c.valid-state heaviest-block.c.loaded)
          =(~(wyt h-by blocks.c.valid-state) ~(wyt h-by blocks.c.loaded))
          =(balance.c.valid-state balance.c.loaded)
          =(genesis-seal.c.valid-state genesis-seal.c.loaded)
      ==
::
++  test-zoe-realnet-genesis-checkpoint-is-enforced
  =/  con  ~(. dcon initial-consensus-state:h der constants)
  =/  canonical=hash:t  (~(got z-by checkpointed-digests:con) 0)
  =/  realnet-seal=genesis-seal:t  `[0 realnet-genesis-msg]
  =/  fakenet-seal=genesis-seal:t  `[0 *hash:t]
  %+  expect-eq
    !>([%.y %.n %.y])
  !>  :*  (checkpoint-digest-valid:con [0 canonical realnet-seal])
          (checkpoint-digest-valid:con [0 *hash:t realnet-seal])
          (checkpoint-digest-valid:con [0 *hash:t fakenet-seal])
      ==
::
::  These identity vectors are independent of the generated checkpoint tree.
++  check-legacy-checkpoint-vector
  |=  [height=page-number:t encoded=@t]
  ^-  tang
  =/  con  ~(. dcon initial-consensus-state:h der constants)
  =/  canonical=hash:t  (from-b58:hash:t encoded)
  =/  realnet=genesis-seal:t  `[0 realnet-genesis-msg]
  =/  fakenet=genesis-seal:t  `[0 *hash:t]
  ;:  weld
    (expect-eq !>(`canonical) !>((checkpoint-digest:con height)))
    %+  expect-eq  !>([%.y %.n %.y %.n])
    !>  :*  (checkpoint-digest-valid:con height canonical realnet)
            (checkpoint-digest-valid:con height *hash:t realnet)
            (checkpoint-digest-valid:con height *hash:t fakenet)
            (checkpoint-digest-valid:con height canonical ~)
        ==
  ==
::
++  test-checkpoint-legacy-start
  (check-legacy-checkpoint-vector 126.000 '38yLHeX4zRSaCWxB7QGmrnJUSVUsJRyNk1h5rhmoZ2XPPvGaSsK3NJy')
::
++  test-checkpoint-legacy-interior
  (check-legacy-checkpoint-vector 140.000 '48dEtWLddEnUm6p9dGJRVTmDjwQgxusPKeEQvoHeG5HfaJ4MatzXtXW')
::
++  test-checkpoint-legacy-end
  (check-legacy-checkpoint-vector 154.499 '6LYLt1haP91TEXJ5vXdnTiPAiv6CbPnSPP8pXxjWAvDu3XsGVrij2Nn')
::
++  test-checkpoint-legacy-outside-boundaries
  =/  con  ~(. dcon initial-consensus-state:h der constants)
  =/  realnet=genesis-seal:t  `[0 realnet-genesis-msg]
  %+  expect-eq  !>([~ ~ %.y %.y %.n %.n])
  !>  :*  (checkpoint-digest:con 125.999)
          (checkpoint-digest:con 154.500)
          (checkpoint-digest-valid:con 125.999 *hash:t realnet)
          (checkpoint-digest-valid:con 154.500 *hash:t realnet)
          (checkpoint-digest-valid:con 125.999 *hash:t ~)
          (checkpoint-digest-valid:con 154.500 *hash:t ~)
      ==
::
::  Self-consistent candidate headers use a synthetic accepted parent, not a
::  fabricated canonical identity. The current tip is taller but has less work;
::  checkpoint rejection must precede both pending insertion and fork choice.
++  check-legacy-checkpoint-admission
  |=  [height=page-number:t ai=?]
  ^-  tang
  (check-legacy-checkpoint-admission-with-map height ai %.n)
::
::  Missing-entry cases mutate only the direct validator's inherited map.
::  Kernel pokes below use the unchanged imported map and run only for the
::  ordinary canonical-pin cases.
++  check-legacy-checkpoint-admission-with-map
  |=  [height=page-number:t ai=? missing=?]
  ^-  tang
  =/  bc=blockchain-constants:t
    %*  .  constants
      v1-phase          1
      ai-pow-activation-height  1
      phase.zk-asert    200.000
      max-target-atom   max-tip5-atom:tip5
    ==
  =/  ht  ~(. helpers bc)
  =/  mt  ~(. tx-engine bc)
  =/  target-atom=@
    ?:(ai max-ai-target-atom:mt max-tip5-atom:tip5)
  =/  parent=page:v1:t
    %*  .  *page:v1:t
      height            (dec height)
      target            (chunk:bignum:t target-atom)
      accumulated-work  (chunk:bignum:t 100)
    ==
  =.  parent  parent(digest (compute-digest:page:mt parent))
  =/  best=page:v1:t
    %*  .  *page:v1:t
      height            (add height 5)
      accumulated-work  (chunk:bignum:t 1)
    ==
  =.  best  best(digest (compute-digest:page:mt best))
  =/  state=kernel-state
    %*  .  *kernel-state
      c
        %*  .  *consensus-state
          blocks
            %-  ~(gas h-by *(h-map block-id:t local-page:t))
            :~  [digest.parent (to-local-page:page:t parent)]
                [digest.best (to-local-page:page:t best)]
            ==
          heaviest-block  `digest.best
          genesis-seal    `[0 realnet-genesis-msg]
          targets
            (~(put h-by *(h-map block-id:t bignum:bignum:t)) digest.parent target.parent)
          min-timestamps
            (~(put h-by *(h-map block-id:t @)) digest.parent 0)
        ==
      d
        %*  .  *derived-state
          highest-block-height  `height.best
        ==
      constants  bc
    ==
  =.  init.a.state  %.n
  =/  con  ~(. dcon c.state d.state bc)
  =.  con
    ?:  missing
      con(legacy-checkpoints (~(del z-by legacy-checkpoints:con) height))
    con
  =/  pag=page:t  (make-empty-page:ht parent)
  ?>  ?=(@ -.pag)
  =/  base=proof:t  *proof:t
  =/  version  (height-to-proof-version:con height)
  =.  pag
    pag(pow `?:(ai (sample-ai-pow-artifact:ht 4) [version objects.base hashes.base read-index.base]))
  =.  accumulated-work.pag
    %-  chunk:bignum:t
    %+  add  (merge:bignum:t accumulated-work.parent)
    (merge:bignum:t (block-compute-work:con pag))
  ::  If admitted, this unknown transaction would force pending mutation.
  =.  tx-ids.pag  (~(put z-in *(z-set tx-id:t)) *tx-id:t)
  =.  digest.pag  (compute-digest:page:mt pag)
  =/  validation
    (validate-page-without-txs:con pag timestamp.pag)
  =/  fakenet=consensus-state  c.state(genesis-seal `[0 *hash:t])
  =/  without-realnet-pin
    (~(validate-page-without-txs con fakenet d.state bc) pag timestamp.pag)
  =/  direct-checks=tang
    ;:  weld
      (expect-eq !>(%.y) !>((check-digest:page:mt pag)))
      (expect-eq !>(%.y) !>((compare-heaviness:page:mt pag (to-local-page:page:t best))))
      (expect-eq !>([%.n %checkpoint-match-failed]) !>(validation))
      (expect-eq !>([%.y ~]) !>(without-realnet-pin))
    ==
  ?:  missing
    %+  weld  direct-checks
    (expect-eq !>(~) !>((~(get z-by legacy-checkpoints:con) height)))
  =/  submission=[wir=path eny=@ our=@ux now=@da dat=*]
    [libp2p-gossip-wire:h 0 0x0 ~2026.1.1 [%fact %0 %heard-block pag]]
  =/  rejected=kernel-state  state
  =^  effects=(list effect)  rejected
    (~(poke inner:dumb state) submission)
  =/  checkpoint-rejected=?
    %+  lien  effects
    |=  eff=effect
    ?&  ?=([%liar-block-id *] eff)
        =(digest.pag p.eff)
        =(%checkpoint-match-failed cause.eff)
    ==
  ;:  weld
    direct-checks
    (expect-eq !>(%.y) !>(checkpoint-rejected))
    (expect-eq !>(c.state) !>(c.rejected))
    (expect-eq !>(d.state) !>(d.rejected))
  ==
::
++  test-checkpoint-legacy-admission-start-shorter-heavier
  (check-legacy-checkpoint-admission 126.000 %.n)
::
++  test-checkpoint-legacy-admission-interior-shorter-heavier
  (check-legacy-checkpoint-admission 140.000 %.n)
::
++  test-checkpoint-legacy-admission-end-shorter-heavier
  (check-legacy-checkpoint-admission 154.499 %.n)
::
++  test-checkpoint-legacy-admission-ai-start-shorter-heavier
  (check-legacy-checkpoint-admission 126.000 %.y)
::
++  test-checkpoint-legacy-admission-ai-interior-shorter-heavier
  (check-legacy-checkpoint-admission 140.000 %.y)
::
++  test-checkpoint-legacy-admission-ai-end-shorter-heavier
  (check-legacy-checkpoint-admission 154.499 %.y)
::
++  test-checkpoint-legacy-missing-start-admission
  (check-legacy-checkpoint-admission-with-map 126.000 %.n %.y)
::
++  test-checkpoint-legacy-missing-end-ai-admission
  (check-legacy-checkpoint-admission-with-map 154.499 %.y %.y)
::
++  test-zoe-pending-page-rechecks-version-after-upgrade
  =/  con  ~(. dcon initial-consensus-state:h der constants)
  =/  base=proof:t  *proof:t
  =/  stale-v2=proof:t  [%2 objects.base hashes.base read-index.base]
  =/  pag=page:v1:t
    %*  .  *page:v1:t
      height  proof-version-3-start:dcon
      pow     (some stale-v2)
    ==
  %+  expect-eq
    !>([%.n %proof-version-invalid])
  !>((validate-page-with-txs:con pag))
::
++  zoe-genesis-chain
  |=  strict=?
  ^-  _nockchain:h
  =/  selected-constants=blockchain-constants:tx-engine
    ?:(strict constants(check-pow-flag %.y) constants)
  =/  chain=_nockchain:h  nockchain:h
  =^  effects=(list effect:h)  chain
    (pok:h [%command %set-constants selected-constants] chain)
  =^  effects  chain  (pok:h [%command %btc-data ~] chain)
  =^  effects  chain
    (pok:h [%command %set-genesis-seal 0 default-genesis-seal:h] chain)
  =^  effects  chain  (pok:h [%command %born ~] chain)
  chain
::
++  zoe-wrong-version-genesis-page
  ^-  page:t
  =/  base-page=page:t  default-genesis-page:h
  =/  base-proof=proof:t  *proof:t
  =/  wrong-proof=proof:t
    [%1 objects.base-proof hashes.base-proof read-index.base-proof]
  =/  pag=page:t  base-page
  =.  pag
    ?^  -.pag
      pag(pow (some wrong-proof))
    pag(pow (some wrong-proof))
  =/  digest=block-id:t  (compute-digest:page:t pag)
  ?^  -.pag
    pag(digest digest)
  pag(digest digest)
::
++  test-zoe-genesis-intake-rejects-wrong-proof-version
  =/  chain=_nockchain:h  (zoe-genesis-chain %.n)
  %+  expect-fail
    |.((pok:h [%fact %0 %heard-block zoe-wrong-version-genesis-page] chain))
  ~
::
++  test-zoe-genesis-intake-rejects-wrong-page-variant
  =/  v1-page=page:v1:t  *page:v1:t
  =.  v1-page  v1-page(digest (compute-digest:page:t v1-page))
  =/  chain=_nockchain:h  (zoe-genesis-chain %.n)
  %+  expect-fail
    |.((pok:h [%fact %0 %heard-block v1-page] chain))
  ~
::
++  test-zoe-genesis-intake-accepts-check-disabled-fakenet-test-page
  =/  base-page=page:t  default-genesis-page:h
  =.  base-page
    ?^  -.base-page
      base-page(pow mock-pow:h)
    base-page(pow mock-pow:h)
  =/  digest=block-id:t  (compute-digest:page:t base-page)
  =.  base-page
    ?^  -.base-page
      base-page(digest digest)
    base-page(digest digest)
  =/  chain=_nockchain:h  (zoe-genesis-chain %.n)
  =^  effects=(list effect:h)  chain
    (pok:h [%fact %0 %heard-block base-page] chain)
  %+  expect-eq
    !>(`~(digest get:page:t base-page))
  !>(heaviest-block.c.internal.outer.chain)
::
++  test-zoe-genesis-intake-rejects-missing-production-proof
  =/  base-page=page:t  default-genesis-page:h
  =/  chain=_nockchain:h  (zoe-genesis-chain %.y)
  %+  expect-fail
    |.((pok:h [%fact %0 %heard-block base-page] chain))
  ~
::
++  test-zoe-proof-version-3-retag-cannot-preserve-block-id
  =/  v2=proof:t  *proof:t
  =/  v3=proof:t  [%3 objects.v2 hashes.v2 read-index.v2]
  =/  template=page:v1:t  *page:v1:t
  =/  page-v2=page:v1:t  template(pow (some v2))
  =/  page-v3=page:v1:t  template(pow (some v3))
  =/  valid-v3=page:v1:t
    page-v3(digest (compute-digest:page:t page-v3))
  =/  retagged=page:v1:t  valid-v3(pow (some v2))
  =/  retagged-digest=block-id:t  (compute-digest:page:t retagged)
  %+  expect-eq
    !>([%.n %.y %.n])
  !>  :*  =((compute-digest:page:t page-v2) (compute-digest:page:t page-v3))
          =((compute-digest:page:t valid-v3) ~(digest get:page:t valid-v3))
          =(retagged-digest ~(digest get:page:t retagged))
      ==
::  +der: pre-activation derived-state (read-only extra arg for consensus/miner doors)
++  der  ^-  derived-state  *derived-state
::
++  consensus-h-apt
  |=  con=consensus-state
  ^-  ?
  ?&  ~(apt h-by blocks-needed-by.con)
      %-  ~(all h-by blocks-needed-by.con)
      |=  ids=(h-set block-id:t)
      ~(apt h-in ids)
      ~(apt h-in excluded-txs.con)
      ~(apt h-by spent-by.con)
      %-  ~(all h-by spent-by.con)
      |=  ids=(h-set tx-id:t)
      ~(apt h-in ids)
      ~(apt h-by pending-blocks.con)
      ~(apt h-by balance.con)
      %-  ~(all h-by balance.con)
      |=  bal=(h-map nname:t nnote:t)
      ~(apt h-by bal)
      ~(apt h-by txs.con)
      %-  ~(all h-by txs.con)
      |=  block-txs=(h-map tx-id:t tx:t)
      ~(apt h-by block-txs)
      ~(apt h-by raw-txs.con)
      ~(apt h-by blocks.con)
      ~(apt h-by min-timestamps.con)
      ~(apt h-by epoch-start.con)
      ~(apt h-by targets.con)
  ==
::
++  with-msg
  |=  [pag=page:t msg=cord]
  ^-  page:t
  =.  pag
    ?^  -.pag
      pag(msg (new:page-msg:t msg))
    pag(msg (new:page-msg:t msg))
  =/  new-digest=block-id:t  (compute-digest:page:t pag)
  ?^  -.pag
    pag(digest new-digest)
  pag(digest new-digest)
::
++  test-consensus-accept-page-h-container-invariants
  =/  con=consensus-state  initial-consensus-state:h
  =^  pag=page:t  con  (add-n-pages:h 1 con default-retain:h)
  =/  bid=block-id:t  ~(digest get:page:t pag)
  =/  h-bal=(h-map nname:t nnote:t)  (~(got h-by balance.con) bid)
  =/  z-blocks=(z-map block-id:t page:t)
    (hz-molt (~(run h-by blocks.con) to-page:local-page:t))
  =/  z-balance=(z-mip block-id:t nname:t nnote:t)  (hz-milt balance.con)
  %+  expect-eq
    !>([%.y %.y %.y %.y %.y %.y %.y %.y])
  !>  :*  (consensus-h-apt con)
          (~(has h-by blocks.con) bid)
          (~(has h-by balance.con) bid)
          (~(has h-by min-timestamps.con) bid)
          (~(has h-by epoch-start.con) bid)
          (~(has h-by targets.con) bid)
          =((~(got z-by z-blocks) bid) pag)
          =((~(got z-by z-balance) bid) (hz-molt h-bal))
      ==
::
++  test-kernel-state-8-load-upgrades-consensus-to-h
  =/  con8=consensus-state  initial-consensus-state:h
  =^  pag=page:t  con8  (add-n-pages:h 1 con8 default-retain:h)
  =/  legacy-c=consensus-state-8
    %*  .  *consensus-state-8
      blocks-needed-by  (hz-jult blocks-needed-by.con8)
      excluded-txs      (hz-silt excluded-txs.con8)
      spent-by          (hz-jult spent-by.con8)
      pending-blocks    (hz-molt pending-blocks.con8)
      balance           (hz-milt balance.con8)
      txs               (hz-milt txs.con8)
      raw-txs           (hz-molt raw-txs.con8)
      blocks            (hz-molt blocks.con8)
      heaviest-block    heaviest-block.con8
      min-timestamps    (hz-molt min-timestamps.con8)
      epoch-start       (hz-molt epoch-start.con8)
      targets           (hz-molt targets.con8)
      btc-data          btc-data.con8
      genesis-seal      genesis-seal.con8
    ==
  =/  d=derived-state  (~(update dder *derived-state constants) con8 pag)
  =/  k8=kernel-state-8
    %*  .  *kernel-state-8
      c  legacy-c
      d  [highest-block-height.d heaviest-chain.d]
      constants  constants
    ==
  =/  current=kernel-state  (load:inner:dumb k8)
  =/  upgraded=consensus-state  c.current
  =/  bid=block-id:t  ~(digest get:page:t pag)
  =/  upgraded-block=local-page:t  (~(got h-by blocks.upgraded) bid)
  =/  original-block=local-page:t  (~(got h-by blocks.con8) bid)
  %+  expect-eq
    !>([%.y %.y %.y %.y %.y %.y %.y])
  !>  :*  (consensus-h-apt upgraded)
          =((hz-molt blocks.upgraded) (hz-molt blocks.con8))
          =((hz-milt balance.upgraded) (hz-milt balance.con8))
          =((hz-milt txs.upgraded) (hz-milt txs.con8))
          =((hz-molt min-timestamps.upgraded) (hz-molt min-timestamps.con8))
          =((hz-molt targets.upgraded) (hz-molt targets.con8))
          =(upgraded-block original-block)
      ==
::
++  test-kernel-state-8-load-upgrades-all-consensus-h-indexes
  =/  con=consensus-state  initial-consensus-state:h
  =^  cb-a=page:t  con  (add-n-pages:h 1 con default-retain:h)
  =^  cb-b=page:t  con  (add-n-pages:h 1 con default-retain:h)
  =/  raw-needed=raw-tx:t  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h cb-a)
  =/  needed-id=tx-id:t  ~(id get:raw-tx:t raw-needed)
  =/  pending-page=page:t  (make-page-with-coinbase-spend:v0:h cb-b cb-a)
  =/  pending-id=block-id:t  ~(digest get:page:t pending-page)
  =^  missing=(list tx-id:t)  con  (~(add-pending-block dcon con der constants) pending-page)
  =^  ready=(list block-id:t)  con  (~(add-raw-tx dcon con der constants) raw-needed)
  =/  raw-extra=raw-tx:t  (make-raw-tx-from-coinbase:v0:h p:default-keys-1:h cb-b)
  =/  extra-id=tx-id:t  ~(id get:raw-tx:t raw-extra)
  =^  extra-ready=(list block-id:t)  con  (~(add-raw-tx dcon con der constants) raw-extra)
  =/  legacy-c=consensus-state-8
    %*  .  *consensus-state-8
      blocks-needed-by  (hz-jult blocks-needed-by.con)
      excluded-txs      (hz-silt excluded-txs.con)
      spent-by          (hz-jult spent-by.con)
      pending-blocks    (hz-molt pending-blocks.con)
      balance           (hz-milt balance.con)
      txs               (hz-milt txs.con)
      raw-txs           (hz-molt raw-txs.con)
      blocks            (hz-molt blocks.con)
      heaviest-block    heaviest-block.con
      min-timestamps    (hz-molt min-timestamps.con)
      epoch-start       (hz-molt epoch-start.con)
      targets           (hz-molt targets.con)
      btc-data          btc-data.con
      genesis-seal      genesis-seal.con
    ==
  =/  d=derived-state  (~(update dder *derived-state constants) con cb-b)
  =/  k8=kernel-state-8
    %*  .  *kernel-state-8
      c  legacy-c
      d  [highest-block-height.d heaviest-chain.d]
      constants  constants
    ==
  =/  current=kernel-state  (load:inner:dumb k8)
  =/  up=consensus-state  c.current
  %+  expect-eq
    !>  %.y
  !>  ?&  =(~[needed-id] missing)
          =(~[pending-id] ready)
          =(~ extra-ready)
          (consensus-h-apt up)
          ?!((~(has h-by blocks-needed-by.up) needed-id))
          (~(has h-in excluded-txs.up) needed-id)
          (~(has h-in excluded-txs.up) extra-id)
          =((hz-jult spent-by.up) (hz-jult spent-by.con))
          =(0 ~(wyt h-by pending-blocks.up))
          =((hz-milt balance.up) (hz-milt balance.con))
          =((hz-milt txs.up) (hz-milt txs.con))
          =((hz-molt raw-txs.up) (hz-molt raw-txs.con))
          =((hz-molt blocks.up) (hz-molt blocks.con))
          (~(has h-by raw-txs.up) needed-id)
      ==
::
++  test-kernel-state-8-upgrade-releases-shared-pending-fanout
  =/  con=consensus-state  initial-consensus-state:h
  =^  cb-page=page:t  con  (add-n-pages:h 1 con default-retain:h)
  =/  raw-needed=raw-tx:t  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h cb-page)
  =/  needed-id=tx-id:t  ~(id get:raw-tx:t raw-needed)
  =/  pag-a=page:t  (make-page-with-coinbase-spend:v0:h cb-page cb-page)
  =/  pag-b=page:t  (with-msg (make-page-with-coinbase-spend:v0:h cb-page cb-page) 'UPGRADE-FANOUT')
  =/  bid-a=block-id:t  ~(digest get:page:t pag-a)
  =/  bid-b=block-id:t  ~(digest get:page:t pag-b)
  =^  missing-a=(list tx-id:t)  con  (~(add-pending-block dcon con der constants) pag-a)
  =^  missing-b=(list tx-id:t)  con  (~(add-pending-block dcon con der constants) pag-b)
  =/  raw-extra=raw-tx:t  (make-raw-tx-from-coinbase:v0:h p:default-keys-3:h cb-page)
  =/  extra-id=tx-id:t  ~(id get:raw-tx:t raw-extra)
  =^  extra-ready=(list block-id:t)  con  (~(add-raw-tx dcon con der constants) raw-extra)
  =/  legacy-c=consensus-state-8
    %*  .  *consensus-state-8
      blocks-needed-by  (hz-jult blocks-needed-by.con)
      excluded-txs      (hz-silt excluded-txs.con)
      spent-by          (hz-jult spent-by.con)
      pending-blocks    (hz-molt pending-blocks.con)
      balance           (hz-milt balance.con)
      txs               (hz-milt txs.con)
      raw-txs           (hz-molt raw-txs.con)
      blocks            (hz-molt blocks.con)
      heaviest-block    heaviest-block.con
      min-timestamps    (hz-molt min-timestamps.con)
      epoch-start       (hz-molt epoch-start.con)
      targets           (hz-molt targets.con)
      btc-data          btc-data.con
      genesis-seal      genesis-seal.con
    ==
  =/  d=derived-state  (~(update dder *derived-state constants) con cb-page)
  =/  k8=kernel-state-8
    %*  .  *kernel-state-8
      c  legacy-c
      d  [highest-block-height.d heaviest-chain.d]
      constants  constants
    ==
  =/  current=kernel-state  (load:inner:dumb k8)
  =/  up=consensus-state  c.current
  =/  upgraded-ok=?
    ?&  =(~[needed-id] missing-a)
        =(~[needed-id] missing-b)
        =(~ extra-ready)
        ?!((~(has h-by pending-blocks.up) bid-a))
        ?!((~(has h-by pending-blocks.up) bid-b))
        ?!((~(has h-by blocks-needed-by.up) needed-id))
        (~(has h-by raw-txs.up) extra-id)
        (~(has h-in excluded-txs.up) extra-id)
        ?!((~(has h-in excluded-txs.up) needed-id))
        (consensus-h-apt up)
    ==
  %+  expect-eq
    !>  %.y
  !>  upgraded-ok
::
++  test-consensus-add-raw-tx-h-index-flow
  =/  con=consensus-state  initial-consensus-state:h
  =^  cb-page=page:t  con  (add-n-pages:h 1 con default-retain:h)
  =/  raw=raw-tx:t  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h cb-page)
  =/  tid=tx-id:t  ~(id get:raw-tx:t raw)
  =/  names=(z-set nname:t)  ~(input-names get:raw-tx:t raw)
  =^  ready=(list block-id:t)  con  (~(add-raw-tx dcon con der constants) raw)
  =/  spent-ok=?
    %-  ~(all z-in names)
    |=  nam=nname:t
    (~(has h-ju spent-by.con) nam tid)
  =/  legacy-raws=(z-map tx-id:t [=raw-tx:t heard-at=@])  (hz-molt raw-txs.con)
  =/  legacy-excluded=(z-set tx-id:t)  (hz-silt excluded-txs.con)
  %+  expect-eq
    !>([%.y %.y %.y %.n %.y %.y %.y %.y])
  !>  :*  =(~ ready)
          (~(has h-by raw-txs.con) tid)
          (~(has h-in excluded-txs.con) tid)
          (~(has h-by blocks-needed-by.con) tid)
          spent-ok
          (consensus-h-apt con)
          (~(has z-by legacy-raws) tid)
          (~(has z-in legacy-excluded) tid)
      ==
::
++  test-consensus-pending-block-h-index-flow
  =/  con=consensus-state  initial-consensus-state:h
  =^  cb-page=page:t  con  (add-n-pages:h 1 con default-retain:h)
  =/  raw=raw-tx:t  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h cb-page)
  =/  tid=tx-id:t  ~(id get:raw-tx:t raw)
  =/  pag=page:t  (make-page-with-coinbase-spend:v0:h cb-page cb-page)
  =/  bid=block-id:t  ~(digest get:page:t pag)
  =/  header-ok=?  -:(~(validate-page-without-txs dcon con der constants) pag ~(timestamp get:page:t pag))
  =^  missing=(list tx-id:t)  con  (~(add-pending-block dcon con der constants) pag)
  =/  pending-ok=?
    ?&  =(~[tid] missing)
        (~(has h-by pending-blocks.con) bid)
        (~(has h-ju blocks-needed-by.con) tid bid)
        ?!((~(has h-in excluded-txs.con) tid))
        (consensus-h-apt con)
    ==
  =^  ready=(list block-id:t)  con  (~(add-raw-tx dcon con der constants) raw)
  =/  raw-ok=?
    ?&  =(~[bid] ready)
        (~(has h-by raw-txs.con) tid)
        (~(has h-ju blocks-needed-by.con) tid bid)
        ?!((~(has h-in excluded-txs.con) tid))
        (consensus-h-apt con)
    ==
  =/  r=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) pag)
  =/  txs-ok=?  ?=(%.y -.r)
  ?.  ?=(%.y -.r)
    ~&  h-zoon-pending-validate-failed++.r  !!
  =/  acc=tx-acc:t  +.r
  =.  con  (~(accept-page dcon con der constants) pag acc *@da)
  %+  expect-eq
    !>([%.y %.y %.y %.y %.y %.n %.y])
  !>  :*  header-ok
          pending-ok
          raw-ok
          txs-ok
          (~(has h-by blocks.con) bid)
          (~(has h-by pending-blocks.con) bid)
          (consensus-h-apt con)
      ==
::
++  test-consensus-raw-first-accept-moves-tx-from-excluded-to-needed
  =/  con=consensus-state  initial-consensus-state:h
  =^  cb-page=page:t  con  (add-n-pages:h 1 con default-retain:h)
  =/  raw=raw-tx:t  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h cb-page)
  =/  tid=tx-id:t  ~(id get:raw-tx:t raw)
  =/  pag=page:t  (make-page-with-coinbase-spend:v0:h cb-page cb-page)
  =/  bid=block-id:t  ~(digest get:page:t pag)
  =^  ready=(list block-id:t)  con  (~(add-raw-tx dcon con der constants) raw)
  =/  raw-only-ok=?  ?&  =(~ ready)  (~(has h-in excluded-txs.con) tid)  ==
  =^  missing=(list tx-id:t)  con  (~(add-pending-block dcon con der constants) pag)
  =/  no-pending-ok=?
    ?&  =(~ missing)
        ?!((~(has h-by pending-blocks.con) bid))
        (~(has h-in excluded-txs.con) tid)
    ==
  =/  r=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) pag)
  ?.  ?=(%.y -.r)
    ~&  h-zoon-raw-first-validate-failed++.r  !!
  =.  con  (~(accept-page dcon con der constants) pag +.r *@da)
  %+  expect-eq
    !>([%.y %.y %.y %.y %.y %.n %.y])
  !>  :*  raw-only-ok
          no-pending-ok
          (~(has h-by blocks.con) bid)
          (~(has h-ju blocks-needed-by.con) tid bid)
          (~(has h-by raw-txs.con) tid)
          (~(has h-in excluded-txs.con) tid)
          (consensus-h-apt con)
      ==
::
++  test-consensus-shared-pending-dependency-survives-one-rejection
  =/  con=consensus-state  initial-consensus-state:h
  =^  cb-page=page:t  con  (add-n-pages:h 1 con default-retain:h)
  =/  raw=raw-tx:t  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h cb-page)
  =/  tid=tx-id:t  ~(id get:raw-tx:t raw)
  =/  pag-a=page:t  (make-page-with-coinbase-spend:v0:h cb-page cb-page)
  =/  pag-b=page:t  (with-msg (make-page-with-coinbase-spend:v0:h cb-page cb-page) 'ALT-PENDING')
  =/  bid-a=block-id:t  ~(digest get:page:t pag-a)
  =/  bid-b=block-id:t  ~(digest get:page:t pag-b)
  =^  missing-a=(list tx-id:t)  con  (~(add-pending-block dcon con der constants) pag-a)
  =^  missing-b=(list tx-id:t)  con  (~(add-pending-block dcon con der constants) pag-b)
  =^  ready=(list block-id:t)  con  (~(add-raw-tx dcon con der constants) raw)
  =/  ready-set=(z-set block-id:t)  (z-silt ready)
  =/  both-pending-ok=?
    ?&  =(~[tid] missing-a)
        =(~[tid] missing-b)
        (~(has z-in ready-set) bid-a)
        (~(has z-in ready-set) bid-b)
        (~(has h-ju blocks-needed-by.con) tid bid-a)
        (~(has h-ju blocks-needed-by.con) tid bid-b)
        ?!((~(has h-in excluded-txs.con) tid))
    ==
  =.  con  (~(reject-pending-block dcon con der constants) bid-a)
  =/  one-rejected-ok=?
    ?&  ?!((~(has h-by pending-blocks.con) bid-a))
        (~(has h-by pending-blocks.con) bid-b)
        ?!((~(has h-ju blocks-needed-by.con) tid bid-a))
        (~(has h-ju blocks-needed-by.con) tid bid-b)
        ?!((~(has h-in excluded-txs.con) tid))
        (consensus-h-apt con)
    ==
  =.  con  (~(reject-pending-block dcon con der constants) bid-b)
  %+  expect-eq
    !>([%.y %.y %.n %.n %.y %.y])
  !>  :*  both-pending-ok
          one-rejected-ok
          (~(has h-by pending-blocks.con) bid-b)
          (~(has h-by blocks-needed-by.con) tid)
          (~(has h-in excluded-txs.con) tid)
          (consensus-h-apt con)
      ==
::
++  test-consensus-reject-pending-block-restores-raw-tx-index
  =/  con=consensus-state  initial-consensus-state:h
  =^  cb-page=page:t  con  (add-n-pages:h 1 con default-retain:h)
  =/  raw=raw-tx:t  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h cb-page)
  =/  tid=tx-id:t  ~(id get:raw-tx:t raw)
  =/  pag=page:t  (make-page-with-coinbase-spend:v0:h cb-page cb-page)
  =/  bid=block-id:t  ~(digest get:page:t pag)
  =^  missing=(list tx-id:t)  con  (~(add-pending-block dcon con der constants) pag)
  =^  ready=(list block-id:t)  con  (~(add-raw-tx dcon con der constants) raw)
  =.  con  (~(reject-pending-block dcon con der constants) bid)
  %+  expect-eq
    !>([%.y %.y %.y %.n %.n %.y %.y])
  !>  :*  =(~[tid] missing)
          =(~[bid] ready)
          (~(has h-by raw-txs.con) tid)
          (~(has h-by pending-blocks.con) bid)
          (~(has h-by blocks-needed-by.con) tid)
          (~(has h-in excluded-txs.con) tid)
          (consensus-h-apt con)
      ==
::
++  test-consensus-drop-raw-tx-clears-excluded-and-spent-indexes
  =/  con=consensus-state  initial-consensus-state:h
  =^  cb-page=page:t  con  (add-n-pages:h 1 con default-retain:h)
  =/  raw=raw-tx:t  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h cb-page)
  =/  tid=tx-id:t  ~(id get:raw-tx:t raw)
  =/  names=(z-set nname:t)  ~(input-names get:raw-tx:t raw)
  =^  ready=(list block-id:t)  con  (~(add-raw-tx dcon con der constants) raw)
  =/  before-ok=?
    ?&  =(~ ready)
        (~(has h-by raw-txs.con) tid)
        (~(has h-in excluded-txs.con) tid)
        %-  ~(all z-in names)
        |=  nam=nname:t
        (~(has h-ju spent-by.con) nam tid)
    ==
  =.  con  (~(drop-tx dcon con der constants) tid)
  =/  spent-cleared=?
    %-  ~(all z-in names)
    |=  nam=nname:t
    ?!((~(has h-by spent-by.con) nam))
  %+  expect-eq
    !>([%.y %.n %.n %.y %.y])
  !>  :*  before-ok
          (~(has h-by raw-txs.con) tid)
          (~(has h-in excluded-txs.con) tid)
          spent-cleared
          (consensus-h-apt con)
      ==
::
++  test-consensus-check-and-repair-adds-fallen-through-raw-tx-to-excluded
  =/  con=consensus-state  initial-consensus-state:h
  =^  cb-page=page:t  con  (add-n-pages:h 1 con default-retain:h)
  =/  raw=raw-tx:t  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h cb-page)
  =/  tid=tx-id:t  ~(id get:raw-tx:t raw)
  =/  broken=consensus-state
    con(raw-txs (~(put h-by raw-txs.con) tid [raw 0]))
  =/  repaired=consensus-state  ~(check-and-repair dcon broken der constants)
  %+  expect-eq
    !>([`%txs-fell-through-cracks %.y %.y %.y])
  !>  :*  ~(apt dcon broken der constants)
          (~(has h-by raw-txs.repaired) tid)
          (~(has h-in excluded-txs.repaired) tid)
          (consensus-h-apt repaired)
      ==
::
++  balance-kernel-peeks
  |=  [con=consensus-state bid=block-id:t]
  ^-  (list (unit (unit *)))
  =/  k=kernel-state
    %*  .  *kernel-state
      c  con
    ==
  =/  kern  ~(. inner:dumb k)
  =/  bid-b58=@t  (to-b58:hash:t bid)
  =/  first-name=@t
    (to-b58:hash:t ~(first-name get:nnote:t make-default-coinbase:h))
  =/  pubkey=@t  (to-b58:schnorr-pubkey:t default-a-pt-1:h)
  :~  (peek:kern [%balance bid-b58 ~])
      (peek:kern [%h-balance bid-b58 ~])
      (peek:kern [%current-balance ~])
      (peek:kern [%h-current-balance ~])
      (peek:kern [%balance-by-first-name first-name ~])
      (peek:kern [%balance-by-pubkey pubkey ~])
  ==
::
++  test-kernel-peek-known-empty-genesis-balances
  =/  con=consensus-state  initial-consensus-state:h
  =/  bid=block-id:t  default-genesis-id:h
  ?>  ?!((~(has h-by balance.con) bid))
  =/  empty-z=(z-map nname:t nnote:t)  *(z-map nname:t nnote:t)
  =/  empty-h=(h-map nname:t nnote:t)  *(h-map nname:t nnote:t)
  =/  snapshot=[page-number:t block-id:t (z-map nname:t nnote:t)]
    [0 bid empty-z]
  %+  expect-eq
    !>(~[``empty-z ``empty-h ``empty-z ``empty-h ``snapshot ``snapshot])
  !>((balance-kernel-peeks con bid))
::
++  test-kernel-peek-no-chain-balances-absent
  %+  expect-eq
    !>(~[[~ ~] [~ ~] [~ ~] [~ ~] [~ ~] [~ ~]])
  !>((balance-kernel-peeks *consensus-state default-genesis-id:h))
::
++  test-kernel-peek-unknown-block-balances-absent
  =/  con=consensus-state  initial-consensus-state:h
  =/  unknown=page:t  (make-empty-page:h default-genesis-page:h)
  =/  bid=block-id:t  ~(digest get:page:t unknown)
  ?>  ?!((~(has h-by blocks.con) bid))
  =/  results  (balance-kernel-peeks con bid)
  %+  expect-eq
    !>(~[[~ ~] [~ ~]])
  !>((scag 2 results))
::
++  test-kernel-peek-historical-empty-genesis-balances
  =/  con=consensus-state  initial-consensus-state:h
  =^  pag=page:t  con  (add-n-pages:h 1 con default-retain:h)
  =/  bid=block-id:t  default-genesis-id:h
  =/  results  (balance-kernel-peeks con bid)
  =/  empty-z=(z-map nname:t nnote:t)  *(z-map nname:t nnote:t)
  =/  empty-h=(h-map nname:t nnote:t)  *(h-map nname:t nnote:t)
  %+  expect-eq
    !>(~[``empty-z ``empty-h])
  !>((scag 2 results))
::
++  test-kernel-peek-missing-non-genesis-balances-absent
  =/  con=consensus-state  initial-consensus-state:h
  =^  pag=page:t  con  (add-n-pages:h 1 con default-retain:h)
  =/  bid=block-id:t  ~(digest get:page:t pag)
  ?>  (~(has h-by balance.con) bid)
  =.  balance.con  (~(del h-by balance.con) bid)
  %+  expect-eq
    !>(~[[~ ~] [~ ~] [~ ~] [~ ~] [~ ~] [~ ~]])
  !>((balance-kernel-peeks con bid))
::
++  test-get-cur-balance-missing-non-genesis-fails
  =/  con=consensus-state  initial-consensus-state:h
  =^  pag=page:t  con  (add-n-pages:h 1 con default-retain:h)
  =/  bid=block-id:t  ~(digest get:page:t pag)
  =.  balance.con  (~(del h-by balance.con) bid)
  %+  expect-fail
    |.(~(get-cur-balance dcon con der constants))
  ~
::
++  test-kernel-peek-legacy-and-native-h-scries-agree
  =/  con=consensus-state  initial-consensus-state:h
  =^  cb-page=page:t  con  (add-n-pages:h 1 con default-retain:h)
  =/  raw=raw-tx:t  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h cb-page)
  =/  tid=tx-id:t  ~(id get:raw-tx:t raw)
  =/  pag=page:t  (make-page-with-coinbase-spend:v0:h cb-page cb-page)
  =/  bid=block-id:t  ~(digest get:page:t pag)
  =^  ready=(list block-id:t)  con  (~(add-raw-tx dcon con der constants) raw)
  ?>  =(~ ready)
  =/  r=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) pag)
  ?.  ?=(%.y -.r)
    ~&  h-zoon-scry-validate-failed++.r  !!
  =.  con  (~(accept-page dcon con der constants) pag +.r *@da)
  =/  bid-b58=@t  (to-b58:hash:t bid)
  =/  k=kernel-state
    %*  .  *kernel-state
      c  con
    ==
  =/  kern  ~(. inner:dumb k)
  =/  z-blocks-res=(unit (unit *))  (peek:kern [%blocks ~])
  =/  h-blocks-res=(unit (unit *))  (peek:kern [%h-blocks ~])
  =/  z-transactions-res=(unit (unit *))  (peek:kern [%transactions ~])
  =/  h-transactions-res=(unit (unit *))  (peek:kern [%h-transactions ~])
  =/  z-raw-res=(unit (unit *))  (peek:kern [%raw-transactions ~])
  =/  h-raw-res=(unit (unit *))  (peek:kern [%h-raw-transactions ~])
  =/  z-excluded-res=(unit (unit *))  (peek:kern [%excluded-txs ~])
  =/  h-excluded-res=(unit (unit *))  (peek:kern [%h-excluded-txs ~])
  =/  z-block-txs-res=(unit (unit *))  (peek:kern [%block-transactions bid-b58 ~])
  =/  h-block-txs-res=(unit (unit *))  (peek:kern [%h-block-transactions bid-b58 ~])
  =/  z-balance-at-res=(unit (unit *))  (peek:kern [%balance bid-b58 ~])
  =/  h-balance-at-res=(unit (unit *))  (peek:kern [%h-balance bid-b58 ~])
  =/  z-current-balance-res=(unit (unit *))  (peek:kern [%current-balance ~])
  =/  h-current-balance-res=(unit (unit *))  (peek:kern [%h-current-balance ~])
  =/  summary-res=(unit (unit *))  (peek:kern [%blocks-summary ~])
  ?>  ?&  ?=(^ z-blocks-res)  ?=(^ u.z-blocks-res)
          ?=(^ h-blocks-res)  ?=(^ u.h-blocks-res)
          ?=(^ z-transactions-res)  ?=(^ u.z-transactions-res)
          ?=(^ h-transactions-res)  ?=(^ u.h-transactions-res)
          ?=(^ z-raw-res)  ?=(^ u.z-raw-res)
          ?=(^ h-raw-res)  ?=(^ u.h-raw-res)
          ?=(^ z-excluded-res)  ?=(^ u.z-excluded-res)
          ?=(^ h-excluded-res)  ?=(^ u.h-excluded-res)
          ?=(^ z-block-txs-res)  ?=(^ u.z-block-txs-res)
          ?=(^ h-block-txs-res)  ?=(^ u.h-block-txs-res)
          ?=(^ z-balance-at-res)  ?=(^ u.z-balance-at-res)
          ?=(^ h-balance-at-res)  ?=(^ u.h-balance-at-res)
          ?=(^ z-current-balance-res)  ?=(^ u.z-current-balance-res)
          ?=(^ h-current-balance-res)  ?=(^ u.h-current-balance-res)
          ?=(^ summary-res)  ?=(^ u.summary-res)
      ==
  =/  z-blocks=(z-map block-id:t page:t)
    ;;((z-map block-id:t page:t) u.u.z-blocks-res)
  =/  h-blocks=(h-map block-id:t page:t)
    ;;((h-map block-id:t page:t) u.u.h-blocks-res)
  =/  z-transactions=(z-mip block-id:t tx-id:t tx:t)
    ;;((z-mip block-id:t tx-id:t tx:t) u.u.z-transactions-res)
  =/  h-transactions=(h-mip block-id:t tx-id:t tx:t)
    ;;((h-mip block-id:t tx-id:t tx:t) u.u.h-transactions-res)
  =/  z-raw=(z-map tx-id:t [=raw-tx:t heard-at=@])
    ;;((z-map tx-id:t [=raw-tx:t heard-at=@]) u.u.z-raw-res)
  =/  h-raw=(h-map tx-id:t [=raw-tx:t heard-at=@])
    ;;((h-map tx-id:t [=raw-tx:t heard-at=@]) u.u.h-raw-res)
  =/  z-excluded=(z-set tx-id:t)
    ;;((z-set tx-id:t) u.u.z-excluded-res)
  =/  h-excluded=(h-set tx-id:t)
    ;;((h-set tx-id:t) u.u.h-excluded-res)
  =/  z-block-txs=(z-map tx-id:t tx:t)
    ;;((z-map tx-id:t tx:t) u.u.z-block-txs-res)
  =/  h-block-txs=(h-map tx-id:t tx:t)
    ;;((h-map tx-id:t tx:t) u.u.h-block-txs-res)
  =/  z-balance-at=(z-map nname:t nnote:t)
    ;;((z-map nname:t nnote:t) u.u.z-balance-at-res)
  =/  h-balance-at=(h-map nname:t nnote:t)
    ;;((h-map nname:t nnote:t) u.u.h-balance-at-res)
  =/  z-current-balance=(z-map nname:t nnote:t)
    ;;((z-map nname:t nnote:t) u.u.z-current-balance-res)
  =/  h-current-balance=(h-map nname:t nnote:t)
    ;;((h-map nname:t nnote:t) u.u.h-current-balance-res)
  =/  summary=(list [block-id:t page:t])
    ;;((list [block-id:t page:t]) u.u.summary-res)
  %+  expect-eq
    !>([%.y %.y %.y %.y %.y %.y %.y %.y %.y %.n %.y %.n %.y %.y])
  !>  :*  =((hz-molt h-blocks) z-blocks)
          =((hz-milt h-transactions) z-transactions)
          =((hz-molt h-raw) z-raw)
          =((hz-silt h-excluded) z-excluded)
          =((hz-molt h-block-txs) z-block-txs)
          =((hz-molt h-balance-at) z-balance-at)
          =((hz-molt h-current-balance) z-current-balance)
          =((~(got z-by z-blocks) bid) pag)
          (~(has z-by z-raw) tid)
          (~(has z-in z-excluded) tid)
          (~(has h-by h-raw) tid)
          (~(has h-in h-excluded) tid)
          =((lent summary) ~(wyt h-by blocks.con))
          ?=(^ summary)
      ==
::
++  test-miner-add-txs-to-candidate-no-keys-is-noop
  =/  con=consensus-state  initial-consensus-state:h
  =^  pag=page:t  con  (add-n-pages:h 1 con default-retain:h)
  =/  seeded=mining-state  initial-mining-state:h
  =.  seeded  (~(heard-new-block dmin seeded der constants) con *@da)
  =/  raw=raw-tx:t  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h pag)
  =^  ready=(list block-id:t)  con  (~(add-raw-tx dcon con der constants) raw)
  ?>  =(~ ready)
  =/  no-key=mining-state
    %*  .  seeded
      v0-shares  *(z-map sig:t @)
      shares     *(z-map hash:t @)
    ==
  ?>  ?&  ?=(^ -.candidate-block.no-key)
          (~(has h-in excluded-txs.con) ~(id get:raw-tx:t raw))
      ==
  =/  after=mining-state  (~(add-txs-to-candidate dmin no-key der constants) con)
  %+  expect-eq
    !>([%.y %.y %.y])
  !>  :*  =(no-key after)
          =(*(z-set tx-id:t) ~(tx-ids get:page:t candidate-block.after))
          =(candidate-acc.no-key candidate-acc.after)
      ==
::  +|  %random-walk
::
::    +stateful-consensus-walk picks an op from
::      {add-coinbase, add-raw-tx, add-pending-block,
::       reject-pending-block, drop-tx, accept-page,
::       check-and-repair}
::    at each step. when a new mutator lands, add a matching variant to
::    +draw-consensus-op below and surface it from the same op-id space.
::    the invariant after every step is the pair:
::      ?&  (consensus-h-apt con)
::          ::  every h-* container survives a hz-* -> zh-* round trip
::          =(con (zh-roundtrip con))
::      ==
::    on failure the walker crashes with the failing op id and step index
::    so the trace can be pinned as a regression.
::  z-equivalence check: every field's z-projection round-trips back
::  through the inverse conversion to the same h-noun. round-tripping
::  is equivalent to "this h-noun is in the image of zh-* applied to its
::  hz-* image" — i.e., the noun is the same shape the migration would
::  produce. h-treaps are uniquely determined by their key set, so this
::  holds iff hz-/zh-* preserve key sets in both directions.
++  consensus-h-z-equivalent
  |=  con=consensus-state
  ^-  ?
  ?&
      =(blocks-needed-by.con (zh-jult (hz-jult blocks-needed-by.con)))
      =(excluded-txs.con (zh-silt (hz-silt excluded-txs.con)))
      =(spent-by.con (zh-jult (hz-jult spent-by.con)))
      =(pending-blocks.con (zh-molt (hz-molt pending-blocks.con)))
      =(balance.con (zh-milt (hz-milt balance.con)))
      =(txs.con (zh-milt (hz-milt txs.con)))
      =(raw-txs.con (zh-molt (hz-molt raw-txs.con)))
      =(blocks.con (zh-molt (hz-molt blocks.con)))
      =(min-timestamps.con (zh-molt (hz-molt min-timestamps.con)))
      =(epoch-start.con (zh-molt (hz-molt epoch-start.con)))
      =(targets.con (zh-molt (hz-molt targets.con)))
  ==
::
++  walk-rand
  |=  [seed=@uv ctr=@]
  ^-  @
  =/  digest=noun-digest:tip5:z  (hash-noun-varlen:tip5:z [%walk seed ctr])
  (digest-to-atom:tip5:z digest)
::
::  available ops:
::    %0 add a coinbase (extends the chain by one page)
::    %1 add a raw tx for a known coinbase
::    %2 add a pending block for two known coinbases (spend + parent)
::    %3 reject a known pending block
::    %4 drop a known raw tx
::    %5 check-and-repair (no-op when state is sound)
++  draw-consensus-op
  |=  [seed=@uv ctr=@ have-cb=? have-raw=? have-pending=?]
  ^-  @
  ?.  have-cb  %0  :: must add a coinbase first
  =/  bucket=@  (mod (walk-rand seed ctr) 16)
  ?:  (lth bucket 3)   %0
  ?:  (lth bucket 7)
    ?:(have-cb %1 %0)
  ?:  (lth bucket 10)
    ?:(have-cb %2 %0)
  ?:  (lth bucket 12)
    ?:(have-pending %3 ?:(have-cb %1 %0))
  ?:  (lth bucket 14)
    ?:(have-raw %4 ?:(have-cb %1 %0))
  %5
::
::  64 ops per seed * 3 seeds keeps the run under the existing test
::  budget. when a regression turns up, pin the seed and bump
::  walk-steps locally to reproduce.
++  walk-seeds  `(list @uv)`~[0v1 0v2 0v3.cafe7]
++  walk-steps  64
::
::  state machine threaded by the walker:
::    coinbases - pages we have a coinbase for
::    raw-ids   - raw-tx ids the consensus already heard
::    pending   - pending block ids the consensus is tracking
::    counter   - monotonic message salt so pending blocks are distinct
++  walk-state
  $:  con=consensus-state
      coinbases=(list page:t)
      raw-ids=(list tx-id:t)
      pending=(list block-id:t)
      tick=@
  ==
::
::  pick-by-rand: pull a deterministic element out of a typed list. one
::  thin wrapper per list type because hoon's wet-gate type inference
::  loses the head type when threading +snag through a typed result.
::  the index is normalized outside the trap so the loop body sees lst
::  as possibly empty without a (mod salt 0) crash on degenerate input.
++  pick-page
  |=  [lst=(list page:t) salt=@]
  ^-  page:t
  =/  size=@  (lent lst)
  =/  idx=@  ?:(=(0 size) 0 (mod salt size))
  |-  ^-  page:t
  ?~  lst  ~|(%pick-page-empty !!)
  ?:  =(0 idx)  i.lst
  $(lst t.lst, idx (dec idx))
::
++  pick-block-id
  |=  [lst=(list block-id:t) salt=@]
  ^-  block-id:t
  =/  size=@  (lent lst)
  =/  idx=@  ?:(=(0 size) 0 (mod salt size))
  |-  ^-  block-id:t
  ?~  lst  ~|(%pick-block-id-empty !!)
  ?:  =(0 idx)  i.lst
  $(lst t.lst, idx (dec idx))
::
++  pick-tx-id
  |=  [lst=(list tx-id:t) salt=@]
  ^-  tx-id:t
  =/  size=@  (lent lst)
  =/  idx=@  ?:(=(0 size) 0 (mod salt size))
  |-  ^-  tx-id:t
  ?~  lst  ~|(%pick-tx-id-empty !!)
  ?:  =(0 idx)  i.lst
  $(lst t.lst, idx (dec idx))
::
::  +skim is wet and inferred its return type from the input. wrapping
::  the result in a face-typed cell update tripped a deep nest-fail on
::  block-id and tx-id lists, so we hand-roll the filter to keep the
::  element type stable.
++  filter-block-ids
  |=  [lst=(list block-id:t) drop=block-id:t]
  ^-  (list block-id:t)
  ?~  lst  ~
  ?:  =(drop i.lst)  $(lst t.lst)
  [i.lst $(lst t.lst)]
::
++  filter-tx-ids
  |=  [lst=(list tx-id:t) drop=tx-id:t]
  ^-  (list tx-id:t)
  ?~  lst  ~
  ?:  =(drop i.lst)  $(lst t.lst)
  [i.lst $(lst t.lst)]
::
::  one walker step. dispatches on op and returns the updated state.
::  when an op cannot fire cleanly (e.g., accept-page would diverge from
::  the tx validation oracle), we no-op rather than corrupt the state.
++  walk-step
  |=  [seed=@uv ws=walk-state ctr=@]
  ^-  walk-state
  =/  have-cb=?      ?=(^ coinbases.ws)
  =/  have-raw=?     ?=(^ raw-ids.ws)
  =/  have-pending=?  ?=(^ pending.ws)
  =/  op=@  (draw-consensus-op seed ctr have-cb have-raw have-pending)
  ?+  op  ws
      %0
    =^  pag=page:t  con.ws  (add-n-pages:h 1 con.ws default-retain:h)
    ws(con con.ws, coinbases [pag coinbases.ws], tick +(tick.ws))
  ::
      %1
    ?.  ?=(^ coinbases.ws)  ws
    =/  cb=page:t  (pick-page coinbases.ws (walk-rand seed (add ctr 11)))
    ::  v0 raw-tx ctor accepts only v0 coinbase pages; bail on v1 picks.
    ?.  ?^(-.cb %.y %.n)  ws
    =/  raw=raw-tx:t  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h cb)
    =/  tid=tx-id:t  ~(id get:raw-tx:t raw)
    ?:  (~(has h-by raw-txs.con.ws) tid)  ws
    =^  ready=(list block-id:t)  con.ws  (~(add-raw-tx dcon con.ws der constants) raw)
    ws(con con.ws, raw-ids [tid raw-ids.ws], tick +(tick.ws))
  ::
      %2
    ?.  ?=(^ coinbases.ws)  ws
    =/  cb=page:t  (pick-page coinbases.ws (walk-rand seed (add ctr 53)))
    ::  v0-helpers reject v1 pages; if the picked coinbase is post-asert
    ::  we skip rather than crash. -.cb is the digest (cell of limbs) for
    ::  v0 and the version atom for v1.
    ?.  ?^(-.cb %.y %.n)  ws
    =/  msg=@t  (cat 3 'WALK-' (scot %ud tick.ws))
    =/  pag=page:t  (with-msg (make-page-with-coinbase-spend:v0:h cb cb) msg)
    =/  bid=block-id:t  ~(digest get:page:t pag)
    ?:  (~(has h-by pending-blocks.con.ws) bid)  ws
    ?:  (~(has h-by blocks.con.ws) bid)  ws
    =/  ok=?  -:(~(validate-page-without-txs dcon con.ws der constants) pag ~(timestamp get:page:t pag))
    ?.  ok  ws
    =^  missing=(list tx-id:t)  con.ws  (~(add-pending-block dcon con.ws der constants) pag)
    ws(con con.ws, pending [bid pending.ws], tick +(tick.ws))
  ::
      %3
    ?.  ?=(^ pending.ws)  ws
    =/  bid=block-id:t  (pick-block-id pending.ws (walk-rand seed (add ctr 71)))
    ?.  (~(has h-by pending-blocks.con.ws) bid)
      ::  drift between our shadow pending list and consensus; drop the
      ::  stale id from the shadow and keep walking.
      ws(pending (filter-block-ids pending.ws bid))
    =.  con.ws  (~(reject-pending-block dcon con.ws der constants) bid)
    ws(con con.ws, pending (filter-block-ids pending.ws bid), tick +(tick.ws))
  ::
      %4
    ?.  ?=(^ raw-ids.ws)  ws
    =/  tid=tx-id:t  (pick-tx-id raw-ids.ws (walk-rand seed (add ctr 89)))
    ?.  (~(has h-by raw-txs.con.ws) tid)
      ws(raw-ids (filter-tx-ids raw-ids.ws tid))
    ::  drop-tx pre-condition (consensus.hoon:857): tx must not be needed
    ::  by any pending block. otherwise the assertion crashes.
    ?:  (~(has h-by blocks-needed-by.con.ws) tid)  ws
    =.  con.ws  (~(drop-tx dcon con.ws der constants) tid)
    ws(con con.ws, raw-ids (filter-tx-ids raw-ids.ws tid), tick +(tick.ws))
  ::
      %5
    =.  con.ws  ~(check-and-repair dcon con.ws der constants)
    ws(con con.ws, tick +(tick.ws))
  ==
::
++  walk-consensus
  |=  [seed=@uv n=@]
  ^-  ?
  =/  ws=walk-state
    [initial-consensus-state:h ~ ~ ~ 0]
  =/  ctr=@  0
  |-
  ?:  =(ctr n)
    ?.  (consensus-h-apt con.ws)
      ~&  [%walk-final-apt-failed seed]  !!
    ?.  (consensus-h-z-equivalent con.ws)
      ~&  [%walk-final-zequiv-failed seed]  !!
    %.y
  =/  ws-next  (walk-step seed ws ctr)
  ?.  (consensus-h-apt con.ws-next)
    ~&  [%walk-step-apt-failed seed step=ctr]  !!
  ?.  (consensus-h-z-equivalent con.ws-next)
    ~&  [%walk-step-zequiv-failed seed step=ctr]  !!
  $(ctr +(ctr), ws ws-next)
::
++  test-consensus-random-walk-preserves-z-equivalence
  =/  seeds=(list @uv)  walk-seeds
  %+  expect-eq
    !>(~[%.y %.y %.y])
  !>  %+  turn  seeds
      |=  seed=@uv
      (walk-consensus seed walk-steps)
--
