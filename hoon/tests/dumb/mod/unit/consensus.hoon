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
  =/  loaded=kernel-state  (load:inner:dumb state)
  %+  expect-eq
    !>([%.y %.y 2 %.n %.y])
  !>  :*  =(heaviest-block.c.state heaviest-block.c.loaded)
          (~(has h-by blocks.c.loaded) tip-id)
          ~(wyt h-by blocks.c.loaded)
          (~(has h-by blocks.c.loaded) stale-id)
          =(genesis-seal.c.state genesis-seal.c.loaded)
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
++  test-load-refuses-invalid-canonical-zoe-page-without-reset
  =/  base=proof:t  *proof:t
  =/  stale-v2=proof:t  [%2 objects.base hashes.base read-index.base]
  =/  state=kernel-state  (zoe-load-state [%.y `stale-v2])
  %+  expect-fail
    |.((load:inner:dumb state))
  `"preserving state and refusing to boot"
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
