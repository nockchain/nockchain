::  tests/dumb/consensus.hoon
/=  dcon  /apps/dumbnet/lib/consensus
/=  dmin  /apps/dumbnet/lib/miner
/=  dder  /apps/dumbnet/lib/derived
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
--
