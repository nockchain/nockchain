/=  helpers  /tests/dumb/helpers
/=  txe  /common/tx-engine
/=  zoon  /common/zoon
/=  *  /common/test
|%
++  h  ~(. helpers bc-pending-integration-tests:helpers)
++  t  ~(. txe bc-pending-integration-tests:helpers)
+$  heavy-tx  [=tx-id:t =raw-tx:t]
+$  heavy-txs
  $:  =page-number:t
      =block-id:t
      =page:t
      txs=(list heavy-tx)
  ==
++  heavy-tx-present
  |=  [id=tx-id:t raw=raw-tx:t txs=(list heavy-tx)]
  ^-  ?
  ?~  txs
    %.n
  ?:  ?&  =(id tx-id.i.txs)
          =(raw raw-tx.i.txs)
      ==
    %.y
  $(txs t.txs)
::
++  test-bnb-excluded-mutually-exclusive
  =+  [nockchain genesis]=init-nockchain:h
  ::
  ::  add 1 block following genesis
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 1 nockchain)
  ::
  ::  make tx that spends coinbase from block 1
  =/  raw1  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h (snag 0 pages))
  ::
  ::
  ::  hear the tx
  =^  effs=(list effect:h)  nockchain
    (pok:h [%fact %0 %heard-tx raw1] nockchain)
  ::  check that invariant holds:
  ::  tx is exclusively in excluded and in raw-tx
  ?>  (~(check-excluded k-by:h nockchain) id.raw1)
  ::
  =/  block2  (make-page-with-txs:v0:h (snag 0 pages) ~[id.raw1])
  ::
  ::  hear block 2
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) block2)
  ::  check that invariant holds:
  ::  tx is exclusively in bnb and contains block digest
  ?>  (~(check-bnb k-by:h nockchain) id.raw1 ~(digest get:page:t block2))
  ~
  ::
  ::  tests that pending blocks with txs flushed by garbage collection
  ::  are not marked as ready
  ++  test-block-not-ready-when-txs-flushed
    =+  [nockchain genesis]=init-nockchain:h
    ::
    ::  add block 1
    =^  pages  nockchain
      (add-n-pages-integration:h genesis 1 nockchain)
    ::
    ::  make tx that spends coinbase from block 1
    =/  raw1  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h (snag 0 pages))
    ::
    ::  hear the tx - it should go into excluded set
    =^  effs=(list effect:h)  nockchain
      (~(heard-tx k-by:h nockchain) raw1)
    ?>  (~(check-excluded k-by:h nockchain) id.raw1)
    ::
    ::  add 20 blocks, making the new heaviest block 21
    ::  and triggering the tx retention policy
    =^  pages  nockchain
      (add-n-pages-integration:h (snag 0 pages) 20 nockchain)
    ?>  ?&  =(21 ~(heaviest-chain-height k-by:h nockchain))
            !(~(has-excluded k-by:h nockchain) id.raw1)
        ==
    ::
    ::  If we hear a block containing the tx...
    =/  block-22  (make-page-with-txs:v0:h (snag 19 pages) ~[id.raw1])
    =^  effs=(list effect:h)  nockchain
      (~(heard-block k-by:h nockchain) block-22)
    ::  raw-tx should be exclusively in blocks needed by
    ?>  (~(check-bnb k-by:h nockchain) id.raw1 ~(digest get:page:t block-22))
    ::  block should be pending because tx was garbage collected.
    ?>  (~(has-pending-block k-by:h nockchain) ~(digest get:page:t block-22))
    ::  block should have been requested
    ?>  (~(has z-in:zoon (filter-request-tx-effects:h effs)) id.raw1)
    ~
  ::
  ::  Test that double spend detection works via spent-by
  ++  test-spent-by-reject-double-spend
  =+  [nockchain genesis]=init-nockchain:h
  ::
  ::  add 1 block following genesis
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 1 nockchain)
  ::
  ::  make tx that spends from that coinbase
  =/  coinbase1  (new:v0:coinbase:t (snag 0 pages) p:default-keys-1:h)
  ?>  ?=(^ -.coinbase1)
  =/  raw1  (simple-from-note:new:raw-tx:v0:t p:default-keys-2:h coinbase1 s:default-keys-1:h)
  ::
  ::  hear the tx
  =^  effs=(list effect:h)  nockchain
    (~(heard-tx k-by:h nockchain) raw1)
  ::
  ::  assert that tx is in spent-by
  ?>  (~(has-spent-by k-by:h nockchain) ~(name get:nnote:t coinbase1) id.raw1)
  ::
  ::  check that invariant holds:
  ::  tx is exclusively in excluded and in raw-tx
  ?>  (~(check-excluded k-by:h nockchain) id.raw1)
  ::
  ::  create a new tx from the same coinbase, to a different recipient
  =/  raw2  (simple-from-note:new:raw-tx:v0:t p:default-keys-3:h coinbase1 s:default-keys-1:h)
  ::  hear the second tx
  =^  effs=(list effect:h)  nockchain
    (~(heard-tx k-by:h nockchain) raw2)
  ::  assert that original tx is still in spent-by
  ::  and raw2 was not accepted
  ?>  ?&  (~(has-spent-by k-by:h nockchain) ~(name get:nnote:t coinbase1) id.raw1)
          !(~(has-spent-by k-by:h nockchain) ~(name get:nnote:t coinbase1) id.raw2)
          (~(check-excluded k-by:h nockchain) id.raw1)
          !(~(check-excluded k-by:h nockchain) id.raw2)
      ==
  ~
::
::  Test how pending txs get handled when there's a fork
::
::  We build this chain:
::        G --> 1 --> 2 --> 3
::                    └--> 3'(tx1) --> 4'(tx2)
::
::  tx1 and tx2 spend the coinbases from blocks 1 and 2. We submit them
::  after block 2, so both should sit in the raw-tx map. When 3' comes in,
::  nothing changes heaviness-wise but both txs get regossiped. When 4'
::  shows up, it becomes the heaviest block and tx1/tx2 get removed from
::  raw-txs since they're now spent.
::
++  test-pending-txs-reorg-1
  =+  [nockchain genesis]=init-nockchain:h
  ::
  ::  build up to block 2
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 2 nockchain)
  ::
  ::  make txs that spend those coinbases
  =/  raw1  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h (snag 0 pages))
  =/  raw2  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h (snag 1 pages))
  ::
  ::  hear the txs - they should go into pending since they're valid
  ::  but not in any block yet
  =^  effs=(list effect:h)  nockchain
    (~(heard-tx k-by:h nockchain) raw1)
  ?>  (~(check-excluded k-by:h nockchain) id.raw1)
  =^  effs=(list effect:h)  nockchain
    (~(heard-tx k-by:h nockchain) raw2)
  ?>  (~(check-excluded k-by:h nockchain) id.raw2)
  ::
  ::  add block 3 (empty)
  =/  block-3  (make-empty-page:h (snag 1 pages))
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) block-3)
  ::
  ::  block 3 should be heaviest now
  ?>  =(~(digest get:page:t block-3) ~(heaviest-block k-by:h nockchain))
  ::  txs should get regossiped since they're still valid
  ::
  =/  regossiped-raw=(z-set:zoon raw-tx:t)  (filter-heard-tx-effects:h effs)
  ?>  ?&  (~(has z-in:zoon regossiped-raw) raw1)
          (~(has z-in:zoon regossiped-raw) raw2)
      ==
  ::
  ::  now add block 3' with tx1 (fork starts here)
  =/  block-3-p  (make-page-with-txs:v0:h (snag 1 pages) ~[id.raw1])
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) block-3-p)
  ::
  ::  heaviest shouldn't change - both forks have same weight
  ?>  =(~(digest get:page:t block-3) ~(heaviest-block k-by:h nockchain))
  ::
  ::  add block 4' with tx2 - this makes the fork heavier
  =/  block-4-p  (make-page-with-txs:v0:h block-3-p ~[id.raw2])
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) block-4-p)
  ::  now the fork should be heaviest
  ?>  =(~(digest get:page:t block-4-p) ~(heaviest-block k-by:h nockchain))
  ::
  ::  txs should be gone from excluded since they're spent on the heaviest chain
  ?>  ?&  !(~(has-excluded k-by:h nockchain) id.raw1)
          !(~(has-excluded k-by:h nockchain) id.raw2)
      ==
  ~
  ::
  ++  test-heavy-txs-peek-bundles-accepted-block
    =+  [nockchain genesis]=init-nockchain:h
    ::
    =^  pages  nockchain
      (add-n-pages-integration:h genesis 3 nockchain)
    ::
    =/  raw1  (make-raw-tx-from-coinbase:v0:h p:default-keys-3:h (snag 0 pages))
    =/  raw2  (make-raw-tx-from-coinbase:v0:h p:default-keys-3:h (snag 1 pages))
    =/  raw3  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h (snag 2 pages))
    =^  effs  nockchain
      (~(heard-txs k-by:h nockchain) ~[raw1 raw2 raw3])
    ::
    =/  block-4  (make-empty-page:h (snag 2 pages))
    =/  block-5  (make-page-with-txs:v0:h block-4 ~[id.raw1 id.raw2])
    =/  block-6  (make-empty-page:h block-5)
    =^  effs  nockchain
      (~(heard-blocks k-by:h nockchain) ~[block-4 block-5 block-6])
    ?>  =(~(digest get:page:t block-6) ~(heaviest-block k-by:h nockchain))
    ::
    =/  absent=(unit (unit *))  (peek:nockchain [%heavy-txs `@ta`99 ~])
    ?~  absent  !!
    ?>  ?=(~ u.absent)
    ::
    =/  peeked=(unit (unit *))  (peek:nockchain [%heavy-txs `@ta`5 ~])
    ?~  peeked  !!
    ?~  u.peeked  !!
    =/  got=heavy-txs  ;;(heavy-txs u.u.peeked)
    ?>  =(5 page-number.got)
    ?>  =(~(digest get:page:t block-5) block-id.got)
    ?>  =(block-5 page.got)
    ?>  =(2 (lent txs.got))
    ?>  (heavy-tx-present id.raw1 raw1 txs.got)
    ?>  (heavy-tx-present id.raw2 raw2 txs.got)
    ?>  !(heavy-tx-present id.raw3 raw3 txs.got)
    ~
    ::
  ::  Tests which txs a fork leaves claimed by blocks-needed-by, and which it
  ::  hands back to the mempool. A tx stays claimed only while a block on the
  ::  WINNING chain still carries it; a tx left behind on the abandoned branch
  ::  goes back to excluded-txs, where it can be re-gossiped and mined again.
  ::  (This test previously required EVERY tx to stay in blocks-needed-by after
  ::  the fork -- see the note at the post-reorg assertions: that expectation
  ::  was the stranded-tx bug, and +release-orphaned-branch corrects it.)
  ::
  ::  We build this chain:
  ::        G --> 1 --> 2 --> 3 --> 4 --> 5(tx1, tx2) --> 6
  ::                          └-->  4'(tx3) --> 5' --> 6'(tx2) --> 7'
  ++  test-pending-txs-reorg-2
    =+  [nockchain genesis]=init-nockchain:h
    ::
    ::  add 3 blocks
    =^  pages  nockchain
      (add-n-pages-integration:h genesis 3 nockchain)
    ::
    ::  create transactions that spend from the coinbase
    =/  raw1  (make-raw-tx-from-coinbase:v0:h p:default-keys-3:h (snag 0 pages))
    =/  raw2  (make-raw-tx-from-coinbase:v0:h p:default-keys-3:h (snag 1 pages))
    =/  raw3  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h (snag 2 pages))
    ::  hear transactions
    =^  effs  nockchain
      (~(heard-txs k-by:h nockchain) ~[raw1 raw2 raw3])
    ::
    ::  all txs should be in excluded because they are not attached to a block
    ?>  (~(check-excluded k-by:h nockchain) id.raw1)
    ?>  (~(check-excluded k-by:h nockchain) id.raw2)
    ?>  (~(check-excluded k-by:h nockchain) id.raw3)
    ::
    =/  block-4  (make-empty-page:h (snag 2 pages))
    =/  block-5  (make-page-with-txs:v0:h block-4 ~[id.raw1 id.raw2])
    =/  block-6  (make-empty-page:h block-5)
    ::
    ::  hear 3 more blocks
    =^  effs  nockchain
      (~(heard-blocks k-by:h nockchain) ~[block-4 block-5 block-6])
    ?>  =(~(digest get:page:t block-6) ~(heaviest-block k-by:h nockchain))
    ?>  (~(check-bnb k-by:h nockchain) id.raw1 ~(digest get:page:t block-5))
    ?>  (~(check-bnb k-by:h nockchain) id.raw2 ~(digest get:page:t block-5))
    ?>  (~(check-excluded k-by:h nockchain) id.raw3)
    ?>  (~(has-spent-by-set k-by:h nockchain) ~(input-names get:raw-tx:t raw1) id.raw1)
    ?>  (~(has-spent-by-set k-by:h nockchain) ~(input-names get:raw-tx:t raw2) id.raw2)
    ?>  (~(has-spent-by-set k-by:h nockchain) ~(input-names get:raw-tx:t raw3) id.raw3)
    =/  block-4-p  (make-page-with-txs:v0:h (snag 2 pages) ~[id.raw3])
    =/  block-5-p  (make-empty-page:h block-4-p)
    =/  block-6-p  (make-page-with-txs:v0:h block-5-p ~[id.raw2])
    =/  block-7-p  (make-empty-page:h block-6-p)
    ::
    ::  hear re-org
    =^  effs  nockchain
      (~(heard-blocks k-by:h nockchain) ~[block-4-p block-5-p block-6-p block-7-p])
    ::  confirm re-org was successful
    ?>  =(~(digest get:page:t block-7-p) ~(heaviest-block k-by:h nockchain))
    ::
    ::  A tx is claimed by blocks-needed-by (and so kept OUT of the mempool)
    ::  only while a block that still counts actually carries it. The reorg
    ::  above abandoned blocks 4, 5 and 6, so which txs are still claimed
    ::  depends on whether the WINNING chain carries them:
    ::
    ::    raw1  only ever in block 5, which is now orphaned. The winning chain
    ::          never spends its note (it spends page-1's via raw2 and page-2's
    ::          via raw3), so raw1 is a perfectly good UNMINED tx and belongs
    ::          back in the mempool, to be re-gossiped and mined again.
    ::
    ::          This assertion used to read `check-bnb raw1 block-5` -- i.e. it
    ::          required raw1 to stay claimed by the orphaned block forever.
    ::          That WAS the bug: +accept-block claimed it, nothing ever
    ::          released an accepted block's claim, so raw1 could never be
    ::          mined (the miner only sees excluded-txs plus pending-block
    ::          txs), never be re-gossiped, and never be dropped -- while its
    ::          input stayed pinned in spent-by, blocking any replacement tx
    ::          spending the same note. +release-orphaned-branch fixes that, so
    ::          the expectation flips: raw1 is back in the mempool.
    ::
    ::    raw2  in orphaned block 5 AND in canonical block 6'. Block 5's claim
    ::          is released, but 6' still carries it, so it stays claimed and
    ::          out of the mempool: it really is mined.
    ::
    ::    raw3  in canonical block 4'. Untouched by the reorg.
    ?>  (~(check-excluded k-by:h nockchain) id.raw1)
    ?>  !(~(has-bnb-block-id k-by:h nockchain) id.raw1 ~(digest get:page:t block-5))
    ?>  (~(check-bnb k-by:h nockchain) id.raw2 ~(digest get:page:t block-6-p))
    ?>  (~(check-bnb k-by:h nockchain) id.raw3 ~(digest get:page:t block-4-p))
    ::  all three are still held in raw-txs, so their notes stay reserved in
    ::  spent-by. (raw1's reservation is released only if it later ages out of
    ::  the mempool unmined, which is +drop-tx's job, not the reorg's.)
    ?>  (~(has-spent-by-set k-by:h nockchain) ~(input-names get:raw-tx:t raw1) id.raw1)
    ?>  (~(has-spent-by-set k-by:h nockchain) ~(input-names get:raw-tx:t raw2) id.raw2)
    ?>  (~(has-spent-by-set k-by:h nockchain) ~(input-names get:raw-tx:t raw3) id.raw3)
    ::  and the raw-txs partition still holds across the whole reorg
    ?>  =(~ ~(consensus-invariants k-by:h nockchain))
    ~
    ::
    :: TODO: pending blocks retention policy and state management
    ::       possibly scrutinize spend-by more
  ::
::  +accept-block claims every tx in a block (blocks-needed-by += block, tx
::  leaves excluded-txs) and, before +release-orphaned-branch existed, nothing
::  ever released that claim for an ACCEPTED block -- only +reject-pending-block
::  did, and only for pending ones. So a tx carried by a block that then lost a
::  chain race was stranded permanently: out of the mempool, so invisible to the
::  miner (whose candidate set is excluded-txs plus pending-block txs) and never
::  re-gossiped (that walks excluded-txs); yet still in blocks-needed-by, so
::  +drop-tx refused to garbage collect it and its inputs stayed pinned in
::  spent-by, blocking even a replacement tx spending the same notes.
::
::  Chain:  G --> 1 --> 2 --> 3(tx1)          <- accepted, then orphaned
::                           └-> 3' --> 4'    <- heavier fork, does NOT carry tx1
++  test-reorg-orphaned-tx-returns-to-mempool
  =+  [nockchain genesis]=init-nockchain:h
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 2 nockchain)
  ::
  =/  raw1  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h (snag 0 pages))
  =^  effs=(list effect:h)  nockchain
    (~(heard-tx k-by:h nockchain) raw1)
  ::  in the mempool to begin with
  ?>  (~(check-excluded k-by:h nockchain) id.raw1)
  ::
  ::  block 3 mines tx1 and becomes heaviest: the tx leaves the mempool
  =/  block-3  (make-page-with-txs:v0:h (snag 1 pages) ~[id.raw1])
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) block-3)
  ?>  =(~(digest get:page:t block-3) ~(heaviest-block k-by:h nockchain))
  ?>  ?&  !(~(has-excluded k-by:h nockchain) id.raw1)  ::  mined, not in mempool
          (~(has-raw-tx k-by:h nockchain) id.raw1)     ::  but still held
      ==
  ::
  ::  a competing fork from block 2 that does NOT carry tx1, and outweighs it
  =/  block-3-p  (make-empty-page:h (snag 1 pages))
  =/  block-4-p  (make-empty-page:h block-3-p)
  =^  effs=(list effect:h)  nockchain
    (~(heard-blocks k-by:h nockchain) ~[block-3-p block-4-p])
  ::  the fork won: block 3 (and tx1 with it) is now orphaned
  ?>  =(~(digest get:page:t block-4-p) ~(heaviest-block k-by:h nockchain))
  ::
  ::  tx1 is unmined on the winning chain, so it must be back in the mempool --
  ::  mineable and re-gossipable again, not stranded in blocks-needed-by.
  ::  ~ from +consensus-invariants means the raw-txs partition still holds; in
  ::  particular no tx fell through the cracks (in raw-txs but in neither
  ::  blocks-needed-by nor excluded-txs), which is exactly the stranded state.
  %+  expect-eq
    !>([%.y %.y ~])
  !>  :*  (~(has-excluded k-by:h nockchain) id.raw1)
          (~(has-raw-tx k-by:h nockchain) id.raw1)
          ~(consensus-invariants k-by:h nockchain)
      ==
::
::  A returned tx must actually be OFFERED again, not just be a set membership:
::  the per-block re-gossip walks excluded-txs, so once tx1 is back the next new
::  heaviest block must re-announce it to peers.
++  test-reorg-orphaned-tx-is-regossiped
  =+  [nockchain genesis]=init-nockchain:h
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 2 nockchain)
  =/  raw1  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h (snag 0 pages))
  =^  effs=(list effect:h)  nockchain
    (~(heard-tx k-by:h nockchain) raw1)
  ::
  =/  block-3  (make-page-with-txs:v0:h (snag 1 pages) ~[id.raw1])
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) block-3)
  ?>  !(~(has-excluded k-by:h nockchain) id.raw1)
  ::
  ::  losing fork overtakes, orphaning block 3 and returning tx1
  =/  block-3-p  (make-empty-page:h (snag 1 pages))
  =/  block-4-p  (make-empty-page:h block-3-p)
  =^  effs=(list effect:h)  nockchain
    (~(heard-blocks k-by:h nockchain) ~[block-3-p block-4-p])
  ?>  (~(has-excluded k-by:h nockchain) id.raw1)
  ::
  ::  extend the winning chain: tx1 is in the mempool, so this block's
  ::  new-heaviest re-gossip must carry it
  =/  block-5-p  (make-empty-page:h block-4-p)
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) block-5-p)
  =/  regossiped=(z-set:zoon raw-tx:t)  (filter-heard-tx-effects:h effs)
  %+  expect-eq
    !>([%.y ~])
  !>  :*  (~(has z-in:zoon regossiped) raw1)
          ~(consensus-invariants k-by:h nockchain)
      ==
::
::  A multi-block orphaned branch: the walk must release EVERY block on it, not
::  just the old tip. Both tx1 (in block 3) and tx2 (in block 4) come back.
::
::  Chain:  G --> 1 --> 2 --> 3(tx1) --> 4(tx2)      <- orphaned, 2 blocks deep
::                           └-> 3' --> 4' --> 5'    <- wins, carries neither
++  test-reorg-orphaned-branch-releases-every-block
  =+  [nockchain genesis]=init-nockchain:h
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 2 nockchain)
  ::
  =/  raw1  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h (snag 0 pages))
  =/  raw2  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h (snag 1 pages))
  =^  effs=(list effect:h)  nockchain
    (~(heard-txs k-by:h nockchain) ~[raw1 raw2])
  ::
  =/  block-3  (make-page-with-txs:v0:h (snag 1 pages) ~[id.raw1])
  =/  block-4  (make-page-with-txs:v0:h block-3 ~[id.raw2])
  =^  effs=(list effect:h)  nockchain
    (~(heard-blocks k-by:h nockchain) ~[block-3 block-4])
  ?>  =(~(digest get:page:t block-4) ~(heaviest-block k-by:h nockchain))
  ::  both mined, so both out of the mempool
  ?>  ?&  !(~(has-excluded k-by:h nockchain) id.raw1)
          !(~(has-excluded k-by:h nockchain) id.raw2)
      ==
  ::
  ::  a three-block fork from block 2 outweighs the two-block branch
  =/  block-3-p  (make-empty-page:h (snag 1 pages))
  =/  block-4-p  (make-empty-page:h block-3-p)
  =/  block-5-p  (make-empty-page:h block-4-p)
  =^  effs=(list effect:h)  nockchain
    (~(heard-blocks k-by:h nockchain) ~[block-3-p block-4-p block-5-p])
  ?>  =(~(digest get:page:t block-5-p) ~(heaviest-block k-by:h nockchain))
  ::
  ::  the walk went past the old tip (block 4) to block 3 as well, so BOTH txs
  ::  are back in the mempool
  %+  expect-eq
    !>([%.y %.y %.y %.y ~])
  !>  :*  (~(has-excluded k-by:h nockchain) id.raw1)
          (~(has-raw-tx k-by:h nockchain) id.raw1)
          (~(has-excluded k-by:h nockchain) id.raw2)
          (~(has-raw-tx k-by:h nockchain) id.raw2)
          ~(consensus-invariants k-by:h nockchain)
      ==
::
::  The winning chain double-spends the orphaned tx's inputs (a different tx
::  spends the same coinbase). tx1 is unminable now, so returning it to the
::  mempool must not park it there: the spent-input sweep in the same
::  +garbage-collect pass drops it, freeing its inputs from spent-by so the
::  replacement can be spent.
::
::  Chain:  G --> 1 --> 2 --> 3(tx1)             <- orphaned
::                           └-> 3'(tx1-alt) --> 4'  <- wins, spends same note
++  test-reorg-orphaned-tx-double-spent-is-dropped
  =+  [nockchain genesis]=init-nockchain:h
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 2 nockchain)
  ::
  ::  two different txs spending the SAME coinbase, to different recipients
  =/  raw1      (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h (snag 0 pages))
  =/  raw1-alt  (make-raw-tx-from-coinbase:v0:h p:default-keys-3:h (snag 0 pages))
  ?>  !=(id.raw1 id.raw1-alt)
  ::
  =^  effs=(list effect:h)  nockchain
    (~(heard-tx k-by:h nockchain) raw1)
  =/  block-3  (make-page-with-txs:v0:h (snag 1 pages) ~[id.raw1])
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) block-3)
  ?>  =(~(digest get:page:t block-3) ~(heaviest-block k-by:h nockchain))
  ?>  !(~(has-excluded k-by:h nockchain) id.raw1)
  ::
  ::  The fork's block 3' carries the CONFLICTING tx. Note we cannot simply hear
  ::  raw1-alt first: +heard-tx would refuse it outright, because raw1 already
  ::  reserved that note in spent-by (+inputs-spent). The way a conflicting tx
  ::  really reaches us is behind a block -- 3' parks as a PENDING block waiting
  ::  on the tx, and the pending path in +heard-tx admits it without the
  ::  mempool's spent check.
  =/  block-3-p  (make-page-with-txs:v0:h (snag 1 pages) ~[id.raw1-alt])
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) block-3-p)
  =^  effs=(list effect:h)  nockchain
    (~(heard-tx k-by:h nockchain) raw1-alt)
  ::
  ::  4' extends the fork, making it heaviest and orphaning block 3
  =/  block-4-p  (make-empty-page:h block-3-p)
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) block-4-p)
  ?>  =(~(digest get:page:t block-4-p) ~(heaviest-block k-by:h nockchain))
  ::
  ::  block 3's claim on tx1 is released, but the winning chain spent tx1's note
  ::  with tx1-alt. So tx1 is unmineable: it must be dropped outright, not parked
  ::  in the mempool to be re-gossiped and re-offered to the miner forever. The
  ::  drop is also what frees its inputs from spent-by.
  %+  expect-eq
    !>([%.n %.n ~])
  !>  :*  (~(has-excluded k-by:h nockchain) id.raw1)
          (~(has-raw-tx k-by:h nockchain) id.raw1)
          ~(consensus-invariants k-by:h nockchain)
      ==
::
::  The converse, and the case a naive "return every orphaned block's txs" would
::  get wrong: when the winning block carries the SAME tx (the common reorg),
::  the tx really is mined and must stay OUT of the mempool. Its claim from the
::  orphaned block is released, but the winning block's claim keeps it excluded.
::
::  The boot repair. +release-orphaned-branch only walks the branch a LIVE reorg
::  abandons, so it cannot reach a tx stranded by a reorg that happened while
::  the node ran the OLD kernel -- and every upgrading node is carrying exactly
::  those. +repair-orphaned-claims (run once from +load) is what frees them, and
::  it is the reason this fix is not merely prospective.
::
::  The new kernel will not strand a tx for us any more, so we build the reorg,
::  then re-strand raw1 by hand exactly as the old kernel left it: still held in
::  raw-txs, still claimed by the now-orphaned block, absent from excluded-txs.
::  Note this state satisfies +apt -- the tx IS in exactly one of
::  blocks-needed-by / excluded-txs -- which is precisely why the bug went
::  unnoticed: nothing was structurally broken, the claim was just never
::  released.
++  test-boot-repair-frees-stranded-orphan-tx
  =+  [nockchain genesis]=init-nockchain:h
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 2 nockchain)
  =/  raw1  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h (snag 0 pages))
  =^  effs=(list effect:h)  nockchain
    (~(heard-tx k-by:h nockchain) raw1)
  ::
  ::  block 3 mines raw1, then a longer fork orphans it
  =/  block-3  (make-page-with-txs:v0:h (snag 1 pages) ~[id.raw1])
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) block-3)
  =/  block-3-p  (make-empty-page:h (snag 1 pages))
  =/  block-4-p  (make-empty-page:h block-3-p)
  =^  effs=(list effect:h)  nockchain
    (~(heard-blocks k-by:h nockchain) ~[block-3-p block-4-p])
  ?>  =(~(digest get:page:t block-4-p) ~(heaviest-block k-by:h nockchain))
  ::  the live path already freed it
  ?>  (~(has-excluded k-by:h nockchain) id.raw1)
  ::
  ::  now put it back the way the OLD kernel would have left it
  =/  orphan-id  ~(digest get:page:t block-3)
  =/  stranded  (~(strand-tx-on-block k-by:h nockchain) id.raw1 orphan-id)
  ::  this is the stranded state: held, claimed by a dead block, out of the
  ::  mempool -- and structurally "valid" (+apt is ~), which is exactly why the
  ::  bug went unnoticed for so long
  ?>  (~(con-raw-tx k-by:h nockchain) stranded id.raw1)
  ?>  (~(con-claimed k-by:h nockchain) stranded id.raw1)
  ?>  !(~(con-excluded k-by:h nockchain) stranded id.raw1)
  ?>  =(~ (~(con-invariants k-by:h nockchain) stranded))
  ::
  ::  Boot the node on the new kernel: this drives the REAL +load, not the repair
  ::  arm directly, so it also proves +load actually runs the repair. If the
  ::  wiring were wrong -- or heaviest-chain.d were empty at load -- the repair
  ::  would find no orphans, quietly no-op, and every stranded tx would stay
  ::  stranded while the logs looked fine.
  =/  repaired  (~(boot-with k-by:h nockchain) stranded)
  ::
  ::  the dead block's claim is gone and the tx is back in the mempool, where
  ::  the miner and the re-gossip can finally see it again
  %+  expect-eq
    !>([%.y %.n %.y ~])
  !>  :*  (~(con-excluded k-by:h nockchain) repaired id.raw1)
          (~(con-claimed k-by:h nockchain) repaired id.raw1)
          (~(con-raw-tx k-by:h nockchain) repaired id.raw1)
          (~(con-invariants k-by:h nockchain) repaired)
      ==
::
::  +load deletes an orphaned block from every block-keyed map, and leaves a
::  block on the heaviest chain alone.
++  test-boot-deletes-orphaned-block
  =+  [nockchain genesis]=init-nockchain:h
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 2 nockchain)
  =/  raw1  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h (snag 0 pages))
  =^  effs=(list effect:h)  nockchain
    (~(heard-tx k-by:h nockchain) raw1)
  ::  block-3 carries raw1; the block-3-p / block-4-p fork then out-weighs it,
  ::  so block-3 is orphaned and block-3-p is the winner at the same height.
  =/  block-3  (make-page-with-txs:v0:h (snag 1 pages) ~[id.raw1])
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) block-3)
  =/  block-3-p  (make-empty-page:h (snag 1 pages))
  =/  block-4-p  (make-empty-page:h block-3-p)
  =^  effs=(list effect:h)  nockchain
    (~(heard-blocks k-by:h nockchain) ~[block-3-p block-4-p])
  ?>  =(~(digest get:page:t block-4-p) ~(heaviest-block k-by:h nockchain))
  ::
  =/  orphan-id  ~(digest get:page:t block-3)
  =/  winner-id  ~(digest get:page:t block-3-p)
  =/  before  ~(con k-by:h nockchain)
  ::  the state every node carries today: the orphan is still there in full.
  ::  It carried raw1, so it has a .txs entry; block-3-p is empty and has none.
  ?>  .=  ~[%blocks %balance %txs %min-timestamps %epoch-start %targets]
      (~(con-block-residue k-by:h nockchain) before orphan-id)
  ::
  =/  booted  (~(boot-with k-by:h nockchain) before)
  %+  expect-eq
    !>  :*  ~
            ~[%blocks %balance %min-timestamps %epoch-start %targets]
            ~
        ==
  !>  :*  (~(con-block-residue k-by:h nockchain) booted orphan-id)
          (~(con-block-residue k-by:h nockchain) booted winner-id)
          (~(con-invariants k-by:h nockchain) booted)
      ==
::
::  Deleting a multi-block orphan branch leaves every cross-map reference
::  resolvable. The pre-boot assertion keeps the oracle from passing vacuously.
++  test-boot-deletion-preserves-referential-integrity
  =+  [nockchain genesis]=init-nockchain:h
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 2 nockchain)
  =/  raw1  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h (snag 0 pages))
  =^  effs=(list effect:h)  nockchain
    (~(heard-tx k-by:h nockchain) raw1)
  ::  a TWO-block orphan branch (block-3 -> block-4), out-weighed by a three
  ::  block fork. Both orphans must go: keeping block-4 while deleting its
  ::  parent block-3 is exactly the dangling-parent break.
  =/  block-3  (make-page-with-txs:v0:h (snag 1 pages) ~[id.raw1])
  =/  block-4  (make-empty-page:h block-3)
  =^  effs=(list effect:h)  nockchain
    (~(heard-blocks k-by:h nockchain) ~[block-3 block-4])
  =/  block-3-p  (make-empty-page:h (snag 1 pages))
  =/  block-4-p  (make-empty-page:h block-3-p)
  =/  block-5-p  (make-empty-page:h block-4-p)
  =^  effs=(list effect:h)  nockchain
    (~(heard-blocks k-by:h nockchain) ~[block-3-p block-4-p block-5-p])
  ?>  =(~(digest get:page:t block-5-p) ~(heaviest-block k-by:h nockchain))
  ::
  =/  before  ~(con k-by:h nockchain)
  ::  the chain is sound BEFORE the boot, orphan branch and all
  ?>  =(~ (~(con-referential-integrity k-by:h nockchain) before))
  ?>  =(~ (~(con-invariants k-by:h nockchain) before))
  ::
  =/  booted  (~(boot-with k-by:h nockchain) before)
  ::  both orphans gone, the winning chain intact, and every reference still
  ::  resolves
  %+  expect-eq
    !>  :*  ~  ~  ~
            ~[%blocks %balance %min-timestamps %epoch-start %targets]
            ~
        ==
  !>  :*  (~(con-referential-integrity k-by:h nockchain) booted)
          (~(con-block-residue k-by:h nockchain) booted ~(digest get:page:t block-3))
          (~(con-block-residue k-by:h nockchain) booted ~(digest get:page:t block-4))
          (~(con-block-residue k-by:h nockchain) booted ~(digest get:page:t block-3-p))
          (~(con-invariants k-by:h nockchain) booted)
      ==
::
::  A block arriving for a deleted orphan parent takes +heard-block's
::  missing-parent path: not accepted, no crash. This is what keeps the deleted
::  block's cross-map reads (.balance, .targets, .epoch-start, the 11-deep
::  +update-min-timestamps walk) from ever being reached.
++  test-boot-deleted-orphan-child-is-handled
  =+  [nockchain genesis]=init-nockchain:h
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 2 nockchain)
  =/  raw1  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h (snag 0 pages))
  =^  effs=(list effect:h)  nockchain
    (~(heard-tx k-by:h nockchain) raw1)
  =/  block-3  (make-page-with-txs:v0:h (snag 1 pages) ~[id.raw1])
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) block-3)
  =/  block-3-p  (make-empty-page:h (snag 1 pages))
  =/  block-4-p  (make-empty-page:h block-3-p)
  =^  effs=(list effect:h)  nockchain
    (~(heard-blocks k-by:h nockchain) ~[block-3-p block-4-p])
  ?>  =(~(digest get:page:t block-4-p) ~(heaviest-block k-by:h nockchain))
  ::
  ::  boot deletes block-3, then the node is handed block-3's child
  =/  booted  (~(boot-with k-by:h nockchain) ~(con k-by:h nockchain))
  ?>  .=  ~
      (~(con-block-residue k-by:h nockchain) booted ~(digest get:page:t block-3))
  =/  booted-chain  (~(with-con k-by:h nockchain) booted)
  =/  block-4  (make-empty-page:h block-3)
  =^  effs=(list effect:h)  booted-chain
    (~(heard-block k-by:h booted-chain) block-4)
  ::  not accepted (its parent is gone), heaviest chain untouched, state sound
  %+  expect-eq
    !>([~ ~(digest get:page:t block-4-p) ~])
  !>  :*  %+  ~(con-block-residue k-by:h nockchain)
            ~(con k-by:h booted-chain)
          ~(digest get:page:t block-4)
          ~(heaviest-block k-by:h booted-chain)
          ~(consensus-invariants k-by:h booted-chain)
      ==
::
::  A block left PENDING across a boot that deletes its parent is rejected, not
::  crashed and not accepted.
::
::    The one path reaching +validate-page-with-txs with a deleted parent: a
::    pending block passed +heard-block's parent check before the boot, and
::    +process-block-with-txs makes no second one.
++  test-boot-deleted-orphan-pending-child-is-rejected
  =+  [nockchain genesis]=init-nockchain:h
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 2 nockchain)
  =/  raw1  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h (snag 0 pages))
  =^  effs=(list effect:h)  nockchain
    (~(heard-tx k-by:h nockchain) raw1)
  ::  block-3 is orphaned by the block-3-p / block-4-p fork, as before
  =/  block-3  (make-page-with-txs:v0:h (snag 1 pages) ~[id.raw1])
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) block-3)
  =/  block-3-p  (make-empty-page:h (snag 1 pages))
  =/  block-4-p  (make-empty-page:h block-3-p)
  =^  effs=(list effect:h)  nockchain
    (~(heard-blocks k-by:h nockchain) ~[block-3-p block-4-p])
  ?>  =(~(digest get:page:t block-4-p) ~(heaviest-block k-by:h nockchain))
  ::
  ::  block-4 extends the orphan and carries raw2, which the node has NOT heard.
  ::  Its parent (block-3) is still present here, so it passes +heard-block's
  ::  parent check and parks in .pending-blocks awaiting the tx.
  =/  raw2  (make-raw-tx-from-coinbase:v0:h p:default-keys-3:h (snag 1 pages))
  =/  block-4  (make-page-with-txs:v0:h block-3 ~[id.raw2])
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) block-4)
  ?>  (~(has-pending-block k-by:h nockchain) ~(digest get:page:t block-4))
  ::
  ::  boot deletes block-3 out from under the pending block-4
  =/  booted  (~(boot-with k-by:h nockchain) ~(con k-by:h nockchain))
  =/  booted-chain  (~(with-con k-by:h nockchain) booted)
  ::  now the missing tx arrives and block-4 is finally validated
  =^  effs=(list effect:h)  booted-chain
    (~(heard-tx k-by:h booted-chain) raw2)
  ::  rejected, not accepted, and the state is still sound
  %+  expect-eq
    !>([~ ~(digest get:page:t block-4-p) ~])
  !>  :*  %+  ~(con-block-residue k-by:h nockchain)
            ~(con k-by:h booted-chain)
          ~(digest get:page:t block-4)
          ~(heaviest-block k-by:h booted-chain)
          ~(consensus-invariants k-by:h booted-chain)
      ==
::
::  A returned tx must get a fresh retention lease. The mempool keeps only ~4
::  blocks of history (tx-retain), measured from the tx's heard-at. An orphaned
::  tx was necessarily heard BEFORE the block that mined it, so by the time the
::  reorg returns it, its original heard-at is already older than the retention
::  window -- and the retention sweep, which runs later in the very same
::  +garbage-collect, would drop it on sight. It would be evicted rather than
::  re-mined, and the whole release would be pointless. +release-orphan-claims
::  refreshes heard-at to the current height to prevent exactly that; this test
::  fails if that refresh is removed.
::
::  The chain is deep enough for the tx's original heard-at to be stale: tx1 is
::  heard at height 2 and the reorg lands at height 8, well past 4 blocks.
++  test-reorg-orphaned-tx-survives-retention-sweep
  =+  [nockchain genesis]=init-nockchain:h
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 2 nockchain)
  ::
  ::  heard at height 2
  =/  raw1  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h (snag 0 pages))
  =^  effs=(list effect:h)  nockchain
    (~(heard-tx k-by:h nockchain) raw1)
  ?>  (~(check-excluded k-by:h nockchain) id.raw1)
  ::
  ::  mined at height 3, on a branch we then extend to height 7
  =/  block-3  (make-page-with-txs:v0:h (snag 1 pages) ~[id.raw1])
  =/  block-4  (make-empty-page:h block-3)
  =/  block-5  (make-empty-page:h block-4)
  =/  block-6  (make-empty-page:h block-5)
  =/  block-7  (make-empty-page:h block-6)
  =^  effs=(list effect:h)  nockchain
    (~(heard-blocks k-by:h nockchain) ~[block-3 block-4 block-5 block-6 block-7])
  ?>  =(~(digest get:page:t block-7) ~(heaviest-block k-by:h nockchain))
  ?>  !(~(has-excluded k-by:h nockchain) id.raw1)
  ::
  ::  a longer fork from block 2 overtakes it, orphaning the whole branch. tx1
  ::  comes back with heard-at = 8, so the retention sweep in this same event
  ::  (which drops excluded txs older than ~4 blocks) must NOT evict it.
  =/  block-3-p  (make-empty-page:h (snag 1 pages))
  =/  block-4-p  (make-empty-page:h block-3-p)
  =/  block-5-p  (make-empty-page:h block-4-p)
  =/  block-6-p  (make-empty-page:h block-5-p)
  =/  block-7-p  (make-empty-page:h block-6-p)
  =/  block-8-p  (make-empty-page:h block-7-p)
  =^  effs=(list effect:h)  nockchain
    %-  ~(heard-blocks k-by:h nockchain)
    ~[block-3-p block-4-p block-5-p block-6-p block-7-p block-8-p]
  ?>  =(~(digest get:page:t block-8-p) ~(heaviest-block k-by:h nockchain))
  ::
  ::  still in the mempool, with a full window to be re-mined
  %+  expect-eq
    !>([%.y %.y ~])
  !>  :*  (~(has-excluded k-by:h nockchain) id.raw1)
          (~(has-raw-tx k-by:h nockchain) id.raw1)
          ~(consensus-invariants k-by:h nockchain)
      ==
::
::  Chain:  G --> 1 --> 2 --> 3(tx1)            <- orphaned
::                           └-> 3' --> 4'(tx1)  <- wins, and re-mines tx1
::
::  (3' is empty rather than carrying tx1 itself: a block is identified by its
::  content, so a same-parent same-txs "fork" of block 3 would just BE block 3.)
++  test-reorg-tx-in-winning-block-stays-mined
  =+  [nockchain genesis]=init-nockchain:h
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 2 nockchain)
  ::
  =/  raw1  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h (snag 0 pages))
  =^  effs=(list effect:h)  nockchain
    (~(heard-tx k-by:h nockchain) raw1)
  ?>  (~(check-excluded k-by:h nockchain) id.raw1)
  ::
  =/  block-3  (make-page-with-txs:v0:h (snag 1 pages) ~[id.raw1])
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) block-3)
  ?>  =(~(digest get:page:t block-3) ~(heaviest-block k-by:h nockchain))
  ?>  !(~(has-excluded k-by:h nockchain) id.raw1)
  ::
  ::  the winning fork re-mines tx1 at height 4
  =/  block-3-p  (make-empty-page:h (snag 1 pages))
  =/  block-4-p  (make-page-with-txs:v0:h block-3-p ~[id.raw1])
  =^  effs=(list effect:h)  nockchain
    (~(heard-blocks k-by:h nockchain) ~[block-3-p block-4-p])
  ?>  =(~(digest get:page:t block-4-p) ~(heaviest-block k-by:h nockchain))
  ::
  ::  block 3's claim is released, but block 4' still carries tx1, so it is
  ::  genuinely mined and must NOT be handed back to the mempool
  %+  expect-eq
    !>([%.n %.y ~])
  !>  :*  (~(has-excluded k-by:h nockchain) id.raw1)
          (~(has-raw-tx k-by:h nockchain) id.raw1)
          ~(consensus-invariants k-by:h nockchain)
      ==

::  A reorg onto a shorter, heavier chain releases the whole abandoned branch.
::
::    Heaviness is accumulated-work, not height, so the winning chain can end
::    BELOW the branch it abandons. +prune-above then leaves heaviest-chain's
::    keys at exactly 0..tip, and +release-orphaned-branch reads absence above
::    the tip as proof a block is orphaned. Drives the real +update into the
::    real +release-orphaned-branch: either half alone releases nothing.
++  test-reorg-lowering-tip-releases-abandoned-branch
  =+  [nockchain genesis]=init-nockchain:h
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 2 nockchain)
  =/  raw1  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h (snag 0 pages))
  =/  raw2  (make-raw-tx-from-coinbase:v0:h p:default-keys-3:h (snag 1 pages))
  =^  effs=(list effect:h)  nockchain
    (~(heard-tx k-by:h nockchain) raw1)
  =^  effs=(list effect:h)  nockchain
    (~(heard-tx k-by:h nockchain) raw2)
  ::  blocks 3 and 4 carry the txs and become the tip
  =/  block-3  (make-page-with-txs:v0:h (snag 1 pages) ~[id.raw1])
  =/  block-4  (make-page-with-txs:v0:h block-3 ~[id.raw2])
  =^  effs=(list effect:h)  nockchain
    (~(heard-blocks k-by:h nockchain) ~[block-3 block-4])
  ?>  =(~(digest get:page:t block-4) ~(heaviest-block k-by:h nockchain))
  ::  claimed by their blocks, so out of the mempool
  ?>  ?&  !(~(has-excluded k-by:h nockchain) id.raw1)
          !(~(has-excluded k-by:h nockchain) id.raw2)
      ==
  ::
  ::  A heavier chain forks at block 1 and ends at block-2-p, height 2: the tip
  ::  drops from 4 to 2 while blocks 2, 3 and 4 are still held. It must fork
  ::  rather than end at block-2 itself -- a prefix of the abandoned chain can
  ::  never out-weigh it, and the walk would stop at the fork point without ever
  ::  taking the "index names a different block at this height" branch.
  ::  a different miner key gives a distinct block at the same height
  =/  block-2-p  (make-empty-page-multisig:h (snag 0 pages) p:default-keys-2:h)
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) block-2-p)
  =/  lowered
    %+  ~(with-heaviest-block k-by:h nockchain)
      ~(con k-by:h nockchain)
    ~(digest get:page:t block-2-p)
  =/  d  (~(der-update k-by:h nockchain) ~(der k-by:h nockchain) lowered block-2-p)
  ::  the index stops at the tip and names the fork at height 2
  ?>  ?&  =(~ (~(heaviest-chain-at k-by:h nockchain) d 4))
          =(~ (~(heaviest-chain-at k-by:h nockchain) d 3))
          =(`~(digest get:page:t block-2-p) (~(heaviest-chain-at k-by:h nockchain) d 2))
      ==
  ::
  =/  released
    %^  ~(release-branch k-by:h nockchain)  lowered
      ~(digest get:page:t block-4)
    heaviest-chain.d
  ::  blocks 4 and 3 release via absence above the tip, block-2 via the index
  ::  naming block-2-p at its height, and the walk stops at block 1
  %+  expect-eq
    !>([%.y %.y %.n %.n ~])
  !>  :*  (~(con-excluded k-by:h nockchain) released id.raw1)
          (~(con-excluded k-by:h nockchain) released id.raw2)
          (~(con-claimed k-by:h nockchain) released id.raw1)
          (~(con-claimed k-by:h nockchain) released id.raw2)
          (~(con-invariants k-by:h nockchain) released)
      ==
::
::  +update drops heaviest-chain entries above the tip and keeps the tip's own.
++  test-derived-update-prunes-heaviest-chain-above-tip
  =+  [nockchain genesis]=init-nockchain:h
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 4 nockchain)
  =/  tip  ~(tip-page k-by:h nockchain)
  =/  tip-height  ~(height get:page:t tip)
  =/  stale-id  ~(digest get:page:t (snag 0 pages))
  =/  stale-d
    %^  ~(put-heaviest-chain-at k-by:h nockchain)
        (~(put-heaviest-chain-at k-by:h nockchain) ~(der k-by:h nockchain) +(tip-height) stale-id)
      +(+(tip-height))
    stale-id
  =/  updated
    %^  ~(der-update k-by:h nockchain)  stale-d  ~(con k-by:h nockchain)  tip
  %+  expect-eq
    !>([~ ~ `~(digest get:page:t tip)])
  !>  :*  (~(heaviest-chain-at k-by:h nockchain) updated +(tip-height))
          (~(heaviest-chain-at k-by:h nockchain) updated +(+(tip-height)))
          (~(heaviest-chain-at k-by:h nockchain) updated tip-height)
      ==
::
::  A gap at or below the tip proves nothing, so the release stops rather than
::  hand back a tx that is really mined.
++  test-release-orphaned-branch-stops-at-gap-below-tip
  =+  [nockchain genesis]=init-nockchain:h
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 2 nockchain)
  =/  raw1  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h (snag 0 pages))
  =^  effs=(list effect:h)  nockchain
    (~(heard-tx k-by:h nockchain) raw1)
  =/  block-3  (make-page-with-txs:v0:h (snag 1 pages) ~[id.raw1])
  =/  block-3-p  (make-empty-page:h (snag 1 pages))
  =/  block-4-p  (make-empty-page:h block-3-p)
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) block-3)
  =^  effs=(list effect:h)  nockchain
    (~(heard-blocks k-by:h nockchain) ~[block-3-p block-4-p])
  ?>  =(~(digest get:page:t block-4-p) ~(heaviest-block k-by:h nockchain))
  ::  the live reorg already released raw1, so put it back under block-3's
  ::  claim: otherwise this asserts that release rather than the walk below
  =/  stranded
    %+  ~(strand-tx-on-block k-by:h nockchain)
      id.raw1
    ~(digest get:page:t block-3)
  ?>  (~(con-claimed k-by:h nockchain) stranded id.raw1)
  ::  block-3 sits at height 3, below the tip at 4. Punch the index out there:
  ::  its absence is now unexplained rather than proof of an orphan.
  =/  gapped  (~(del-heaviest-chain-at k-by:h nockchain) ~(der k-by:h nockchain) 3)
  =/  released
    %^  ~(release-branch k-by:h nockchain)  stranded
      ~(digest get:page:t block-3)
    heaviest-chain.gapped
  ::  nothing released: the tx stays claimed by block-3
  %+  expect-eq
    !>([%.n %.y])
  !>  :*  (~(con-excluded k-by:h nockchain) released id.raw1)
          (~(con-claimed k-by:h nockchain) released id.raw1)
      ==
::
::  The %heaviest-chain peek reports the tip, not the highest block ever seen.
::
::    highest-block-height is a monotone max over every accepted block and is
::    never lowered, so after a reorg onto a shorter heavier chain it names a
::    height the heaviest chain has not reached. Indexing by it returns a block
::    that is not the tip, or -- once +prune-above drops the entry -- nothing at
::    all, which reads as "this node has no chain".
++  test-heaviest-chain-peek-reports-tip
  =+  [nockchain genesis]=init-nockchain:h
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 2 nockchain)
  =/  block-3-p  (make-empty-page:h (snag 1 pages))
  =/  block-4-p  (make-empty-page:h block-3-p)
  =^  effs=(list effect:h)  nockchain
    (~(heard-blocks k-by:h nockchain) ~[block-3-p block-4-p])
  ?>  =(~(digest get:page:t block-4-p) ~(heaviest-block k-by:h nockchain))
  ::  a heavier chain ending at block 2 wins: the tip drops to 2 while
  ::  highest-block-height stays at 4
  =/  block-2  (snag 1 pages)
  =/  lowered
    %+  ~(with-heaviest-block k-by:h nockchain)
      ~(con k-by:h nockchain)
    ~(digest get:page:t block-2)
  =/  chain  (~(with-con k-by:h nockchain) lowered)
  =/  peeked=(unit (unit *))  (peek:chain [%heaviest-chain ~])
  ?~  peeked  !!
  ?~  u.peeked  !!
  %+  expect-eq
    !>  [~(height get:page:t block-2) ~(digest get:page:t block-2)]
  !>  ;;([page-number:t block-id:t] u.u.peeked)

::  +release-orphaned-branch rejects an index that does not describe the tip.
::
::    Its whole result depends on +update having revised and pruned the index
::    for the new tip first: handed the index from before the reorg, it reads
::    old-heavy's own stale entry as the common ancestor and releases nothing.
::    Nothing in the type enforces that ordering, so the arm checks it.
++  test-release-orphaned-branch-rejects-stale-heaviest-chain
  =+  [nockchain genesis]=init-nockchain:h
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 2 nockchain)
  =/  raw1  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h (snag 0 pages))
  =^  effs=(list effect:h)  nockchain
    (~(heard-tx k-by:h nockchain) raw1)
  =/  block-3  (make-page-with-txs:v0:h (snag 1 pages) ~[id.raw1])
  =/  block-4  (make-empty-page:h block-3)
  =^  effs=(list effect:h)  nockchain
    (~(heard-blocks k-by:h nockchain) ~[block-3 block-4])
  ?>  =(~(digest get:page:t block-4) ~(heaviest-block k-by:h nockchain))
  ::  the index as it stood before the reorg: still naming block-4 at height 4,
  ::  while the tip has moved to block-2-p at height 2
  =/  stale-chain  heaviest-chain:~(der k-by:h nockchain)
  =/  block-2-p  (make-empty-page-multisig:h (snag 0 pages) p:default-keys-2:h)
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) block-2-p)
  =/  lowered
    %+  ~(with-heaviest-block k-by:h nockchain)
      ~(con k-by:h nockchain)
    ~(digest get:page:t block-2-p)
  %+  expect-fail
    |.  %^  ~(release-branch k-by:h nockchain)  lowered
          ~(digest get:page:t block-4)
        stale-chain
  `"release-orphaned-branch-stale-heaviest-chain"
--
