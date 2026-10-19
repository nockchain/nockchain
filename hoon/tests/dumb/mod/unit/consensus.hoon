::  tests/dumb/consensus.hoon
/=  dcon  /apps/dumbnet/lib/consensus
/=  dmin  /apps/dumbnet/lib/miner
/=  dder  /apps/dumbnet/lib/derived
/=  dumb  /apps/dumbnet/inner
/=  helpers  /tests/dumb/helpers
/=  tx-engine  /common/tx-engine
/=  *  /apps/dumbnet/lib/types
/=  *  /common/h-zoon
/=  *  /common/test
::
|_  constants=blockchain-constants:tx-engine
+*  t  ~(. tx-engine constants)
    h  ~(. helpers constants)
::
::  +der: pre-activation derived-state (read-only extra door arg); arms that
::  need the real derived-state shadow this with a local `=/  der`.
++  der  ^-  derived-state  *derived-state
::
++  zoe-load-state
  |=  [rooted=? page-pow=(unit proof:t)]
  ^-  kernel-state
  =/  genesis-page=page:v1:t  *page:v1:t
  =.  genesis-page  genesis-page(digest (compute-digest:page:t genesis-page))
  =/  genesis-id=block-id:t  ~(digest get:page:t genesis-page)
  =/  tip-page=page:v1:t
    %*  .  *page:v1:t
      height  proof-version-3-start:dcon
      parent  genesis-id
      pow     page-pow
    ==
  =.  tip-page  tip-page(digest (compute-digest:page:t tip-page))
  =/  tip-id=block-id:t  ~(digest get:page:t tip-page)
  =/  blocks=(h-map block-id:t local-page:t)
    (~(put h-by *(h-map block-id:t local-page:t)) tip-id (to-local-page:page:t tip-page))
  =?  blocks  rooted
    (~(put h-by blocks) genesis-id (to-local-page:page:t genesis-page))
  =/  con=consensus-state
    %*  .  *consensus-state
      blocks          blocks
      heaviest-block  `tip-id
      genesis-seal    `[0 *hash:t]
      min-timestamps
        %-  ~(gas h-by *(h-map block-id:t @))
        :~  [genesis-id 0]
            [tip-id 0]
        ==
    ==
  =/  der=derived-state
    %*  .  *derived-state
      highest-block-height  `proof-version-3-start:dcon
      heaviest-chain
        %-  ~(gas z-by *(z-map page-number:t block-id:t))
        :~  [0 genesis-id]
            [proof-version-3-start:dcon tip-id]
        ==
    ==
  %+  reconstruct-asert-caches:dumb  %.y
  %*  .  *kernel-state
    c          con
    d          der
    constants  constants(check-pow-flag %.n)
  ==
::
++  test-load-prunes-stale-zoe-side-fork-without-reset
  =/  base=proof:t  *proof:t
  =/  v3=proof:t  [%3 objects.base hashes.base read-index.base]
  =/  stale-v2=proof:t  [%2 objects.base hashes.base read-index.base]
  =/  state=kernel-state  (zoe-load-state [%.y `v3])
  =/  tip-id=block-id:t  (need heaviest-block.c.state)
  =/  stale-page=page:v1:t
    %*  .  *page:v1:t
      height  proof-version-3-start:dcon
      pow     (some stale-v2)
    ==
  =.  stale-page  stale-page(digest (compute-digest:page:t stale-page))
  =/  stale-id=block-id:t  ~(digest get:page:t stale-page)
  =.  blocks.c.state
    (~(put h-by blocks.c.state) stale-id (to-local-page:page:t stale-page))
  =.  block-versions.c.state
    (~(put h-by block-versions.c.state) stale-id %2)
  =.  puzzle-asert-states.d.state
    (~(put h-by puzzle-asert-states.d.state) stale-id *puzzle-asert-state)
  =/  loaded=kernel-state  (load:inner:dumb state)
  %+  expect-eq
    !>([%.y %.y 2 %.n %.n %.n %.y])
  !>  :*  =(heaviest-block.c.state heaviest-block.c.loaded)
          (~(has h-by blocks.c.loaded) tip-id)
          ~(wyt h-by blocks.c.loaded)
          (~(has h-by blocks.c.loaded) stale-id)
          (~(has h-by block-versions.c.loaded) stale-id)
          (~(has h-by puzzle-asert-states.d.loaded) stale-id)
          =(genesis-seal.c.state genesis-seal.c.loaded)
      ==
::
++  test-load-cleans-deleted-block-metadata
  =/  base=proof:t  *proof:t
  =/  v3=proof:t  [%3 objects.base hashes.base read-index.base]
  =/  state=kernel-state  (zoe-load-state [%.y `v3])
  =/  tip-id=block-id:t  (need heaviest-block.c.state)
  =/  genesis-id=block-id:t  (~(got z-by heaviest-chain.d.state) 0)
  =.  block-versions.c.state
    (~(put h-by block-versions.c.state) tip-id %3)
  =.  puzzle-asert-states.d.state
    (~(put h-by puzzle-asert-states.d.state) tip-id [7 3 `tip-id `genesis-id])
  =/  expected=kernel-state  state
  ::  Seed metadata for an absent block; the block scan has no corresponding
  ::  orphan to discover.
  =/  deleted-id=block-id:t  *block-id:t
  ?<  (~(has h-by blocks.c.state) deleted-id)
  =.  block-versions.c.state
    (~(put h-by block-versions.c.state) deleted-id %4)
  =.  puzzle-asert-states.d.state
    (~(put h-by puzzle-asert-states.d.state) deleted-id [2 1 `deleted-id ~])
  =/  loaded=kernel-state  (load:inner:dumb state)
  ;:  weld
    (expect-eq !>(expected) !>(loaded))
    (expect-eq !>(loaded) !>((load:inner:dumb loaded)))
  ==
::
++  test-load-cleans-block-metadata-without-chain
  =/  expected=kernel-state  *kernel-state
  =/  state=kernel-state  expected
  =/  deleted-id=block-id:t  *block-id:t
  =.  block-versions.c.state
    (~(put h-by block-versions.c.state) deleted-id %4)
  =.  puzzle-asert-states.d.state
    (~(put h-by puzzle-asert-states.d.state) deleted-id [2 1 `deleted-id ~])
  =/  loaded=kernel-state  (load:inner:dumb state)
  ;:  weld
    (expect-eq !>(expected) !>(loaded))
    (expect-eq !>(loaded) !>((load:inner:dumb loaded)))
  ==
::
++  test-load-refuses-incomplete-ancestry-without-reset
  =/  base=proof:t  *proof:t
  =/  v3=proof:t  [%3 objects.base hashes.base read-index.base]
  =/  state=kernel-state  (zoe-load-state [%.n `v3])
  %+  expect-fail
    |.((load:inner:dumb state))
  `"preserving state and refusing to boot"
::
::
++  test-load-refuses-unsafe-version-10-migration-without-reset
  =/  legacy-page=page:v1:t
    %*  .  *page:v1:t
      height  proof-version-3-start:dcon
    ==
  =.  legacy-page  legacy-page(digest (compute-digest:page:t legacy-page))
  =/  legacy-id=block-id:t  ~(digest get:page:t legacy-page)
  =/  legacy=kernel-state-10
    %*  .  *kernel-state-10
      blocks.c
        (~(put h-by *(h-map block-id:t local-page:t)) legacy-id (to-local-page:page:t legacy-page))
      heaviest-block.c  `legacy-id
    ==
  %+  expect-fail
    |.((load:inner:dumb legacy))
  `"Version-10 state cannot be migrated safely"
::
++  test-load-refreshes-same-version-mining-candidate
  =/  con=consensus-state  initial-consensus-state:h
  =^  tip=page:t  con  (add-n-pages:h 2 con default-retain:h)
  =/  derived=derived-state  (update:dder con tip)
  =/  mining=mining-state  initial-mining-state:h
  =.  mining  (~(heard-new-block dmin mining derived constants) con *@da)
  =/  candidate=page:t  candidate-block.mining
  ::  A changed target with the same parent must not survive reload.
  ?^  -.candidate
    =.  candidate  candidate(target (chunk:bignum:t 1))
    =/  state=kernel-state
      %*  .  *kernel-state
        c  con
        d  derived
        m  mining(candidate-block candidate)
        constants  constants
      ==
    =/  loaded=kernel-state  (load:inner:dumb state)
    ;:  weld
      (expect-eq !>(%12) !>(-.loaded))
      (expect-eq !>(heaviest-block.c.state) !>(heaviest-block.c.loaded))
      (expect-eq !>(~(parent get:page:t candidate)) !>(~(parent get:page:t candidate-block.m.loaded)))
      (expect-eq !>(~(target get:page:t candidate-block.mining)) !>(~(target get:page:t candidate-block.m.loaded)))
      (expect-eq !>(mining.mining) !>(mining.m.loaded))
    ==
  ~|  %candidate-fixture-must-be-v0  !!
::
++  accepted-load-state
  |=  [height=@ wide-target=? stamp=@]
  ^-  kernel-state
  =/  bc  constants
  =?  max-target-atom.bc  wide-target  (bex 384)
  =/  base=proof:t  *proof:t
  =/  state=kernel-state
    (zoe-load-state [%.y `[%3 objects.base hashes.base read-index.base]])
  =/  genesis-id=block-id:t  (~(got z-by heaviest-chain.d.state) 0)
  =/  genesis=local-page:t  (~(got h-by blocks.c.state) genesis-id)
  =.  genesis-seal.c.state
    `[0 (hash:page-msg:t ~(msg get:local-page:t genesis))]
  =.  constants.state  bc
  =.  mining.m.state  %.n
  =/  zoe-id=block-id:t  (need heaviest-block.c.state)
  =/  parent=page:v1:t
    %*  .  *page:v1:t
      height     height
      parent     zoe-id
      timestamp  stamp
      pow        `[%5 objects.base hashes.base read-index.base]
    ==
  =.  parent  parent(digest (compute-digest:page:t parent))
  =/  pid=block-id:t  digest.parent
  =.  blocks.c.state
    (~(put h-by blocks.c.state) pid (to-local-page:page:t parent))
  =.  min-timestamps.c.state
    (~(put h-by min-timestamps.c.state) pid stamp)
  =.  heaviest-block.c.state  `pid
  =.  heaviest-chain.d.state
    (~(put z-by heaviest-chain.d.state) height pid)
  =.  highest-block-height.d.state  `height
  =.  puzzle-asert-states.d.state
    (~(put h-by puzzle-asert-states.d.state) pid ?:(=(height 154.499) [91 73 `pid `pid] [1 0 `pid ~]))
  =.  c.state
    (~(update-asert-anchor-min-timestamps dcon c.state d.state bc) %zk parent)
  =.  c.state
    (~(update-asert-anchor-min-timestamps dcon c.state d.state bc) %ai parent)
  state
:::
++  hardened-load-extend
  |=  [state=kernel-state ai=?]
  ^-  kernel-state
  =/  bc  constants.state
  =/  ht  ~(. helpers bc)
  =/  mt  ~(. tx-engine bc)
  =/  parent=page:t
    (to-page:local-page:t (~(got h-by blocks.c.state) (need heaviest-block.c.state)))
  =/  pag=page:t
    ?:  ai  (make-ai-pow-page:ht parent c.state d.state)
    =/  candidate=page:t  (make-empty-page:ht parent)
    ?>  ?=(@ -.candidate)
    =/  target
      (~(compute-target-zk-asert dcon c.state d.state bc) height.candidate parent.candidate)
    =/  base=proof:t  *proof:t
    =.  candidate  candidate(target target, pow `[%5 objects.base hashes.base read-index.base])
    =.  accumulated-work.candidate
      %-  chunk:bignum:t
      %+  add  (merge:bignum:t ~(accumulated-work get:page:t parent))
      (merge:bignum:t (~(block-compute-work dcon c.state d.state bc) candidate))
    candidate(digest (compute-digest:page:mt candidate))
  =/  valid
    (~(validate-page-without-txs dcon c.state d.state bc) pag ~(timestamp get:page:t pag))
  ~|  [%synthetic-header-rejected valid]
  ?>  -.valid
  =.  c.state  (~(accept-block dcon c.state d.state bc) pag)
  =/  bid=block-id:t  ~(digest get:page:t pag)
  ::  Controlled branch-local MTP, as in the existing ASERT fixtures. This
  ::  isolates reconstruction from the separate median-of-eleven algorithm.
  =.  min-timestamps.c.state
    (~(put h-by min-timestamps.c.state) bid ~(timestamp get:page:t pag))
  =.  c.state
    (~(update-asert-anchor-min-timestamps dcon c.state d.state bc) %zk pag)
  =.  c.state
    (~(update-asert-anchor-min-timestamps dcon c.state d.state bc) %ai pag)
  =.  heaviest-block.c.state  `bid
  =.  d.state  (~(update dder d.state bc) c.state pag)
  state
:::
++  stale-hardened-load-state
  |=  state=kernel-state
  ^-  kernel-state
  =.  asert-anchor-min-timestamps.c.state
    %-  ~(run by asert-anchor-min-timestamps.c.state)
    |=  timestamps=(h-map block-id:t @)
    %-  ~(rep h-by timestamps)
    |=  [[bid=block-id:t timestamp=@] updated=_timestamps]
    =/  pag=local-page:t  (~(got h-by blocks.c.state) bid)
    %+  ~(put h-by updated)  bid
    ?:  (lth ~(height get:local-page:t pag) (dec harden-phase:page:t))
      timestamp
    777
  =.  puzzle-asert-states.d.state
    %-  ~(rep h-by blocks.c.state)
    |=  [[bid=block-id:t pag=local-page:t] cache=(h-map block-id:t puzzle-asert-state)]
    ?:  (lth ~(height get:local-page:t pag) 154.500)
      =/  prior=(unit puzzle-asert-state)
        (~(get h-by puzzle-asert-states.d.state) bid)
      ?~  prior  cache
      (~(put h-by cache) bid u.prior)
    (~(put h-by cache) bid [999 999 `bid `bid])
  state
:::
++  hardened-next-targets
  |=  state=kernel-state
  =/  pid=block-id:t  (need heaviest-block.c.state)
  =/  height=@  +(~(height get:local-page:t (~(got h-by blocks.c.state) pid)))
  =/  con  ~(. dcon c.state d.state constants.state)
  [(compute-target-zk-asert:con height pid) (compute-target-ai-asert:con height pid)]
:::
++  check-hardened-load-continuation
  |=  expected=kernel-state
  ^-  tang
  =/  loaded=kernel-state
    (load:inner:dumb (stale-hardened-load-state expected))
  =/  repeated=kernel-state  (load:inner:dumb loaded)
  =/  continued=kernel-state  (hardened-load-extend expected %.y)
  =/  resumed=kernel-state  (hardened-load-extend repeated %.y)
  ;:  weld
    (expect-eq !>(blocks.c.expected) !>(blocks.c.loaded))
    (expect-eq !>(heaviest-chain.d.expected) !>(heaviest-chain.d.loaded))
    (expect-eq !>(asert-anchor-min-timestamps.c.expected) !>(asert-anchor-min-timestamps.c.loaded))
    (expect-eq !>((hardened-next-targets expected)) !>((hardened-next-targets loaded)))
    (expect-eq !>(puzzle-asert-states.d.expected) !>(puzzle-asert-states.d.loaded))
    (expect-eq !>(loaded) !>(repeated))
    (expect-eq !>(heaviest-block.c.continued) !>(heaviest-block.c.resumed))
    (expect-eq !>((hardened-next-targets continued)) !>((hardened-next-targets resumed)))
    (expect-eq !>(puzzle-asert-states.d.continued) !>(puzzle-asert-states.d.resumed))
  ==
:::
++  test-load-hardening-predecessor-only
  (check-hardened-load-continuation (accepted-load-state 154.499 %.n 123.456))
:::
++  test-load-hardening-activation
  =/  state=kernel-state  (accepted-load-state 154.499 %.n 123.456)
  (check-hardened-load-continuation (hardened-load-extend state %.y))
:::
++  test-load-hardening-reconstructs-asert-caches-before-children
  =/  state=kernel-state  (accepted-load-state 154.499 %.n 123.456)
  =.  state  (hardened-load-extend state %.y)
  (check-hardened-load-continuation (hardened-load-extend state %.y))
:::
++  test-load-hardening-mixed-branches
  ::  Opposite lane orders and different predecessor timestamps must not share
  ::  counters/heads/anchors. +load deliberately prunes the unselected branch.
  =/  a=kernel-state  (accepted-load-state 154.499 %.y 123.456)
  =/  b=kernel-state  (accepted-load-state 154.499 %.y 223.456)
  =.  a  (hardened-load-extend (hardened-load-extend a %.n) %.y)
  =.  b  (hardened-load-extend (hardened-load-extend b %.y) %.n)
  =/  both=kernel-state  a
  =.  blocks.c.both  (~(uni h-by blocks.c.a) blocks.c.b)
  =.  block-versions.c.both  (~(uni h-by block-versions.c.a) block-versions.c.b)
  =.  min-timestamps.c.both  (~(uni h-by min-timestamps.c.a) min-timestamps.c.b)
  =.  puzzle-asert-states.d.both
    (~(uni h-by puzzle-asert-states.d.a) puzzle-asert-states.d.b)
  =/  loaded-a=kernel-state
    (load:inner:dumb (stale-hardened-load-state both))
  =.  heaviest-block.c.both  heaviest-block.c.b
  =.  heaviest-chain.d.both  heaviest-chain.d.b
  =/  loaded-b=kernel-state
    (load:inner:dumb (stale-hardened-load-state both))
  ;:  weld
    (check-hardened-load-continuation a)
    (check-hardened-load-continuation b)
    (expect-eq !>(blocks.c.a) !>(blocks.c.loaded-a))
    (expect-eq !>(blocks.c.b) !>(blocks.c.loaded-b))
    (expect-eq !>(puzzle-asert-states.d.a) !>(puzzle-asert-states.d.loaded-a))
    (expect-eq !>(puzzle-asert-states.d.b) !>(puzzle-asert-states.d.loaded-b))
    (expect-eq !>((hardened-next-targets a)) !>((hardened-next-targets loaded-a)))
    (expect-eq !>((hardened-next-targets b)) !>((hardened-next-targets loaded-b)))
  ==
:::
++  check-accepted-load-preservation
  |=  state=kernel-state
  ^-  tang
  =.  init.a.state  %.n
  =.  state  (hardened-load-extend state %.y)
  =/  old-id=block-id:t  (need heaviest-block.c.state)
  =/  pag=page:t  (to-page:local-page:t (~(got h-by blocks.c.state) old-id))
  ?>  ?=(@ -.pag)
  ::  Accepted pages retain their header and artifact without new admission.
  =.  pag  pag(target (chunk:bignum:t 1))
  =.  pag  pag(digest (compute-digest:page:t pag))
  =.  blocks.c.state
    (~(put h-by (~(del h-by blocks.c.state) old-id)) digest.pag (to-local-page:page:t pag))
  =.  min-timestamps.c.state
    (~(put h-by (~(del h-by min-timestamps.c.state) old-id)) digest.pag timestamp.pag)
  =.  heaviest-block.c.state  `digest.pag
  =.  heaviest-chain.d.state
    (~(put z-by heaviest-chain.d.state) height.pag digest.pag)
  =/  loaded=kernel-state  (load:inner:dumb state)
  =/  next=kernel-state  (hardened-load-extend loaded %.y)
  =/  fresh=page:t
    (to-page:local-page:t (~(got h-by blocks.c.next) (need heaviest-block.c.next)))
  ?<  (~(has h-by blocks.c.loaded) ~(digest get:page:t fresh))
  =/  submission=[wir=path eny=@ our=@ux now=@da dat=*]
    [libp2p-gossip-wire:h 0 0x0 ~2026.1.1 [%fact %0 %heard-block fresh]]
  =/  rejected=kernel-state  loaded
  =^  effs=(list effect)  rejected
    (~(poke inner:dumb loaded) submission)
  =/  proof-rejected=?
    %+  lien  effs
    |=  eff=effect
    ?&  ?=([%liar-block-id *] eff)
        =(~(digest get:page:t fresh) p.eff)
        =(%failed-pow-check cause.eff)
    ==
  ;:  weld
    (expect-eq !>(blocks.c.state) !>(blocks.c.loaded))
    (expect-eq !>(heaviest-chain.d.state) !>(heaviest-chain.d.loaded))
    (expect-eq !>(heaviest-block.c.state) !>(heaviest-block.c.loaded))
    (expect-eq !>(%.y) !>(check-pow-flag.constants.loaded))
    (expect-eq !>(loaded) !>((load:inner:dumb loaded)))
    (expect-eq !>(%.y) !>(proof-rejected))
    (expect-eq !>(blocks.c.loaded) !>(blocks.c.rejected))
    (expect-eq !>(heaviest-block.c.loaded) !>(heaviest-block.c.rejected))
    (expect-eq !>(%.y) !>(check-pow-flag.constants.rejected))
  ==
::::
++  test-load-preserves-accepted-hardening-pages-with-proof-checking
  (check-accepted-load-preservation (accepted-load-state 154.499 %.n 123.456))
::::
++  consensus-batch-load-state
  |=  [wide-target=? stamp=@]
  ^-  kernel-state
  (accepted-load-state 169.999 wide-target stamp)
::::
++  test-load-consensus-batch-predecessor-only
  (check-hardened-load-continuation (consensus-batch-load-state %.n 123.456))
::::
++  test-load-consensus-batch-activation
  =/  state=kernel-state  (consensus-batch-load-state %.n 123.456)
  (check-hardened-load-continuation (hardened-load-extend state %.y))
::::
++  test-load-consensus-batch-reconstructs-asert-caches-before-children
  =/  state=kernel-state  (consensus-batch-load-state %.n 123.456)
  =.  state  (hardened-load-extend state %.y)
  (check-hardened-load-continuation (hardened-load-extend state %.y))
::::
++  test-load-consensus-batch-mixed-branches
  ::  Both fork selections reconstruct their own timestamps and lane heads.
  ::  The existing load policy discards the unselected fork.
  =/  a=kernel-state  (consensus-batch-load-state %.y 123.456)
  =/  b=kernel-state  (consensus-batch-load-state %.y 223.456)
  =.  a  (hardened-load-extend (hardened-load-extend a %.n) %.y)
  =.  b  (hardened-load-extend (hardened-load-extend b %.y) %.n)
  =/  both=kernel-state  a
  =.  blocks.c.both  (~(uni h-by blocks.c.a) blocks.c.b)
  =.  block-versions.c.both  (~(uni h-by block-versions.c.a) block-versions.c.b)
  =.  min-timestamps.c.both  (~(uni h-by min-timestamps.c.a) min-timestamps.c.b)
  =.  puzzle-asert-states.d.both
    (~(uni h-by puzzle-asert-states.d.a) puzzle-asert-states.d.b)
  =/  loaded-a=kernel-state
    (load:inner:dumb (stale-hardened-load-state both))
  =.  heaviest-block.c.both  heaviest-block.c.b
  =.  heaviest-chain.d.both  heaviest-chain.d.b
  =/  loaded-b=kernel-state
    (load:inner:dumb (stale-hardened-load-state both))
  ;:  weld
    (check-hardened-load-continuation a)
    (check-hardened-load-continuation b)
    (expect-eq !>(blocks.c.a) !>(blocks.c.loaded-a))
    (expect-eq !>(blocks.c.b) !>(blocks.c.loaded-b))
    (expect-eq !>(puzzle-asert-states.d.a) !>(puzzle-asert-states.d.loaded-a))
    (expect-eq !>(puzzle-asert-states.d.b) !>(puzzle-asert-states.d.loaded-b))
    (expect-eq !>((hardened-next-targets a)) !>((hardened-next-targets loaded-a)))
    (expect-eq !>((hardened-next-targets b)) !>((hardened-next-targets loaded-b)))
  ==
::::
++  test-load-preserves-accepted-consensus-batch-pages-with-proof-checking
  (check-accepted-load-preservation (consensus-batch-load-state %.n 123.456))
:::
++  test-load-discards-pending-preserves-transactions-repeatedly
  =/  con=consensus-state  initial-consensus-state:h
  =^  first=page:t  con  (add-n-pages:h 1 con default-retain:h)
  =^  tip=page:t  con  (add-n-pages:h 1 con default-retain:h)
  =/  retained=raw-tx:t  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h first)
  =/  absent=raw-tx:t  (make-raw-tx-from-coinbase:v0:h p:default-keys-3:h tip)
  =/  kept=tx-id:t  ~(id get:raw-tx:t retained)
  =/  missing=tx-id:t  ~(id get:raw-tx:t absent)
  =^  ready  con  (~(add-raw-tx dcon con der constants) retained)
  =/  pending=page:t  (make-page-with-txs:v0:h tip ~[kept missing])
  =/  state=kernel-state
    %*  .  *kernel-state
      c  con
      d  (update:dder con tip)
      constants  constants
      mining.m  %.n
    ==
  =/  expected=kernel-state  (load:inner:dumb state)
  =|  count=@
  =|  results=tang
  |-
  ?:  =(count 2)  results
  =^  needed  c.state
    (~(add-pending-block dcon c.state d.state constants) pending)
  ?>  =(~[missing] needed)
  ?>  (~(has h-by blocks-needed-by.c.state) kept)
  =.  state  (load:inner:dumb state)
  =.  results
    ;:  weld
      results
      (expect-eq !>(c.expected) !>(c.state))
      (expect-eq !>(d.expected) !>(d.state))
      (expect-eq !>(%.y) !>((~(has h-in excluded-txs.c.state) kept)))
      (expect-eq !>(%.n) !>((~(has h-by blocks-needed-by.c.state) missing)))
      (expect-eq !>(retained) !>(raw-tx:(~(got h-by raw-txs.c.state) kept)))
      (expect-eq !>(state) !>((load:inner:dumb state)))
    ==
  $(count +(count))
::
++  test-garbage-collect-after-genesis
  =/  con=consensus-state  initial-consensus-state:h
  =.  con  (~(garbage-collect dcon con der constants) default-retain:h)
  =/  balance=(h-map nname:t nnote:t)
    ~(get-cur-balance dcon con der constants)
  ;:  weld
    %+  expect-eq
      !>  default-genesis-id:h
    !>  (need heaviest-block.con)
    %+  expect-eq
      !>  0
    !>  ~(wyt h-by balance)
  ==
::
++  test-get-elders-genesis
  =/  con=consensus-state  initial-consensus-state:h
  ::  add a few more pages to test stopping at genesis
  =^  genesis-test-page=page:t  con  (add-n-pages:h 2 con default-retain:h)
  ::  get ancestors of page 2, should only return 3 blocks including genesis
  =/  der=derived-state  (update:dder con genesis-test-page)
  =/  genesis-ancestors=(unit [page-number:t (list block-id:t)])
    %+  ~(get-elders dcon con der constants)
      der
    ~(digest get:page:t genesis-test-page)
  ?~  genesis-ancestors
    !!
  ~&  >  "genesis ancestors: {<genesis-ancestors>}"
  ::
  %+  weld
  %+  expect-eq
    !>  %.y
  =+  height=-:u.genesis-ancestors
  !>  =(height 0)
  ::
  %+  expect-eq
    !>  %.y
  !>  =(3 (lent +:u.genesis-ancestors))
::
++  test-get-elders
  =/  con=consensus-state  initial-consensus-state:h
  ::  add 30 pages to test the 24 block limit
  =^  last-page=page:t  con  (add-n-pages:h 29 con default-retain:h)
  =/  der=derived-state  (update:dder con last-page)
  ::  get ancestors of last page
  ~&  >  "getting ancestors of last page: {<~(digest get:page:t last-page)>}"
  =/  ancestors=(unit [page-number:t (list block-id:t)])
    %+  ~(get-elders dcon con der constants)
      der
    ~(digest get:page:t last-page)
  ?~  ancestors
    !!
  ::  check length is 24 (truncated from 30)
  =/  len-check=?  =(24 (lent +:u.ancestors))
  ::  check heights are sequential and descending
  =/  [height=page-number:t bids=(list block-id:t)]
    u.ancestors
  =/  height-check=?
    =(height 6)
  ::  check block-ids match what is in consensus state
  =/  id-check=?
    =/  match=?  %.y
    =+  bids=(flop bids)
    |-
    ?~  bids
      match
    =/  pag=page:t  (to-page:local-page:t (~(got h-by blocks.con) i.bids))
    $(height +(height), bids t.bids, match &(match =(~(height get:page:t pag) height)))
  %+  expect-eq
    !>  [%.y %.y %.y]
  !>  [len-check height-check id-check]
:::
++  test-state-12-loads-frozen-pre-ai-state
  =/  legacy=kernel-state-10  *kernel-state-10
  =/  loaded=kernel-state  (load:inner:dumb legacy)
  %+  expect-eq
    !>  [%12 %.y 0]
    !>  :*  -.loaded
            ?=(~ heaviest-block.c.loaded)
            ~(wyt h-by blocks.c.loaded)
        ==
::
++  test-state-12-loads-frozen-zoe-state
  =/  legacy=kernel-state-11  *kernel-state-11
  =/  loaded=kernel-state  (load:inner:dumb legacy)
  %+  expect-eq
    !>  [%12 %.y 0]
    !>  :*  -.loaded
            ?=(~ heaviest-block.c.loaded)
            ~(wyt h-by blocks.c.loaded)
        ==
++  test-anthropos-zk-v5-activation-preserves-ai-v4-discriminator
  =/  con=consensus-state  *consensus-state
  =/  v5-start=page-number:t  zk-pow-v5-phase:page:t
  %+  expect-eq
    !>([147.500 %3 %5 %5 %ai-pow %dumb-zkpow %.y %.n %.n %.y %.y %.y])
  !>  :*  v5-start
          (height-to-proof-version-legacy:dcon (dec v5-start))
          (height-to-proof-version-legacy:dcon v5-start)
          (height-to-proof-version-legacy:dcon +(v5-start))
          (version-to-puzzle-type:dcon %4)
          (version-to-puzzle-type:dcon %5)
          (~(proof-version-valid-at-height dcon con der constants) %3 (dec v5-start))
          (~(proof-version-valid-at-height dcon con der constants) %5 (dec v5-start))
          (~(proof-version-valid-at-height dcon con der constants) %3 v5-start)
          (~(proof-version-valid-at-height dcon con der constants) %5 v5-start)
          (~(proof-version-valid-at-height dcon con der constants) %4 (dec v5-start))
          (~(proof-version-valid-at-height dcon con der constants) %4 v5-start)
      ==
::
--
