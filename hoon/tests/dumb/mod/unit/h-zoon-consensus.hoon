::  tests/dumb/mod/unit/h-zoon-consensus.hoon
::
::    hand-computed consensus flows for h-backed state.
::
/=  dcon  /apps/dumbnet/lib/consensus
/=  dmin  /apps/dumbnet/lib/miner
/=  dumb  /apps/dumbnet/inner
/=  helpers  /tests/dumb/helpers
/=  tx-engine  /common/tx-engine
/=  *  /apps/dumbnet/lib/types
/=  *  /common/h-zoon
/=  *  /common/test
::
|_  constants=_bc-no-timelock:helpers
+*  t  ~(. tx-engine constants)
    h  ~(. helpers constants)
::
++  test-proof-version-3-activation-boundary
  =/  con  ~(. dcon initial-consensus-state:h constants)
  %+  expect-eq
    !>(~[%2 %3 %3])
  !>  :~  (height-to-proof-version:con 120.399)
          (height-to-proof-version:con 120.400)
          (height-to-proof-version:con 120.401)
      ==
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
  =/  k8=kernel-state-8
    %*  .  *kernel-state-8
      c  legacy-c
    ==
  =/  k9=kernel-state  (load:inner:dumb k8)
  =/  c9=consensus-state  c.k9
  =/  bid=block-id:t  ~(digest get:page:t pag)
  =/  upgraded-block=local-page:t  (~(got h-by blocks.c9) bid)
  =/  original-block=local-page:t  (~(got h-by blocks.con8) bid)
  %+  expect-eq
    !>([%.y %.y %.y %.y %.y %.y %.y %.y %.y])
  !>  :*  =(%10 -.k9)
          (consensus-h-apt c9)
          =((hz-molt blocks.c9) (hz-molt blocks.con8))
          =((hz-milt balance.c9) (hz-milt balance.con8))
          =((hz-milt txs.c9) (hz-milt txs.con8))
          =((hz-molt min-timestamps.c9) (hz-molt min-timestamps.con8))
          =((hz-molt targets.c9) (hz-molt targets.con8))
          =(*(map @tas (h-map block-id:t @)) asert-anchor-min-timestamps.c9)
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
  =^  missing=(list tx-id:t)  con  (~(add-pending-block dcon con constants) pending-page)
  =^  ready=(list block-id:t)  con  (~(add-raw-tx dcon con constants) raw-needed)
  =/  raw-extra=raw-tx:t  (make-raw-tx-from-coinbase:v0:h p:default-keys-1:h cb-b)
  =/  extra-id=tx-id:t  ~(id get:raw-tx:t raw-extra)
  =^  extra-ready=(list block-id:t)  con  (~(add-raw-tx dcon con constants) raw-extra)
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
  =/  k8=kernel-state-8
    %*  .  *kernel-state-8
      c  legacy-c
    ==
  =/  k9=kernel-state  (load:inner:dumb k8)
  =/  up=consensus-state  c.k9
  %+  expect-eq
    !>([%.y %.y %.y %.y %.y %.y %.y %.y %.y %.y %.y %.y %.y %.y %.y])
  !>  :*  =(%10 -.k9)
          =(~[needed-id] missing)
          =(~[pending-id] ready)
          =(~ extra-ready)
          (consensus-h-apt up)
          =((hz-jult blocks-needed-by.up) (hz-jult blocks-needed-by.con))
          =((hz-silt excluded-txs.up) (hz-silt excluded-txs.con))
          =((hz-jult spent-by.up) (hz-jult spent-by.con))
          =((hz-molt pending-blocks.up) (hz-molt pending-blocks.con))
          =((hz-milt balance.up) (hz-milt balance.con))
          =((hz-milt txs.up) (hz-milt txs.con))
          =((hz-molt raw-txs.up) (hz-molt raw-txs.con))
          =((hz-molt blocks.up) (hz-molt blocks.con))
          (~(has h-by pending-blocks.up) pending-id)
          (~(has h-in excluded-txs.up) extra-id)
      ==
::
++  test-kernel-state-8-upgrade-keeps-shared-pending-fanout
  =/  con=consensus-state  initial-consensus-state:h
  =^  cb-page=page:t  con  (add-n-pages:h 1 con default-retain:h)
  =/  raw-needed=raw-tx:t  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h cb-page)
  =/  needed-id=tx-id:t  ~(id get:raw-tx:t raw-needed)
  =/  pag-a=page:t  (make-page-with-coinbase-spend:v0:h cb-page cb-page)
  =/  pag-b=page:t  (with-msg (make-page-with-coinbase-spend:v0:h cb-page cb-page) 'UPGRADE-FANOUT')
  =/  bid-a=block-id:t  ~(digest get:page:t pag-a)
  =/  bid-b=block-id:t  ~(digest get:page:t pag-b)
  =^  missing-a=(list tx-id:t)  con  (~(add-pending-block dcon con constants) pag-a)
  =^  missing-b=(list tx-id:t)  con  (~(add-pending-block dcon con constants) pag-b)
  =/  raw-extra=raw-tx:t  (make-raw-tx-from-coinbase:v0:h p:default-keys-3:h cb-page)
  =/  extra-id=tx-id:t  ~(id get:raw-tx:t raw-extra)
  =^  extra-ready=(list block-id:t)  con  (~(add-raw-tx dcon con constants) raw-extra)
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
  =/  k8=kernel-state-8
    %*  .  *kernel-state-8
      c  legacy-c
    ==
  =/  k9=kernel-state  (load:inner:dumb k8)
  =/  up=consensus-state  c.k9
  =/  upgraded-ok=?
    ?&  =(~[needed-id] missing-a)
        =(~[needed-id] missing-b)
        =(~ extra-ready)
        (~(has h-by pending-blocks.up) bid-a)
        (~(has h-by pending-blocks.up) bid-b)
        (~(has h-ju blocks-needed-by.up) needed-id bid-a)
        (~(has h-ju blocks-needed-by.up) needed-id bid-b)
        (~(has h-by raw-txs.up) extra-id)
        (~(has h-in excluded-txs.up) extra-id)
        ?!((~(has h-in excluded-txs.up) needed-id))
        (consensus-h-apt up)
    ==
  =.  up  (~(reject-pending-block dcon up constants) bid-a)
  =/  after-one-reject-ok=?
    ?&  ?!((~(has h-by pending-blocks.up) bid-a))
        (~(has h-by pending-blocks.up) bid-b)
        ?!((~(has h-ju blocks-needed-by.up) needed-id bid-a))
        (~(has h-ju blocks-needed-by.up) needed-id bid-b)
        ?!((~(has h-in excluded-txs.up) needed-id))
        (~(has h-in excluded-txs.up) extra-id)
        (consensus-h-apt up)
    ==
  %+  expect-eq
    !>([%.y %.y %.y])
  !>  [=(%10 -.k9) upgraded-ok after-one-reject-ok]
::
++  test-consensus-add-raw-tx-h-index-flow
  =/  con=consensus-state  initial-consensus-state:h
  =^  cb-page=page:t  con  (add-n-pages:h 1 con default-retain:h)
  =/  raw=raw-tx:t  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h cb-page)
  =/  tid=tx-id:t  ~(id get:raw-tx:t raw)
  =/  names=(z-set nname:t)  ~(input-names get:raw-tx:t raw)
  =^  ready=(list block-id:t)  con  (~(add-raw-tx dcon con constants) raw)
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
  =/  header-ok=?  -:(~(validate-page-without-txs dcon con constants) pag ~(timestamp get:page:t pag))
  =^  missing=(list tx-id:t)  con  (~(add-pending-block dcon con constants) pag)
  =/  pending-ok=?
    ?&  =(~[tid] missing)
        (~(has h-by pending-blocks.con) bid)
        (~(has h-ju blocks-needed-by.con) tid bid)
        ?!((~(has h-in excluded-txs.con) tid))
        (consensus-h-apt con)
    ==
  =^  ready=(list block-id:t)  con  (~(add-raw-tx dcon con constants) raw)
  =/  raw-ok=?
    ?&  =(~[bid] ready)
        (~(has h-by raw-txs.con) tid)
        (~(has h-ju blocks-needed-by.con) tid bid)
        ?!((~(has h-in excluded-txs.con) tid))
        (consensus-h-apt con)
    ==
  =/  r=(reason tx-acc:t)  (~(validate-page-with-txs dcon con constants) pag)
  =/  txs-ok=?  ?=(%.y -.r)
  ?.  ?=(%.y -.r)
    ~&  h-zoon-pending-validate-failed++.r  !!
  =/  acc=tx-acc:t  +.r
  =.  con  (~(accept-page dcon con constants) pag acc *@da)
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
  =^  ready=(list block-id:t)  con  (~(add-raw-tx dcon con constants) raw)
  =/  raw-only-ok=?  ?&  =(~ ready)  (~(has h-in excluded-txs.con) tid)  ==
  =^  missing=(list tx-id:t)  con  (~(add-pending-block dcon con constants) pag)
  =/  no-pending-ok=?
    ?&  =(~ missing)
        ?!((~(has h-by pending-blocks.con) bid))
        (~(has h-in excluded-txs.con) tid)
    ==
  =/  r=(reason tx-acc:t)  (~(validate-page-with-txs dcon con constants) pag)
  ?.  ?=(%.y -.r)
    ~&  h-zoon-raw-first-validate-failed++.r  !!
  =.  con  (~(accept-page dcon con constants) pag +.r *@da)
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
  =^  missing-a=(list tx-id:t)  con  (~(add-pending-block dcon con constants) pag-a)
  =^  missing-b=(list tx-id:t)  con  (~(add-pending-block dcon con constants) pag-b)
  =^  ready=(list block-id:t)  con  (~(add-raw-tx dcon con constants) raw)
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
  =.  con  (~(reject-pending-block dcon con constants) bid-a)
  =/  one-rejected-ok=?
    ?&  ?!((~(has h-by pending-blocks.con) bid-a))
        (~(has h-by pending-blocks.con) bid-b)
        ?!((~(has h-ju blocks-needed-by.con) tid bid-a))
        (~(has h-ju blocks-needed-by.con) tid bid-b)
        ?!((~(has h-in excluded-txs.con) tid))
        (consensus-h-apt con)
    ==
  =.  con  (~(reject-pending-block dcon con constants) bid-b)
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
  =^  missing=(list tx-id:t)  con  (~(add-pending-block dcon con constants) pag)
  =^  ready=(list block-id:t)  con  (~(add-raw-tx dcon con constants) raw)
  =.  con  (~(reject-pending-block dcon con constants) bid)
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
  =^  ready=(list block-id:t)  con  (~(add-raw-tx dcon con constants) raw)
  =/  before-ok=?
    ?&  =(~ ready)
        (~(has h-by raw-txs.con) tid)
        (~(has h-in excluded-txs.con) tid)
        %-  ~(all z-in names)
        |=  nam=nname:t
        (~(has h-ju spent-by.con) nam tid)
    ==
  =.  con  (~(drop-tx dcon con constants) tid)
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
  =/  repaired=consensus-state  ~(check-and-repair dcon broken constants)
  %+  expect-eq
    !>([`%txs-fell-through-cracks %.y %.y %.y])
  !>  :*  ~(apt dcon broken constants)
          (~(has h-by raw-txs.repaired) tid)
          (~(has h-in excluded-txs.repaired) tid)
          (consensus-h-apt repaired)
      ==
::
++  test-kernel-peek-legacy-and-native-h-scries-agree
  =/  con=consensus-state  initial-consensus-state:h
  =^  cb-page=page:t  con  (add-n-pages:h 1 con default-retain:h)
  =/  raw=raw-tx:t  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h cb-page)
  =/  tid=tx-id:t  ~(id get:raw-tx:t raw)
  =/  pag=page:t  (make-page-with-coinbase-spend:v0:h cb-page cb-page)
  =/  bid=block-id:t  ~(digest get:page:t pag)
  =^  ready=(list block-id:t)  con  (~(add-raw-tx dcon con constants) raw)
  ?>  =(~ ready)
  =/  r=(reason tx-acc:t)  (~(validate-page-with-txs dcon con constants) pag)
  ?.  ?=(%.y -.r)
    ~&  h-zoon-scry-validate-failed++.r  !!
  =.  con  (~(accept-page dcon con constants) pag +.r *@da)
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
  =.  seeded  (~(heard-new-block dmin seeded constants) con *@da)
  =/  raw=raw-tx:t  (make-raw-tx-from-coinbase:v0:h p:default-keys-2:h pag)
  =^  ready=(list block-id:t)  con  (~(add-raw-tx dcon con constants) raw)
  ?>  =(~ ready)
  =/  no-key=mining-state
    %*  .  seeded
      v0-shares  *(z-map sig:t @)
      shares     *(z-map hash:t @)
    ==
  ?>  ?&  ?=(^ -.candidate-block.no-key)
          (~(has h-in excluded-txs.con) ~(id get:raw-tx:t raw))
      ==
  =/  after=mining-state  (~(add-txs-to-candidate dmin no-key constants) con)
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
    =^  ready=(list block-id:t)  con.ws  (~(add-raw-tx dcon con.ws constants) raw)
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
    =/  ok=?  -:(~(validate-page-without-txs dcon con.ws constants) pag ~(timestamp get:page:t pag))
    ?.  ok  ws
    =^  missing=(list tx-id:t)  con.ws  (~(add-pending-block dcon con.ws constants) pag)
    ws(con con.ws, pending [bid pending.ws], tick +(tick.ws))
  ::
      %3
    ?.  ?=(^ pending.ws)  ws
    =/  bid=block-id:t  (pick-block-id pending.ws (walk-rand seed (add ctr 71)))
    ?.  (~(has h-by pending-blocks.con.ws) bid)
      ::  drift between our shadow pending list and consensus; drop the
      ::  stale id from the shadow and keep walking.
      ws(pending (filter-block-ids pending.ws bid))
    =.  con.ws  (~(reject-pending-block dcon con.ws constants) bid)
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
    =.  con.ws  (~(drop-tx dcon con.ws constants) tid)
    ws(con con.ws, raw-ids (filter-tx-ids raw-ids.ws tid), tick +(tick.ws))
  ::
      %5
    =.  con.ws  ~(check-and-repair dcon con.ws constants)
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
