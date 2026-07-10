/=  helpers  /tests/dumb/helpers
/=  txe  /common/tx-engine
/=  zoon  /common/zoon
/=  *  /common/test
|%
++  h  ~(. helpers bc-v1-phase:helpers)
++  t  ~(. txe bc-v1-phase:helpers)
++  bc-v1-timelock
  %*  .  bc-v1-phase:helpers
    coinbase-timelock-min  2
  ==
::  v1 mempool context validation tests
++  test-v1-mempool-accept-valid
  =+  [nockchain genesis]=init-nockchain:h
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 2 nockchain)
  =/  page-v1=page:t  (snag 1 pages)
  =/  bal  ~(get-cur-balance k-by:h nockchain)
  =/  coin=nnote:t
    (get-coinbase-from-balance:v1:h page-v1 bal)
  =/  pks=(list schnorr-pubkey:t)
    ~(tap z-in:zoon pubkeys.p:default-keys-1:h)
  =/  m=@  (lent pks)
  =/  [root=hash:t sc=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h m pks)
  =/  fee=coins:t  0
  =/  sed=seed:v1:t
    (make-seed:v1:h root (sub assets.coin fee) (hash:nnote:t coin))
  =/  seds=seeds:v1:t  (~(put z-in:zoon *seeds:v1:t) sed)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds
      fee  fee
    ==
  =/  sig-h=hash:t  (sig-hash:spend-1:v1:t sp1)
  =/  pk=schnorr-pubkey:t  (snag 0 pks)
  =/  wit=witness:t
    (make-pkh-witness:v1:h root sc sig-h ~[[s:default-keys-1:h pk]])
  =/  sp1=spend-1:v1:t  sp1(witness wit)
  =/  nam=nname:t  ~(name get:nnote:t coin)
  =/  sps=spends:v1:t  (~(put z-by:zoon *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:t  (new:raw-tx:v1:t sps)
  =/  =cause:h  [%fact %0 %heard-tx raw]
  ~&  [%v1-mempool-accept-valid-raw-tx raw]
  ~&  [%v1-mempool-accept-valid-cause cause]
  =^  effs=(list effect:h)  nockchain
    (pok:h cause nockchain)
  =/  tx-id=tx-id:t  ~(id get:raw-tx:t raw)
  %+  expect-eq
    !>([%.y %.n %.y])
  !>  :*  (~(has-excluded k-by:h nockchain) tx-id)
          (~(has-bnb-raw-tx k-by:h nockchain) tx-id)
          (~(has-raw-tx k-by:h nockchain) tx-id)
      ==
::
++  test-v1-mempool-reject-gifts-fee-mismatch
  =+  [nockchain genesis]=init-nockchain:h
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 2 nockchain)
  =/  page-v0=page:t  (snag 0 pages)
  =/  coin=coinbase:t
    (new:v0:coinbase:t page-v0 p:default-keys-1:h)
  =/  pks=(list schnorr-pubkey:t)
    ~(tap z-in:zoon pubkeys.p:default-keys-1:h)
  =/  pk=schnorr-pubkey:t  (snag 0 pks)
  =/  [root=hash:t * *]
    (make-pkh-lock:v1:h 1 ~[pk])
  =/  fee=coins:t  0
  =/  bad-gift=coins:t  (sub assets.coin 1)
  =/  sed=seed:v1:t
    (make-seed:v1:h root bad-gift (hash:nnote:t coin))
  =/  seds=seeds:v1:t  (~(put z-in:zoon *seeds:v1:t) sed)
  =/  sp0=spend-0:v1:t  (new:spend-0:v1:t seds fee)
  =/  sp0=spend-0:v1:t
    (sign:spend-0:v1:t sp0 s:default-keys-1:h)
  =/  nam=nname:t  ~(name get:nnote:t coin)
  =/  sps=spends:v1:t  (~(put z-by:zoon *spends:v1:t) nam [%0 sp0])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  ~&  [%v1-mempool-reject-gifts-fee-mismatch raw]
  =^  effs=(list effect:h)  nockchain
    (pok:h [%fact %0 %heard-tx raw] nockchain)
  ?>  ?&  !(~(has-raw-tx k-by:h nockchain) id.raw)
          !(~(has-excluded k-by:h nockchain) id.raw)
      ==
  ~
::
++  test-v1-mempool-reject-pkh-missing-sigs
  =+  [nockchain genesis]=init-nockchain:h
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 2 nockchain)
  =/  page-v1=page:t  (snag 1 pages)
  =/  bal  ~(get-cur-balance k-by:h nockchain)
  =/  coin=nnote:t
    (get-coinbase-from-balance:v1:h page-v1 bal)
  =/  pks=(list schnorr-pubkey:t)
    ~(tap z-in:zoon pubkeys.p:default-keys-1:h)
  =/  m=@  (lent pks)
  =/  [root=hash:t sc=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h m pks)
  =/  fee=coins:t  0
  =/  sed=seed:v1:t
    (make-seed:v1:h root (sub assets.coin fee) (hash:nnote:t coin))
  =/  seds=seeds:v1:t  (~(put z-in:zoon *seeds:v1:t) sed)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds
      fee  fee
    ==
  =/  wit=witness:t
    %*  .  *witness:t
      lmp  (build-lock-merkle-proof:lock:t sc 1)
      pkh  *(z-map:zoon hash:t [pk=schnorr-pubkey:t sig=schnorr-signature:t])
      hax  *(z-map:zoon hash:t *)
      tim  ~
    ==
  =/  sp1=spend-1:v1:t  sp1(witness wit)
  =/  nam=nname:t  ~(name get:nnote:t coin)
  =/  sps=spends:v1:t  (~(put z-by:zoon *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  ~&  [%v1-mempool-reject-pkh-missing-sigs raw]
  =^  effs=(list effect:h)  nockchain
    (pok:h [%fact %0 %heard-tx raw] nockchain)
  ?>  ?&  !(~(has-raw-tx k-by:h nockchain) id.raw)
          !(~(has-excluded k-by:h nockchain) id.raw)
      ==
  ~
::
++  test-v1-mempool-reject-pkh-wrong-key
  =+  [nockchain genesis]=init-nockchain:h
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 2 nockchain)
  =/  page-v1=page:t  (snag 1 pages)
  =/  bal  ~(get-cur-balance k-by:h nockchain)
  =/  coin=nnote:t
    (get-coinbase-from-balance:v1:h page-v1 bal)
  =/  pks=(list schnorr-pubkey:t)
    ~(tap z-in:zoon pubkeys.p:default-keys-1:h)
  =/  m=@  (lent pks)
  =/  [root=hash:t sc=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h m pks)
  =/  fee=coins:t  0
  =/  sed=seed:v1:t
    (make-seed:v1:h root (sub assets.coin fee) (hash:nnote:t coin))
  =/  seds=seeds:v1:t  (~(put z-in:zoon *seeds:v1:t) sed)
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds
      fee  fee
    ==
  =/  sig-h=hash:t  (sig-hash:spend-1:v1:t sp1)
  =/  pk-wrong=schnorr-pubkey:t
    (snag 0 ~(tap z-in:zoon pubkeys.p:default-keys-2:h))
  =/  wit=witness:t
    (make-pkh-witness:v1:h root sc sig-h ~[[s:default-keys-2:h pk-wrong]])
  =/  sp1=spend-1:v1:t  sp1(witness wit)
  =/  nam=nname:t  ~(name get:nnote:t coin)
  =/  sps=spends:v1:t  (~(put z-by:zoon *spends:v1:t) nam [%1 sp1])
  =/  raw=raw-tx:v1:t  (new:raw-tx:v1:t sps)
  ~&  [%v1-mempool-reject-pkh-wrong-key raw]
  =^  effs=(list effect:h)  nockchain
    (pok:h [%fact %0 %heard-tx raw] nockchain)
  ?>  ?&  !(~(has-raw-tx k-by:h nockchain) id.raw)
          !(~(has-excluded k-by:h nockchain) id.raw)
      ==
  ~
::
++  test-v1-mempool-reject-timelock
  =+  h-tim=~(. helpers bc-v1-timelock)
  =+  t-tim=~(. txe bc-v1-timelock)
  =+  [nockchain genesis]=init-nockchain:h-tim
  =^  pages  nockchain
    (add-n-pages-integration:h-tim genesis 2 nockchain)
  =/  page-v1=page:t-tim  (snag 1 pages)
  =/  bal  ~(get-cur-balance k-by:h-tim nockchain)
  =/  coin=nnote:t-tim
    (get-coinbase-from-balance:v1:h-tim page-v1 bal)
  =/  pks=(list schnorr-pubkey:t-tim)
    ~(tap z-in:zoon pubkeys.p:default-keys-1:h-tim)
  =/  m=@  (lent pks)
  =/  [root=hash:t-tim sc=spend-condition:v1:t-tim *]
    (make-coinbase-lock:v1:h-tim m pks)
  =/  fee=coins:t-tim  0
  =/  sed=seed:v1:t-tim
    (make-seed:v1:h-tim root (sub assets.coin fee) (hash:nnote:t-tim coin))
  =/  seds=seeds:v1:t-tim  (~(put z-in:zoon *seeds:v1:t-tim) sed)
  =/  sp1=spend-1:v1:t-tim
    %*  .  *spend-1:v1:t-tim
      witness  *witness:v1:t-tim
      seeds    seds
      fee  fee
    ==
  =/  sig-h=hash:t-tim  (sig-hash:spend-1:v1:t-tim sp1)
  =/  pk=schnorr-pubkey:t-tim  (snag 0 pks)
  =/  wit=witness:t-tim
    (make-pkh-witness:v1:h-tim root sc sig-h ~[[s:default-keys-1:h-tim pk]])
  =/  sp1=spend-1:v1:t-tim  sp1(witness wit)
  =/  nam=nname:t-tim  ~(name get:nnote:t-tim coin)
  =/  sps=spends:v1:t-tim  (~(put z-by:zoon *spends:v1:t-tim) nam [%1 sp1])
  =/  raw=raw-tx:v1:t-tim  (new:raw-tx:v1:t-tim sps)
  ~&  [%v1-mempool-reject-timelock raw]
  =^  effs=(list effect:h-tim)  nockchain
    (pok:h-tim [%fact %0 %heard-tx raw] nockchain)
  ?>  ?&  !(~(has-raw-tx k-by:h-tim nockchain) id.raw)
          !(~(has-excluded k-by:h-tim nockchain) id.raw)
      ==
  ~
::
::  +test-v1-mempool-reject-oversize-tx: a transaction too large to ever fit
::  in a block must be discarded on receipt (not stored, not relayed). This
::  closes the pre-packing / block-creation asymmetry that let an oversize tx
::  reach candidate blocks that were then self-rejected as %block-too-large,
::  wedging the chain. Uses a ~10 KB block-size limit and a 25-input coinbase
::  fan-in transaction, which is comfortably over the limit.
++  test-v1-mempool-reject-oversize-tx
  =+  h-med=~(. helpers bc-max-block-size-medium-v0:helpers)
  =+  t-med=~(. txe bc-max-block-size-medium-v0:helpers)
  =+  [nockchain genesis]=init-nockchain:h-med
  =^  pages  nockchain
    (add-n-pages-integration:h-med genesis 85 nockchain)
  =/  raw=raw-tx:t
    %-  from-inputs:v0:raw-tx:t
    %-  multi:new:v0:inputs:t
    %+  turn  (scag 80 pages)
    |=  =page:t
    =/  coin=coinbase:t  (new:v0:coinbase:t page p:default-keys-1:h-med)
    ?>  ?=(^ -.coin)
    %:  simple-from-note:new:v0:input:t
        p:default-keys-2:h-med
        coin
        s:default-keys-1:h-med
    ==
  =/  tx-id=tx-id:t  ~(id get:raw-tx:t raw)
  ::  sanity: this fan-in tx genuinely cannot fit in a block under the small
  ::  size limit, so a rejection is really the size guard firing (not some
  ::  other check). +compute-size-without-txs is the per-block overhead floor.
  =/  overhead=@  (compute-size-without-txs:page:t *page:t)
  ?>  (gth (add ~(size get:raw-tx:t raw) overhead) max-block-size:t-med)
  =^  effs  nockchain
    (pok:h-med [%fact %0 %heard-tx raw] nockchain)
  %+  expect-eq
    !>(%.n)
  !>((~(has-raw-tx k-by:h-med nockchain) tx-id))
--
