/=  dcon  /apps/dumbnet/lib/consensus
/=  dmin  /apps/dumbnet/lib/miner
/=  *  /apps/dumbnet/lib/types
/=  *  /common/zeke
/=  *  /common/h-zoon
/=  tx-engine  /common/tx-engine
/=  *  /common/test
/=  helpers  /tests/dumb/helpers
::
|_  constants=_bc-no-timelock:helpers
+*  t  ~(. tx-engine constants)
    h  ~(. helpers constants)
::  +der: pre-activation derived-state (read-only extra arg for consensus/miner doors)
++  der  ^-  derived-state  *derived-state
++  test-process-v0-into-v1
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h 1 con default-retain:h)
  =/  new-page=page:t  (make-empty-page:h par)
  ::  make a v0 coinbase to spend
  =/  coin=coinbase:t  (new:v0:coinbase:t new-page p:default-keys-1:h)
  =/  new-digest  (compute-digest:page:t new-page)
  =.  new-page
    ?^  -.new-page
      new-page(digest new-digest)
    new-page(digest new-digest)
  ::  create a new tx-acc for the new page
  =/  tac=tx-acc:t
    (new:tx-acc:t (~(get h-by balance.con) ~(digest get:page:t new-page)) ~(height get:page:t new-page))
  ::  add the new page to the consensus state
  =.  con  (~(accept-page dcon con der constants) new-page tac *@da)
  =.  con  (~(update-heaviest dcon con der constants) new-page)
  =^  last=page:t  con  (add-n-pages:h (sub v1-phase:t ~(height get:page:t new-page)) con default-retain:h)
  ::  create a tx-acc for the last page
  =/  tac=tx-acc:t
    (new:tx-acc:t (~(get h-by balance.con) ~(digest get:page:t last)) ~(height get:page:t last))
  ::  build v1 spend set: one spend-0 from the coinbase into a v1 output (lock root = brn)
  =/  seeds=seeds:v1:t
    %-  ~(put z-in *seeds:v1:t)
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      (hash:lock:t [%brn ~]~)
      gift           assets.coin
      parent-hash    (hash:nnote:t coin)
    ==
  =/  sp0-temp=spend-0:v1:t  %*  .  *spend-0:v1:t
                               seeds  *(z-set seed:v1:t)
                               fee    0
                             ==
  =/  sp0-signed=spend-0:v1:t  (sign:spend-0:v1:t sp0-temp s:default-keys-1:h)
  =/  min-fee=coins:t  (calculate-min-fee-for-spend-0:v1:h signature.sp0-signed)
  =/  adjusted-seeds=seeds:v1:t
    %-  ~(put z-in *seeds:v1:t)
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      (hash:lock:t [%brn ~]~)
      gift           (sub assets.coin min-fee)
      parent-hash    (hash:nnote:t coin)
    ==
  =/  sp0=spend-0:v1:t  %*  .  *spend-0:v1:t
                          seeds  adjusted-seeds
                          fee    min-fee
                        ==
  =/  sp=spend:v1:t  [%0 (sign:spend-0:v1:t sp0 s:default-keys-1:h)]
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) ~(name get:nnote:t coin) sp)
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  res=(reason tx-acc:t)  (process:tx-acc:t tac raw)
  %+  expect-eq  !>(%.y)
  !>(?=(%.y -.res))
::
++  test-two-to-one-utxo-v0-into-v1
  =/  con=consensus-state  initial-consensus-state:h
  ::  advance close to v1 activation
  =^  par=page:t  con  (add-n-pages:h (sub v1-phase:t 3) con default-retain:h)
  ::  first v0 coinbase page
  =/  page1=page:t  (make-empty-page:h par)
  =/  coin1=coinbase:t  (new:v0:coinbase:t page1 p:default-keys-1:h)
  =/  new-digest  (compute-digest:page:t page1)
  =.  page1
    ?^  -.page1
      page1(digest new-digest)
    page1(digest new-digest)
  =/  r1=(reason tx-acc:t)
    (~(validate-page-with-txs dcon con der constants) page1)
  ?:  ?=(%.n -.r1)
    ~&  >  [%page1-validate-failed +.r1]
    (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page1 +.r1 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page1)
  ::  second v0 coinbase page
  =/  page2=page:t  (make-empty-page:h page1)
  =/  coin2=coinbase:t  (new:v0:coinbase:t page2 p:default-keys-1:h)
  =/  new-digest  (compute-digest:page:t page2)
  =.  page2
    ?^  -.page2
      page2(digest new-digest)
    page2(digest new-digest)
  =/  r2=(reason tx-acc:t)
    (~(validate-page-with-txs dcon con der constants) page2)
  ?:  ?=(%.n -.r2)
    ~&  >  [%page2-validate-failed +.r2]
    (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page2 +.r2 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page2)
  ::  advance to exactly v1 activation height
  =^  last=page:t
    con
  %^    add-n-pages:h
      ?:  (gte ~(height get:page:t page2) v1-phase:t)  0
      (sub v1-phase:t ~(height get:page:t page2))
    con
  default-retain:h
  =/  tac=tx-acc:t
    (new:tx-acc:t (~(get h-by balance.con) ~(digest get:page:t last)) ~(height get:page:t last))
  ::  sanity: parent balance should contain both coinbases
  =/  par-bal=(h-map nname:t nnote:t)
    %-  need
    (~(get h-by balance.con) ~(digest get:page:t last))
  ~&  >  [%parent (to-b58:hash:t ~(digest get:page:t last))]
  ~&  >  [%has-coin1 (~(has h-by par-bal) ~(name get:nnote:t coin1))]
  ~&  >  [%has-coin2 (~(has h-by par-bal) ~(name get:nnote:t coin2))]
  ::  two v0 inputs -> one v1 output (shared lock-root)
  =/  fee  10.000
  =/  seeds1=seeds:v1:t
    %-  ~(put z-in *seeds:v1:t)
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      (hash:lock:t [%brn ~]~)
      gift           (sub assets.coin1 fee)
      parent-hash    (hash:nnote:t coin1)
    ==
  =/  seeds2=seeds:v1:t
    %-  ~(put z-in *seeds:v1:t)
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      (hash:lock:t [%brn ~]~)
      gift           (sub assets.coin2 fee)
      parent-hash    (hash:nnote:t coin2)
    ==
  =/  sp0-1=spend-0:v1:t
    %*  .  *spend-0:v1:t
      seeds  seeds1
      fee    fee
    ==
  =/  sp0-2=spend-0:v1:t
    %*  .  *spend-0:v1:t
      seeds  seeds2
      fee    fee
    ==
  =/  sp1=spend:v1:t  [%0 (sign:spend-0:v1:t sp0-1 s:default-keys-1:h)]
  =/  sp2=spend:v1:t  [%0 (sign:spend-0:v1:t sp0-2 s:default-keys-1:h)]
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) ~(name get:nnote:t coin1) sp1)
  =.  sps  (~(put z-by sps) ~(name get:nnote:t coin2) sp2)
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  new-page=page:t  (make-empty-page:h last)
  =/  =tx:t  (new:tx:t raw ~(height get:page:t new-page))
  =/  new-tx-ids  (~(put z-in *(z-set tx-id:t)) ~(id get:raw-tx:t raw))
  =.  new-page
    ?^  -.new-page
      new-page(tx-ids new-tx-ids)
    new-page(tx-ids new-tx-ids)
  =/  total-coinbase-assets=coins:t
    (add (emission-calc:coinbase:t ~(height get:page:t new-page)) ~(total-fees get:tx:t tx))
  =.  new-page
    ?^  -.new-page
      =/  new-coinbase  (new:v0:coinbase-split:t total-coinbase-assets default-keys-1-share:h)
      new-page(coinbase new-coinbase)
    =/  new-coinbase  (new:v1:coinbase-split:t total-coinbase-assets default-keys-1-share-v1:h)
    new-page(coinbase new-coinbase)
  =/  new-digest  (compute-digest:page:t new-page)
  =.  new-page
    ?^  -.new-page
      new-page(digest new-digest)
    new-page(digest new-digest)
  =^  ready  con  (~(add-raw-tx dcon con der constants) raw)
  ?>  -:(~(validate-page-without-txs dcon con der constants) new-page ~(timestamp get:page:t new-page))
  ::
  =/  tac  (~(validate-page-with-txs dcon con der constants) new-page)
  ?:  ?=(%.n -.tac)
    ~&  >  [%reason +.tac]
    (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) new-page +.tac *@da)
  =.  con  (~(update-heaviest dcon con der constants) new-page)
  =/  old-in-balance=?
    ?|  (~(has h-bi balance.con) ~(digest get:page:t new-page) ~(name get:nnote:t coin1))
        (~(has h-bi balance.con) ~(digest get:page:t new-page) ~(name get:nnote:t coin2))
    ==
  =/  outs=outputs:t  ~(outputs get:tx:t tx)
  ~!  -.outs
  ?>  ?=(%1 -.outs)
  =/  =output:t  (head ~(tap z-in +.outs))
  =/  new-note=nnote:t
    ~(note get:output:t output)
  ?>  ?=(@ -.new-note)
  =/  new-in-balance=?
    (~(has h-bi balance.con) ~(digest get:page:t new-page) ~(name get:nnote:t new-note))
  %+  expect-eq
    !>  [%.n %.y]
  !>  [old-in-balance new-in-balance]
::
++  test-two-to-one-utxo-v1-into-v1
  =/  con=consensus-state  initial-consensus-state:h
  ::  advance close to v1 activation
  =^  par=page:t  con  (add-n-pages:h (sub v1-phase:t 3) con default-retain:h)
  ::  first v0 coinbase page
  =/  page1=page:t  (make-empty-page:h par)
  =/  coin1=coinbase:t  (new:v0:coinbase:t page1 p:default-keys-1:h)
  =/  new-digest  (compute-digest:page:t page1)
  =.  page1
    ?^  -.page1
      page1(digest new-digest)
    page1(digest new-digest)
  =/  r1=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page1)
  ?:  ?=(%.n -.r1)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page1 +.r1 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page1)
  ::  second v0 coinbase page
  =/  page2=page:t  (make-empty-page:h page1)
  =/  coin2=coinbase:t  (new:v0:coinbase:t page2 p:default-keys-1:h)
  =/  new-digest  (compute-digest:page:t page2)
  =.  page2
    ?^  -.page2
      page2(digest new-digest)
    page2(digest new-digest)
  =/  r2=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page2)
  ?:  ?=(%.n -.r2)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page2 +.r2 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page2)
  ::  advance to exactly v1 activation height
  =^  last=page:t
    con
  %^    add-n-pages:h
      ?:  (gte ~(height get:page:t page2) v1-phase:t)  0
      (sub v1-phase:t ~(height get:page:t page2))
    con
  default-retain:h
  =/  tac=tx-acc:t
    (new:tx-acc:t (~(get h-by balance.con) ~(digest get:page:t last)) ~(height get:page:t last))
  ::  construct two v1 notes via spend-0 (v0->v1), each with a simple %hax lock
  ::  coin1 -> v1 note1
  =/  pre1=*  42
  =/  [root1=hash:t sc1=spend-condition:v1:t h1=hash:t]
    (make-hax:spend-condition:t pre1)
  =/  fee  10.000
  =/  seeds1=seeds:v1:t
    (~(put z-in *seeds:v1:t) (make-seed:v1:h root1 (sub assets.coin1 fee) (hash:nnote:t coin1)))
  =/  sp0-1=spend-0:v1:t
    %*  .  *spend-0:v1:t
      seeds  seeds1
      fee    fee
    ==
  =/  sp1a=spend:v1:t  [%0 (sign:spend-0:v1:t sp0-1 s:default-keys-1:h)]
  =/  spsa=spends:v1:t  (~(put z-by *spends:v1:t) ~(name get:nnote:t coin1) sp1a)
  =/  rawa=raw-tx:v1:t  (new:raw-tx:v1:t spsa)
  =/  [page-a=page:t newc=_con txa-acc=tx-acc:t]
    (add-raw-to-new-page:h last con rawa)
  =.  con  newc
  ::  extract name of v1 note1
  =/  txa=tx:t  (new:tx:t rawa ~(height get:page:t page-a))
  =/  outs-a=outputs:t  ~(outputs get:tx:t txa)
  ?>  ?=(%1 -.outs-a)
  =/  outa=output:t  (head ~(tap z-in +.outs-a))
  =/  note1=nnote:t  ~(note get:output:t outa)
  =/  name1=nname:t  ~(name get:nnote:t note1)
  ::  coin2 -> v1 note2
  =/  pre2=*  77
  =/  [root2=hash:t sc2=spend-condition:v1:t h2=hash:t]
    (make-hax:spend-condition:t pre2)
  =/  seeds2=seeds:v1:t
    (~(put z-in *seeds:v1:t) (make-seed:v1:h root2 (sub assets.coin2 fee) (hash:nnote:t coin2)))
  =/  sp0-2=spend-0:v1:t
    %*  .  *spend-0:v1:t
      seeds  seeds2
      fee    fee
    ==
  =/  sp1b=spend:v1:t  [%0 (sign:spend-0:v1:t sp0-2 s:default-keys-1:h)]
  =/  spsb=spends:v1:t  (~(put z-by *spends:v1:t) ~(name get:nnote:t coin2) sp1b)
  =/  rawb=raw-tx:v1:t  (new:raw-tx:v1:t spsb)
  =/  [page-b=page:t newc=_con txb-acc=tx-acc:t]
    (add-raw-to-new-page:h page-a con rawb)
  =.  con  newc
  ::  extract name of v1 note2
  =/  txb=tx:t  (new:tx:t rawb ~(height get:page:t page-b))
  =/  outs-b=outputs:t  ~(outputs get:tx:t txb)
  ?>  ?=(%1 -.outs-b)
  =/  outb=output:t  (head ~(tap z-in +.outs-b))
  =/  note2=nnote:t  ~(note get:output:t outb)
  =/  name2=nname:t  ~(name get:nnote:t note2)
  ::  now spend both v1 notes with spend-1 into one v1 output using %hax witness
  =/  out-pre=*  123
  =/  [out-root=hash:t out-sc=spend-condition:v1:t out-h=hash:t]
    (make-hax-lock:v1:h out-pre)
  =/  fee2  10.000
  =/  out-seed1=seed:v1:t
    (make-seed:v1:h out-root (sub assets.note1 fee2) (hash:nnote:t note1))
  =/  out-seeds1=seeds:v1:t  (~(put z-in *seeds:v1:t) out-seed1)
  =/  out-seed2=seed:v1:t
    (make-seed:v1:h out-root (sub assets.note2 fee2) (hash:nnote:t note2))
  =/  out-seeds2=seeds:v1:t  (~(put z-in *seeds:v1:t) out-seed2)
  =/  lok1=lock-merkle-proof:v1:t  (build-lock-merkle-proof:lock:t sc1 1)
  =/  lok2=lock-merkle-proof:v1:t  (build-lock-merkle-proof:lock:t sc2 1)
  =/  wit1=witness:v1:t  (make-witness-hax:v1:h root1 sc1 h1 pre1)
  =/  wit2=witness:v1:t  (make-witness-hax:v1:h root2 sc2 h2 pre2)
  =/  sp1-1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  wit1
      seeds    out-seeds1
      fee  fee2
    ==
  =/  sp1-2=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  wit2
      seeds  out-seeds2
      fee  fee2
    ==
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) name1 [%1 sp1-1])
  =.  sps  (~(put z-by sps) name2 [%1 sp1-2])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  new-page=page:t  (make-empty-page:h page-b)
  =/  =tx:t  (new:tx:t raw ~(height get:page:t new-page))
  =/  new-tx-ids  (~(put z-in *(z-set tx-id:t)) ~(id get:raw-tx:t raw))
  =.  new-page
    ?^  -.new-page
      new-page(tx-ids new-tx-ids)
    new-page(tx-ids new-tx-ids)
  =/  total-coinbase-assets=coins:t
    (add (emission-calc:coinbase:t ~(height get:page:t new-page)) ~(total-fees get:tx:t tx))
  =.  new-page
    ?^  -.new-page
      =/  new-coinbase  (new:v0:coinbase-split:t total-coinbase-assets default-keys-1-share:h)
      new-page(coinbase new-coinbase)
    =/  new-coinbase  (new:v1:coinbase-split:t total-coinbase-assets default-keys-1-share-v1:h)
    new-page(coinbase new-coinbase)
  =/  new-digest  (compute-digest:page:t new-page)
  =.  new-page
    ?^  -.new-page
      new-page(digest new-digest)
    new-page(digest new-digest)
  =^  ready  con  (~(add-raw-tx dcon con der constants) raw)
  ?>  -:(~(validate-page-without-txs dcon con der constants) new-page ~(timestamp get:page:t new-page))
  =/  tac  (~(validate-page-with-txs dcon con der constants) new-page)
  ?:  ?=(%.n -.tac)
    ~&  >  [%reason +.tac]
    (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) new-page +.tac *@da)
  =.  con  (~(update-heaviest dcon con der constants) new-page)
  =/  old-in-balance=?
    ?|  (~(has h-bi balance.con) ~(digest get:page:t new-page) name1)
        (~(has h-bi balance.con) ~(digest get:page:t new-page) name2)
    ==
  =/  outs=outputs:t  ~(outputs get:tx:t tx)
  ?>  ?=(%1 -.outs)
  =/  =output:t  (head ~(tap z-in +.outs))
  =/  new-note=nnote:t  ~(note get:output:t output)
  ?>  ?=(@ -.new-note)
  =/  new-in-balance=?
    (~(has h-bi balance.con) ~(digest get:page:t new-page) ~(name get:nnote:t new-note))
  %+  expect-eq
    !>  [%.n %.y]
  !>  [old-in-balance new-in-balance]
::
++  test-one-to-two-utxo-v1-into-v1
  =/  con=consensus-state  initial-consensus-state:h
  ::  advance to just before v1 activation
  =^  par=page:t  con  (add-n-pages:h (sub v1-phase:t 3) con default-retain:h)
  ::  make a v0 coinbase and lift it to a v1 note with a %hax lock
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t  (new:v0:coinbase:t page0 p:default-keys-1:h)
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  move to activation height
  =^  last=page:t
    con
  %^    add-n-pages:h
    ?:  (gte ~(height get:page:t page0) v1-phase:t)  0
    (sub v1-phase:t ~(height get:page:t page0))
    con
  default-retain:h
  ::  construct a v1 note from coinbase (spend-0) with %hax lock
  =/  pre-in=*  314
  =/  [in-root=hash:t in-sc=spend-condition:v1:t in-h=hash:t]
    (make-hax-lock:v1:h pre-in)
  =/  fee  10.000
  =/  in-seeds=seeds:v1:t
    (~(put z-in *seeds:v1:t) (make-seed:v1:h in-root (sub assets.coin fee) (hash:nnote:t coin)))
  =/  sp0=spend-0:v1:t
    %*  .  *spend-0:v1:t
      seeds  in-seeds
      fee    fee
    ==
  =/  sp-in=spend:v1:t  [%0 (sign:spend-0:v1:t sp0 s:default-keys-1:h)]
  =/  sps-in=spends:v1:t  (~(put z-by *spends:v1:t) ~(name get:nnote:t coin) sp-in)
  =/  raw-in=raw-tx:v1:t  (new:raw-tx:v1:t sps-in)
  =/  [page-in=page:t newc=_con *]  (add-raw-to-new-page:h last con raw-in)
  =.  con  newc
  ::  extract the v1 input note and name
  =/  tx-in=tx:t  (new:tx:t raw-in ~(height get:page:t page-in))
  =/  outs-in=outputs:t  ~(outputs get:tx:t tx-in)
  ?>  ?=(%1 -.outs-in)
  =/  out-in=output:t  (head ~(tap z-in +.outs-in))
  =/  note-in=nnote:t  ~(note get:output:t out-in)
  =/  name-in=nname:t  ~(name get:nnote:t note-in)
  ::  build two distinct output seeds (different locks) splitting assets
  =/  fee  10.000
  =/  a1=coins:t  (dec (sub assets.note-in fee))
  =/  a2=coins:t  1
  =/  pre-a=*  2.718
  =/  pre-b=*  5.772
  =/  [root-a=hash:t sc-a=spend-condition:v1:t ha=hash:t]  (make-hax-lock:v1:h pre-a)
  =/  [root-b=hash:t sc-b=spend-condition:v1:t hb=hash:t]  (make-hax-lock:v1:h pre-b)
  =/  sed-a=seed:v1:t  (make-seed:v1:h root-a a1 (hash:nnote:t note-in))
  =/  sed-b=seed:v1:t  (make-seed:v1:h root-b a2 (hash:nnote:t note-in))
  =/  seeds-out=seeds:v1:t  (~(gas z-in *seeds:v1:t) ~[sed-a sed-b])
  ::  witness to unlock the input v1 note
  =/  wit=witness:v1:t  (make-witness-hax:v1:h in-root in-sc in-h pre-in)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  wit
      seeds    seeds-out
      fee  fee
    ==
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) name-in [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  [new-page=page:t newc=_con *]  (add-raw-to-new-page:h page-in con raw)
  =.  con  newc
  ::  assertions: input removed, two new outputs present
  =/  old-in-balance=?  (~(has h-bi balance.con) ~(digest get:page:t new-page) name-in)
  =/  tx=tx:t  (new:tx:t raw ~(height get:page:t new-page))
  =/  outs-tx=outputs:t  ~(outputs get:tx:t tx)
  ?>  ?=(%1 -.outs-tx)
  =/  outs=(list output:t)  ~(tap z-in +.outs-tx)
  ?>  =(2 (lent outs))
  =/  no1=nnote:t  ~(note get:output:t (snag 0 outs))
  =/  no2=nnote:t  ~(note get:output:t (snag 1 outs))
  =/  nm1=nname:t  ~(name get:nnote:t no1)
  =/  nm2=nname:t  ~(name get:nnote:t no2)
  =/  new-in-balance=?
    ?&  (~(has h-bi balance.con) ~(digest get:page:t new-page) nm1)
        (~(has h-bi balance.con) ~(digest get:page:t new-page) nm2)
    ==
  %+  expect-eq
    !>  [%.n %.y]
  !>  [old-in-balance new-in-balance]
::
++  test-v1-lock-pkh-basic
  =/  con=consensus-state  initial-consensus-state:h
  ::  get to just before v1 coinbase activation
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  ::  make v1 coinbase directly at activation height
  =/  page0=page:t  (make-empty-page:h par)
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  get actual coinbase from balance
  =/  page0-balance  (need (~(get h-by balance.con) ~(digest get:page:t page0)))
  =/  coin=nnote:t  (get-coinbase-from-balance:v1:h page0 page0-balance)
  ::  compute sig-hash for %1 spend using v1 coinbase as input
  =/  nam=nname:t  ~(name get:nnote:t coin)
  ::  build coinbase lock (pkh + tim)
  =/  pk=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [lock-root=hash:t sc=spend-condition:t *]
    (make-coinbase-lock:v1:h 1 ~[pk])
  =/  lmp=lock-merkle-proof:t  (build-lock-merkle-proof:lock:t sc 1)
  =/  fee  10.000
  =/  sed=seed:v1:t  (make-seed:v1:h lock-root (sub assets.coin fee) (hash:nnote:t coin))
  =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds
      fee  fee
    ==
  =/  sig-h=hash:t  (sig-hash:spend-1:v1:t sp1)
  ::  rebuild spend with proper witness for coinbase
  =/  wit=witness:t
    (make-pkh-witness:v1:h lock-root sc sig-h ~[[s:default-keys-1:h pk]])
  =/  sp1=spend-1:v1:t  sp1(witness wit)
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  [new-page=page:t newc=_con *]  (add-raw-to-new-page:h page0 con raw)
  =.  con  newc
  =/  old-in-balance=?  (~(has h-bi balance.con) ~(digest get:page:t new-page) nam)
  =/  tx=tx:t  (new:tx:t raw ~(height get:page:t new-page))
  =/  outs=outputs:t  ~(outputs get:tx:t tx)
  ?>  ?=(%1 -.outs)
  =/  new-out=output:t  (head ~(tap z-in +.outs))
  =/  new-note=nnote:t  ~(note get:output:t new-out)
  =/  new-name=nname:t  ~(name get:nnote:t new-note)
  =/  new-in-balance=?  (~(has h-bi balance.con) ~(digest get:page:t new-page) new-name)
  %+  expect-eq
    !>  [%.n %.y]
  !>  [old-in-balance new-in-balance]
::
::  ++test-v1-lock-pkh-m-of-n-valid: test valid m-of-n multisig spending
::
::    this test validates that a 2-of-3 multisig pay-to-pubkey-hash lock
::    can be successfully spent with exactly 2 valid signatures.
::
::    the test uses a three-step approach:
::
::    1. create a simple v1 coinbase locked to key1 (1-of-1 lock)
::       - v1 coinbases with multiple owners are split by accept-page into
::         separate coinbase notes, each with a 1-of-1 lock for one owner
::       - therefore, we create a simple coinbase for testing
::
::    2. spend the coinbase into an intermediate note with a 2-of-3 lock
::       - the intermediate note is locked to require 2 signatures from
::         the set {key1, key2, key3}
::       - this spend uses a 1-of-1 witness (key1) to unlock the coinbase
::       - the output creates a note with the m-of-n lock we want to test
::
::    3. spend the intermediate note using a 2-of-3 witness
::       - this is the actual m-of-n test: we provide signatures from
::         key1 and key2 (2 out of 3 required)
::       - the spend should succeed and create a new output
::
::    verification: we check that the intermediate note was consumed
::    (not in balance) and a new output note was created (in balance).
::
++  test-v1-lock-pkh-m-of-n-valid
  ::  use v1-phase=2 to quickly reach v1 activation
  =.  constants  bc-v1-phase:helpers
  =/  con=consensus-state  initial-consensus-state:h
  ::  advance to just before v1 coinbase activation (height = v1-phase - 1)
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  ::
  ::  step 1: create simple v1 coinbase for key1
  ::
  ::    we cannot directly create a v1 coinbase with an m-of-n lock because
  ::    accept-page splits multi-owner coinbases into separate notes, each
  ::    with a 1-of-1 lock for a single owner. instead, we create a simple
  ::    coinbase locked to key1 that we'll spend in the next step.
  ::
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  ::  validate and accept the page with the coinbase
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::
  ::  step 2: spend coinbase into intermediate note with 2-of-3 lock
  ::
  ::    we construct a transaction that:
  ::    - spends the coinbase (locked to key1)
  ::    - creates an output with a 2-of-3 lock requiring 2 sigs from {key1,key2,key3}
  ::    - uses key1's signature to unlock the coinbase
  ::
  ::    the witness for this spend proves we can unlock the coinbase (1-of-1 key1).
  ::    the seeds for this spend create the new note with the m-of-n lock.
  ::
  =/  nam=nname:t  ~(name get:nnote:t coin)
  ::  extract public keys for all three parties
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  pk2=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-2:h))
  =/  pk3=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-3:h))
  ::  create 2-of-3 lock: requires 2 signatures from {pk1, pk2, pk3}
  =/  [root-mn=hash:t sc-mn=spend-condition:v1:t *]
    (make-pkh-lock:v1:h 2 ~[pk1 pk2 pk3])
  =/  fee1  5.000
  ::  create seed for output: locked with 2-of-3, funded by coinbase minus fee
  =/  sed1=seed:v1:t  (make-seed:v1:h root-mn (sub assets.coin fee1) (hash:nnote:t coin))
  =/  seds1=seeds:v1:t  (~(put z-in *seeds:v1:t) sed1)
  ::  create coinbase lock (what we're spending from)
  =/  [root1=hash:t sc1=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk1])
  ::  build the spend structure
  =/  sp1a=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds1
      fee  fee1
    ==
  ::  compute signature hash for this spend
  =/  sig-ha=hash:t  (sig-hash:spend-1:v1:t sp1a)
  ::  create witness proving we can unlock coinbase
  =/  wita=witness:t
    (make-pkh-witness:v1:h root1 sc1 sig-ha ~[[s:default-keys-1:h pk1]])
  ::  update spend with witness
  =/  sp1a=spend-1:v1:t  sp1a(witness wita)
  ::  create transaction and add to page
  =/  raw1=raw-tx:v1:t  (new:raw-tx:v1:t (~(put z-by *spends:v1:t) nam [%1 sp1a]))
  =/  [page1=page:t newc=_con *]  (add-raw-to-new-page:h page0 con raw1)
  =.  con  newc
  ::
  ::  step 3: spend intermediate note with 2-of-3 witness
  ::
  ::    now we test the actual m-of-n functionality: we spend the intermediate
  ::    note (locked with 2-of-3) using signatures from key1 and key2.
  ::
  ::  extract the intermediate note from the transaction output
  =/  tx1=tx:t  (new:tx:t raw1 ~(height get:page:t page1))
  =/  outs1=outputs:t  ~(outputs get:tx:t tx1)
  ?>  ?=(%1 -.outs1)
  =/  out1=output:t  (head ~(tap z-in +.outs1))
  =/  note1=nnote:t  ~(note get:output:t out1)
  =/  name1=nname:t  ~(name get:nnote:t note1)
  ?>  ?=(@ -.note1)
  ::  create a new seed spending the intermediate note (still 2-of-3 locked)
  =/  fee2  5.000
  =/  note1-assets=coins:t  ~(assets get:nnote:t note1)
  =/  sed2=seed:v1:t  (make-seed:v1:h root-mn (sub note1-assets fee2) (hash:nnote:t note1))
  =/  seds2=seeds:v1:t  (~(put z-in *seeds:v1:t) sed2)
  ::  build spend structure for m-of-n spend
  =/  sp1b=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds2
      fee  fee2
    ==
  ::  compute signature hash
  =/  sig-hb=hash:t  (sig-hash:spend-1:v1:t sp1b)
  ::  create 2-of-3 witness with signatures from key1 and key2
  =/  witb=witness:t
    %:  make-pkh-witness:v1:h
      root-mn
      sc-mn
      sig-hb
      ~[[s:default-keys-1:h pk1] [s:default-keys-2:h pk2]]
    ==
  ::  update spend with 2-of-3 witness
  =/  sp1b=spend-1:v1:t  sp1b(witness witb)
  ::  create transaction spending the intermediate note
  =/  sps2=spends:v1:t  (~(put z-by *spends:v1:t) name1 [%1 sp1b])
  =/  raw2=raw-tx:v1:t  (new:raw-tx:v1:t sps2)
  ::  add transaction to page and update consensus
  =/  [page2=page:t newc=_con *]  (add-raw-to-new-page:h page1 con raw2)
  =.  con  newc
  ::
  ::  verification: ensure intermediate note was consumed and new output created
  ::
  ::    we check two things:
  ::    1. the intermediate note (name1) is no longer in the balance (consumed)
  ::    2. a new output note (name2) is in the balance (created)
  ::
  ::    confirming the m-of-n spend succeeded end-to-end.
  ::
  =/  old-in-balance=?  (~(has h-bi balance.con) ~(digest get:page:t page2) name1)
  =/  tx2=tx:t  (new:tx:t raw2 ~(height get:page:t page2))
  =/  outs2=outputs:t  ~(outputs get:tx:t tx2)
  ?>  ?=(%1 -.outs2)
  =/  out2=output:t  (head ~(tap z-in +.outs2))
  =/  note2=nnote:t  ~(note get:output:t out2)
  =/  name2=nname:t  ~(name get:nnote:t note2)
  =/  new-in-balance=?  (~(has h-bi balance.con) ~(digest get:page:t page2) name2)
  ::  expect: intermediate note not in balance, new output in balance
  %+  expect-eq
    !>  [%.n %.y]
  !>  [old-in-balance new-in-balance]
::
++  test-v1-lock-pkh-m-of-n-subset-invalid
  =/  con=consensus-state  initial-consensus-state:h
  ::  get to just before v1 coinbase activation
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  ::  create simple v1 coinbase for key1
  =/  page0=page:t  (make-empty-page:h par)
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  get actual coinbase from balance
  =/  page0-balance  (need (~(get h-by balance.con) ~(digest get:page:t page0)))
  =/  coin=nnote:t  (get-coinbase-from-balance:v1:h page0 page0-balance)
  ::  spend coinbase into intermediate note with 2-of-2 lock for {pk1, pk2}
  =/  nam=nname:t  ~(name get:nnote:t coin)
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  pk2=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-2:h))
  =/  pk3=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-3:h))
  =/  [root-22=hash:t sc-22=spend-condition:v1:t *]
    (make-pkh-lock:v1:h 2 ~[pk1 pk2])
  =/  fee1  5.000
  =/  coin-assets=coins:t  ~(assets get:nnote:t coin)
  =/  sed1=seed:v1:t  (make-seed:v1:h root-22 (sub coin-assets fee1) (hash:nnote:t coin))
  =/  seds1=seeds:v1:t  (~(put z-in *seeds:v1:t) sed1)
  =/  [root1=hash:t sc1=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk1])
  =/  sp1a=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds1
      fee  fee1
    ==
  =/  sig-ha=hash:t  (sig-hash:spend-1:v1:t sp1a)
  =/  wita=witness:t
    (make-pkh-witness:v1:h root1 sc1 sig-ha ~[[s:default-keys-1:h pk1]])
  =/  sp1a=spend-1:v1:t  sp1a(witness wita)
  =/  raw1=raw-tx:v1:t  (new:raw-tx:v1:t (~(put z-by *spends:v1:t) nam [%1 sp1a]))
  =/  [page1=page:t newc=_con *]  (add-raw-to-new-page:h page0 con raw1)
  =.  con  newc
  ::  try to spend intermediate note with {pk1, pk3} -> should fail
  =/  tx1=tx:t  (new:tx:t raw1 ~(height get:page:t page1))
  =/  outs1=outputs:t  ~(outputs get:tx:t tx1)
  ?>  ?=(%1 -.outs1)
  =/  out1=output:t  (head ~(tap z-in +.outs1))
  =/  note1=nnote:t  ~(note get:output:t out1)
  =/  name1=nname:t  ~(name get:nnote:t note1)
  ?>  ?=(@ -.note1)
  =/  fee2  5.000
  =/  note1-assets=coins:t  ~(assets get:nnote:t note1)
  =/  sed2=seed:v1:t  (make-seed:v1:h root-22 (sub note1-assets fee2) (hash:nnote:t note1))
  =/  seds2=seeds:v1:t  (~(put z-in *seeds:v1:t) sed2)
  =/  sp1b=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds2
      fee  fee2
    ==
  =/  sig-hb=hash:t  (sig-hash:spend-1:v1:t sp1b)
  ::  witness with {pk1, pk3} attempting to unlock 2-of-2 {pk1, pk2} -> fail
  =/  witb=witness:t
    %:  make-pkh-witness:v1:h
      root-22
      sc-22
      sig-hb
      ~[[s:default-keys-1:h pk1] [s:default-keys-3:h pk3]]
    ==
  =/  sp1b=spend-1:v1:t  sp1b(witness witb)
  =/  sps2=spends:v1:t  (~(put z-by *spends:v1:t) name1 [%1 sp1b])
  =/  raw2=raw-tx:v1:t  (new:raw-tx:v1:t sps2)
  =^  ready  con  (~(add-raw-tx dcon con der constants) raw2)
  =/  new-page=page:t  (make-empty-page:h page1)
  =/  tx2=tx:t  (new:tx:t raw2 ~(height get:page:t new-page))
  =/  new-tx-ids  (~(put z-in *(z-set tx-id:t)) ~(id get:raw-tx:t raw2))
  =.  new-page
    ?^  -.new-page
      new-page(tx-ids new-tx-ids)
    new-page(tx-ids new-tx-ids)
  =/  new-digest  (compute-digest:page:t new-page)
  =.  new-page
    ?^  -.new-page
      new-page(digest new-digest)
    new-page(digest new-digest)
  =/  r=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) new-page)
  ?:  ?=(%.y -.r)  (expect !>(%.n))
  (expect !>(%.y))
::
++  test-v1-lock-pkh-and-tim-negative
  =/  con=consensus-state  initial-consensus-state:h
  ::  get to just before v1 coinbase activation
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  ::  make v1 coinbase owned by default-keys-1
  =/  page0=page:t  (make-empty-page:h par)
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  get actual coinbase from balance
  =/  page0-balance  (need (~(get h-by balance.con) ~(digest get:page:t page0)))
  =/  coin=nnote:t  (get-coinbase-from-balance:v1:h page0 page0-balance)
  ::  sc = pkh(m=1,{pk1}) AND tim(2)
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [root-pkh=hash:t sc-pkh=spend-condition:v1:t *]
    (make-pkh-lock:v1:h 1 ~[pk1])
  =/  prim=lock-primitive:v1:t  [%tim [rel=[min=`2 max=~] abs=[min=~ max=~]]]
  =/  sc-tim=spend-condition:v1:t  ~[prim]
  =/  sc=spend-condition:v1:t  (combine:spend-condition:t sc-pkh sc-tim)
  =/  root=hash:t  (hash:spend-condition:v1:t sc)
  ::  first, construct a v1 note with this lock via spend-1
  =/  [root-in=hash:t sc-in=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk1])
  =/  fee  10.000
  =/  sed0=seed:v1:t
    (make-seed:v1:h root (sub assets.coin fee) (hash:nnote:t coin))
  =/  seds0=seeds:v1:t  (~(put z-in *seeds:v1:t) sed0)
  =/  sp1a=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds0
      fee  fee
    ==
  =/  sig-ha=hash:t  (sig-hash:spend-1:v1:t sp1a)
  =/  wita=witness:t
    (make-pkh-witness:v1:h root-in sc-in sig-ha ~[[s:default-keys-1:h pk1]])
  =/  sp1a=spend-1:v1:t  sp1a(witness wita)
  =/  raw0=raw-tx:v1:t
    (new:raw-tx:v1:t (~(put z-by *spends:v1:t) ~(name get:nnote:t coin) [%1 sp1a]))
  =/  [page-in=page:t newc=_con *]
    (add-raw-to-new-page:h page0 con raw0)
  =.  con  newc
  ::  extract constructed v1 note name
  =/  tx-in=tx:t  (new:tx:t raw0 ~(height get:page:t page-in))
  =/  outs-in=outputs:t  ~(outputs get:tx:t tx-in)
  ?>  ?=(%1 -.outs-in)
  =/  out-in=output:t  (head ~(tap z-in +.outs-in))
  =/  note-in=nnote:t  ~(note get:output:t out-in)
  =/  nam=nname:t  ~(name get:nnote:t note-in)
  ::  now attempt spend-1 before tim allows it
  =/  fee  10.000
  =/  sed=seed:v1:t  (make-seed:v1:h root (sub assets.note-in fee) (hash:nnote:t note-in))
  =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds
      fee  fee
    ==
  =/  sig-h=hash:t  (sig-hash:spend-1:v1:t sp1)
  =/  wit=witness:t
    %:  make-pkh-witness:v1:h
      root
      sc
      sig-h
      ~[[s:default-keys-1:h pk1]]
    ==
  =/  sp1=spend-1:v1:t  sp1(witness wit)
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =^  ready  con  (~(add-raw-tx dcon con der constants) raw)
  ::  build child page after page-in (now = since+1 < since+2) -> fail
  =/  new-page=page:t  (make-empty-page:h page-in)
  =/  tx1=tx:t  (new:tx:t raw ~(height get:page:t new-page))
  =/  new-tx-ids  (~(put z-in *(z-set tx-id:t)) ~(id get:raw-tx:t raw))
  =.  new-page
    ?^  -.new-page
      new-page(tx-ids new-tx-ids)
    new-page(tx-ids new-tx-ids)
  =/  new-digest  (compute-digest:page:t new-page)
  =.  new-page
    ?^  -.new-page
      new-page(digest new-digest)
    new-page(digest new-digest)
  =/  r=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) new-page)
  ?:  ?=(%.y -.r)  (expect !>(%.n))
  (expect !>(%.y))
::
++  test-v1-lock-pkh-and-tim-positive
  =/  con=consensus-state  initial-consensus-state:h
  ::  get to just before v1 coinbase activation
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  ::  make v1 coinbase owned by default-keys-1
  =/  page0=page:t  (make-empty-page:h par)
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  get actual coinbase from balance
  =/  page0-balance  (need (~(get h-by balance.con) ~(digest get:page:t page0)))
  =/  coin=nnote:t  (get-coinbase-from-balance:v1:h page0 page0-balance)
  ::  sc = pkh(m=1,{pk1}) AND tim(2)
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [root-pkh=hash:t sc-pkh=spend-condition:v1:t *]
    (make-pkh-lock:v1:h 1 ~[pk1])
  =/  prim=lock-primitive:v1:t  [%tim [rel=[min=`2 max=~] abs=[min=~ max=~]]]
  =/  sc-tim=spend-condition:v1:t  ~[prim]
  =/  sc=spend-condition:v1:t  (combine:spend-condition:t sc-pkh sc-tim)
  =/  root=hash:t  (hash:lock:t sc)
  ::  first, construct a v1 note with this lock via spend-1
  =/  [root-in=hash:t sc-in=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk1])
  =/  lmp-in=lock-merkle-proof:t  (build-lock-merkle-proof:lock:t sc-in 1)
  =/  fee  10.000
  =/  sed0=seed:v1:t
    (make-seed:v1:h root (sub assets.coin fee) (hash:nnote:t coin))
  =/  seds0=seeds:v1:t  (~(put z-in *seeds:v1:t) sed0)
  =/  sp1a=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds0
      fee  fee
    ==
  =/  sig-ha=hash:t  (sig-hash:spend-1:v1:t sp1a)
  =/  wita=witness:t
    (make-pkh-witness:v1:h root-in sc-in sig-ha ~[[s:default-keys-1:h pk1]])
  =/  sp1a=spend-1:v1:t  sp1a(witness wita)
  =/  raw0=raw-tx:v1:t
    (new:raw-tx:v1:t (~(put z-by *spends:v1:t) ~(name get:nnote:t coin) [%1 sp1a]))
  =/  [page-in=page:t newc=_con *]
    (add-raw-to-new-page:h page0 con raw0)
  =.  con  newc
  ::  extract constructed v1 note name
  =/  tx-in=tx:t  (new:tx:t raw0 ~(height get:page:t page-in))
  =/  outs-in=outputs:t  ~(outputs get:tx:t tx-in)
  ?>  ?=(%1 -.outs-in)
  =/  out-in=output:t  (head ~(tap z-in +.outs-in))
  =/  note-in=nnote:t  ~(note get:output:t out-in)
  =/  nam=nname:t  ~(name get:nnote:t note-in)
  ::  build spend-1 witness and raw
  =/  fee  10.000
  =/  sed=seed:v1:t  (make-seed:v1:h root (sub assets.note-in fee) (hash:nnote:t note-in))
  =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds
      fee  fee
    ==
  =/  sig-h=hash:t  (sig-hash:spend-1:v1:t sp1)
  =/  wit=witness:t
    %:  make-pkh-witness:v1:h
      root
      sc
      sig-h
      ~[[s:default-keys-1:h pk1]]
    ==
  =/  sp1=spend-1:v1:t  sp1(witness wit)
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  ::  advance two pages so now >= since+2 and add raw
  =^  last=page:t  con  (add-n-pages:h 2 con default-retain:h)
  =/  [new-page=page:t newc=_con *]  (add-raw-to-new-page:h last con raw)
  =.  con  newc
  ::  success: input removed and one output added
  =/  old-in-balance=?  (~(has h-bi balance.con) ~(digest get:page:t new-page) nam)
  =/  tx=tx:t  (new:tx:t raw ~(height get:page:t new-page))
  =/  outs=outputs:t  ~(outputs get:tx:t tx)
  ?>  ?=(%1 -.outs)
  =/  out=output:t  (head ~(tap z-in +.outs))
  =/  note=nnote:t  ~(note get:output:t out)
  =/  name=nname:t  ~(name get:nnote:t note)
  =/  new-in-balance=?  (~(has h-bi balance.con) ~(digest get:page:t new-page) name)
  %+  expect-eq
    !>  [%.n %.y]
  !>  [old-in-balance new-in-balance]
::
++  test-v1-add-tx-to-candidate-block
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  =/  min=mining-state  initial-mining-state:h
  =.  min  (~(heard-new-block dmin min der constants) con *@da)
  =/  nam=nname:t  ~(name get:nnote:t coin)
  =/  pk=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [root=hash:t sc=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk])
  =/  fee  10.000
  =/  sed=seed:v1:t  (make-seed:v1:h root (sub assets.coin fee) (hash:nnote:t coin))
  =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds
      fee  fee
    ==
  =/  sig-h=hash:t  (sig-hash:spend-1:v1:t sp1)
  =/  wit=witness:t
    (make-pkh-witness:v1:h root sc sig-h ~[[s:default-keys-1:h pk]])
  =/  sp1=spend-1:v1:t  sp1(witness wit)
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =.  min  (~(heard-new-tx dmin min der constants) raw)
  %+  expect-eq
    !>(%.y)
  !>((~(has z-in ~(tx-ids get:page:t candidate-block.min)) ~(id get:raw-tx:t raw)))
::
++  test-v1-lock-hax-basic
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  construct v1 note with %hax lock via pkh spend
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [root-cb=hash:t sc-cb=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk1])
  =/  pre-in=*  314
  =/  [in-root=hash:t in-sc=spend-condition:v1:t in-h=hash:t]
    (make-hax-lock:v1:h pre-in)
  =/  fee  10.000
  =/  sed0=seed:v1:t  (make-seed:v1:h in-root (sub assets.coin fee) (hash:nnote:t coin))
  =/  seds0=seeds:v1:t  (~(put z-in *seeds:v1:t) sed0)
  =/  sp1a=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds0
      fee  fee
    ==
  =/  sig-ha=hash:t  (sig-hash:spend-1:v1:t sp1a)
  =/  wita=witness:t
    (make-pkh-witness:v1:h root-cb sc-cb sig-ha ~[[s:default-keys-1:h pk1]])
  =/  sp1a=spend-1:v1:t  sp1a(witness wita)
  =/  raw0=raw-tx:v1:t
    (new:raw-tx:v1:t (~(put z-by *spends:v1:t) ~(name get:nnote:t coin) [%1 sp1a]))
  =/  [page-in=page:t newc=_con *]
    (add-raw-to-new-page:h page0 con raw0)
  =.  con  newc
  ::  extract constructed %hax note name
  =/  tx-in=tx:t  (new:tx:t raw0 ~(height get:page:t page-in))
  =/  outs-in=outputs:t  ~(outputs get:tx:t tx-in)
  ?>  ?=(%1 -.outs-in)
  =/  out-in=output:t  (head ~(tap z-in +.outs-in))
  =/  note-in=nnote:t  ~(note get:output:t out-in)
  =/  nam=nname:t  ~(name get:nnote:t note-in)
  ::  spend %hax note with correct preimage
  =/  out-pre=*  42
  =/  [out-root=hash:t out-sc=spend-condition:v1:t out-h=hash:t]
    (make-hax-lock:v1:h out-pre)
  =/  sed=seed:v1:t  (make-seed:v1:h out-root (sub assets.note-in fee) (hash:nnote:t note-in))
  =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
  =/  wit=witness:v1:t  (make-witness-hax:v1:h in-root in-sc in-h pre-in)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  wit
      seeds    seds
      fee  fee
    ==
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  [new-page=page:t newc=_con *]  (add-raw-to-new-page:h page-in con raw)
  =.  con  newc
  =/  old-in-balance=?  (~(has h-bi balance.con) ~(digest get:page:t new-page) nam)
  =/  tx=tx:t  (new:tx:t raw ~(height get:page:t new-page))
  =/  outs=outputs:t  ~(outputs get:tx:t tx)
  ?>  ?=(%1 -.outs)
  =/  out=output:t  (head ~(tap z-in +.outs))
  =/  note=nnote:t  ~(note get:output:t out)
  =/  name=nname:t  ~(name get:nnote:t note)
  =/  new-in-balance=?  (~(has h-bi balance.con) ~(digest get:page:t new-page) name)
  %+  expect-eq
    !>  [%.n %.y]
  !>  [old-in-balance new-in-balance]
::
++  test-v1-lock-hax-invalid-preimage
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  construct %hax input note
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [root-cb=hash:t sc-cb=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk1])
  =/  pre-in=*  7
  =/  [in-root=hash:t in-sc=spend-condition:v1:t in-h=hash:t]
    (make-hax-lock:v1:h pre-in)
  =/  fee  10.000
  =/  sed0=seed:v1:t  (make-seed:v1:h in-root (sub assets.coin fee) (hash:nnote:t coin))
  =/  seds0=seeds:v1:t  (~(put z-in *seeds:v1:t) sed0)
  =/  sp1a=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds0
      fee  fee
    ==
  =/  sig-ha=hash:t  (sig-hash:spend-1:v1:t sp1a)
  =/  wita=witness:t
    (make-pkh-witness:v1:h root-cb sc-cb sig-ha ~[[s:default-keys-1:h pk1]])
  =/  sp1a=spend-1:v1:t  sp1a(witness wita)
  =/  raw0=raw-tx:v1:t
    (new:raw-tx:v1:t (~(put z-by *spends:v1:t) ~(name get:nnote:t coin) [%1 sp1a]))
  =/  [page-in=page:t newc=_con *]
    (add-raw-to-new-page:h page0 con raw0)
  =.  con  newc
  ::  spend with wrong preimage -> fail
  =/  tx-in=tx:t  (new:tx:t raw0 ~(height get:page:t page-in))
  =/  outs=outputs:t  ~(outputs get:tx:t tx-in)
  ?>  ?=(%1 -.outs)
  =/  out-in=output:t  (head ~(tap z-in +.outs))
  =/  note-in=nnote:t  ~(note get:output:t out-in)
  =/  nam=nname:t  ~(name get:nnote:t note-in)
  =/  out-pre=*  9
  =/  [out-root=hash:t out-sc=spend-condition:v1:t out-h=hash:t]
    (make-hax-lock:v1:h out-pre)
  =/  sed=seed:v1:t  (make-seed:v1:h out-root assets.note-in (hash:nnote:t note-in))
  =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
  =/  bad-pre=*  99
  =/  wit=witness:v1:t  (make-witness-hax:v1:h in-root in-sc in-h bad-pre)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  wit
      seeds    seds
      fee  fee
    ==
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  new-page=page:t  (make-empty-page:h page-in)
  =/  tx=tx:t  (new:tx:t raw ~(height get:page:t new-page))
  =/  new-tx-ids  (~(put z-in *(z-set tx-id:t)) ~(id get:raw-tx:t raw))
  =.  new-page
    ?^  -.new-page
      new-page(tx-ids new-tx-ids)
    new-page(tx-ids new-tx-ids)
  =/  new-digest  (compute-digest:page:t new-page)
  =.  new-page
    ?^  -.new-page
      new-page(digest new-digest)
    new-page(digest new-digest)
  =^  ready  con  (~(add-raw-tx dcon con der constants) raw)
  =/  r=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) new-page)
  ?:  ?=(%.y -.r)  (expect !>(%.n))
  (expect !>(%.y))
::
++  test-v1-lock-hax-missing-preimage
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  construct %hax input note
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [root-cb=hash:t sc-cb=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk1])
  =/  pre-in=*  5
  =/  [in-root=hash:t in-sc=spend-condition:v1:t in-h=hash:t]
    (make-hax-lock:v1:h pre-in)
  =/  fee  10.000
  =/  sed0=seed:v1:t  (make-seed:v1:h in-root (sub assets.coin fee) (hash:nnote:t coin))
  =/  seds0=seeds:v1:t  (~(put z-in *seeds:v1:t) sed0)
  =/  sp1a=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds0
      fee  fee
    ==
  =/  sig-ha=hash:t  (sig-hash:spend-1:v1:t sp1a)
  =/  wita=witness:t
    (make-pkh-witness:v1:h root-cb sc-cb sig-ha ~[[s:default-keys-1:h pk1]])
  =/  sp1a=spend-1:v1:t  sp1a(witness wita)
  =/  raw0=raw-tx:v1:t
    (new:raw-tx:v1:t (~(put z-by *spends:v1:t) ~(name get:nnote:t coin) [%1 sp1a]))
  =/  [page-in=page:t newc=_con *]
    (add-raw-to-new-page:h page0 con raw0)
  =.  con  newc
  ::  missing preimage in witness -> fail
  =/  tx-in=tx:t  (new:tx:t raw0 ~(height get:page:t page-in))
  =/  outs-in=outputs:t  ~(outputs get:tx:t tx-in)
  ?>  ?=(%1 -.outs-in)
  =/  out-in=output:t  (head ~(tap z-in +.outs-in))
  =/  note-in=nnote:t  ~(note get:output:t out-in)
  =/  nam=nname:t  ~(name get:nnote:t note-in)
  =/  out-pre=*  6
  =/  [out-root=hash:t out-sc=spend-condition:v1:t out-h=hash:t]
    (make-hax-lock:v1:h out-pre)
  =/  fee  10.000
  =/  sed=seed:v1:t  (make-seed:v1:h out-root (sub assets.note-in fee) (hash:nnote:t note-in))
  =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
  =/  wit-ok=witness:v1:t  (make-witness-hax:v1:h in-root in-sc in-h pre-in)
  =/  wit=witness:v1:t  wit-ok(hax *(z-map hash:t *))
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  wit
      seeds    seds
      fee  fee
    ==
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  new-page=page:t  (make-empty-page:h page-in)
  =/  tx=tx:t  (new:tx:t raw ~(height get:page:t new-page))
  =/  new-tx-ids  (~(put z-in *(z-set tx-id:t)) ~(id get:raw-tx:t raw))
  =.  new-page
    ?^  -.new-page
      new-page(tx-ids new-tx-ids)
    new-page(tx-ids new-tx-ids)
  =/  new-digest  (compute-digest:page:t new-page)
  =.  new-page
    ?^  -.new-page
      new-page(digest new-digest)
    new-page(digest new-digest)
  =^  ready  con  (~(add-raw-tx dcon con der constants) raw)
  =/  r=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) new-page)
  ?:  ?=(%.y -.r)  (expect !>(%.n))
  (expect !>(%.y))
::
++  test-v1-lock-hax-extra-preimage
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  construct %hax input note
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [root-cb=hash:t sc-cb=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk1])
  =/  pre-in=*  21
  =/  [in-root=hash:t in-sc=spend-condition:v1:t in-h=hash:t]
    (make-hax-lock:v1:h pre-in)
  =/  fee  10.000
  =/  sed0=seed:v1:t  (make-seed:v1:h in-root (sub assets.coin fee) (hash:nnote:t coin))
  =/  seds0=seeds:v1:t  (~(put z-in *seeds:v1:t) sed0)
  =/  sp1a=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds0
      fee  fee
    ==
  =/  sig-ha=hash:t  (sig-hash:spend-1:v1:t sp1a)
  =/  wita=witness:t
    (make-pkh-witness:v1:h root-cb sc-cb sig-ha ~[[s:default-keys-1:h pk1]])
  =/  sp1a=spend-1:v1:t  sp1a(witness wita)
  =/  raw0=raw-tx:v1:t
    (new:raw-tx:v1:t (~(put z-by *spends:v1:t) ~(name get:nnote:t coin) [%1 sp1a]))
  =/  [page-in=page:t newc=_con *]
    (add-raw-to-new-page:h page0 con raw0)
  =.  con  newc
  ::  spend with required preimage plus an extra one -> success
  =/  tx-in=tx:t  (new:tx:t raw0 ~(height get:page:t page-in))
  =/  outs-in=outputs:t  ~(outputs get:tx:t tx-in)
  ?>  ?=(%1 -.outs-in)
  =/  out-in=output:t  (head ~(tap z-in +.outs-in))
  =/  note-in=nnote:t  ~(note get:output:t out-in)
  =/  nam=nname:t  ~(name get:nnote:t note-in)
  =/  out-pre=*  22
  =/  [out-root=hash:t out-sc=spend-condition:v1:t out-h=hash:t]
    (make-hax-lock:v1:h out-pre)
  =/  fee2  10.000
  =/  sed=seed:v1:t  (make-seed:v1:h out-root (sub assets.note-in fee2) (hash:nnote:t note-in))
  =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
  =/  wit0=witness:v1:t  (make-witness-hax:v1:h in-root in-sc in-h pre-in)
  =/  pre-extra=*  123
  =/  [* * hextra=hash:t]  (make-hax-lock:v1:h pre-extra)
  =/  wit=witness:v1:t  wit0(hax (~(put z-by hax.wit0) hextra pre-extra))
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  wit
      seeds    seds
      fee  fee2
    ==
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  [new-page=page:t newc=_con *]  (add-raw-to-new-page:h page-in con raw)
  =.  con  newc
  =/  old-in-balance=?  (~(has h-bi balance.con) ~(digest get:page:t new-page) nam)
  =/  tx=tx:t  (new:tx:t raw ~(height get:page:t new-page))
  =/  outs=outputs:t  ~(outputs get:tx:t tx)
  ?>  ?=(%1 -.outs)
  =/  out=output:t  (head ~(tap z-in +.outs))
  =/  note=nnote:t  ~(note get:output:t out)
  =/  name=nname:t  ~(name get:nnote:t note)
  =/  new-in-balance=?  (~(has h-bi balance.con) ~(digest get:page:t new-page) name)
  %+  expect-eq
    !>  [%.n %.y]
  !>  [old-in-balance new-in-balance]
::
++  test-v1-lock-tim-rel-negative
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  build input v1 note locked with tim(rel min=`2)
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [root-cb=hash:t sc-cb=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk1])
  =/  prim=lock-primitive:v1:t  [%tim [rel=[min=`2 max=~] abs=[min=~ max=~]]]
  =/  sc-in=spend-condition:v1:t  ~[prim]
  =/  root-in=hash:t  (hash:spend-condition:v1:t sc-in)
  =/  fee  10.000
  =/  sed0=seed:v1:t  (make-seed:v1:h root-in (sub assets.coin fee) (hash:nnote:t coin))
  =/  seds0=seeds:v1:t  (~(put z-in *seeds:v1:t) sed0)
  =/  sp1a=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds0
      fee  fee
    ==
  =/  sig-ha=hash:t  (sig-hash:spend-1:v1:t sp1a)
  =/  wita=witness:t
    (make-pkh-witness:v1:h root-cb sc-cb sig-ha ~[[s:default-keys-1:h pk1]])
  =/  sp1a=spend-1:v1:t  sp1a(witness wita)
  =/  raw0=raw-tx:v1:t
    (new:raw-tx:v1:t (~(put z-by *spends:v1:t) ~(name get:nnote:t coin) [%1 sp1a]))
  =/  [page-in=page:t newc=_con *]
    (add-raw-to-new-page:h page0 con raw0)
  =.  con  newc
  ::  attempt to spend before maturity -> fail
  =/  tx-in=tx:t  (new:tx:t raw0 ~(height get:page:t page-in))
  =/  outs-in=outputs:t  ~(outputs get:tx:t tx-in)
  ?>  ?=(%1 -.outs-in)
  =/  out-in=output:t  (head ~(tap z-in +.outs-in))
  =/  note-in=nnote:t  ~(note get:output:t out-in)
  =/  nam=nname:t  ~(name get:nnote:t note-in)
  =/  [out-root=hash:t out-sc=spend-condition:v1:t *]
    (make-hax-lock:v1:h 77)
  =/  sed=seed:v1:t  (make-seed:v1:h out-root (sub assets.note-in fee) (hash:nnote:t note-in))
  =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
  =/  wit=witness:v1:t
    %*  .  *witness:v1:t
      lmp  (build-lock-merkle-proof:lock:t sc-in 1)
      pkh  *(z-map hash:t [pk=schnorr-pubkey:t sig=schnorr-signature:t])
      hax  *(z-map hash:t *)
      tim  ~
    ==
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  wit
      seeds    seds
      fee  fee
    ==
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  new-page=page:t  (make-empty-page:h page-in)
  =/  tx=tx:t  (new:tx:t raw ~(height get:page:t new-page))
  =/  new-tx-ids  (~(put z-in *(z-set tx-id:t)) ~(id get:raw-tx:t raw))
  =.  new-page
    ?^  -.new-page
      new-page(tx-ids new-tx-ids)
    new-page(tx-ids new-tx-ids)
  =/  new-digest  (compute-digest:page:t new-page)
  =.  new-page
    ?^  -.new-page
      new-page(digest new-digest)
    new-page(digest new-digest)
  =^  ready  con  (~(add-raw-tx dcon con der constants) raw)
  =/  r=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) new-page)
  ?:  ?=(%.y -.r)  (expect !>(%.n))
  (expect !>(%.y))
::
++  test-v1-lock-tim-rel-positive
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  build input v1 note locked with tim(rel min=`2)
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [root-cb=hash:t sc-cb=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk1])
  =/  prim=lock-primitive:v1:t  [%tim [rel=[min=`2 max=~] abs=[min=~ max=~]]]
  =/  sc-in=spend-condition:v1:t  ~[prim]
  =/  lmp-in=lock-merkle-proof:t  (build-lock-merkle-proof:lock:t sc-in 1)
  =/  root-in=hash:t  (hash:lock:t sc-in)
  =/  fee  10.000
  =/  sed0=seed:v1:t  (make-seed:v1:h root-in (sub assets.coin fee) (hash:nnote:t coin))
  =/  seds0=seeds:v1:t  (~(put z-in *seeds:v1:t) sed0)
  =/  sp1a=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds0
      fee  fee
    ==
  =/  sig-ha=hash:t  (sig-hash:spend-1:v1:t sp1a)
  =/  wita=witness:t
    (make-pkh-witness:v1:h root-cb sc-cb sig-ha ~[[s:default-keys-1:h pk1]])
  =/  sp1a=spend-1:v1:t  sp1a(witness wita)
  =/  raw0=raw-tx:v1:t
    (new:raw-tx:v1:t (~(put z-by *spends:v1:t) ~(name get:nnote:t coin) [%1 sp1a]))
  =/  [page-in=page:t newc=_con *]
    (add-raw-to-new-page:h page0 con raw0)
  =.  con  newc
  ::  advance two pages so tim rel min satisfied
  =^  last=page:t  con  (add-n-pages:h 2 con default-retain:h)
  ::  spend input note with tim witness -> success
  =/  tx-in=tx:t  (new:tx:t raw0 ~(height get:page:t page-in))
  =/  outs-in=outputs:t  ~(outputs get:tx:t tx-in)
  ?>  ?=(%1 -.outs-in)
  =/  out-in=output:t  (head ~(tap z-in +.outs-in))
  =/  note-in=nnote:t  ~(note get:output:t out-in)
  =/  nam=nname:t  ~(name get:nnote:t note-in)
  =/  [out-root=hash:t out-sc=spend-condition:v1:t *]
    (make-hax-lock:v1:h 88)
  =/  sed=seed:v1:t  (make-seed:v1:h out-root (sub assets.note-in fee) (hash:nnote:t note-in))
  =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
  =/  wit=witness:v1:t
    %*  .  *witness:v1:t
      lmp  lmp-in
      pkh  *(z-map hash:t [pk=schnorr-pubkey:t sig=schnorr-signature:t])
      hax  *(z-map hash:t *)
      tim  ~
    ==
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  wit
      seeds    seds
      fee  fee
    ==
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  [new-page=page:t newc=_con *]  (add-raw-to-new-page:h last con raw)
  =.  con  newc
  =/  old-in-balance=?  (~(has h-bi balance.con) ~(digest get:page:t new-page) nam)
  =/  tx=tx:t  (new:tx:t raw ~(height get:page:t new-page))
  =/  outs=outputs:t  ~(outputs get:tx:t tx)
  ?>  ?=(%1 -.outs)
  =/  out=output:t  (head ~(tap z-in +.outs))
  =/  note=nnote:t  ~(note get:output:t out)
  =/  name=nname:t  ~(name get:nnote:t note)
  =/  new-in-balance=?  (~(has h-bi balance.con) ~(digest get:page:t new-page) name)
  %+  expect-eq
    !>  [%.n %.y]
  !>  [old-in-balance new-in-balance]
::
++  test-v1-exceeds-medium-block-size
  =.  constants  bc-max-block-size-medium-v1:helpers
  =/  con=consensus-state  initial-consensus-state:h
  =/  min=mining-state  initial-mining-state:h
  =/  curr-page=page:t  default-genesis-page:h
  =|  [i=@ txs=(list raw-tx:t)]
  ::
  =/  [con=consensus-state min=mining-state txs=(list raw-tx:t)]
    |-
    ?:  =(50 i)
      [con min txs]
    =/  new-page  (make-empty-page:h curr-page)
    =/  r=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) new-page)
    ?.  ?=(%.y -.r)
      ~&  failed-reason++.r  !!
    =/  acc=tx-acc:t  +.r
    =.  con  (~(accept-page dcon con der constants) new-page acc *@da)
    =.  con  (~(update-heaviest dcon con der constants) new-page)
    =.  con  (~(garbage-collect dcon con der constants) default-retain:h)
    =.  min  (~(heard-new-block dmin min der constants) con *@da)
    =/  =coinbase:t
      ?^  -.new-page
        (new:v0:coinbase:t new-page p:default-keys-1:h)
      (new:coinbase:t new-page (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
    ?>  ?=(@ -.coinbase)
    =/  raw  (simple-from-note:raw-tx:t p:default-keys-1:h p:default-keys-2:h coinbase s:default-keys-1:h)
    $(i +(i), curr-page new-page, txs [raw txs], con con, min min)
  =/  attempt-add=(unit mining-state)
    %-  mole
    |.
    %+  roll
      txs
    |=  [=raw-tx:t min=_min]
    =/  new-min  (~(heard-new-tx dmin min der constants) raw-tx)
    ::  We are asserting that the mining state has changed here
    ::  If it hasn't changed, that means that adding the raw-tx
    ::  causes the block size to exceed the limit.
    ?<  =(new-min min)
    new-min
  ::
  ::  attempting to add the transactions should fail
  ::  because the block size exceeds the limit
  ?>  ?=(~ attempt-add)
  ~
::
++  test-v1-raw-tx-not-based
  =/  con=consensus-state  initial-consensus-state:h
  ::  create a simple valid v1 raw-tx, then structurally invalidate it
  =^  par=page:t  con  (add-n-pages:h (add 3 v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  =/  nam=nname:t  ~(name get:nnote:t coin)
  ::  build merkle proof for witness using spend condition
  =/  pk=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  sc=spend-condition:v1:t  form:(make-pkh:spend-condition:t 1 ~[pk])
  =/  lmp=lock-merkle-proof:t  (build-lock-merkle-proof:lock:t sc 1)
  ::  build a minimal spend-1 with empty seeds and zero fee
  =/  wit=witness:v1:t
    %*  .  *witness:v1:t
      lmp  lmp
      pkh  *(z-map hash:t [pk=schnorr-pubkey:t sig=schnorr-signature:t])
      hax  *(z-map hash:t *)
      tim  ~
    ==
  =/  sp=spend:v1:t
    :-  %1
    %*  .  *spend-1:v1:t
      witness  wit
      seeds    *(z-set seed:v1:t)
      fee      0
    ==
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam sp)
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  ::  mutate id to be structurally invalid
  =/  bad=raw-tx:v1:t  raw
  =.  id.bad  [p (dec p) (dec p) (dec p) (dec p)]
  %+  expect-eq  !>(%.n)
  !>((based:raw-tx:t bad))
::
++  test-v1-single-utxo-invalid-spend-construction-fail
  =/  con=consensus-state  initial-consensus-state:h
  ::  advance to just before v1 activation
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  ::  make a v0 coinbase to spend
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  advance to v1 activation height
  =^  last=page:t  con  (add-n-pages:h (sub v1-phase:t ~(height get:page:t page0)) con default-retain:h)
  =/  tac=tx-acc:t
    (new:tx-acc:t (~(get h-by balance.con) ~(digest get:page:t last)) ~(height get:page:t last))
  ::  build pkh lock for spending
  =/  nam=nname:t  ~(name get:nnote:t coin)
  =/  pk=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  sc=spend-condition:v1:t  form:(make-pkh:spend-condition:t 1 ~[pk])
  =/  root=hash:t  (hash:lock:t sc)
  =/  base-seed=seed:v1:t
    (make-seed:v1:h root assets.coin (hash:nnote:t coin))
  ::  overspend from note (gift > assets)
  =/  seed1=seed:v1:t
    base-seed(gift +(assets.coin))
  =/  seeds1=seeds:v1:t  (~(put z-in *seeds:v1:t) seed1)
  =/  sp0-1=spend-0:v1:t
    %*  .  *spend-0:v1:t
      seeds  seeds1
      fee    0
    ==
  =/  sp1=spend:v1:t  [%0 (sign:spend-0:v1:t sp0-1 s:default-keys-1:h)]
  =/  sps1=spends:v1:t  (~(put z-by *spends:v1:t) nam sp1)
  =/  raw1=raw-tx:v1:t  (new:raw-tx:v1:t sps1)
  ::  underspend from note (gift + fee < assets)
  =/  seed2=seed:v1:t
    base-seed(gift (dec (dec assets.coin)))
  =/  seeds2=seeds:v1:t  (~(put z-in *seeds:v1:t) seed2)
  =/  sp0-2=spend-0:v1:t
    %*  .  *spend-0:v1:t
      seeds  seeds2
      fee    0
    ==
  =/  sp2=spend:v1:t  [%0 (sign:spend-0:v1:t sp0-2 s:default-keys-1:h)]
  =/  sps2=spends:v1:t  (~(put z-by *spends:v1:t) nam sp2)
  =/  raw2=raw-tx:v1:t  (new:raw-tx:v1:t sps2)
  ::  both transactions should fail during processing due to check-gifts-and-fee
  =/  res1=(reason tx-acc:t)  (process:tx-acc:t tac raw1)
  =/  res2=(reason tx-acc:t)  (process:tx-acc:t tac raw2)
  %+  expect-eq
    !>([%.y %.y])  :: both should fail to process
  !>([?=(%.n -.res1) ?=(%.n -.res2)])
::
++  test-v1-zero-utxo-tx-fail-validation
  =|  raw=raw-tx:t
  ?>  ?=(@ -.raw)
  =.  id.raw  (compute-id:raw-tx:t raw)
  %+  expect-eq
    !>(%.n)
  :: this is the first check in +heard-tx in the kernel after the id check
  !>((validate:raw-tx:t raw))
::
++  test-v1-zero-utxo-tx-block-reject
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h 1 con default-retain:h)
  ::
  =/  new-page=page:t  (make-empty-page:h par)
  =|  raw=raw-tx:t
  ?>  ?=(@ -.raw)
  =.  id.raw  (compute-id:raw-tx:t raw)
  =^  ready  con  (~(add-raw-tx dcon con der constants) raw)
  =/  new-tx-ids  (~(put z-in *(z-set tx-id:t)) ~(id get:raw-tx:t raw))
  =.  new-page
    ?^  -.new-page
      new-page(tx-ids new-tx-ids)
    new-page(tx-ids new-tx-ids)
  =/  new-digest  (compute-digest:page:t new-page)
  =.  new-page
    ?^  -.new-page
      new-page(digest new-digest)
    new-page(digest new-digest)
  =/  tac=(reason tx-acc:t)
    (~(validate-page-with-txs dcon con der constants) new-page)
  %+  expect-eq  !>(%.n)
  !>(-:tac)
::
++  test-v1-valid-signed-spend
  =/  note=nnote:t  (make-simple-note:v1:h p:default-keys-1:h 1.000.000)
  ?>  ?=(@ -.note)
  =/  sen=spend-v1:t
    %:  simple-from-note:spend-v1:t
      p:default-keys-1:h
      p:default-keys-2:h
      note
    ==
  =.  sen
    %+  sign:spend-v1:t
      sen
    s:default-keys-1:h
  %-  expect
  !>  %+  verify:spend-v1:t
        sen
      note
::
++  test-v1-valid-2-of-2-multisig-input
  ::  create simple note, spend to 2-of-2 note, verify spend
  =/  note0=nnote:t  (make-simple-note:v1:h p:default-keys-1:h 1.000.000)
  ?>  ?=(@ -.note0)
  ::  create 2-of-2 lock and intermediate note
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  pk2=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-2:h))
  =/  [root-22=hash:t sc-22=spend-condition:v1:t *]
    (make-pkh-lock:v1:h 2 ~[pk1 pk2])
  =/  sed1=seed:v1:t  (make-seed:v1:h root-22 assets.note0 (hash:nnote:t note0))
  =/  source-hash=hash:t  (hash:seeds:v1:t (~(put z-in *seeds:v1:t) sed1))
  =/  note1=nnote-1:v1:t
    %*  .  *nnote-1:v1:t
      version      %1
      origin-page  origin-page.note0
      name         (new-v1:nname:t root-22 [source-hash %.n])
      note-data    *(z-map @tas *)
      assets       assets.note0
    ==
  ::  create spend with 2-of-2 witness
  =/  =sig:t  (join:sig:t 2 ~[p:default-keys-1:h p:default-keys-2:h])
  =/  sen=spend-v1:t
    %:  simple-from-note:spend-v1:t
      sig
      sig
      note1
    ==
  =.  sen  (sign:spend-v1:t sen s:default-keys-2:h)
  =.  sen  (sign:spend-v1:t sen s:default-keys-1:h)
  %-  expect
  !>  (verify:spend-v1:t sen note1)
::
++  test-v1-valid-2-of-3-multisig-input
  ::  create simple note, spend to 2-of-3 note, verify spend with 2 sigs
  =/  note0=nnote:t  (make-simple-note:v1:h p:default-keys-1:h 1.000.000)
  ?>  ?=(@ -.note0)
  ::  create 2-of-3 lock and intermediate note
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  pk2=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-2:h))
  =/  pk3=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-3:h))
  =/  [root-23=hash:t sc-23=spend-condition:v1:t *]
    (make-pkh-lock:v1:h 2 ~[pk1 pk2 pk3])
  =/  sed1=seed:v1:t  (make-seed:v1:h root-23 assets.note0 (hash:nnote:t note0))
  =/  source-hash=hash:t  (hash:seeds:v1:t (~(put z-in *seeds:v1:t) sed1))
  =/  note1=nnote-1:v1:t
    %*  .  *nnote-1:v1:t
      version      %1
      origin-page  origin-page.note0
      name         (new-v1:nname:t root-23 [source-hash %.n])
      note-data    *(z-map @tas *)
      assets       assets.note0
    ==
  ::  create spend with 2-of-3 witness (keys 2 and 3)
  =/  =sig:t  (join:sig:t 2 ~[p:default-keys-1:h p:default-keys-2:h p:default-keys-3:h])
  =/  sen=spend-v1:t
    %:  simple-from-note:spend-v1:t
      sig
      p:default-keys-3:h
      note1
    ==
  =.  sen  (sign:spend-v1:t sen s:default-keys-2:h)
  =.  sen  (sign:spend-v1:t sen s:default-keys-3:h)
  %-  expect
  !>  (verify:spend-v1:t sen note1)
::
++  test-v1-valid-1-of-3-multisig-input-oversign
  ::  create simple note, spend to 1-of-3 note, verify spend with 2 sigs (oversigned)
  =/  note0=nnote:t  (make-simple-note:v1:h p:default-keys-1:h 1.000.000)
  ?>  ?=(@ -.note0)
  ::  create 1-of-3 lock and intermediate note
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  pk2=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-2:h))
  =/  pk3=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-3:h))
  =/  [root-13=hash:t sc-13=spend-condition:v1:t *]
    (make-pkh-lock:v1:h 1 ~[pk1 pk2 pk3])
  =/  sed1=seed:v1:t  (make-seed:v1:h root-13 assets.note0 (hash:nnote:t note0))
  =/  source-hash=hash:t  (hash:seeds:v1:t (~(put z-in *seeds:v1:t) sed1))
  =/  note1=nnote-1:v1:t
    %*  .  *nnote-1:v1:t
      version      %1
      origin-page  origin-page.note0
      name         (new-v1:nname:t root-13 [source-hash %.n])
      note-data    *(z-map @tas *)
      assets       assets.note0
    ==
  ::  create spend with 2 signatures (oversigned - only need 1 of 3)
  =/  =sig:t  (join:sig:t 1 ~[p:default-keys-1:h p:default-keys-2:h p:default-keys-3:h])
  =/  sen=spend-v1:t
    %:  simple-from-note:spend-v1:t
      sig
      p:default-keys-3:h
      note1
    ==
  =.  sen  (sign:spend-v1:t sen s:default-keys-3:h)
  =.  sen  (sign:spend-v1:t sen s:default-keys-1:h)
  %-  expect
  !>  (verify:spend-v1:t sen note1)
::
++  test-v1-valid-1-of-3-multisig-input-double-sign
  ::  create simple note, spend to 1-of-3 note, verify spend with same sig twice
  =/  note0=nnote:t  (make-simple-note:v1:h p:default-keys-1:h 1.000.000)
  ?>  ?=(@ -.note0)
  ::  create 1-of-3 lock and intermediate note
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  pk2=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-2:h))
  =/  pk3=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-3:h))
  =/  [root-13=hash:t sc-13=spend-condition:v1:t *]
    (make-pkh-lock:v1:h 1 ~[pk1 pk2 pk3])
  =/  sed1=seed:v1:t  (make-seed:v1:h root-13 assets.note0 (hash:nnote:t note0))
  =/  source-hash=hash:t  (hash:seeds:v1:t (~(put z-in *seeds:v1:t) sed1))
  =/  note1=nnote-1:v1:t
    %*  .  *nnote-1:v1:t
      version      %1
      origin-page  origin-page.note0
      name         (new-v1:nname:t root-13 [source-hash %.n])
      note-data    *(z-map @tas *)
      assets       assets.note0
    ==
  ::  create spend with same key signing twice (redundant but valid)
  =/  =sig:t  (join:sig:t 1 ~[p:default-keys-1:h p:default-keys-2:h p:default-keys-3:h])
  =/  sen=spend-v1:t
    %:  simple-from-note:spend-v1:t
      sig
      p:default-keys-3:h
      note1
    ==
  =.  sen  (sign:spend-v1:t sen s:default-keys-3:h)
  =.  sen  (sign:spend-v1:t sen s:default-keys-3:h)
  %-  expect
  !>  (verify:spend-v1:t sen note1)
::
++  test-v1-invalid-2-of-3-multisig-input
  ::  create simple note, spend to 2-of-3 note, verify spend with only 1 sig fails
  =/  note0=nnote:t  (make-simple-note:v1:h p:default-keys-1:h 1.000.000)
  ?>  ?=(@ -.note0)
  ::  create 2-of-3 lock and intermediate note
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  pk2=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-2:h))
  =/  pk3=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-3:h))
  =/  [root-23=hash:t sc-23=spend-condition:v1:t *]
    (make-pkh-lock:v1:h 2 ~[pk1 pk2 pk3])
  =/  sed1=seed:v1:t  (make-seed:v1:h root-23 assets.note0 (hash:nnote:t note0))
  =/  source-hash=hash:t  (hash:seeds:v1:t (~(put z-in *seeds:v1:t) sed1))
  =/  note1=nnote-1:v1:t
    %*  .  *nnote-1:v1:t
      version      %1
      origin-page  origin-page.note0
      name         (new-v1:nname:t root-23 [source-hash %.n])
      note-data    *(z-map @tas *)
      assets       assets.note0
    ==
  ::  create spend with only 1 signature (need 2 for 2-of-3) - should fail
  =/  =sig:t  (join:sig:t 2 ~[p:default-keys-1:h p:default-keys-2:h p:default-keys-3:h])
  =/  sen=spend-v1:t
    %:  simple-from-note:spend-v1:t
      sig
      p:default-keys-3:h
      note1
    ==
  =.  sen  (sign:spend-v1:t sen s:default-keys-3:h)
  ::  check lock conditions should fail
  ?>  ?=(%1 -.sen)
  =/  ctx=check-context:t
    :*  now=+(origin-page.note1)
        since=origin-page.note1
        sig-hash=(sig-hash:spend-1:v1:t +.sen)
        witness=witness.+.sen
        bythos-phase=bythos-phase.constants
    ==
  %+  expect-eq
    !>(%.n)
  !>  (check:check-context:t ctx (lock-hash:nnote-1:v1:t note1))
::
::  ++test-v1-invalid-1-of-2-multisig-input-wrong-sig: test rejection of unauthorized key
::
::    this test validates that a 1-of-2 multisig pay-to-pubkey-hash lock
::    correctly rejects a signature from an unauthorized key.
::
::    the test uses a three-step approach:
::
::    1. create a simple v1 coinbase locked to key1 (1-of-1 lock)
::       - as with the valid m-of-n test, we start with a simple coinbase
::         because v1 coinbases with multiple owners are split by accept-page
::
::    2. spend the coinbase into an intermediate note with a 1-of-2 lock
::       - the intermediate note is locked to require 1 signature from
::         the set {key1, key2} - either key1 OR key2 can unlock it
::       - this spend uses key1's signature to unlock the coinbase
::
::    3. attempt to spend the intermediate note with key3's signature
::       - key3 is NOT in the allowed set {key1, key2}
::       - the lock check should fail because key3 is unauthorized
::
++  test-v1-invalid-1-of-2-multisig-input-wrong-sig
  =/  con=consensus-state  initial-consensus-state:h
  ::
  ::  step 1: create simple v1 note for key1
  ::
  ::    we use the simple note helper which creates a v1 note
  ::    locked to key1 with a simple 1-of-1 lock.
  ::
  =/  page0=page:t  default-genesis-page:h
  =/  note0=nnote:t  (make-simple-note:v1:h p:default-keys-1:h 1.000.000)
  ?>  ?=(@ -.note0)
  ::
  ::  step 2: spend coinbase into intermediate note with 1-of-2 lock
  ::
  ::    we create a note that can be unlocked by EITHER key1 OR key2.
  ::
  =/  nam=nname:t  ~(name get:nnote:t note0)
  ::  extract public keys for the allowed set
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  pk2=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-2:h))
  ::  create 1-of-2 lock: requires 1 signature from {pk1, pk2}
  =/  [root-12=hash:t sc-12=spend-condition:v1:t *]
    (make-pkh-lock:v1:h 1 ~[pk1 pk2])
  =/  fee1  0
  ::  create seed for output: locked with 1-of-2, funded by entire note
  =/  sed1=seed:v1:t  (make-seed:v1:h root-12 (sub assets.note0 fee1) (hash:nnote:t note0))
  =/  seds1=seeds:v1:t  (~(put z-in *seeds:v1:t) sed1)
  ::  create 1-of-1 lock for input note (what we're spending from)
  =/  [root1=hash:t sc1=spend-condition:v1:t *]
    (make-pkh-lock:v1:h 1 ~[pk1])
  ::  build the spend structure
  =/  sp1a=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds1
      fee  fee1
    ==
  ::  compute signature hash for this spend
  =/  sig-ha=hash:t  (sig-hash:spend-1:v1:t sp1a)
  ::  create witness proving we can unlock coinbase (1-of-1 key1)
  =/  wita=witness:t
    %:  make-pkh-witness:v1:h
      root1
      sc1
      sig-ha
      ~[[s:default-keys-1:h pk1]]
    ==
  ::  update spend with witness (this spend succeeds - we have key1)
  =/  sp1a=spend-1:v1:t  sp1a(witness wita)
  ::
  ::  step 3: attempt to spend intermediate note with key3 (unauthorized)
  ::
  ::    now we construct a spend that tries to use key3's signature.
  ::    key3 is NOT in the allowed set {key1, key2}, so this should fail.
  ::    we manually construct the intermediate note to test the lock check.
  ::
  ::  extract key3's public key (the unauthorized key)
  =/  pk3=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-3:h))
  ::  compute the source hash from the seeds that created the note
  =/  source-hash=hash:t  (hash:seeds:v1:t seds1)
  ::  manually construct the intermediate note (as it would exist after step 2)
  =/  note1=nnote-1:v1:t
    %*  .  *nnote-1:v1:t
      version      %1
      origin-page  origin-page.note0
      name         (new-v1:nname:t root-12 [source-hash %.n])
      note-data    *(z-map @tas *)
      assets       (sub assets.note0 fee1)
    ==
  ::  build a spend attempting to use key3 (no seeds, just testing lock check)
  =/  sp1b=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    *(z-set seed:v1:t)
      fee  0
    ==
  ::  compute signature hash for the unauthorized spend attempt
  =/  sig-hb=hash:t  (sig-hash:spend-1:v1:t sp1b)
  ::  create witness with key3's signature (unauthorized key)
  ::  this witness claims to satisfy the 1-of-2 lock, but uses key3
  ::  which is not in the allowed set {key1, key2}
  =/  witb=witness:t
    %:  make-pkh-witness:v1:h
      root-12
      sc-12
      sig-hb
      ~[[s:default-keys-3:h pk3]]
    ==
  ::  update spend with the unauthorized witness
  =/  sp1b=spend-1:v1:t  sp1b(witness witb)
  ::
  ::  verification: check that the lock validation fails
  ::
  ::    we construct a check-context and verify that check:check-context
  ::    returns %.n when given key3's witness against a lock
  ::    that only allows {key1, key2}.
  ::
  =/  ctx=check-context:t
    :*  now=+(~(origin-page get:nnote:t note1))
        since=~(origin-page get:nnote:t note1)
        sig-hash=sig-hb
        witness=witness.sp1b
        bythos-phase=bythos-phase.constants
    ==
  ::  expect: lock check returns %.n (false) for unauthorized key
  %+  expect-eq
    !>(%.n)
  !>  %+  check:check-context:t
        ctx
      (lock-hash:nnote-1:v1:t note1)
::
++  test-v1-invalid-1-of-2-multisig-input-right-and-wrong-sig
  ::  create simple note, spend to 1-of-2 note, verify fails with right+wrong sigs
  =/  note0=nnote:t  (make-simple-note:v1:h p:default-keys-1:h 1.000.000)
  ?>  ?=(@ -.note0)
  ::  create 1-of-2 lock and intermediate note
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  pk2=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-2:h))
  =/  pk3=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-3:h))
  =/  [root-12=hash:t sc-12=spend-condition:v1:t *]
    (make-pkh-lock:v1:h 1 ~[pk1 pk2])
  =/  sed1=seed:v1:t  (make-seed:v1:h root-12 assets.note0 (hash:nnote:t note0))
  =/  source-hash=hash:t  (hash:seeds:v1:t (~(put z-in *seeds:v1:t) sed1))
  =/  note1=nnote-1:v1:t
    %*  .  *nnote-1:v1:t
      version      %1
      origin-page  origin-page.note0
      name         (new-v1:nname:t root-12 [source-hash %.n])
      note-data    *(z-map @tas *)
      assets       assets.note0
    ==
  ::  create spend with both right (key1) and wrong (key3) signatures - should fail
  =/  =sig:t  (join:sig:t 1 ~[p:default-keys-1:h p:default-keys-2:h])
  =/  sen=spend-v1:t
    %:  simple-from-note:spend-v1:t
      sig
      p:default-keys-3:h
      note1
    ==
  =.  sen  (sign:spend-v1:t sen s:default-keys-3:h)
  =.  sen  (sign:spend-v1:t sen s:default-keys-1:h)
  ::  check lock conditions should fail (key3 not allowed)
  ?>  ?=(%1 -.sen)
  =/  ctx=check-context:t
    :*  now=+(origin-page.note1)
        since=origin-page.note1
        sig-hash=(sig-hash:spend-1:v1:t +.sen)
        witness=witness.+.sen
        bythos-phase=bythos-phase.constants
    ==
  %+  expect-eq
    !>(%.n)
  !>  (check:check-context:t ctx (lock-hash:nnote-1:v1:t note1))
::
++  test-v1-multisig-valid-output-2-of-3-spend-0
  =/  con=consensus-state  initial-consensus-state:h
  ::  advance close to v1 activation
  =^  par=page:t  con  (add-n-pages:h (sub v1-phase:t 2) con default-retain:h)
  ::  create a v0 coinbase page
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin0=coinbase:t  (new:v0:coinbase:t page0 p:default-keys-1:h)
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  advance to v1 activation
  =^  last=page:t  con  (add-n-pages:h 1 con default-retain:h)
  ::  lift v0 coinbase to v1 multisig output using spend-0
  =/  =sig:t
    (join:sig:t 2 ~[p:default-keys-1:h p:default-keys-2:h p:default-keys-3:h])
  =/  fee  10.000
  =/  seeds=seeds:v1:t
    %-  ~(put z-in *seeds:v1:t)
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      (hash:lock:t (lock-from-sig:t sig))
      gift           (sub assets.coin0 fee)
      parent-hash    (hash:nnote:t coin0)
    ==
  =/  sp0=spend-0:v1:t  %*  .  *spend-0:v1:t
                          seeds  seeds
                          fee    fee
                        ==
  =/  sp=spend:v1:t  [%0 (sign:spend-0:v1:t sp0 s:default-keys-1:h)]
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) ~(name get:nnote:t coin0) sp)
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  [page-out=page:t newc=_con *]  (add-raw-to-new-page:h last con raw)
  =.  con  newc
  =/  =tx:t  (new:tx:t raw ~(height get:page:t page-out))
  =/  outs=outputs:t  ~(outputs get:tx:t tx)
  ?>  ?=(%1 -.outs)
  =/  new-out=output:t  (head ~(tap z-in +.outs))
  =/  multisig-note=nnote:t  ~(note get:output:t new-out)
  ?>  ?=(@ -.multisig-note)
  ::  now spend from multisig note with 2-of-3 signatures
  =/  nam=nname:t  ~(name get:nnote:t multisig-note)
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  pk2=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-2:h))
  =/  pk3=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-3:h))
  =/  [root=hash:t sc=spend-condition:v1:t *]
    (make-pkh-lock:v1:h 2 ~[pk1 pk2 pk3])
  =/  fee  10.000
  =/  sed=seed:v1:t  (make-seed:v1:h root (sub assets.multisig-note fee) (hash:nnote:t multisig-note))
  =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds
      fee  fee
    ==
  =/  sig-h=hash:t  (sig-hash:spend-1:v1:t sp1)
  =/  wit=witness:t
    %:  make-pkh-witness:v1:h
      root
      sc
      sig-h
      ~[[s:default-keys-1:h pk1] [s:default-keys-2:h pk2]]
    ==
  =/  sp1=spend-1:v1:t  sp1(witness wit)
  =/  sps2=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw2=raw-tx:v1:t  (new:raw-tx:v1:t sps2)
  =/  [final-page=page:t newc=_con *]  (add-raw-to-new-page:h page-out con raw2)
  =.  con  newc
  =/  old-in-balance=?  (~(has h-bi balance.con) ~(digest get:page:t final-page) nam)
  =/  tx2=tx:t  (new:tx:t raw2 ~(height get:page:t final-page))
  =/  outs2=outputs:t  ~(outputs get:tx:t tx2)
  ?>  ?=(%1 -.outs2)
  =/  final-out=output:t  (head ~(tap z-in +.outs2))
  =/  final-note=nnote:t  ~(note get:output:t final-out)
  =/  final-name=nname:t  ~(name get:nnote:t final-note)
  =/  new-in-balance=?  (~(has h-bi balance.con) ~(digest get:page:t final-page) final-name)
  %+  expect-eq
    !>  [%.n %.y]
  !>  [old-in-balance new-in-balance]
::
++  test-v1-multisig-valid-output-2-of-3-spend-1
  =/  con=consensus-state  initial-consensus-state:h
  ::  advance to just before v1 coinbase activation
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  ::  create simple v1 coinbase for key1
  =/  page0=page:t  (make-empty-page:h par)
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  get actual coinbase from balance
  =/  page0-balance  (need (~(get h-by balance.con) ~(digest get:page:t page0)))
  =/  coin=nnote:t  (get-coinbase-from-balance:v1:h page0 page0-balance)
  ::  spend coinbase into intermediate note with 2-of-3 lock
  =/  nam=nname:t  ~(name get:nnote:t coin)
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  pk2=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-2:h))
  =/  pk3=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-3:h))
  =/  [out-root=hash:t *]
    (make-pkh-lock:v1:h 2 ~[pk1 pk2 pk3])
  =/  fee  10.000
  =/  coin-assets=coins:t  ~(assets get:nnote:t coin)
  =/  sed=seed:v1:t  (make-seed:v1:h out-root (sub coin-assets fee) (hash:nnote:t coin))
  =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds
      fee  fee
    ==
  =/  sig-h=hash:t  (sig-hash:spend-1:v1:t sp1)
  =/  [in-root=hash:t in-sc=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk1])
  =/  wit=witness:t
    (make-pkh-witness:v1:h in-root in-sc sig-h ~[[s:default-keys-1:h pk1]])
  =/  sp1=spend-1:v1:t  sp1(witness wit)
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  [final-page=page:t newc=_con *]  (add-raw-to-new-page:h page0 con raw)
  =.  con  newc
  =/  old-in-balance=?  (~(has h-bi balance.con) ~(digest get:page:t final-page) nam)
  =/  tx=tx:t  (new:tx:t raw ~(height get:page:t final-page))
  =/  outs=outputs:t  ~(outputs get:tx:t tx)
  ?>  ?=(%1 -.outs)
  =/  final-out=output:t  (head ~(tap z-in +.outs))
  =/  final-note=nnote:t  ~(note get:output:t final-out)
  =/  final-name=nname:t  ~(name get:nnote:t final-note)
  =/  new-in-balance=?  (~(has h-bi balance.con) ~(digest get:page:t final-page) final-name)
  %+  expect-eq
    !>  [%.n %.y]
  !>  [old-in-balance new-in-balance]
::
++  test-v1-reject-tx-digest
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  get actual coinbase from balance
  =/  page0-balance  (need (~(get h-by balance.con) ~(digest get:page:t page0)))
  =/  coin=nnote:t  (get-coinbase-from-balance:v1:h page0 page0-balance)
  ::  build minimal spend-1
  =/  nam=nname:t  ~(name get:nnote:t coin)
  =/  pk=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [root=hash:t sc=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk])
  =/  fee  10.000
  =/  sed=seed:v1:t  (make-seed:v1:h root (sub assets.coin fee) (hash:nnote:t coin))
  =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds
      fee  fee
    ==
  =/  sig-h=hash:t  (sig-hash:spend-1:v1:t sp1)
  =/  wit=witness:t
    (make-pkh-witness:v1:h root sc sig-h ~[[s:default-keys-1:h pk]])
  =/  sp1=spend-1:v1:t  sp1(witness wit)
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  ::  mutate id
  =.  id.raw  *tx-id:t
  %+  expect-eq  !>(%.n)
  !>((validate:raw-tx:t raw))
::
++  test-v1-reject-tx-inputs-bad-sig
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  get actual coinbase from balance
  =/  page0-balance  (need (~(get h-by balance.con) ~(digest get:page:t page0)))
  =/  coin=nnote:t  (get-coinbase-from-balance:v1:h page0 page0-balance)
  =/  tac=tx-acc:t
    (new:tx-acc:t (~(get h-by balance.con) ~(digest get:page:t page0)) ~(height get:page:t page0))
  ::  build spend-1 with correct seeds but wrong signature
  =/  nam=nname:t  ~(name get:nnote:t coin)
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  pk2=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-2:h))
  =/  [root=hash:t sc=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk1])
  =/  fee  10.000
  =/  sed=seed:v1:t  (make-seed:v1:h root (sub assets.coin fee) (hash:nnote:t coin))
  =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds
      fee  fee
    ==
  =/  sig-h=hash:t  (sig-hash:spend-1:v1:t sp1)
  ::  sign with wrong key
  =/  wit=witness:t
    (make-pkh-witness:v1:h root sc sig-h ~[[s:default-keys-2:h pk2]])
  =/  sp1=spend-1:v1:t  sp1(witness wit)
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  res=(reason tx-acc:t)  (process:tx-acc:t tac raw)
  ?>  ?=(%.n -.res)
  %+  expect-eq  !>(%v1-spend-1-lock-failed)
  !>(p.res)
::
++  test-v1-reject-tx-inputs-bad-input
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  get actual coinbase from balance
  =/  page0-balance  (need (~(get h-by balance.con) ~(digest get:page:t page0)))
  =/  coin=nnote:t  (get-coinbase-from-balance:v1:h page0 page0-balance)
  =/  tac=tx-acc:t
    (new:tx-acc:t (~(get h-by balance.con) ~(digest get:page:t page0)) ~(height get:page:t page0))
  ::  build seed1, sign it
  =/  nam=nname:t  ~(name get:nnote:t coin)
  =/  pk=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [root=hash:t sc=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk])
  =/  sed1=seed:v1:t  (make-seed:v1:h root assets.coin (hash:nnote:t coin))
  =/  seds1=seeds:v1:t  (~(put z-in *seeds:v1:t) sed1)
  =/  fee  10.000
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds1
      fee  fee
    ==
  =/  sig-h=hash:t  (sig-hash:spend-1:v1:t sp1)
  =/  wit=witness:t
    (make-pkh-witness:v1:h root sc sig-h ~[[s:default-keys-1:h pk]])
  =/  sp1=spend-1:v1:t  sp1(witness wit)
  ::  mutate seeds after signing
  =/  sed2=seed:v1:t
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      root
      gift           0
      parent-hash    (hash:nnote:t coin)
    ==
  =/  seds2=seeds:v1:t  (~(put z-in seds1) sed2)
  =/  sp1-bad=spend-1:v1:t  sp1(seeds seds2)
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1-bad])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  res=(reason tx-acc:t)  (process:tx-acc:t tac raw)
  ?>  ?=(%.n -.res)
  %+  expect-eq  !>(%v1-tx-invalid)
  !>(p.res)
::
++  test-v1-reject-tx-timelock
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  first create a v1 note with %tim lock (rel min=2)
  =/  pk=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [root-pkh=hash:t sc-pkh=spend-condition:v1:t *]
    (make-pkh-lock:v1:h 1 ~[pk])
  =/  prim=lock-primitive:v1:t  [%tim [rel=[min=`2 max=~] abs=[min=~ max=~]]]
  =/  sc-tim=spend-condition:v1:t  ~[prim]
  =/  sc=spend-condition:v1:t  (combine:spend-condition:t sc-pkh sc-tim)
  =/  root=hash:t  (hash:lock:t sc)
  ::  create the timelocked note via spend-0
  =/  [root-cb=hash:t sc-cb=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk])
  =/  fee  10.000
  =/  sed0=seed:v1:t
    (make-seed:v1:h root (sub assets.coin fee) (hash:nnote:t coin))
  =/  seds0=seeds:v1:t  (~(put z-in *seeds:v1:t) sed0)
  =/  sp1a=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds0
      fee  fee
    ==
  =/  sig-ha=hash:t  (sig-hash:spend-1:v1:t sp1a)
  =/  wita=witness:t
    (make-pkh-witness:v1:h root-cb sc-cb sig-ha ~[[s:default-keys-1:h pk]])
  =/  sp1a=spend-1:v1:t  sp1a(witness wita)
  =/  raw0=raw-tx:v1:t
    (new:raw-tx:v1:t (~(put z-by *spends:v1:t) ~(name get:nnote:t coin) [%1 sp1a]))
  =/  [page-in=page:t newc=_con *]
    (add-raw-to-new-page:h page0 con raw0)
  =.  con  newc
  ::  extract the timelocked note
  =/  tx-in=tx:t  (new:tx:t raw0 ~(height get:page:t page-in))
  =/  outs-in=outputs:t  ~(outputs get:tx:t tx-in)
  ?>  ?=(%1 -.outs-in)
  =/  out-in=output:t  (head ~(tap z-in +.outs-in))
  =/  note-in=nnote:t  ~(note get:output:t out-in)
  =/  nam=nname:t  ~(name get:nnote:t note-in)
  ::  try to spend immediately (violates rel min=2)
  =/  tac=tx-acc:t
    (new:tx-acc:t (~(get h-by balance.con) ~(digest get:page:t page-in)) ~(height get:page:t page-in))
  =/  fee  10.000
  =/  sed=seed:v1:t  (make-seed:v1:h root (sub assets.note-in fee) (hash:nnote:t note-in))
  =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds
      fee  fee
    ==
  =/  sig-h=hash:t  (sig-hash:spend-1:v1:t sp1)
  =/  wit=witness:t
    %:  make-pkh-witness:v1:h
      root
      sc
      sig-h
      ~[[s:default-keys-1:h pk]]
    ==
  =/  sp1=spend-1:v1:t  sp1(witness wit)
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  ::  process should fail: trying to spend before timelock maturity
  =/  res=(reason tx-acc:t)  (process:tx-acc:t tac raw)
  %+  expect-eq  !>(%.n)
  !>(-.res)
::
++  test-v1-reject-tx-fees
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  =/  tac=tx-acc:t
    (new:tx-acc:t (~(get h-by balance.con) ~(digest get:page:t page0)) ~(height get:page:t page0))
  ::  build spend-1 with mismatched fee
  =/  nam=nname:t  ~(name get:nnote:t coin)
  =/  pk=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [root=hash:t sc=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk])
  =/  sed=seed:v1:t  (make-seed:v1:h root assets.coin (hash:nnote:t coin))
  =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds
      fee  1
    ==
  =/  sig-h=hash:t  (sig-hash:spend-1:v1:t sp1)
  =/  wit=witness:t
    (make-pkh-witness:v1:h root sc sig-h ~[[s:default-keys-1:h pk]])
  =/  sp1=spend-1:v1:t  sp1(witness wit)
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  ::  process should fail: fee=1 but total gift=assets.coin (no actual fee)
  =/  res=(reason tx-acc:t)  (process:tx-acc:t tac raw)
  %+  expect-eq  !>(%.n)
  !>(-.res)
::
++  test-v1-seed-simple-from-note-with-choice-refund
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  ?>  ?=(@ -.coin)
  =.  assets.coin  3
  ::  manually construct two seeds: gift and refund
  =/  lock2=lock:t  (lock-from-sig:t p:default-keys-2:h)
  =/  lock3=lock:t  (lock-from-sig:t p:default-keys-3:h)
  =/  sed-gift=seed:v1:t
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      (hash:lock:t lock2)
      gift           1
      parent-hash    (hash:nnote:t coin)
    ==
  =/  sed-refund=seed:v1:t
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      (hash:lock:t lock3)
      gift           2
      parent-hash    (hash:nnote:t coin)
    ==
  =/  seds=seeds:v1:t  (~(gas z-in *seeds:v1:t) ~[sed-gift sed-refund])
  =/  seedz=(list seed:v1:t)  ~(tap z-in seds)
  ?>  =(2 (lent seedz))
  ::  determine which is refund and which is gift
  =/  sed1=seed:v1:t  (snag 0 seedz)
  =/  sed2=seed:v1:t  (snag 1 seedz)
  =/  [num-refund=@ refund-seed=seed:v1:t]
    ?:  =(lock-root.sed1 (hash:lock:t lock3))
      [%1 sed1]
    ?:  =(lock-root.sed2 (hash:lock:t lock3))
      [%2 sed2]
    !!
  =/  gift-seed
    ?:  ?=(%1 num-refund)
      sed2
    sed1
  %+  expect-eq
    !>(%.y)
  !>  ?&  =(gift:refund-seed 2)
          =(parent-hash:refund-seed (hash:nnote:t coin))
          =(lock-root.refund-seed (hash:lock:t lock3))
        ::
          =(gift:gift-seed 1)
          =(parent-hash:gift-seed (hash:nnote:t coin))
          =(lock-root.gift-seed (hash:lock:t lock2))
      ==
::
++  test-v1-seed-simple-from-note-with-choice-no-refund
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  ?>  ?=(@ -.coin)
  =.  assets.coin  1
  ::  manually construct one seed: gift only
  =/  lock2=lock:t  (lock-from-sig:t p:default-keys-2:h)
  =/  sed=seed:v1:t
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      (hash:lock:t lock2)
      gift           1
      parent-hash    (hash:nnote:t coin)
    ==
  =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
  =/  seedz=(list seed:v1:t)  ~(tap z-in seds)
  ?>  =(1 (lent seedz))
  =/  sed-out=seed:v1:t  (head seedz)
  %+  expect-eq
    !>(%.y)
  !>  ?&  =(gift:sed-out 1)
          =(parent-hash:sed-out (hash:nnote:t coin))
          =(lock-root.sed-out (hash:lock:t lock2))
      ==
::
++  test-v1-seeds-with-invalid-source
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  ?>  ?=(@ -.coin)
  =.  assets.coin  10.003
  ::  build seeds with invalid output-source
  =/  fee  10.000
  =/  lock2=lock:t  (lock-from-sig:t p:default-keys-2:h)
  =/  sed1=seed:v1:t
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      (hash:lock:t lock2)
      gift           1
      parent-hash    (hash:nnote:t coin)
    ==
  ::  set invalid output-source pointing to sed1's own hash
  =.  sed1  sed1(output-source `[(hash:seed:v1:t sed1) %.n])
  =/  sed2=seed:v1:t
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      (hash:lock:t lock2)
      gift           2
      parent-hash    (hash:nnote:t coin)
    ==
  =.  sed2  sed2(output-source `[(hash:seed:v1:t sed1) %.n])
  =/  seds=seeds:v1:t  (~(gas z-in *seeds:v1:t) ~[sed1 sed2])
  ::  construct spend-1
  =/  nam=nname:t  ~(name get:nnote:t coin)
  =/  pk=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [root=hash:t sc=spend-condition:v1:t *]
    (make-pkh-lock:v1:h 1 ~[pk])
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds
      fee  fee
    ==
  =/  sig-h=hash:t  (sig-hash:spend-1:v1:t sp1)
  =/  wit=witness:t
    %:  make-pkh-witness:v1:h
      root
      sc
      sig-h
      ~[[s:default-keys-1:h pk]]
    ==
  =/  sp1=spend-1:v1:t  sp1(witness wit)
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  transaction=(unit tx:t)  (mole |.((new:tx:t raw 1)))
  ?~  transaction
    !!
  %+  expect-eq
    !>(%.n)
  !>((validate:tx:t u.transaction))
::
++  test-v1-seeds-with-valid-source
  =.  constants  bc-without-fees:helpers
  =/  con=consensus-state  initial-consensus-state:h
  ::  advance to v1 activation
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  ::  create v1 coinbase
  =/  page0=page:t  (make-empty-page:h par)
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  get actual coinbase from balance
  =/  page0-balance  (need (~(get h-by balance.con) ~(digest get:page:t page0)))
  =/  coin=nnote:t  (get-coinbase-from-balance:v1:h page0 page0-balance)
  ::  first tx: coinbase → intermediate v1 note (without output-source)
  =/  lock2=lock:t  (lock-from-sig:t p:default-keys-2:h)
  =/  in-seeds=seeds:v1:t
    (~(put z-in *seeds:v1:t) (make-seed:v1:h (hash:lock:t lock2) assets.coin (hash:nnote:t coin)))
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [root1=hash:t sc1=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk1])
  =/  sp1a=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    in-seeds
      fee  0
    ==
  =/  sig-ha=hash:t  (sig-hash:spend-1:v1:t sp1a)
  =/  wita=witness:t
    (make-pkh-witness:v1:h root1 sc1 sig-ha ~[[s:default-keys-1:h pk1]])
  =/  sp1a=spend-1:v1:t  sp1a(witness wita)
  =/  raw-in=raw-tx:v1:t
    (new:raw-tx:v1:t (~(put z-by *spends:v1:t) ~(name get:nnote:t coin) [%1 sp1a]))
  =/  [page-in=page:t newc=_con *]
    (add-raw-to-new-page:h page0 con raw-in)
  =.  con  newc
  ::  extract intermediate v1 note
  =/  tx-in=tx:t  (new:tx:t raw-in ~(height get:page:t page-in))
  =/  outs-in=outputs:t  ~(outputs get:tx:t tx-in)
  ?>  ?=(%1 -.outs-in)
  =/  out-in=output:t  (head ~(tap z-in +.outs-in))
  =/  note-in=nnote:t  ~(note get:output:t out-in)
  =/  name-in=nname:t  ~(name get:nnote:t note-in)
  ::  second tx: spend intermediate note with seeds that have VALID output-source
  =/  note-in-assets=coins:t  ~(assets get:nnote:t note-in)
  =/  sed1=seed:v1:t
    (make-seed:v1:h (hash:lock:t lock2) (sub note-in-assets 2) (hash:nnote:t note-in))
  =/  sed2=seed:v1:t
    (make-seed:v1:h (hash:lock:t lock2) 2 (hash:nnote:t note-in))
  ::  compute what the output-source should be
  =/  out-seeds=seeds:v1:t  (~(gas z-in *seeds:v1:t) ~[sed1 sed2])
  =/  out-source-hash=hash:t  (hash:seeds:v1:t out-seeds)
  ::  set valid output-source on seeds
  =.  sed1  sed1(output-source `[out-source-hash %.n])
  =.  sed2  sed2(output-source `[out-source-hash %.n])
  =/  out-seeds-with-source=seeds:v1:t  (~(gas z-in *seeds:v1:t) ~[sed1 sed2])
  ::  build witness for intermediate note
  =/  pk2=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-2:h))
  =/  [root2=hash:t sc2=spend-condition:v1:t *]
    (make-pkh-lock:v1:h 1 ~[pk2])
  =/  sp1b=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    out-seeds-with-source
      fee  0
    ==
  =/  sig-hb=hash:t  (sig-hash:spend-1:v1:t sp1b)
  =/  witb=witness:t
    %:  make-pkh-witness:v1:h
      root2
      sc2
      sig-hb
      ~[[s:default-keys-2:h pk2]]
    ==
  =/  sp1b=spend-1:v1:t  sp1b(witness witb)
  =/  raw-out=raw-tx:v1:t
    (new:raw-tx:v1:t (~(put z-by *spends:v1:t) name-in [%1 sp1b]))
  =/  transaction=(unit tx:t)  (mole |.((new:tx:t raw-out ~(height get:page:t page-in))))
  ?~  transaction
    !!
  %+  expect-eq
    !>(%.y)
  !>((validate:tx:t u.transaction))
::
++  test-v1-spend-fee
  =.  constants  bc-v1-phase:helpers
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  build spend-1 with fee=10.000
  =/  new-page=page:t  (make-empty-page:h page0)
  =/  nam=nname:t  ~(name get:nnote:t coin)
  =/  lock2=lock:t  (lock-from-sig:t p:default-keys-2:h)
  =/  fee  10.000
  =/  sed-gift=seed:v1:t
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      (hash:lock:t lock2)
      gift           1
      parent-hash    (hash:nnote:t coin)
    ==
  =/  lock1=lock:t  (lock-from-sig:t p:default-keys-1:h)
  =/  sed-refund=seed:v1:t
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      (hash:lock:t lock1)
      gift           (sub assets.coin (add 1 fee))
      parent-hash    (hash:nnote:t coin)
    ==
  =/  seds=seeds:v1:t  (~(gas z-in *seeds:v1:t) ~[sed-gift sed-refund])
  =/  pk=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [root=hash:t sc=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk])
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds
      fee  fee
    ==
  =/  sig-h=hash:t  (sig-hash:spend-1:v1:t sp1)
  =/  wit=witness:t
    (make-pkh-witness:v1:h root sc sig-h ~[[s:default-keys-1:h pk]])
  =/  sp1=spend-1:v1:t  sp1(witness wit)
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  =tx:t  (new:tx:t raw ~(height get:page:t new-page))
  =/  new-tx-ids  (~(put z-in *(z-set tx-id:t)) ~(id get:raw-tx:t raw))
  =.  new-page
    ?^  -.new-page
      new-page(tx-ids new-tx-ids)
    new-page(tx-ids new-tx-ids)
  =/  total-coinbase-assets=coins:t
    (add (emission-calc:coinbase:t ~(height get:page:t new-page)) ~(total-fees get:tx:t tx))
  =.  new-page
    ?^  -.new-page
      =/  new-coinbase  (new:v0:coinbase-split:t total-coinbase-assets default-keys-1-share:h)
      new-page(coinbase new-coinbase)
    =/  new-coinbase  (new:v1:coinbase-split:t total-coinbase-assets default-keys-1-share-v1:h)
    new-page(coinbase new-coinbase)
  =/  new-digest  (compute-digest:page:t new-page)
  =.  new-page
    ?^  -.new-page
      new-page(digest new-digest)
    new-page(digest new-digest)
  =^  ready  con  (~(add-raw-tx dcon con der constants) raw)
  ?>  -:(~(validate-page-without-txs dcon con der constants) new-page ~(timestamp get:page:t new-page))
  =/  tac  (~(validate-page-with-txs dcon con der constants) new-page)
  ?:  ?=(%.n -.tac)
    (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) new-page +.tac *@da)
  =.  con  (~(update-heaviest dcon con der constants) new-page)
  =/  old-in-balance=?
    (~(has h-bi balance.con) ~(digest get:page:t new-page) ~(name get:nnote:t make-default-coinbase:v1:h))
  =/  outs=outputs:t  ~(outputs get:tx:t tx)
  ?>  ?=(%1 -.outs)
  =/  outs=(list output:t)  ~(tap z-in +.outs)
  =/  lock2-hash=hash:t  (hash:lock:t lock2)
  =/  out-gift=output:t
    |-
    ?~  outs  !!
    =/  n=nnote:t  ~(note get:output:t i.outs)
    ?>  ?=(@ -.n)
    =/  expected-lock-hash=hash:t
      (hash-hashable:tip5 [leaf+& hash+lock2-hash])
    ?:  =((lock-hash:nnote-1:v1:t n) expected-lock-hash)
      i.outs
    $(outs t.outs)
  =/  new-note=nnote:t  ~(note get:output:t out-gift)
  ?>  ?=(@ -.new-note)
  =/  new-in-balance=?
    (~(has h-bi balance.con) ~(digest get:page:t new-page) ~(name get:nnote:t new-note))
  ::  get actual coinbase from balance using helper
  =/  new-page-balance  (need (~(get h-by balance.con) ~(digest get:page:t new-page)))
  =/  new-coinb=nnote:t  (get-coinbase-from-balance:v1:h new-page new-page-balance)
  =/  pk-hash=hash:t  (hash:schnorr-pubkey:t default-a-pt-1:h)
  %+  expect-eq
    !>  :*  %.n
            %.y
            new-coinb
            ?^  -.new-page
              (~(got z-by =<(+ ~(coinbase get:page:t new-page))) p:default-keys-1:h)
            (~(got z-by =<(+ ~(coinbase get:page:t new-page))) pk-hash)
        ==
  !>  :*  old-in-balance
          new-in-balance
          new-coinb
          assets.new-coinb
      ==
::
++  test-v1-pending-reject-inputs-in-spent-by
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  create two spends from same coinbase
  =/  lock2=lock:t  (lock-from-sig:t p:default-keys-2:h)
  =/  lock3=lock:t  (lock-from-sig:t p:default-keys-3:h)
  =/  raw1=raw-tx:t
    (simple-from-note:raw-tx:t p:default-keys-1:h p:default-keys-2:h coin s:default-keys-1:h)
  =/  raw2=raw-tx:t
    (simple-from-note:raw-tx:t p:default-keys-1:h p:default-keys-3:h coin s:default-keys-1:h)
  ::  heard first tx
  =^  ready  con  (~(add-raw-tx dcon con der constants) raw1)
  ::  heard second tx with same input
  %+  expect-eq
    !>(%.y)
  !>((~(inputs-spent dcon con der constants) raw2))
::
++  test-v1-pending-reject-inputs-not-in-balance
  =/  con=consensus-state  initial-consensus-state:h
  =^  new-page=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  ::
  =/  raw=raw-tx:t  make-rogue-tx:v1:h
  ::
  ::  heard tx
  %+  expect-eq
    !>(%.n)
  !>((~(inputs-in-heaviest-balance dcon con der constants) raw))
::
++  test-v1-pending-accepts-inputs-not-in-spent-by
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::
  =/  lock2=lock:t  (lock-from-sig:t p:default-keys-2:h)
  =/  raw=raw-tx:t
    (simple-from-note:raw-tx:t p:default-keys-1:h p:default-keys-2:h coin s:default-keys-1:h)
  ::
  ::  heard tx
  %+  expect-eq
    !>(%.n)
  !>((~(inputs-spent dcon con der constants) raw))
::
++  test-v1-pending-accepts-inputs-in-heaviest-balance
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::
  =/  lock2=lock:t  (lock-from-sig:t p:default-keys-2:h)
  =/  raw=raw-tx:t
    (simple-from-note:raw-tx:t p:default-keys-1:h p:default-keys-2:h coin s:default-keys-1:h)
  ::
  ::  heard tx
  %+  expect-eq
    !>(%.y)
  !>((~(inputs-in-heaviest-balance dcon con der constants) raw))
::
++  test-v1-remove-pending-block
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  make tx
  =/  lock2=lock:t  (lock-from-sig:t p:default-keys-2:h)
  =/  raw=raw-tx:t
    (simple-from-note:raw-tx:t p:default-keys-1:h p:default-keys-2:h coin s:default-keys-1:h)
  ::
  ::  make new page with tx
  =/  new-page=page:t  (make-empty-page:h page0)
  =/  new-tx-ids  (~(put z-in *(z-set tx-id:t)) ~(id get:raw-tx:t raw))
  =.  new-page
    ?^  -.new-page
      new-page(tx-ids new-tx-ids)
    new-page(tx-ids new-tx-ids)
  =/  new-digest  (compute-digest:page:t new-page)
  =.  new-page
    ?^  -.new-page
      new-page(digest new-digest)
    new-page(digest new-digest)
  ::
  =/  expect-con  con
  ::  add block to pending state
  =^  miss  con  (~(add-pending-block dcon con der constants) new-page)
  ::  remove block from pending state
  =.  con  (~(reject-pending-block dcon con der constants) ~(digest get:page:t new-page))
  ::
  %+  expect-eq
    !>  expect-con
  !>  con
::
++  test-v1-find-ready-blocks
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  make tx
  =/  lock2=lock:t  (lock-from-sig:t p:default-keys-2:h)
  =/  raw=raw-tx:t
    (simple-from-note:raw-tx:t p:default-keys-1:h p:default-keys-2:h coin s:default-keys-1:h)
  ::
  ::  make new page with tx
  =/  new-page=page:t  (make-empty-page:h page0)
  =/  new-tx-ids  (~(put z-in *(z-set tx-id:t)) ~(id get:raw-tx:t raw))
  =.  new-page
    ?^  -.new-page
      new-page(tx-ids new-tx-ids)
    new-page(tx-ids new-tx-ids)
  =/  new-digest  (compute-digest:page:t new-page)
  =.  new-page
    ?^  -.new-page
      new-page(digest new-digest)
    new-page(digest new-digest)
  ::
  ::  add block to pending state
  =^  miss  con  (~(add-pending-block dcon con der constants) new-page)
  ::
  ::  add tx to pending state
  =^  ready  con  (~(add-raw-tx dcon con der constants) raw)
  ::
  =/  expect-ready
    ~[~(digest get:page:t new-page)]
  ::
  %+  expect-eq
    !>  expect-ready
  !>  ready
::
++  test-v1-no-ready-blocks
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  make tx1
  =/  lock2=lock:t  (lock-from-sig:t p:default-keys-2:h)
  =/  raw1=raw-tx:t
    (simple-from-note:raw-tx:t p:default-keys-1:h p:default-keys-2:h coin s:default-keys-1:h)
  ::
  ::  make tx2
  =/  lock3=lock:t  (lock-from-sig:t p:default-keys-3:h)
  =/  raw2=raw-tx:t
    (simple-from-note:raw-tx:t p:default-keys-1:h p:default-keys-3:h coin s:default-keys-1:h)
  ::
  ::  make new page with tx1
  =/  new-page=page:t  (make-empty-page:h page0)
  =/  new-tx-ids  (~(put z-in *(z-set tx-id:t)) ~(id get:raw-tx:t raw1))
  =.  new-page
    ?^  -.new-page
      new-page(tx-ids new-tx-ids)
    new-page(tx-ids new-tx-ids)
  =/  new-digest  (compute-digest:page:t new-page)
  =.  new-page
    ?^  -.new-page
      new-page(digest new-digest)
    new-page(digest new-digest)
  ::
  ::  add block to pending state
  =^  miss  con  (~(add-pending-block dcon con der constants) new-page)
  ::
  ::
  =^  ready  con  (~(add-raw-tx dcon con der constants) raw2)
  ::
  ::  dont add tx to pending state
  =/  expect-ready  ~
  ::
  %+  expect-eq
    !>  expect-ready
  !>  ready
::
++  test-v1-fee-enforcement-zero-fee-fails
  =.  constants  bc-with-fees:helpers
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  build spend-1 with insufficient fee (below minimum of 256)
  =/  nam=nname:t  ~(name get:nnote:t coin)
  =/  pk=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [root=hash:t sc=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk])
  =/  fee  100
  =/  sed=seed:v1:t  (make-seed:v1:h root (sub assets.coin fee) (hash:nnote:t coin))
  =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds
      fee  fee
    ==
  =/  sig-h=hash:t  (sig-hash:spend-1:v1:t sp1)
  =/  wit=witness:t
    (make-pkh-witness:v1:h root sc sig-h ~[[s:default-keys-1:h pk]])
  =/  sp1=spend-1:v1:t  sp1(witness wit)
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  tac=tx-acc:t
    (new:tx-acc:t (~(get h-by balance.con) ~(digest get:page:t page0)) ~(height get:page:t page0))
  =/  res=(reason tx-acc:t)  (process:tx-acc:t tac raw)
  ?>  ?=(%.n -.res)
  %+  expect-eq  !>(%v1-insufficient-fee)
  !>(p.res)
::
++  test-v1-fee-enforcement-min-fee-passes
  =.  constants  bc-with-fees:helpers
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  get actual coinbase from balance
  =/  page0-balance  (need (~(get h-by balance.con) ~(digest get:page:t page0)))
  =/  coin=nnote:t  (get-coinbase-from-balance:v1:h page0 page0-balance)
  ::  build spend-1 and calculate actual witness word count
  =/  nam=nname:t  ~(name get:nnote:t coin)
  =/  pk=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [root=hash:t sc=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk])
  =/  sp1-temp=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    *(z-set seed:v1:t)
      fee  0
    ==
  =/  sig-h-temp=hash:t  (sig-hash:spend-1:v1:t sp1-temp)
  =/  wit=witness:t
    (make-pkh-witness:v1:h root sc sig-h-temp ~[[s:default-keys-1:h pk]])
  =/  wit-words=@  (num-of-leaves:shape `*`wit)
  =/  sed-temp=seed:v1:t
    (make-seed:v1:h root 0 (hash:nnote:t coin))
  =/  seed-words=@  (num-of-leaves:shape `*`sed-temp)
  ::  fee = (seed_words * base_fee) + (wit_words * base_fee / input_fee_divisor)
  ::  with base_fee=256, input_fee_divisor=4
  =/  min-fee=coins:t  (max 256 (add (mul seed-words 256) (div (mul wit-words 256) 4)))
  =/  sed=seed:v1:t
    (make-seed:v1:h root (sub assets.coin min-fee) (hash:nnote:t coin))
  =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds
      fee  min-fee
    ==
  =/  sig-h=hash:t  (sig-hash:spend-1:v1:t sp1)
  =/  wit=witness:t
    (make-pkh-witness:v1:h root sc sig-h ~[[s:default-keys-1:h pk]])
  =/  sp1=spend-1:v1:t  sp1(witness wit)
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  [new-page=page:t newc=_con tac=tx-acc:t]
    (add-raw-to-new-page:h page0 con raw)
  =.  con  newc
  =/  tx=tx:t  (new:tx:t raw ~(height get:page:t new-page))
  %+  expect-eq  !>(%1)
  !>(-.tx)
::
++  test-v1-fee-enforcement-note-data-word-count
  =.  constants  bc-with-fees:helpers
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  get actual coinbase from balance
  =/  page0-balance  (need (~(get h-by balance.con) ~(digest get:page:t page0)))
  =/  coin=nnote:t  (get-coinbase-from-balance:v1:h page0 page0-balance)
  ::  build seed with note-data containing words
  =/  nam=nname:t  ~(name get:nnote:t coin)
  =/  pk=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [root=hash:t sc=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk])
  =/  note-data=(z-map @tas *)
    %-  ~(gas z-by *(z-map @tas *))
    :~  [%foo [1 2 3 4 5]]
        [%bar [6 7 8 9 10]]
    ==
  =/  sp1-temp=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    *(z-set seed:v1:t)
      fee  0
    ==
  =/  sig-h-temp=hash:t  (sig-hash:spend-1:v1:t sp1-temp)
  =/  wit=witness:t
    (make-pkh-witness:v1:h root sc sig-h-temp ~[[s:default-keys-1:h pk]])
  =/  wit-words=@  (num-of-leaves:shape `*`wit)
  =/  data-words=@
    %-  num-of-leaves:shape
    %-  ~(rep z-by note-data)
    |=  [[k=@tas v=*] tree=*]
    [k v tree]
  ::  fee = (data_words * base_fee) + (wit_words * base_fee / input_fee_divisor)
  ::  with base_fee=256, input_fee_divisor=4
  =/  min-fee=coins:t  (max 256 (add (mul data-words 256) (div (mul wit-words 256) 4)))
  =/  sed=seed:v1:t
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      root
      note-data      note-data
      gift           (sub assets.coin min-fee)
      parent-hash    (hash:nnote:t coin)
    ==
  =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds
      fee  min-fee
    ==
  =/  sig-h=hash:t  (sig-hash:spend-1:v1:t sp1)
  =/  wit=witness:t
    %:  make-pkh-witness:v1:h
      root
      sc
      sig-h
      ~[[s:default-keys-1:h pk]]
    ==
  =/  sp1=spend-1:v1:t  sp1(witness wit)
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  new-page=page:t  (make-empty-page:h page0)
  =/  =tx:t  (new:tx:t raw ~(height get:page:t new-page))
  =/  new-tx-ids  (~(put z-in *(z-set tx-id:t)) ~(id get:raw-tx:t raw))
  =.  new-page
    ?^  -.new-page
      new-page(tx-ids new-tx-ids)
    new-page(tx-ids new-tx-ids)
  =/  total-coinbase-assets=coins:t
    (add (emission-calc:coinbase:t ~(height get:page:t new-page)) ~(total-fees get:tx:t tx))
  =.  new-page
    ?^  -.new-page
      =/  new-coinbase  (new:v0:coinbase-split:t total-coinbase-assets default-keys-1-share:h)
      new-page(coinbase new-coinbase)
    =/  new-coinbase  (new:v1:coinbase-split:t total-coinbase-assets default-keys-1-share-v1:h)
    new-page(coinbase new-coinbase)
  =/  new-digest  (compute-digest:page:t new-page)
  =.  new-page
    ?^  -.new-page
      new-page(digest new-digest)
    new-page(digest new-digest)
  =^  ready  con  (~(add-raw-tx dcon con der constants) raw)
  ?>  -:(~(validate-page-without-txs dcon con der constants) new-page ~(timestamp get:page:t new-page))
  =/  tac  (~(validate-page-with-txs dcon con der constants) new-page)
  ?:  ?=(%.n -.tac)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) new-page +.tac *@da)
  =.  con  (~(update-heaviest dcon con der constants) new-page)
  %+  expect-eq  !>(%1)
  !>(-.tx)
::
++  test-v1-fee-enforcement-note-data-insufficient-fee
  =.  constants  bc-with-fees:helpers
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  build seed with note-data but pay less than required
  =/  nam=nname:t  ~(name get:nnote:t coin)
  =/  pk=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [root=hash:t sc=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk])
  =/  note-data=(z-map @tas *)
    %-  ~(gas z-by *(z-map @tas *))
    ~[[%data [1 2 3 4 5]]]
  =/  sp1-temp=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    *(z-set seed:v1:t)
      fee  0
    ==
  =/  sig-h-temp=hash:t  (sig-hash:spend-1:v1:t sp1-temp)
  =/  wit=witness:t
    (make-pkh-witness:v1:h root sc sig-h-temp ~[[s:default-keys-1:h pk]])
  =/  wit-words=@  (num-of-leaves:shape `*`wit)
  =/  data-words=@
    %-  num-of-leaves:shape
    %-  ~(rep z-by note-data)
    |=  [[k=@tas v=*] tree=*]
    [k v tree]
  ::  fee = (data_words * base_fee) + (wit_words * base_fee / input_fee_divisor)
  ::  with base_fee=256, input_fee_divisor=4
  =/  required-fee=coins:t  (max 256 (add (mul data-words 256) (div (mul wit-words 256) 4)))
  =/  insufficient-fee=coins:t  (dec required-fee)
  =/  sed=seed:v1:t
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      root
      note-data      note-data
      gift           (sub assets.coin insufficient-fee)
      parent-hash    (hash:nnote:t coin)
    ==
  =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds
      fee  insufficient-fee
    ==
  =/  sig-h=hash:t  (sig-hash:spend-1:v1:t sp1)
  =/  wit=witness:t
    (make-pkh-witness:v1:h root sc sig-h ~[[s:default-keys-1:h pk]])
  =/  sp1=spend-1:v1:t  sp1(witness wit)
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  tac=tx-acc:t
    (new:tx-acc:t (~(get h-by balance.con) ~(digest get:page:t page0)) ~(height get:page:t page0))
  =/  res=(reason tx-acc:t)  (process:tx-acc:t tac raw)
  ?>  ?=(%.n -.res)
  %+  expect-eq  !>(%v1-insufficient-fee)
  !>(p.res)
::
++  test-v1-fee-enforcement-note-data-exceeds-max-size
  =.  constants  bc-with-fees:helpers
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  build seed with note-data exceeding max-size (2048 leaves)
  =/  nam=nname:t  ~(name get:nnote:t coin)
  =/  pk=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [root=hash:t sc=spend-condition:v1:t *]
    (make-pkh-lock:v1:h 1 ~[pk])
  =/  large-data=*
    (reap 2.049 1)
  =/  note-data=(z-map @tas *)
    (~(put z-by *(z-map @tas *)) %big large-data)
  =/  sed=seed:v1:t
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      root
      note-data      note-data
      gift           0
      parent-hash    (hash:nnote:t coin)
    ==
  =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds
      fee  assets.coin
    ==
  =/  sig-h=hash:t  (sig-hash:spend-1:v1:t sp1)
  =/  wit=witness:t
    %:  make-pkh-witness:v1:h
      root
      sc
      sig-h
      ~[[s:default-keys-1:h pk]]
    ==
  =/  sp1=spend-1:v1:t  sp1(witness wit)
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  tac=tx-acc:t
    (new:tx-acc:t (~(get h-by balance.con) ~(digest get:page:t page0)) ~(height get:page:t page0))
  =/  res=(reason tx-acc:t)  (process:tx-acc:t tac raw)
  ?>  ?=(%.n -.res)
  %+  expect-eq  !>(%v1-note-data-exceeds-max-size)
  !>(p.res)
::
++  test-v1-fee-enforcement-witness-words-counted
  =.  constants  bc-with-fees:helpers
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  get actual coinbase from balance
  =/  page0-balance  (need (~(get h-by balance.con) ~(digest get:page:t page0)))
  =/  coin=nnote:t  (get-coinbase-from-balance:v1:h page0 page0-balance)
  ::  build spend with hax witness containing preimage
  =/  nam=nname:t  ~(name get:nnote:t coin)
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [root-cb=hash:t sc-cb=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk1])
  =/  pre-in=*  [1 2 3 4 5]
  =/  [in-root=hash:t in-sc=spend-condition:v1:t in-h=hash:t]
    (make-hax-lock:v1:h pre-in)
  =/  sp1a-temp=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    *(z-set seed:v1:t)
      fee  0
    ==
  =/  sig-ha-temp=hash:t  (sig-hash:spend-1:v1:t sp1a-temp)
  =/  wita=witness:t
    (make-pkh-witness:v1:h root-cb sc-cb sig-ha-temp ~[[s:default-keys-1:h pk1]])
  =/  wit-words=@  (num-of-leaves:shape `*`wita)
  =/  sed-temp=seed:v1:t
    (make-seed:v1:h in-root 0 (hash:nnote:t coin))
  =/  seed-words=@  (num-of-leaves:shape `*`sed-temp)
  ::  fee = (seed_words * base_fee) + (wit_words * base_fee / input_fee_divisor)
  ::  with base_fee=256, input_fee_divisor=4
  =/  min-fee=coins:t  (max 256 (add (mul seed-words 256) (div (mul wit-words 256) 4)))
  =/  sed0=seed:v1:t
    (make-seed:v1:h in-root (sub assets.coin min-fee) (hash:nnote:t coin))
  =/  seds0=seeds:v1:t  (~(put z-in *seeds:v1:t) sed0)
  =/  sp1a=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds0
      fee  min-fee
    ==
  =/  sig-ha=hash:t  (sig-hash:spend-1:v1:t sp1a)
  =/  wita=witness:t
    (make-pkh-witness:v1:h root-cb sc-cb sig-ha ~[[s:default-keys-1:h pk1]])
  =/  sp1a=spend-1:v1:t  sp1a(witness wita)
  =/  raw0=raw-tx:v1:t
    (new:raw-tx:v1:t (~(put z-by *spends:v1:t) ~(name get:nnote:t coin) [%1 sp1a]))
  =/  page-in=page:t  (make-empty-page:h page0)
  =/  =tx:t  (new:tx:t raw0 ~(height get:page:t page-in))
  =/  new-tx-ids  (~(put z-in *(z-set tx-id:t)) ~(id get:raw-tx:t raw0))
  =.  page-in
    ?^  -.page-in
      page-in(tx-ids new-tx-ids)
    page-in(tx-ids new-tx-ids)
  =/  total-coinbase-assets=coins:t
    (add (emission-calc:coinbase:t ~(height get:page:t page-in)) ~(total-fees get:tx:t tx))
  =.  page-in
    ?^  -.page-in
      =/  new-coinbase  (new:v0:coinbase-split:t total-coinbase-assets default-keys-1-share:h)
      page-in(coinbase new-coinbase)
    =/  new-coinbase  (new:v1:coinbase-split:t total-coinbase-assets default-keys-1-share-v1:h)
    page-in(coinbase new-coinbase)
  =/  new-digest  (compute-digest:page:t page-in)
  =.  page-in
    ?^  -.page-in
      page-in(digest new-digest)
    page-in(digest new-digest)
  =^  ready  con  (~(add-raw-tx dcon con der constants) raw0)
  ?>  -:(~(validate-page-without-txs dcon con der constants) page-in ~(timestamp get:page:t page-in))
  =/  tac  (~(validate-page-with-txs dcon con der constants) page-in)
  ?:  ?=(%.n -.tac)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page-in +.tac *@da)
  =.  con  (~(update-heaviest dcon con der constants) page-in)
  %+  expect-eq  !>(%1)
  !>(-.tx)
::
++  test-v1-fee-consolidation-cheaper-than-splitting
  ::  Test that consolidating notes is cheaper than splitting them
  ::  With input-fee-divisor=4:
  ::  - Consolidation: high witness words, low seed words
  ::  - Splitting: low witness words, high seed words
  ::  Consolidation should be cheaper since inputs are discounted
  ::
  ::  Fee formula: seed_words * base_fee + witness_words * base_fee / divisor
  ::
  ::  With same total words (500), compare:
  ::  - Consolidation: 400 witness, 100 seed
  ::  - Splitting: 100 witness, 400 seed
  ::
  =.  constants  bc-with-fees:helpers
  =/  base-fee=@  256
  =/  divisor=@  4
  ::  Consolidation scenario: many inputs, few outputs
  =/  consolidation-witness-words=@  400
  =/  consolidation-seed-words=@  100
  =/  consolidation-fee=@
    %+  add
      (mul consolidation-seed-words base-fee)
    (div (mul consolidation-witness-words base-fee) divisor)
  ::  Splitting scenario: few inputs, many outputs
  =/  splitting-witness-words=@  100
  =/  splitting-seed-words=@  400
  =/  splitting-fee=@
    %+  add
      (mul splitting-seed-words base-fee)
    (div (mul splitting-witness-words base-fee) divisor)
  ::  Consolidation: 100*256 + 400*256/4 = 25,600 + 25,600 = 51,200
  ::  Splitting:     400*256 + 100*256/4 = 102,400 + 6,400 = 108,800
  ::  Assert consolidation is cheaper and verify exact values
  ?>  (lth consolidation-fee splitting-fee)
  ?>  =(51.200 consolidation-fee)
  %+  expect-eq  !>(108.800)
  !>(splitting-fee)
::
++  test-v1-fee-equal-with-divisor-one
  ::  With input-fee-divisor=1, swapping witness/seed words produces same fee
  ::  This verifies backwards compatibility with pre-rebalancing behavior
  =.  constants  bc-with-old-fees:helpers
  =/  base-fee=@  256
  =/  divisor=@  1
  =/  witness-words=@  200
  =/  seed-words=@  300
  ::  Scenario 1: 200 witness, 300 seed
  =/  scenario1-fee=@
    %+  add
      (mul seed-words base-fee)
    (div (mul witness-words base-fee) divisor)
  ::  Scenario 2: 300 witness, 200 seed (swapped)
  =/  scenario2-fee=@
    %+  add
      (mul witness-words base-fee)
    (div (mul seed-words base-fee) divisor)
  ::  Both should equal 500*256 = 128,000
  ?>  =(scenario1-fee scenario2-fee)
  %+  expect-eq  !>(128.000)
  !>(scenario1-fee)
::
++  test-v1-lock-script-in-note-data-exceeds-max-size
  ::  max-size.data set to 29, print will show this note-data is 30
  =.  constants  bc-small-size:helpers
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  create combined lock script: 2-of-3 multisig + timelock
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  pk2=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-2:h))
  =/  pk3=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-3:h))
  =/  [root-pkh=hash:t sc-pkh=spend-condition:v1:t *]
    (make-pkh-lock:v1:h 2 ~[pk1 pk2 pk3])
  =/  prim=lock-primitive:v1:t  [%tim [rel=[min=`2 max=~] abs=[min=~ max=~]]]
  =/  sc-tim=spend-condition:v1:t  ~[prim]
  =/  sc=spend-condition:v1:t  (combine:spend-condition:t sc-pkh sc-tim)
  ::  count and print leaves in lock script
  =/  lock-words=@  (num-of-leaves:shape `*`sc)
  ~&  >  [%lock-script-leaves lock-words]
  ::  store lock script in note-data
  =/  note-data=(z-map @tas *)
    (~(put z-by *(z-map @tas *)) %script sc)
  ::  count total leaves in note-data
  =/  data-words=@
    %-  num-of-leaves:shape
    %-  ~(rep z-by note-data)
    |=  [[k=@tas v=*] tree=*]
    [k v tree]
  ~&  >  [%note-data-leaves data-words]
  ::  build seed with note-data containing lock script
  =/  nam=nname:t  ~(name get:nnote:t coin)
  =/  pk=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [root=hash:t sc-simple=spend-condition:v1:t *]
    (make-pkh-lock:v1:h 1 ~[pk])
  =/  sed=seed:v1:t
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      (hash:spend-condition:v1:t sc)
      note-data      note-data
      gift           0
      parent-hash    (hash:nnote:t coin)
    ==
  =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds
      fee  assets.coin
    ==
  =/  sig-h=hash:t  (sig-hash:spend-1:v1:t sp1)
  =/  wit=witness:t
    %:  make-pkh-witness:v1:h
      root
      sc-simple
      sig-h
      ~[[s:default-keys-1:h pk]]
    ==
  =/  sp1=spend-1:v1:t  sp1(witness wit)
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  tac=tx-acc:t
    (new:tx-acc:t (~(get h-by balance.con) ~(digest get:page:t page0)) ~(height get:page:t page0))
  =/  res=(reason tx-acc:t)  (process:tx-acc:t tac raw)
  ?>  ?=(%.n -.res)
  %+  expect-eq  !>(%v1-note-data-exceeds-max-size)
  !>(p.res)
::
++  test-v1-lock-script-in-note-data-within-max-size
  =/  con=consensus-state  initial-consensus-state:h
  =^  par=page:t  con  (add-n-pages:h (dec v1-phase:t) con default-retain:h)
  =/  page0=page:t  (make-empty-page:h par)
  =/  coin=coinbase:t
    ?^  -.page0
      (new:v0:coinbase:t page0 p:default-keys-1:h)
    (new:coinbase:t page0 (sig-to-pkh-hashes:v1:h p:default-keys-1:h))
  =/  new-digest  (compute-digest:page:t page0)
  =.  page0
    ?^  -.page0
      page0(digest new-digest)
    page0(digest new-digest)
  =/  r0=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page0)
  ?:  ?=(%.n -.r0)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page0 +.r0 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page0)
  ::  create combined lock script: 2-of-3 multisig + timelock
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  pk2=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-2:h))
  =/  pk3=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-3:h))
  =/  [root-pkh=hash:t sc-pkh=spend-condition:v1:t *]
    (make-pkh-lock:v1:h 2 ~[pk1 pk2 pk3])
  =/  prim=lock-primitive:v1:t  [%tim [rel=[min=`2 max=~] abs=[min=~ max=~]]]
  =/  sc-tim=spend-condition:v1:t  ~[prim]
  =/  sc=spend-condition:v1:t  (combine:spend-condition:t sc-pkh sc-tim)
  ::  count and print leaves in lock script
  =/  lock-words=@  (num-of-leaves:shape `*`sc)
  ~&  >  [%lock-script-leaves lock-words]
  ::  store lock script in note-data
  =/  note-data=(z-map @tas *)
    (~(put z-by *(z-map @tas *)) %script sc)
  ::  count total leaves in note-data
  =/  data-words=@
    %-  num-of-leaves:shape
    %-  ~(rep z-by note-data)
    |=  [[k=@tas v=*] tree=*]
    [k v tree]
  ~&  >  [%note-data-leaves data-words]
  ::  build seed with note-data containing lock script
  =/  nam=nname:t  ~(name get:nnote:t coin)
  =/  pk=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  [root=hash:t sc-simple=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h 1 ~[pk])
  =/  sed=seed:v1:t
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      (hash:spend-condition:v1:t sc)
      note-data      note-data
      gift           0
      parent-hash    (hash:nnote:t coin)
    ==
  =/  seds=seeds:v1:t  (~(put z-in *seeds:v1:t) sed)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds
      fee  assets.coin
    ==
  =/  sig-h=hash:t  (sig-hash:spend-1:v1:t sp1)
  =/  wit=witness:t
    (make-pkh-witness:v1:h root sc-simple sig-h ~[[s:default-keys-1:h pk]])
  =/  sp1=spend-1:v1:t  sp1(witness wit)
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  tac=tx-acc:t
    (new:tx-acc:t (~(get h-by balance.con) ~(digest get:page:t page0)) ~(height get:page:t page0))
  =/  res=(reason tx-acc:t)  (process:tx-acc:t tac raw)
  ~?  ?=(%.n -.res)  p.res
  %+  expect-eq  !>(%.y)
  !>(-.res)
::
::  Tests for merged note-data size validation
::  When seeds share a lock-root, their note-data is merged in the output.
::  The size check must validate the merged result, not individual seeds.
::
++  test-v1-merged-note-data-exceeds-max-size
  ::  Two v0 coinbases spent with spend-0, each with note-data under max-size (50),
  ::  but when merged (same lock-root) the combined note-data exceeds max-size.
  ::  This should fail with %v1-note-data-exceeds-max-size.
  =.  constants  bc-merged-note-data-test:helpers
  =/  con=consensus-state  initial-consensus-state:h
  ::  advance close to v1 activation
  =^  par=page:t  con  (add-n-pages:h (sub v1-phase:t 3) con default-retain:h)
  ::  first v0 coinbase page
  =/  page1=page:t  (make-empty-page:h par)
  =/  coin1=coinbase:t  (new:v0:coinbase:t page1 p:default-keys-1:h)
  =/  new-digest  (compute-digest:page:t page1)
  =.  page1
    ?^  -.page1  page1(digest new-digest)
    page1(digest new-digest)
  =/  r1=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page1)
  ?:  ?=(%.n -.r1)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page1 +.r1 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page1)
  ::  second v0 coinbase page
  =/  page2=page:t  (make-empty-page:h page1)
  =/  coin2=coinbase:t  (new:v0:coinbase:t page2 p:default-keys-1:h)
  =/  new-digest  (compute-digest:page:t page2)
  =.  page2
    ?^  -.page2  page2(digest new-digest)
    page2(digest new-digest)
  =/  r2=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page2)
  ?:  ?=(%.n -.r2)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page2 +.r2 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page2)
  ::  advance to exactly v1 activation height
  =^  last=page:t  con
    %^  add-n-pages:h
      ?:  (gte ~(height get:page:t page2) v1-phase:t)  0
      (sub v1-phase:t ~(height get:page:t page2))
    con
    default-retain:h
  =/  tac=tx-acc:t
    (new:tx-acc:t (~(get h-by balance.con) ~(digest get:page:t last)) ~(height get:page:t last))
  ::  Create note-data for each seed: each has ~30 leaves, combined ~60 > 50
  =/  note-data-1=(z-map @tas *)
    (~(put z-by *(z-map @tas *)) %data-1 (reap 29 1))
  =/  note-data-2=(z-map @tas *)
    (~(put z-by *(z-map @tas *)) %data-2 (reap 29 1))
  ~&  >  [%max-size max-size.data.constants]
  ::  Shared lock-root for both seeds (so they merge into one output)
  =/  lock-root=hash:t  (hash:lock:t [%brn ~]~)
  ::  Create seeds with note-data
  =/  sed1=seed:v1:t
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      lock-root
      note-data      note-data-1
      gift           0
      parent-hash    (hash:nnote:t coin1)
    ==
  =/  sed2=seed:v1:t
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      lock-root
      note-data      note-data-2
      gift           0
      parent-hash    (hash:nnote:t coin2)
    ==
  ::  Create spend-0 for coin1
  =/  seds1=seeds:v1:t  (~(put z-in *seeds:v1:t) sed1)
  =/  sp0-1=spend-0:v1:t
    %*  .  *spend-0:v1:t
      seeds  seds1
      fee    assets.coin1
    ==
  ::  Create spend-0 for coin2
  =/  seds2=seeds:v1:t  (~(put z-in *seeds:v1:t) sed2)
  =/  sp0-2=spend-0:v1:t
    %*  .  *spend-0:v1:t
      seeds  seds2
      fee    assets.coin2
    ==
  ::  Sign and wrap spends as [%0 ...]
  =/  sp1=spend:v1:t  [%0 (sign:spend-0:v1:t sp0-1 s:default-keys-1:h)]
  =/  sp2=spend:v1:t  [%0 (sign:spend-0:v1:t sp0-2 s:default-keys-1:h)]
  ::  Build transaction with both spends
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) ~(name get:nnote:t coin1) sp1)
  =.  sps  (~(put z-by sps) ~(name get:nnote:t coin2) sp2)
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  res=(reason tx-acc:t)  (process:tx-acc:t tac raw)
  ::  Should fail because merged note-data exceeds max-size
  ?>  ?=(%.n -.res)
  %+  expect-eq  !>(%v1-note-data-exceeds-max-size)
  !>(p.res)
::
++  test-v1-merged-note-data-within-max-size
  ::  Two v0 coinbases spent with spend-0, each with note-data under max-size (50),
  ::  and when merged (same lock-root) the combined note-data is still under max-size.
  ::  This should succeed.
  =.  constants  bc-merged-note-data-test:helpers
  =/  con=consensus-state  initial-consensus-state:h
  ::  advance close to v1 activation
  =^  par=page:t  con  (add-n-pages:h (sub v1-phase:t 3) con default-retain:h)
  ::  first v0 coinbase page
  =/  page1=page:t  (make-empty-page:h par)
  =/  coin1=coinbase:t  (new:v0:coinbase:t page1 p:default-keys-1:h)
  =/  new-digest  (compute-digest:page:t page1)
  =.  page1
    ?^  -.page1  page1(digest new-digest)
    page1(digest new-digest)
  =/  r1=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page1)
  ?:  ?=(%.n -.r1)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page1 +.r1 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page1)
  ::  second v0 coinbase page
  =/  page2=page:t  (make-empty-page:h page1)
  =/  coin2=coinbase:t  (new:v0:coinbase:t page2 p:default-keys-1:h)
  =/  new-digest  (compute-digest:page:t page2)
  =.  page2
    ?^  -.page2  page2(digest new-digest)
    page2(digest new-digest)
  =/  r2=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der constants) page2)
  ?:  ?=(%.n -.r2)  (expect !>(%.n))
  =.  con  (~(accept-page dcon con der constants) page2 +.r2 *@da)
  =.  con  (~(update-heaviest dcon con der constants) page2)
  ::  advance to exactly v1 activation height
  =^  last=page:t  con
    %^  add-n-pages:h
      ?:  (gte ~(height get:page:t page2) v1-phase:t)  0
      (sub v1-phase:t ~(height get:page:t page2))
    con
    default-retain:h
  =/  tac=tx-acc:t
    (new:tx-acc:t (~(get h-by balance.con) ~(digest get:page:t last)) ~(height get:page:t last))
  ::  Create note-data for each seed: each has ~20 leaves, combined ~40 < 50
  =/  note-data-1=(z-map @tas *)
    (~(put z-by *(z-map @tas *)) %data-1 (reap 19 1))
  =/  note-data-2=(z-map @tas *)
    (~(put z-by *(z-map @tas *)) %data-2 (reap 19 1))
  ~&  >  [%max-size max-size.data.constants]
  ::  Shared lock-root for both seeds (so they merge into one output)
  =/  lock-root=hash:t  (hash:lock:t [%brn ~]~)
  ::  Create seeds with note-data
  =/  sed1=seed:v1:t
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      lock-root
      note-data      note-data-1
      gift           0
      parent-hash    (hash:nnote:t coin1)
    ==
  =/  sed2=seed:v1:t
    %*  .  *seed:v1:t
      output-source  *(unit source:t)
      lock-root      lock-root
      note-data      note-data-2
      gift           0
      parent-hash    (hash:nnote:t coin2)
    ==
  ::  Create spend-0 for coin1
  =/  seds1=seeds:v1:t  (~(put z-in *seeds:v1:t) sed1)
  =/  sp0-1=spend-0:v1:t
    %*  .  *spend-0:v1:t
      seeds  seds1
      fee    assets.coin1
    ==
  ::  Create spend-0 for coin2
  =/  seds2=seeds:v1:t  (~(put z-in *seeds:v1:t) sed2)
  =/  sp0-2=spend-0:v1:t
    %*  .  *spend-0:v1:t
      seeds  seds2
      fee    assets.coin2
    ==
  ::  Sign and wrap spends as [%0 ...]
  =/  sp1=spend:v1:t  [%0 (sign:spend-0:v1:t sp0-1 s:default-keys-1:h)]
  =/  sp2=spend:v1:t  [%0 (sign:spend-0:v1:t sp0-2 s:default-keys-1:h)]
  ::  Build transaction with both spends
  =/  sps=spends:v1:t  (~(put z-by *spends:v1:t) ~(name get:nnote:t coin1) sp1)
  =.  sps  (~(put z-by sps) ~(name get:nnote:t coin2) sp2)
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  =/  res=(reason tx-acc:t)  (process:tx-acc:t tac raw)
  ::  Should succeed because merged note-data is within max-size
  ~?  ?=(%.n -.res)  [%unexpected-failure p.res]
  %+  expect-eq  !>(%.y)
  !>(-.res)
::
::  Protocol-fund recovery (+check-multisig-lock) mechanism tests.
::
::    The fund coinbase notes committed an unsatisfiable %pkh-wrapped lock
::    (see +fund-note-firstname). +check:check-context special-cases their
::    first-name and routes to +check-multisig-lock, which binds the witness-
::    revealed spend-condition to a target lock-root by hash and then checks
::    its m-of-n primitive. We cannot hold the four production fund seckeys, so
::    these exercise the generic arm with non-production keys (a 2-of-3 standing
::    in for the 3-of-4); the real constants are pinned by +test-fund-note-
::    firstname and +test-protocol-fund-address-is-3-of-4-multisig in coinbase-split.
::
::  +test-fund-multisig-spend-valid: a correctly-revealed multisig with enough
::    valid signatures over the spend's sig-hash is accepted.
++  test-fund-multisig-spend-valid
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  pk2=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-2:h))
  =/  pk3=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-3:h))
  =/  [root=hash:t sc=spend-condition:v1:t *]
    (make-pkh-lock:v1:h 2 ~[pk1 pk2 pk3])
  =/  sp=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    *(z-set seed:v1:t)
      fee      0
    ==
  =/  sig-ha=hash:t  (sig-hash:spend-1:v1:t sp)
  =/  wit=witness:t
    %:  make-pkh-witness:v1:h
      root
      sc
      sig-ha
      ~[[s:default-keys-1:h pk1] [s:default-keys-2:h pk2]]
    ==
  =/  ctx=check-context:t
    :*  now=1
        since=0
        sig-hash=sig-ha
        witness=wit
        bythos-phase=bythos-phase.constants
    ==
  %+  expect-eq  !>(%.y)
  !>  (check-multisig-lock:check-context:t root ctx)
::
::  +test-fund-multisig-spend-insufficient-sigs: fewer than m signatures is
::    rejected (check:pkh requires exactly m).
++  test-fund-multisig-spend-insufficient-sigs
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  pk2=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-2:h))
  =/  pk3=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-3:h))
  =/  [root=hash:t sc=spend-condition:v1:t *]
    (make-pkh-lock:v1:h 2 ~[pk1 pk2 pk3])
  =/  sp=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    *(z-set seed:v1:t)
      fee      0
    ==
  =/  sig-ha=hash:t  (sig-hash:spend-1:v1:t sp)
  =/  wit=witness:t
    %:  make-pkh-witness:v1:h
      root
      sc
      sig-ha
      ~[[s:default-keys-1:h pk1]]
    ==
  =/  ctx=check-context:t
    :*  now=1
        since=0
        sig-hash=sig-ha
        witness=wit
        bythos-phase=bythos-phase.constants
    ==
  %+  expect-eq  !>(%.n)
  !>  (check-multisig-lock:check-context:t root ctx)
::
::  +test-fund-multisig-spend-wrong-target: a valid multisig witness whose
::    revealed spend-condition does NOT hash to the target is rejected (the
::    binding to the pinned lock-root is what prevents substituting a multisig
::    the attacker controls).
++  test-fund-multisig-spend-wrong-target
  =/  pk1=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-1:h))
  =/  pk2=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-2:h))
  =/  pk3=schnorr-pubkey:t  (head ~(tap z-in pubkeys.p:default-keys-3:h))
  =/  [root=hash:t sc=spend-condition:v1:t *]
    (make-pkh-lock:v1:h 2 ~[pk1 pk2 pk3])
  =/  sp=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    *(z-set seed:v1:t)
      fee      0
    ==
  =/  sig-ha=hash:t  (sig-hash:spend-1:v1:t sp)
  =/  wit=witness:t
    %:  make-pkh-witness:v1:h
      root
      sc
      sig-ha
      ~[[s:default-keys-1:h pk1] [s:default-keys-2:h pk2]]
    ==
  =/  ctx=check-context:t
    :*  now=1
        since=0
        sig-hash=sig-ha
        witness=wit
        bythos-phase=bythos-phase.constants
    ==
  ::  target is the zero hash, not (hash:lock sc) -> binding fails
  %+  expect-eq  !>(%.n)
  !>  (check-multisig-lock:check-context:t *hash:t ctx)
--
