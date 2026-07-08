/=  helpers  /tests/dumb/helpers
/=  txe  /common/tx-engine
/=  zoon  /common/zoon
/=  *  /common/test
|%
++  h  ~(. helpers bc-pending-provable:helpers)
++  t  ~(. txe bc-pending-provable:helpers)
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
  =/  block2  (prove-page:h (make-page-with-txs:v0:h (snag 0 pages) ~[id.raw1]))
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
    =/  block-22  (prove-page:h (make-page-with-txs:v0:h (snag 19 pages) ~[id.raw1]))
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
  =/  block-3  (prove-page:h (make-empty-page:h (snag 1 pages)))
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
  =/  block-3-p  (prove-page:h (make-page-with-txs:v0:h (snag 1 pages) ~[id.raw1]))
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) block-3-p)
  ::
  ::  heaviest shouldn't change - both forks have same weight
  ?>  =(~(digest get:page:t block-3) ~(heaviest-block k-by:h nockchain))
  ::
  ::  add block 4' with tx2 - this makes the fork heavier
  =/  block-4-p  (prove-page:h (make-page-with-txs:v0:h block-3-p ~[id.raw2]))
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
    =/  block-4  (prove-page:h (make-empty-page:h (snag 2 pages)))
    =/  block-5  (prove-page:h (make-page-with-txs:v0:h block-4 ~[id.raw1 id.raw2]))
    =/  block-6  (prove-page:h (make-empty-page:h block-5))
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
  ::  Tests that all txs attached to pending and accepted blocks
  ::  should be exclusively in blocks-needed-by after fork
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
    =/  block-4  (prove-page:h (make-empty-page:h (snag 2 pages)))
    =/  block-5  (prove-page:h (make-page-with-txs:v0:h block-4 ~[id.raw1 id.raw2]))
    =/  block-6  (prove-page:h (make-empty-page:h block-5))
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
    =/  block-4-p  (prove-page:h (make-page-with-txs:v0:h (snag 2 pages) ~[id.raw3]))
    =/  block-5-p  (prove-page:h (make-empty-page:h block-4-p))
    =/  block-6-p  (prove-page:h (make-page-with-txs:v0:h block-5-p ~[id.raw2]))
    =/  block-7-p  (prove-page:h (make-empty-page:h block-6-p))
    ::
    ::  hear re-org
    =^  effs  nockchain
      (~(heard-blocks k-by:h nockchain) ~[block-4-p block-5-p block-6-p block-7-p])
    ::  confirm re-org was successful
    ?>  =(~(digest get:page:t block-7-p) ~(heaviest-block k-by:h nockchain))
    ::
    ::  all txs need to exclusively be in blocks-needed-by because they have been accepted
    ?>  (~(check-bnb k-by:h nockchain) id.raw1 ~(digest get:page:t block-5))
    ?>  (~(check-bnb k-by:h nockchain) id.raw2 ~(digest get:page:t block-5))
    ?>  (~(check-bnb k-by:h nockchain) id.raw2 ~(digest get:page:t block-6-p))
    ?>  (~(check-bnb k-by:h nockchain) id.raw3 ~(digest get:page:t block-4-p))
    ::  all notes should be in spend-by set because they were in accepted blocks
    ?>  (~(has-spent-by-set k-by:h nockchain) ~(input-names get:raw-tx:t raw1) id.raw1)
    ?>  (~(has-spent-by-set k-by:h nockchain) ~(input-names get:raw-tx:t raw2) id.raw2)
    ?>  (~(has-spent-by-set k-by:h nockchain) ~(input-names get:raw-tx:t raw3) id.raw3)
    ~
    ::
    :: TODO: pending blocks retention policy and state management
    ::       possibly scrutinize spend-by more
  ::
--
