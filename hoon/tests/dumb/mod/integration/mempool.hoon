/=  helpers  /tests/dumb/helpers
/=  txe  /common/tx-engine
/=  zoon  /common/zoon
/=  *  /common/test
|%
++  h  ~(. helpers bc-v1-phase-provable:helpers)
++  t  ~(. txe bc-v1-phase-provable:helpers)
++  bc-v1-timelock
  %*  .  bc-v1-phase-provable:helpers
    coinbase-timelock-min  2
  ==
::  v1 mempool context validation tests
++  test-v1-mempool-accept-valid
  =+  [nockchain raw]=(setup-v1-spendable-tx 256)
  =/  =cause:h  [%fact %0 %heard-tx raw]
  =^  effs=(list effect:h)  nockchain
    (pok:h cause nockchain)
  =/  tx-id=tx-id:t  ~(id get:raw-tx:t raw)
  %+  expect-eq
    !>([%.y %.n %.y])
  !>  :*  (~(has-excluded k-by:h nockchain) tx-id)
          (~(has-bnb-raw-tx k-by:h nockchain) tx-id)
          (~(has-raw-tx k-by:h nockchain) tx-id)
      ==
:::
++  test-v1-mempool-reject-under-minimum-fee
  =+  [nockchain raw]=(setup-v1-spendable-tx 0)
  =/  =cause:h  [%fact %0 %heard-tx raw]
  =^  effs=(list effect:h)  nockchain
    (pok:h cause nockchain)
  =/  tx-id=tx-id:t  ~(id get:raw-tx:t raw)
  %+  expect-eq
    !>([%.y %.y %.y])
  !>  :*  !(~(has-excluded k-by:h nockchain) tx-id)
          !(~(has-raw-tx k-by:h nockchain) tx-id)
          !(~(has z-in:zoon (filter-heard-tx-effects:h effs)) raw)
      ==

:::
:::  +test-v0-mempool-reject-tx-at-cutoff: when the next candidate is at the
:::  v1 cutover, v0 txs can no longer be mined, so admission must discard them
:::  instead of retaining and re-gossiping permanently unmineable transactions.
++  test-v0-mempool-reject-tx-at-cutoff
  =+  [nockchain genesis]=init-nockchain:h
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 1 nockchain)
  =/  page-v0=page:t  (snag 0 pages)
  =/  coin=coinbase:t  (new:v0:coinbase:t page-v0 p:default-keys-1:h)
  ?>  ?=(^ -.coin)
  =/  raw=raw-tx:t
    %-  from-inputs:v0:raw-tx:t
    %-  new:v0:inputs:t
    %:  simple-from-note:new:v0:input:t
        p:default-keys-2:h
        coin
        s:default-keys-1:h
    ==
  ::  sanity: the raw-tx is v0-shaped. The current tip is v1-phase - 1, so
  ::  its only candidate height is exactly v1-phase and rejects v0 txs.
  ?>  ?=(^ -.raw)
  =/  tx-id=tx-id:t  ~(id get:raw-tx:t raw)
  =^  effs=(list effect:h)  nockchain
    (pok:h [%fact %0 %heard-tx raw] nockchain)
  %+  expect-eq
    !>([%.y %.y %.y])
  !>  :*  !(~(has-excluded k-by:h nockchain) tx-id)
          !(~(has-raw-tx k-by:h nockchain) tx-id)
          !(~(has z-in:zoon (filter-heard-tx-effects:h effs)) raw)
      ==
:::
:::  +test-v0-mempool-accepts-pre-cutoff: before the v1 cutover, v0 txs are
:::  minable and admission must still accept them.
++  test-v0-mempool-accepts-pre-cutoff
  =+  h-v0=~(. helpers bc-max-block-size-medium-v0-provable:helpers)
  =+  t-v0=~(. txe bc-max-block-size-medium-v0-provable:helpers)
  =+  [nockchain genesis]=init-nockchain:h-v0
  =^  pages  nockchain
    (add-n-pages-integration:h-v0 genesis 2 nockchain)
  =/  page-v0=page:t-v0  (snag 0 pages)
  =/  coin=coinbase:t-v0  (new:v0:coinbase:t-v0 page-v0 p:default-keys-1:h-v0)
  ?>  ?=(^ -.coin)
  =/  raw=raw-tx:t-v0
    %-  from-inputs:v0:raw-tx:t-v0
    %-  new:v0:inputs:t-v0
    %:  simple-from-note:new:v0:input:t-v0
        p:default-keys-2:h-v0
        coin
        s:default-keys-1:h-v0
    ==
  ::  sanity: the raw-tx is v0-shaped; v1 never activates in this config,
  ::  so the chain is before the cutover and v0 txs are minable
  ?>  ?=(^ -.raw)
  =/  tx-id=tx-id:t-v0  ~(id get:raw-tx:t-v0 raw)
  =^  effs=(list effect:h-v0)  nockchain
    (pok:h-v0 [%fact %0 %heard-tx raw] nockchain)
  %+  expect-eq
    !>([%.y %.y])
  !>  :*  (~(has-excluded k-by:h-v0 nockchain) tx-id)
          (~(has-raw-tx k-by:h-v0 nockchain) tx-id)
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
  ::  fee must clear the admission fee floor so this test exercises the
  ::  gifts/fee conservation check, not the fee floor itself
  =/  fee=coins:t  256
  =/  bad-gift=coins:t  (sub (sub assets.coin fee) 1)
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
  ::  fee clears the admission fee floor so this test exercises the
  ::  missing-signature rejection, not the fee floor itself
  =/  fee=coins:t  256
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
  ::  fee clears the admission fee floor so this test exercises the
  ::  wrong-key rejection, not the fee floor itself
  =/  fee=coins:t  256
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
  ::  fee clears the admission fee floor so this test exercises the
  ::  timelock rejection, not the fee floor itself
  =/  fee=coins:t-tim  256
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
  ::  One more parent makes the next candidate exactly mature. Admission
  ::  must not lag the miner by validating against the parent height.
  =^  more  nockchain
    (add-n-pages-integration:h-tim page-v1 1 nockchain)
  =^  accepted=(list effect:h-tim)  nockchain
    (pok:h-tim [%fact %0 %heard-tx raw] nockchain)
  %+  expect-eq  !>(%.y)
  !>  (~(has-raw-tx k-by:h-tim nockchain) id.raw)
::
::  +test-v1-mempool-reject-oversize-tx: a transaction too large to ever fit
::  in a block must be discarded on receipt (not stored, not relayed). This
::  closes the pre-packing / block-creation asymmetry that let an oversize tx
::  reach candidate blocks that were then self-rejected as %block-too-large,
::  wedging the chain. Uses a ~10 KB block-size limit and a 25-input coinbase
::  fan-in transaction, which is comfortably over the limit.
++  test-v1-mempool-reject-oversize-tx
  =+  h-med=~(. helpers bc-max-block-size-medium-v0-provable:helpers)
  =+  t-med=~(. txe bc-max-block-size-medium-v0-provable:helpers)
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
::
:::  Re-broadcast gating for duplicate txs. +heard-tx never re-adds a tx that
:::  is already in raw-txs. Local grpc duplicates are gossiped; peer-origin
:::  duplicates stay silent after %seen.
:::
:::  +setup-v1-spendable-tx: a valid v1 tx spending the coinbase of a 2-block
:::  chain, plus the kernel that chain lives in.
++  setup-v1-spendable-tx
  |=  fee=coins:t
  ^-  [_nockchain:h raw-tx:t]
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
  [nockchain (new:raw-tx:v1:t sps)]
:::
::  The coinbase lock is one combined condition. Rebuild its honest STUB,
::  retaining the real keys, signatures, seeds and fee from the FULL fixture.
++  setup-v1-stub-axis-pair
  ^-  [_nockchain:h raw-tx:t raw-tx:t]
  =+  [nockchain full]=(setup-v1-spendable-tx 256)
  ?>  ?=(@ -.full)
  =/  [nam=nname:t sp=spend:v1:t]  (snag 0 ~(tap z-by:zoon spends.full))
  ?>  ?=(%1 -.sp)
  =/  proof=lock-merkle-proof:t  lmp.witness.+.sp
  ?>  ?=([%full * * *] proof)
  =/  stub=lock-merkle-proof-stub:v1:t
    (build-lock-merkle-proof-stub:lock:t spend-condition.proof 1)
  ?>  ?=(^ spend-condition.stub)
  =/  wit=witness:t  witness.+.sp
  =/  honest-wit=witness:t  wit(lmp stub)
  =/  bad-wit=witness:t  wit(lmp stub(axis 7))
  =/  honest-sp=spend-1:v1:t  +.sp
  =/  honest-sp=spend-1:v1:t  honest-sp(witness honest-wit)
  =/  bad-sp=spend-1:v1:t  honest-sp(witness bad-wit)
  =/  honest=raw-tx:v1:t
    (new:raw-tx:v1:t (~(put z-by:zoon *spends:v1:t) nam [%1 honest-sp]))
  =/  bad=raw-tx:v1:t
    (new:raw-tx:v1:t (~(put z-by:zoon *spends:v1:t) nam [%1 bad-sp]))
  ?>  !=(-.nam fund-note-firstname:t)
  ?>  !=(honest bad)
  ?>  =(id.honest id.bad)
  ?>  =(id.honest (compute-id:raw-tx:t honest))
  ?>  =(id.bad (compute-id:raw-tx:t bad))
  ?>  (based:raw-tx:t honest)
  ?>  (based:raw-tx:t bad)
  ?>  (validate:raw-tx:t honest)
  ?>  (validate:raw-tx:t bad)
  ?>  =(seeds.honest-sp seeds.bad-sp)
  ?>  =(fee.honest-sp fee.bad-sp)
  ?>  =(pkh.witness.honest-sp pkh.witness.bad-sp)
  ?>  !=(pkh.witness.honest-sp *(z-map:zoon hash:t [pk=schnorr-pubkey:t sig=schnorr-signature:t]))
  ?>  (verify-signatures:spend-1:v1:t honest-sp)
  ?>  (verify-signatures:spend-1:v1:t bad-sp)
  =/  tip=page:t  ~(tip-page k-by:h nockchain)
  =/  bal  ~(get-cur-balance k-by:h nockchain)
  =/  tac=tx-acc:t  (new:tx-acc:t `bal +(~(height get:page:t tip)))
  =/  valid  (dispatch:tx-acc:t tac honest)
  =/  invalid  (dispatch:tx-acc:t tac bad)
  ?>  ?=(%.y -.valid)
  ?>  ?=(%.n -.invalid)
  ?>  (valid-stub-axes:raw-tx:t honest)
  ?>  !(valid-stub-axes:raw-tx:t bad)
  [nockchain honest bad]
::::
::  The header must pay the fixture's fee, not the empty-page subsidy alone.
++  stub-axis-child
  |=  [tip=page:t id=tx-id:t]
  ^-  page:t
  =/  candidate=page:t  (make-page-with-txs:v0:h tip ~[id])
  ?>  ?=(@ -.candidate)
  =.  candidate
    candidate(coinbase (new:v1:coinbase-split:t (add 256 (emission-calc:coinbase:t ~(height get:page:t candidate))) default-keys-1-share-v1:h))
  (prove-page:h `page:t`candidate)
::::
++  test-v1-pending-stub-axis-same-id-recovery
  =+  [nockchain honest bad]=setup-v1-stub-axis-pair
  =/  id=tx-id:t  ~(id get:raw-tx:t honest)
  =/  tip=page:t  ~(tip-page k-by:h nockchain)
  =/  proven=page:t  (stub-axis-child tip id)
  =/  bid=block-id:t  ~(digest get:page:t proven)
  =^  requested=(list effect:h)  nockchain
    (pok:h [%fact %0 %heard-block proven] nockchain)
  ?>  (~(has-pending-block k-by:h nockchain) bid)
  ?>  (~(has-bnb-block-id k-by:h nockchain) id bid)
  ?>  !(~(has-raw-tx k-by:h nockchain) id)
  ?>  (~(has z-in:zoon (filter-request-tx-effects:h requested)) id)
  =/  before  ~(con k-by:h nockchain)
  =^  rejected=(list effect:h)  nockchain
    (pok-on-wire:h `path`/poke/libp2p/1/gossip/peer-id/bad [%fact %0 %heard-tx bad] nockchain)
  ?>  =(before ~(con k-by:h nockchain))
  ?>  =(~(digest get:page:t tip) ~(heaviest-block k-by:h nockchain))
  ?>  (~(has-pending-block k-by:h nockchain) bid)
  ?>  (~(check-bnb k-by:h nockchain) id bid)
  ?>  !(~(has-raw-tx k-by:h nockchain) id)
  ?>  !(~(has-excluded k-by:h nockchain) id)
  ?>  !(~(has-spent-by-set k-by:h nockchain) ~(input-names get:raw-tx:t bad) id)
  ?>  =(~ (skim rejected |=(e=effect:h ?=([%seen %tx *] e))))
  ?>  =(~ (filter-heard-tx-effects:h rejected))
  =^  accepted=(list effect:h)  nockchain
    (pok-on-wire:h `path`/poke/libp2p/1/gossip/peer-id/honest [%fact %0 %heard-tx honest] nockchain)
  ?>  =(bid ~(heaviest-block k-by:h nockchain))
  ?>  !(~(has-pending-block k-by:h nockchain) bid)
  ?>  (~(check-bnb k-by:h nockchain) id bid)
  ?>  =(`honest (~(get-bnb-raw-tx k-by:h nockchain) id))
  ?>  (~(has-spent-by-set k-by:h nockchain) ~(input-names get:raw-tx:t honest) id)
  (expect-eq !>(~) !>(~(consensus-invariants k-by:h nockchain)))
::::
++  test-v1-mempool-stub-axis-bad-then-honest
  =+  [nockchain honest bad]=setup-v1-stub-axis-pair
  =/  id=tx-id:t  ~(id get:raw-tx:t honest)
  =/  before  ~(con k-by:h nockchain)
  =^  rejected=(list effect:h)  nockchain
    (pok-on-wire:h `path`/poke/libp2p/1/gossip/peer-id/bad [%fact %0 %heard-tx bad] nockchain)
  ?>  =(before ~(con k-by:h nockchain))
  ?>  !(~(has-raw-tx k-by:h nockchain) id)
  ?>  !(~(has-excluded k-by:h nockchain) id)
  ?>  !(~(has-spent-by-set k-by:h nockchain) ~(input-names get:raw-tx:t bad) id)
  ?>  =(~ (skim rejected |=(e=effect:h ?=([%seen %tx *] e))))
  ?>  =(~ (filter-heard-tx-effects:h rejected))
  =^  accepted=(list effect:h)  nockchain
    (pok-on-wire:h `path`/poke/libp2p/1/gossip/peer-id/honest [%fact %0 %heard-tx honest] nockchain)
  ?>  (~(check-excluded k-by:h nockchain) id)
  ?>  =(`honest (~(get-excluded k-by:h nockchain) id))
  ?>  (~(has z-in:zoon (filter-heard-tx-effects:h accepted)) honest)
  (expect-eq !>(~) !>(~(consensus-invariants k-by:h nockchain)))
::::
++  test-v1-mempool-stub-axis-honest-then-bad-resends
  =+  [nockchain honest bad]=setup-v1-stub-axis-pair
  =/  id=tx-id:t  ~(id get:raw-tx:t honest)
  =/  proven=page:t  (stub-axis-child ~(tip-page k-by:h nockchain) id)
  =/  bid=block-id:t  ~(digest get:page:t proven)
  =^  accepted=(list effect:h)  nockchain
    (pok-on-wire:h libp2p-gossip-wire:h [%fact %0 %heard-tx honest] nockchain)
  ?>  (~(check-excluded k-by:h nockchain) id)
  =/  before  ~(con k-by:h nockchain)
  =^  peer=(list effect:h)  nockchain
    (pok-on-wire:h `path`/poke/libp2p/1/gossip/peer-id/bad [%fact %0 %heard-tx bad] nockchain)
  ?>  =(before ~(con k-by:h nockchain))
  =^  local=(list effect:h)  nockchain
    (pok-on-wire:h grpc-wire:h [%fact %0 %heard-tx bad] nockchain)
  ?>  =(before ~(con k-by:h nockchain))
  ?>  =(`honest (~(get-excluded k-by:h nockchain) id))
  ?>  =(~ (skim (weld peer local) |=(e=effect:h ?=([%seen %tx *] e))))
  ?>  =(~ (filter-heard-tx-effects:h (weld peer local)))
  =^  promoted=(list effect:h)  nockchain
    (pok:h [%fact %0 %heard-block proven] nockchain)
  ?>  =(bid ~(heaviest-block k-by:h nockchain))
  ?>  =(`honest (~(get-bnb-raw-tx k-by:h nockchain) id))
  ?>  (~(check-bnb k-by:h nockchain) id bid)
  (expect-eq !>(~) !>(~(consensus-invariants k-by:h nockchain)))
::::
++  setup-v1-output-source-pair
  |=  [pinned=? valid-source=?]
  ^-  [_nockchain:h raw-tx:t raw-tx:t]
  =+  [nockchain original]=(setup-v1-spendable-tx 256)
  ?>  ?=(@ -.original)
  =/  [nam=nname:t sp=spend:v1:t]
    (snag 0 ~(tap z-by:zoon spends.original))
  ?>  ?=(%1 -.sp)
  =/  honest-sp=spend-1:v1:t  +.sp
  =/  sed=seed:v1:t  (snag 0 ~(tap z-in:zoon seeds.honest-sp))
  ?>  =(~ output-source.sed)
  =/  source=source:t  [(hash:seeds:v1:t seeds.honest-sp) %.n]
  =.  honest-sp
    ?.  pinned  honest-sp
    =/  pinned-seed=seed:v1:t  sed(output-source `source)
    =/  pinned-sp=spend-1:v1:t
      honest-sp(seeds (~(put z-in:zoon *seeds:v1:t) pinned-seed))
    (sign:spend-1:v1:t pinned-sp s:default-keys-1:h)
  =/  honest=raw-tx:v1:t
    (new:raw-tx:v1:t (~(put z-by:zoon *spends:v1:t) nam [%1 honest-sp]))
  =/  bad-source=source:t  ?:(valid-source source [p.source %.y])
  =/  bad-seed=seed:v1:t  sed(output-source `bad-source)
  =/  bad-sp=spend-1:v1:t
    honest-sp(seeds (~(put z-in:zoon *seeds:v1:t) bad-seed))
  =/  bad=raw-tx:v1:t
    (new:raw-tx:v1:t (~(put z-by:zoon *spends:v1:t) nam [%1 bad-sp]))
  ?>  !=(honest bad)
  ?>  =(id.honest id.bad)
  ?>  =(id.honest (compute-id:raw-tx:t honest))
  ?>  =(id.bad (compute-id:raw-tx:t bad))
  ?>  (based:raw-tx:t honest)
  ?>  (based:raw-tx:t bad)
  ?>  =(witness.honest-sp witness.bad-sp)
  ?>  !=(pkh.witness.honest-sp *(z-map:zoon hash:t [pk=schnorr-pubkey:t sig=schnorr-signature:t]))
  ?>  !=((sig-hash:spend-1:v1:t honest-sp) (sig-hash:spend-1:v1:t bad-sp))
  ?>  (verify-signatures:spend-1:v1:t honest-sp)
  ?>  !(verify-signatures:spend-1:v1:t bad-sp)
  ?>  (validate:raw-tx:t honest)
  ?>  !(validate:raw-tx:t bad)
  ?>  (valid-stub-axes:raw-tx:t bad)
  =/  height=page-number:t  +(~(heaviest-chain-height k-by:h nockchain))
  =/  honest-tx=tx:t  (new:tx:t honest height)
  =/  bad-tx=tx:t  (new:tx:t bad height)
  ?>  (validate:outputs:t ~(outputs get:tx:t honest-tx))
  ?>  =(valid-source (validate:outputs:t ~(outputs get:tx:t bad-tx)))
  [nockchain honest bad]
::::
++  check-v1-pending-output-source-recovery
  |=  valid-source=?
  =+  [nockchain honest bad]=(setup-v1-output-source-pair %.n valid-source)
  =/  id=tx-id:t  ~(id get:raw-tx:t honest)
  =/  tip=page:t  ~(tip-page k-by:h nockchain)
  =/  proven=page:t  (stub-axis-child tip id)
  =/  bid=block-id:t  ~(digest get:page:t proven)
  =^  requested=(list effect:h)  nockchain
    (pok:h [%fact %0 %heard-block proven] nockchain)
  ?>  (~(has-pending-block k-by:h nockchain) bid)
  ?>  (~(has-bnb-block-id k-by:h nockchain) id bid)
  ?>  !(~(has-raw-tx k-by:h nockchain) id)
  ?>  (~(has z-in:zoon (filter-request-tx-effects:h requested)) id)
  =/  before  ~(con k-by:h nockchain)
  =^  rejected=(list effect:h)  nockchain
    (pok-on-wire:h `path`/poke/libp2p/1/gossip/peer-id/bad [%fact %0 %heard-tx bad] nockchain)
  ::  Equality covers every pending dependency and all accepted chain state.
  ?>  =(before ~(con k-by:h nockchain))
  ?>  =(~(digest get:page:t tip) ~(heaviest-block k-by:h nockchain))
  ?>  (~(has-pending-block k-by:h nockchain) bid)
  ?>  (~(check-bnb k-by:h nockchain) id bid)
  ?>  !(~(has-raw-tx k-by:h nockchain) id)
  ?>  !(~(has-excluded k-by:h nockchain) id)
  ?>  !(~(has-spent-by-set k-by:h nockchain) ~(input-names get:raw-tx:t bad) id)
  ?>  =(~ (skim rejected |=(e=effect:h ?=([%seen %tx *] e))))
  ?>  =(~ (filter-heard-tx-effects:h rejected))
  =^  accepted=(list effect:h)  nockchain
    (pok-on-wire:h `path`/poke/libp2p/1/gossip/peer-id/honest [%fact %0 %heard-tx honest] nockchain)
  ?>  =(bid ~(heaviest-block k-by:h nockchain))
  ?>  !(~(has-pending-block k-by:h nockchain) bid)
  ?>  (~(check-bnb k-by:h nockchain) id bid)
  ?>  =(`honest (~(get-bnb-raw-tx k-by:h nockchain) id))
  ?>  (~(has-spent-by-set k-by:h nockchain) ~(input-names get:raw-tx:t honest) id)
  =^  pages  nockchain
    (add-n-pages-integration:h proven 1 nockchain)
  ?>  =(+(~(height get:page:t proven)) ~(heaviest-chain-height k-by:h nockchain))
  (expect-eq !>(~) !>(~(consensus-invariants k-by:h nockchain)))
::::
++  test-v1-pending-output-source-same-id-recovery
  (check-v1-pending-output-source-recovery %.n)
::::
++  test-v1-pending-valid-source-same-id-recovery
  (check-v1-pending-output-source-recovery %.y)
::::
++  test-v1-mempool-output-source-local-bad-then-peer-honest
  =+  [nockchain honest bad]=(setup-v1-output-source-pair %.n %.y)
  =/  id=tx-id:t  ~(id get:raw-tx:t honest)
  =/  before  ~(con k-by:h nockchain)
  =^  rejected=(list effect:h)  nockchain
    (pok-on-wire:h grpc-wire:h [%fact %0 %heard-tx bad] nockchain)
  ?>  =(before ~(con k-by:h nockchain))
  ?>  =(~ (skim rejected |=(e=effect:h ?=([%seen %tx *] e))))
  ?>  =(~ (filter-heard-tx-effects:h rejected))
  =^  accepted=(list effect:h)  nockchain
    (pok-on-wire:h libp2p-gossip-wire:h [%fact %0 %heard-tx honest] nockchain)
  ?>  (~(check-excluded k-by:h nockchain) id)
  ?>  =(`honest (~(get-excluded k-by:h nockchain) id))
  ?>  (~(has z-in:zoon (filter-heard-tx-effects:h accepted)) honest)
  (expect-eq !>(~) !>(~(consensus-invariants k-by:h nockchain)))
::::
:::  Non-null honest source pins remain admissible and locally resendable.
++  test-v1-mempool-pinned-source-honest-then-bad-resends
  =+  [nockchain honest bad]=(setup-v1-output-source-pair %.y %.n)
  =/  id=tx-id:t  ~(id get:raw-tx:t honest)
  =/  proven=page:t  (stub-axis-child ~(tip-page k-by:h nockchain) id)
  =/  bid=block-id:t  ~(digest get:page:t proven)
  =^  accepted=(list effect:h)  nockchain
    (pok-on-wire:h libp2p-gossip-wire:h [%fact %0 %heard-tx honest] nockchain)
  ?>  (~(check-excluded k-by:h nockchain) id)
  =/  before  ~(con k-by:h nockchain)
  =^  peer=(list effect:h)  nockchain
    (pok-on-wire:h `path`/poke/libp2p/1/gossip/peer-id/bad [%fact %0 %heard-tx bad] nockchain)
  ?>  =(before ~(con k-by:h nockchain))
  =^  local=(list effect:h)  nockchain
    (pok-on-wire:h grpc-wire:h [%fact %0 %heard-tx bad] nockchain)
  ?>  =(before ~(con k-by:h nockchain))
  ?>  =(`honest (~(get-excluded k-by:h nockchain) id))
  ?>  =(~ (skim (weld peer local) |=(e=effect:h ?=([%seen %tx *] e))))
  ?>  =(~ (filter-heard-tx-effects:h (weld peer local)))
  =^  resend=(list effect:h)  nockchain
    (pok-on-wire:h grpc-wire:h [%fact %0 %heard-tx honest] nockchain)
  ?>  (~(has z-in:zoon (filter-heard-tx-effects:h resend)) honest)
  =^  promoted=(list effect:h)  nockchain
    (pok:h [%fact %0 %heard-block proven] nockchain)
  ?>  =(bid ~(heaviest-block k-by:h nockchain))
  ?>  =(`honest (~(get-bnb-raw-tx k-by:h nockchain) id))
  ?>  (~(check-bnb k-by:h nockchain) id bid)
  (expect-eq !>(~) !>(~(consensus-invariants k-by:h nockchain)))
::::
::  Mine a real output under an OR lock, then spend a complete FULL leaf.
::  The harness target and proof length are reduced; signatures are real.
++  test-v1-mempool-full-multibranch-axis-after-bythos
  =+  [nockchain honest bad]=setup-v1-stub-axis-pair
  ?>  ?=(@ -.honest)
  =/  [nam=nname:t original=spend:v1:t]
    (snag 0 ~(tap z-by:zoon spends.honest))
  ?>  ?=(%1 -.original)
  =/  left  (make-pkh-from-sig:spend-condition:t p:default-keys-1:h)
  =/  right  (make-pkh-from-sig:spend-condition:t p:default-keys-2:h)
  =/  multi=lock:t  [%2 [form.left form.right]]
  =/  root=hash:t  (hash:lock:t multi)
  =/  deposit-sp=spend-1:v1:t  +.original
  =/  sed=seed:v1:t  (snag 0 ~(tap z-in:zoon seeds.deposit-sp))
  =.  deposit-sp
    deposit-sp(seeds (~(put z-in:zoon *seeds:v1:t) sed(lock-root root)))
  =.  deposit-sp  (sign:spend-1:v1:t deposit-sp s:default-keys-1:h)
  =/  deposit=raw-tx:v1:t
    (new:raw-tx:v1:t (~(put z-by:zoon *spends:v1:t) nam [%1 deposit-sp]))
  ?>  (validate:raw-tx:t deposit)
  =/  parent=page:t  ~(tip-page k-by:h nockchain)
  =/  block=page:t  (stub-axis-child parent id.deposit)
  =^  deposit-effs=(list effect:h)  nockchain
    (pok:h [%fact %0 %heard-tx deposit] nockchain)
  =^  block-effs=(list effect:h)  nockchain
    (pok:h [%fact %0 %heard-block block] nockchain)
  ?>  =(~(digest get:page:t block) ~(heaviest-block k-by:h nockchain))
  =/  built=tx:t  (new:tx:t deposit ~(height get:page:t block))
  =/  outs=outputs:t  ~(outputs get:tx:t built)
  ?>  ?=(%1 -.outs)
  =/  output=output:t  (snag 0 ~(tap z-in:zoon +.outs))
  =/  output-note=nnote:t  ~(note get:output:t output)
  =/  bal  ~(get-cur-balance k-by:h nockchain)
  =/  coin=nnote:t  (~(got h-by:h bal) ~(name get:nnote:t output-note))
  ?>  ?=(@ -.coin)
  ?>  (gte +(~(height get:page:t block)) bythos-phase:t)
  =/  proof=lock-merkle-proof-full:v1:t
    (build-lock-merkle-proof-full:lock:t multi 1)
  ?>  (gth axis.proof 1)
  =/  sed=seed:v1:t
    (make-seed:v1:h root (sub assets.coin 256) (hash:nnote:t coin))
  =/  sp=spend-1:v1:t
    (new:spend-1:v1:t (~(put z-in:zoon *seeds:v1:t) sed) 256)
  =/  wit=witness:t  *witness:t
  =.  sp  sp(witness wit(lmp proof))
  =.  sp  (sign:spend-1:v1:t sp s:default-keys-1:h)
  =/  nam=nname:t  ~(name get:nnote:t coin)
  =/  raw=raw-tx:v1:t
    (new:raw-tx:v1:t (~(put z-by:zoon *spends:v1:t) nam [%1 sp]))
  =/  changed=spend-1:v1:t
    sp(witness witness.sp(lmp proof(axis +(axis.proof))))
  =/  changed-raw=raw-tx:v1:t
    (new:raw-tx:v1:t (~(put z-by:zoon *spends:v1:t) nam [%1 changed]))
  ?>  !=(id.raw id.changed-raw)
  ?>  (based:raw-tx:t raw)
  ?>  (validate:raw-tx:t raw)
  ?>  (valid-stub-axes:raw-tx:t raw)
  =^  admitted=(list effect:h)  nockchain
    (pok-on-wire:h libp2p-gossip-wire:h [%fact %0 %heard-tx raw] nockchain)
  ?>  (~(check-excluded k-by:h nockchain) id.raw)
  ?>  (~(has z-in:zoon (filter-heard-tx-effects:h admitted)) raw)
  =/  next=page:t  (stub-axis-child block id.raw)
  =^  mined=(list effect:h)  nockchain
    (pok:h [%fact %0 %heard-block next] nockchain)
  ?>  =(~(digest get:page:t next) ~(heaviest-block k-by:h nockchain))
  ?>  =(`raw (~(get-bnb-raw-tx k-by:h nockchain) id.raw))
  (expect-eq !>(~) !>(~(consensus-invariants k-by:h nockchain)))
::::
::  Preserve the real kernel and production fund routing constants. Add a mature
::  fund-named note to the tip balance; ordinary keys still sign the honest tx.
++  setup-empty-fund-witness
  ^-  [_nockchain:h raw-tx:t raw-tx:t]
  =+  [nockchain honest]=(setup-v1-spendable-tx 256)
  =/  bal  ~(get-cur-balance k-by:h nockchain)
  =/  tip=page:t  ~(tip-page k-by:h nockchain)
  =/  coin=nnote:t  (get-coinbase-from-balance:v1:h tip bal)
  ?>  ?=(@ -.coin)
  =/  fund=nnote:t
    coin(name [fund-note-firstname:t +.name.coin], origin-page 0)
  =/  nam=nname:t  ~(name get:nnote:t fund)
  =.  bal  (~(put h-by:h bal) nam fund)
  =/  con  ~(con k-by:h nockchain)
  =.  balance.con
    (~(put h-by:h balance.con) ~(digest get:page:t tip) bal)
  =.  nockchain  (~(with-con k-by:h nockchain) con)
  =/  sed=seed:v1:t
    (make-seed:v1:h *hash:t (sub assets.fund 256) (hash:nnote:t fund))
  =/  sp=spend-1:v1:t
    %*  .  *spend-1:v1:t
      seeds  (~(put z-in:zoon *seeds:v1:t) sed)
      fee    256
    ==
  =.  witness.sp
    witness.sp(lmp [%full ~ 1 [*hash:t ~]])
  =/  raw=raw-tx:t
    (new:raw-tx:v1:t (~(put z-by:zoon *spends:v1:t) nam [%1 sp]))
  ?>  ?=([~ *] ((soft cause:h) [%fact %0 %heard-tx raw]))
  =/  tac=tx-acc:t  (new:tx-acc:t `bal +(~(height get:page:t tip)))
  =/  res  (dispatch:tx-acc:t tac raw)
  ?>  ?=(%.n -.res)
  ?>  =(%v1-spend-1-lock-failed p.res)
  [nockchain raw honest]
:::
++  test-v1-mempool-empty-fund-full-witness-recovery
  =+  [nockchain raw honest]=setup-empty-fund-witness
  =/  id=tx-id:t  ~(id get:raw-tx:t raw)
  =^  rejected=(list effect:h)  nockchain
    (pok:h [%fact %0 %heard-tx raw] nockchain)
  ?>  !(~(has-raw-tx k-by:h nockchain) id)
  ?>  !(~(has-excluded k-by:h nockchain) id)
  ?>  !(~(has z-in:zoon (filter-heard-tx-effects:h rejected)) raw)
  =^  accepted=(list effect:h)  nockchain
    (pok:h [%fact %0 %heard-tx honest] nockchain)
  =/  honest-id=tx-id:t  ~(id get:raw-tx:t honest)
  ;:  weld
    (expect-eq !>(%.y) !>((~(check-excluded k-by:h nockchain) honest-id)))
    (expect-eq !>(%.y) !>((~(has z-in:zoon (filter-heard-tx-effects:h accepted)) honest)))
  ==
:::
++  test-v1-pending-empty-fund-full-witness-recovery
  =+  [nockchain raw honest]=setup-empty-fund-witness
  =/  tip=page:t  ~(tip-page k-by:h nockchain)
  =/  id=tx-id:t  ~(id get:raw-tx:t raw)
  =/  block=page:t
    =/  candidate=page:t  (make-page-with-txs:v0:h tip ~[id])
    ?>  ?=(@ -.candidate)
    candidate(coinbase (new:v1:coinbase-split:t (add 256 (emission-calc:coinbase:t ~(height get:page:t candidate))) default-keys-1-share-v1:h))
  ::  Keep the v1 refinement local to coinbase construction. The proving gate
  ::  and subsequent kernel events consume the facade's full page union.
  =/  proven=page:t  (prove-page:h `page:t`block)
  =/  bid=block-id:t  ~(digest get:page:t proven)
  =^  requested=(list effect:h)  nockchain
    (pok:h [%fact %0 %heard-block proven] nockchain)
  ?>  (~(has-pending-block k-by:h nockchain) bid)
  ?>  (~(has z-in:zoon (filter-request-tx-effects:h requested)) id)
  =^  rejected=(list effect:h)  nockchain
    (pok:h [%fact %0 %heard-tx raw] nockchain)
  ?>  =(~(digest get:page:t tip) ~(heaviest-block k-by:h nockchain))
  ?>  !(~(has-pending-block k-by:h nockchain) bid)
  ::  A later honest transaction and block both make progress on the same node.
  =^  accepted=(list effect:h)  nockchain
    (pok:h [%fact %0 %heard-tx honest] nockchain)
  ?>  (~(check-excluded k-by:h nockchain) ~(id get:raw-tx:t honest))
  =^  pages  nockchain  (add-n-pages-integration:h tip 1 nockchain)
  (expect-eq !>(3) !>(~(heaviest-chain-height k-by:h nockchain)))
::
:::  Operator re-submission over grpc (`nockchain-wallet send-tx`) of an
:::  already-held tx re-gossips immediately.
++  test-v1-mempool-grpc-resend-re-gossips
  =+  [nockchain raw]=(setup-v1-spendable-tx 256)
  =/  =cause:h  [%fact %0 %heard-tx raw]
  =/  tx-id=tx-id:t  ~(id get:raw-tx:t raw)
  ::  first submission: a new tx, so it is accepted and gossiped
  =^  effs-1=(list effect:h)  nockchain
    (pok-on-wire:h grpc-wire:h cause nockchain)
  ::  we now hold it, so the resend takes the already-seen branch
  ?>  (~(has-raw-tx k-by:h nockchain) tx-id)
  ::  resend over the same grpc wire: must gossip again
  =^  effs-2=(list effect:h)  nockchain
    (pok-on-wire:h grpc-wire:h cause nockchain)
  %+  expect-eq
    !>([%.y %.y %.y])
  !>  :*  (~(has z-in:zoon (filter-heard-tx-effects:h effs-1)) raw)
          (~(has z-in:zoon (filter-heard-tx-effects:h effs-2)) raw)
          ::  still in the mempool, and never double-added
          (~(has-raw-tx k-by:h nockchain) tx-id)
      ==
::
:::  Peer-origin duplicates do not re-gossip. This terminates gossip loops.
++  test-v1-mempool-peer-resend-does-not-re-gossip
  =+  [nockchain raw]=(setup-v1-spendable-tx 256)
  =/  =cause:h  [%fact %0 %heard-tx raw]
  =/  tx-id=tx-id:t  ~(id get:raw-tx:t raw)
  ::  first time we hear it from a peer: new tx, accepted and gossiped onward
  =^  effs-1=(list effect:h)  nockchain
    (pok-on-wire:h libp2p-gossip-wire:h cause nockchain)
  ?>  (~(has-raw-tx k-by:h nockchain) tx-id)
  ::  peer sends it again: we must stay silent
  =^  effs-2=(list effect:h)  nockchain
    (pok-on-wire:h libp2p-gossip-wire:h cause nockchain)
  %+  expect-eq
    !>([%.y %.n %.y])
  !>  :*  (~(has z-in:zoon (filter-heard-tx-effects:h effs-1)) raw)
          (~(has z-in:zoon (filter-heard-tx-effects:h effs-2)) raw)
          (~(has-raw-tx k-by:h nockchain) tx-id)
      ==
:::
::  First acceptance carries a source only for that effect. Local resubmission
::  remains an independent announcement after a peer duplicate.
++  test-v1-mempool-first-peer-source-and-local-resend
  =+  [nockchain raw]=(setup-v1-spendable-tx 256)
  =/  =cause:h  [%fact %0 %heard-tx raw]
  =/  tx-id=tx-id:t  ~(id get:raw-tx:t raw)
  ?>  !(~(has-raw-tx k-by:h nockchain) tx-id)
  =^  effs-1=(list effect:h)  nockchain
    (pok-on-wire:h libp2p-gossip-wire:h cause nockchain)
  ?>  (~(has-raw-tx k-by:h nockchain) tx-id)
  =/  initial-gossip=(list effect:h)
    (skim effs-1 |=(e=effect:h ?=(%gossip -.e)))
  ?>  =(1 (lent initial-gossip))
  =/  initial=effect:h  (snag 0 initial-gossip)
  ?>  ?=(%gossip -.initial)
  ?>  =(context.initial [%peer %initial-acceptance [~ 'peer']])
  ?>  =(p.initial [%0 %heard-tx raw])
  =^  duplicate-effs=(list effect:h)  nockchain
    (pok-on-wire:h `path`/poke/libp2p/1/gossip/peer-id/other cause nockchain)
  ?>  =(~ (skim duplicate-effs |=(e=effect:h ?=(%gossip -.e))))
  =^  local-effs=(list effect:h)  nockchain
    (pok-on-wire:h grpc-wire:h cause nockchain)
  =/  local-gossip=(list effect:h)
    (skim local-effs |=(e=effect:h ?=(%gossip -.e)))
  ?>  =(1 (lent local-gossip))
  =/  local=effect:h  (snag 0 local-gossip)
  ?>  ?=(%gossip -.local)
  %+  expect-eq
    !>([[%local %local-resend ~] [%0 %heard-tx raw] %.y])
  !>  [context.local p.local (~(has-raw-tx k-by:h nockchain) tx-id)]

::::  Chain progress, not every timer tick, re-announces retained mempool txs.
++  test-v1-mempool-new-heaviest-regossips-retained-tx
  =+  [nockchain raw]=(setup-v1-spendable-tx 256)
  =/  tx-id=tx-id:t  ~(id get:raw-tx:t raw)
  =^  effs-1=(list effect:h)  nockchain
    (pok:h [%fact %0 %heard-tx raw] nockchain)
  ?>  (~(has-raw-tx k-by:h nockchain) tx-id)
  =^  timer-effs=(list effect:h)  nockchain
    (pok:h [%command %timer ~] nockchain)
  =/  tip=page:t  ~(tip-page k-by:h nockchain)
  =/  next=page:t  (prove-page:h (make-empty-page:h tip))
  =^  block-effs=(list effect:h)  nockchain
    (pok:h [%fact %0 %heard-block next] nockchain)
  =/  tx-gossips=(list effect:h)
    %+  skim  block-effs
    |=  e=effect:h
    ?.  ?=(%gossip -.e)  %.n
    ?=(%heard-tx -.data.p.e)
  ?>  =(1 (lent tx-gossips))
  =/  retained=effect:h  (snag 0 tx-gossips)
  ?>  ?=(%gossip -.retained)
  ?>  =(context.retained [%chain %new-head ~])
  %+  expect-eq
    !>([%.y %.n %.y %.y])
  !>  :*  (~(has z-in:zoon (filter-heard-tx-effects:h effs-1)) raw)
          (~(has z-in:zoon (filter-heard-tx-effects:h timer-effs)) raw)
          (~(has z-in:zoon (filter-heard-tx-effects:h block-effs)) raw)
          (~(has-raw-tx k-by:h nockchain) tx-id)
      ==
::
::  +stripped: .sed with its output-source cleared, for hashing a seed set
::  the way +build-outputs does
++  stripped
  |=  sed=seed:v1:t
  ^-  seed:v1:t
  sed(output-source ~)
::
::  +signed-spend: a spend-1 of .coin carrying .seds and .fee, signed with
::  the coin's coinbase keys against its own lock
++  signed-spend
  |=  $:  coin=nnote:t
          in-root=hash:t
          in-sc=spend-condition:v1:t
          seds=seeds:v1:t
          fee=coins:t
      ==
  ^-  [nname:t spend-1:v1:t]
  =/  sp1=spend-1:v1:t
    %*  .  *spend-1:v1:t
      witness  *witness:v1:t
      seeds    seds
      fee      fee
    ==
  =/  sig-h=hash:t  (sig-hash:spend-1:v1:t sp1)
  =/  pks=(list schnorr-pubkey:t)
    ~(tap z-in:zoon pubkeys.p:default-keys-1:h)
  =/  wit=witness:t
    (make-pkh-witness:v1:h in-root in-sc sig-h ~[[s:default-keys-1:h (snag 0 pks)]])
  [~(name get:nnote:t coin) sp1(witness wit)]
::
::  +test-v1-mempool-accept-pinned-merged-output: the outputs check must not
::  over-reject. two spends pay the same lock root and every seed pins the
::  source of the merged output they form together -- the shape the wallet
::  emits for a deposit split across input notes (+pin-deposit-sources).
::  This transaction is mineable, and admission takes it.
++  test-v1-mempool-accept-pinned-merged-output
  =+  [nockchain genesis]=init-nockchain:h
  =^  pages  nockchain
    (add-n-pages-integration:h genesis 3 nockchain)
  =/  bal  ~(get-cur-balance k-by:h nockchain)
  =/  coin-1=nnote:t  (get-coinbase-from-balance:v1:h (snag 1 pages) bal)
  =/  coin-2=nnote:t  (get-coinbase-from-balance:v1:h (snag 2 pages) bal)
  =/  pks=(list schnorr-pubkey:t)
    ~(tap z-in:zoon pubkeys.p:default-keys-1:h)
  =/  [in-root=hash:t in-sc=spend-condition:v1:t *]
    (make-coinbase-lock:v1:h (lent pks) pks)
  =/  [dest-root=hash:t * *]
    (make-pkh-lock:v1:h 1 ~[(snag 0 pks)])
  =/  fee=coins:t  256
  =/  sed-1=seed:v1:t
    (make-seed:v1:h dest-root (sub assets.coin-1 fee) (hash:nnote:t coin-1))
  =/  sed-2=seed:v1:t
    (make-seed:v1:h dest-root (sub assets.coin-2 fee) (hash:nnote:t coin-2))
  ::  every seed paying the root pins the merged set's source
  =/  merged-pin=hash:t
    (hash:seeds:v1:t (~(gas z-in:zoon *seeds:v1:t) ~[(stripped sed-1) (stripped sed-2)]))
  =/  sed-1=seed:v1:t  sed-1(output-source `[merged-pin %.n])
  =/  sed-2=seed:v1:t  sed-2(output-source `[merged-pin %.n])
  =/  [name-1=nname:t spend-1=spend-1:v1:t]
    (signed-spend coin-1 in-root in-sc (~(put z-in:zoon *seeds:v1:t) sed-1) fee)
  =/  [name-2=nname:t spend-2=spend-1:v1:t]
    (signed-spend coin-2 in-root in-sc (~(put z-in:zoon *seeds:v1:t) sed-2) fee)
  =/  raw=raw-tx:v1:t
    (new:raw-tx:v1:t (~(gas z-by:zoon *spends:v1:t) ~[[name-1 [%1 spend-1]] [name-2 [%1 spend-2]]]))
  =/  tip=page:t  ~(tip-page k-by:h nockchain)
  =/  cand-height=page-number:t  +(~(height get:page:t tip))
  =/  built=tx:t  (new:tx:t raw cand-height)
  ::  sanity: the pinned merged output validates in the engine, and the
  ::  raw-tx does too
  ?>  (validate:raw-tx:t raw)
  ?>  (validate:outputs:t ~(outputs get:tx:t built))
  =/  waiting=_nockchain:h  nockchain
  =^  effs=(list effect:h)  nockchain
    (pok:h [%fact %0 %heard-tx raw] nockchain)
  ?>  (~(has-raw-tx k-by:h nockchain) id.raw)
  ?>  (~(has-excluded k-by:h nockchain) id.raw)
  ?>  (~(has z-in:zoon (filter-heard-tx-effects:h effs)) raw)
  ::  The same merged pin must also pass body-local pending admission,
  ::  without resolving either input against a different chain context.
  =/  candidate=page:t  (make-page-with-txs:v0:h tip ~[id.raw])
  ?>  ?=(@ -.candidate)
  =.  candidate
    candidate(coinbase (new:v1:coinbase-split:t (add (mul 2 fee) (emission-calc:coinbase:t cand-height)) default-keys-1-share-v1:h))
  =/  proven=page:t  (prove-page:h `page:t`candidate)
  =/  bid=block-id:t  ~(digest get:page:t proven)
  =^  requested=(list effect:h)  waiting
    (pok:h [%fact %0 %heard-block proven] waiting)
  ?>  (~(has-pending-block k-by:h waiting) bid)
  ?>  (~(has z-in:zoon (filter-request-tx-effects:h requested)) id.raw)
  =^  accepted=(list effect:h)  waiting
    (pok-on-wire:h libp2p-gossip-wire:h [%fact %0 %heard-tx raw] waiting)
  ?>  =(bid ~(heaviest-block k-by:h waiting))
  ?>  !(~(has-pending-block k-by:h waiting) bid)
  ?>  =(`raw (~(get-bnb-raw-tx k-by:h waiting) id.raw))
  (expect-eq !>(~) !>(~(consensus-invariants k-by:h waiting)))
--
