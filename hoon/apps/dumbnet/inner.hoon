/=  dk  /apps/dumbnet/lib/types
/=  sp  /common/stark/prover
/=  c-transact  /common/tx-engine
/=  dumb-miner  /apps/dumbnet/lib/miner
/=  dumb-derived  /apps/dumbnet/lib/derived
/=  dumb-consensus  /apps/dumbnet/lib/consensus
/=  mine  /common/pow
/=  nv  /common/nock-verifier
/=  zeke  /common/zeke
/=  *  /common/zoon
/=  *  /common/wrapper
::
::  Never use c-transact face, always use the lustar `t`
::  alias, otherwise the blockchain constants set in the kernel
::  will not be active.
::
|%
++  moat  (keep kernel-state:dk)
++  inner
  |_  k=kernel-state:dk
  +*  min      ~(. dumb-miner m.k constants.k)
      der      ~(. dumb-derived d.k constants.k)
      con      ~(. dumb-consensus c.k constants.k)
      t        ~(. c-transact constants.k)
  ::
  ::  We should be calling the inner kernel load in case of update
  ++  load
    ::  use the below for validation of new state upgrades
    ::  |=  untyped-arg=*
    ::  ~>  %slog.[0 leaf+"typing kernel state"]
    ::  =/  arg  ~>  %bout  ;;(load-kernel-state:dk untyped-arg)
    ::  ~>  %slog.[0 leaf+"loading kernel state"]
    ::
    ::  use this for production
    |=  arg=load-kernel-state:dk
    ~&  [%nockchain-state-version -.arg]
    ::  cut
    |^
    =.  k  ~>  %bout  (update-constants (check-checkpoints (state-n-to-6 arg)))
    =.  c.k  ~>  %bout  check-and-repair:con
    k
    ::  this arm should be renamed each state upgrade to state-n-to-[latest] and extended to loop through all upgrades
    ++  state-n-to-6
      |=  arg=load-kernel-state:dk
      ^-  kernel-state:dk
      ?.  ?=(%6 -.arg)
        ~>  %slog.[0 'load: State upgrade required']
        ?-  -.arg
            ::
          %0  $(arg (state-0-to-1 arg))
          %1  $(arg (state-1-to-2 arg))
          %2  $(arg (state-2-to-3 arg))
          %3  $(arg (state-3-to-4 arg))
          %4  $(arg (state-4-to-5 arg))
          %5  $(arg (state-5-to-6 arg))
        ==
      arg
    ::
    ::  upgrade kernel state 5 to kernel state 6
    ++  state-5-to-6
      |=  arg=kernel-state-5:dk
      ^-  kernel-state-6:dk
      =/  new-txs=(z-mip block-id:t tx-id:t tx:t)
        %-  ~(run z-by txs.c.arg)
        |=  tx-map=(z-map tx-id:t tx:v0:t)
        ^-  (z-map tx-id:t tx:t)
        %-  ~(run z-by tx-map)
        |=  tx0=tx:v0:t
        ^-  tx:t
        [%0 tx0]
      =/  new-c=consensus-state-6:dk
        %*  .  *consensus-state-6:dk
          blocks-needed-by  blocks-needed-by.c.arg
          excluded-txs      excluded-txs.c.arg
          spent-by          spent-by.c.arg
          pending-blocks    pending-blocks.c.arg
          balance           balance.c.arg
          raw-txs           raw-txs.c.arg
          blocks            blocks.c.arg
          heaviest-block    heaviest-block.c.arg
          min-timestamps    min-timestamps.c.arg
          epoch-start       epoch-start.c.arg
          targets           targets.c.arg
          btc-data          btc-data.c.arg
          genesis-seal      genesis-seal.c.arg
          txs               new-txs
        ==
      =/  new-m=mining-state-6:dk
        %*  .  *mining-state-6:dk
          mining           mining.m.arg
          shares           *(z-map hash:t @)
          v0-shares        shares.m.arg
          candidate-block  *page:t
          candidate-acc    *tx-acc:t
          next-nonce       next-nonce.m.arg
        ==
      =/  default-constants=blockchain-constants:t  *blockchain-constants:t
      =/  new-constants=blockchain-constants:t
        default-constants(+>+ constants.arg)
      :*  %6
          c=new-c
          a=a.arg
          m=new-m
          d=d.arg
          constants=new-constants
      ==
    ::  upgrade kernel state 4 to kernel state 5
    ++  state-4-to-5
    |=  arg=kernel-state-4:dk
    ^-  kernel-state-5:dk
    |^
      [%5 new-consensus a.arg m.arg d.arg constants.arg]
    ++  new-consensus
      ^-  consensus-state-5:dk
      ~>  %slog.[0 'load: This upgrade may take some time']
      =/  blocks-needed-by=(z-jug tx-id:v0:t block-id:v0:t)
        %-  ~(rep z-by blocks.c.arg)
        |=  [[=block-id:v0:t pag=local-page:v0:t] bnb=(z-jug tx-id:v0:t block-id:v0:t)]
        ^-  (z-jug tx-id:v0:t block-id:v0:t)
        %-  ~(rep z-in tx-ids.pag)
        |=  [=tx-id:v0:t bnb=_bnb]
        ^-  (z-jug tx-id:v0:t block-id:v0:t)
        =+
          ?.  (~(has z-by raw-txs.c.arg) tx-id)
            ~>  %slog.[1 'load: Missing transaction in consensus state. Please alert the developers.']  ~
            ~
        (~(put z-ju bnb) tx-id block-id)
      ~>  %slog.[0 'load: Indexed blocks by transaction id']
      =/  rtx=(map tx-id:v0:t *)  raw-txs.c.arg
      =/  bnb=(map tx-id:v0:t *)  blocks-needed-by
      =/  excluded-map=(map tx-id:v0:t *)  (~(dif z-by rtx) bnb)
      =/  excluded-txs=(z-set tx-id:v0:t)  ~(key z-by excluded-map)
      =+
        ?:  =(*(z-set tx-id:v0:t) excluded-txs)
          ~>  %slog.[0 'load: Consensus state is consistent']  ~
        :: this is only a concern at upgrade time. After the upgrade this is allowed to happen
        =/  log-message
          %-  crip
          "load: ".
          "There are transactions in consensus state which are not included in any block. ".
          "Please inform the developers."
        ~>  %slog.[1 log-message]  ~
      =/  [spent-by=(z-jug nname:v0:t tx-id:v0:t) raw-txs=(z-map tx-id:v0:t [raw-tx:v0:t @])]
        %-  ~(rep z-by raw-txs.c.arg)
        |=  [[=tx-id:v0:t =raw-tx:v0:t] [sb=(z-jug nname:v0:t tx-id:v0:t) rtx=(z-map tx-id:v0:t [raw-tx:v0:t @])]]
        ^-  [(z-jug nname:v0:t tx-id:v0:t) (z-map tx-id:v0:t [raw-tx:v0:t @])]
        =.  sb
          %-  ~(rep z-in (inputs-names:raw-tx:v0:t raw-tx))
          |=  [=nname:v0:t sb=_sb]
          (~(put z-ju sb) nname tx-id)
        =.  rtx  (~(put z-by rtx) tx-id [raw-tx 0])
        [sb rtx]
      ~>  %slog.[0 'load: Indexed transactions by spent notes']
      ~>  %slog.[0 'load: Upgrade state version 4 to version 5 complete']
      =|  pending-blocks=(z-map block-id:v0:t [=page:v0:t heard-at=@])
      [[blocks-needed-by excluded-txs spent-by pending-blocks] c.arg(raw-txs raw-txs)]
    --
    ::  upgrade kernel state 3 to kernel state 4
    ::  (reset pending state)
    ++  state-3-to-4
      |=  arg=kernel-state-3:dk
      ^-  kernel-state-4:dk
      ~>  %slog.[0 'load: State version 3 to version 4']
      =|  p=pending-state-4:dk :: empty pending state
      :: reset candidate block
      ?~  heaviest-block.c.arg
        [%4 c.arg p.arg a.arg m.arg d.arg constants.arg]
      =.  candidate-acc.m.arg
        %-  new:tx-acc:v0:t
        (~(get z-by balance.c.arg) u.heaviest-block.c.arg)
      =.  tx-ids.candidate-block.m.arg  *(z-set tx-id:v0:t)
      [%4 c.arg p a.arg m.arg d.arg constants.arg]
    ::  upgrade kernel-state-2 to kernel-state-3
    ++  state-2-to-3
      |=  arg=kernel-state-2:dk
      ^-  kernel-state-3:dk
      ~>  %slog.[0 'load: State version 2 to version 3']
      =/  raw-txs=(z-map tx-id:v0:t raw-tx:v0:t)
        %-  ~(rep z-by txs.c.arg)
        |=  [[block-id:v0:t m=(z-map tx-id:v0:t tx:v0:t)] n=(z-map tx-id:v0:t raw-tx:v0:t)]
        %-  ~(uni z-by n)
        %-  ~(run z-by m)
        |=  =tx:v0:t
        ^-  raw-tx:v0:t  -.tx
      =/  c=consensus-state-3:dk
        :*  balance.c.arg
            txs.c.arg
            raw-txs
            blocks.c.arg
            heaviest-block.c.arg
            min-timestamps.c.arg
            epoch-start.c.arg
            targets.c.arg
            btc-data.c.arg
            genesis-seal.c.arg
        ==
      [%3 c p.arg a.arg m.arg d.arg constants.arg]
    ::  upgrade kernel-state-1 to kernel-state-2
    ++  state-1-to-2
      |=  arg=kernel-state-1:dk
      ^-  kernel-state-2:dk
      ~>  %slog.[0 'load: State version 0 to version 1']
      [%2 c.arg p.arg a.arg m.arg d.arg constants.arg]
    ::  upgrade kernel-state-0 to kernel-state-1
    ++  state-0-to-1
      |=  arg=kernel-state-0:dk
      ^-  kernel-state-1:dk
      ~>  %slog.[0 'load: State version 0 to version 1']
      =/  d  [*(unit page-number:v0:t) heaviest-chain.d.arg]
      =.  d  (compute-highest blocks.c.arg pending-blocks.p.arg d constants.arg)
      [%1 c.arg p.arg a.arg m.arg d constants.arg]
    ::  compute the highest block (for the 0-1 upgrade)
    ++  compute-highest
      |=  $:  blocks=(z-map block-id:v0:t local-page:v0:t)
              pending=(z-map block-id:v0:t local-page:v0:t)
              derived-state=derived-state-1:dk
              constants=blockchain-constants:v0:t
          ==
      =/  both  (~(uni z-by blocks) pending)
      =/  list  ~(tap z-by both)
      |-  ^-  derived-state-1:dk
      ?~  list  derived-state
      %=  $
        derived-state  (update-highest-ds-1 derived-state height.q.i.list)
        list  t.list
      ==
    ++  update-highest-ds-1
      |=  [ds=derived-state-1:dk height=page-number:t]
      ^+  ds
      ?~  highest-block-height.ds
        %=  ds
          highest-block-height  `height
        ==
      ?:  (gth height u.highest-block-height.ds)
        ds(highest-block-height `height)
      ds
    ::
    ::  ensure constants get updated to defaults set tx-engine core
    ::  unless we are running fakenet, then we do nothing.
    ++  update-constants
      |=  arg=kernel-state:dk
      =/  mainnet=(unit ?)  (~(is-mainnet dumb-derived d.arg constants.arg) c.arg)
      ?~  mainnet
        arg
      ?.  u.mainnet
        arg
      arg(constants *blockchain-constants:t)
    ::
    ++  check-checkpoints
      |=  arg=kernel-state:dk
      =/  mainnet=(unit ?)  (~(is-mainnet dumb-derived d.arg constants.arg) c.arg)
      ~&  check-checkpoints-mainnet+mainnet
      ?~  mainnet
        arg
      ?.  u.mainnet
        arg
      =/  checkpoints  ~(tap z-by checkpointed-digests:con)
      |-  ^-  kernel-state:dk
      ?~  checkpoints  arg
      =/  block-at-checkpoint  (~(get z-by heaviest-chain.d.arg) -.i.checkpoints)
      ?~  block-at-checkpoint  $(checkpoints t.checkpoints)
      ?.  =(u.block-at-checkpoint +.i.checkpoints)
        ~>  %slog.[1 'load: Mismatched checkpoint when loading, resetting state']
        =|  nk=kernel-state:dk
        :: preserve mining options and init status, otherwise drop all consensus state
        =.  mining.m.nk  mining.m.arg
        =.  shares.m.nk  shares.m.arg
        =.  v0-shares.m.nk  v0-shares.m.arg
        =.  init.a.k  init.a.arg
        nk
      arg
    --
  ::
  ::TODO make referentially transparent by requiring event number in the scry path
  ++  peek
    |=  arg=path
    ^-  (unit (unit *))
    ~>  %slog.[0 (cat 3 'peek: %' -.arg)]
    =/  =(pole)  arg
    ?+  pole  ~
    ::
        [%mainnet ~]
      `(is-mainnet:der c.k)
    ::
        [%genesis-seal-set ~]
      ``?=(^ genesis-seal.c.k)
    ::
        [%blocks ~]
      ^-  (unit (unit (z-map block-id:t page:t)))
      ``(~(run z-by blocks.c.k) to-page:local-page:t)
    ::
        [%transactions ~]
      ^-  (unit (unit (z-mip block-id:t tx-id:t tx:t)))
      ``txs.c.k
    ::
        [%raw-transactions ~]
      ^-  (unit (unit (z-map tx-id:t [=raw-tx:t heard-at=@])))
      ``raw-txs.c.k
    ::
      :: transactions unneeded by any block
        [%excluded-txs ~]
      ^-  (unit (unit (z-set tx-id:t)))
      ``excluded-txs.c.k
    ::
    ::  For %block, %transaction, %raw-transaction, and %balance scries, the ID is
    ::  passed as a base58 encoded string in the scry path.
        [%block bid=@ ~]
      ^-  (unit (unit page:t))
      :: scry for a validated block (this does not look at pending state)
      =/  block-id  (from-b58:hash:t bid.pole)
      `(bind (~(get z-by blocks.c.k) block-id) to-page:local-page:t)
    ::
        [%elders bid=@ ~]
      ::  get ancestor block IDs up to 24 deep for a given block
      ^-  (unit (unit [page-number:t (list block-id:t)]))
      =/  block-id  (from-b58:hash:t bid.pole)
      =/  elders  (get-elders:con d.k block-id)
      ?~  elders
        [~ ~]
      ``u.elders
    ::
        [%block-transactions bid=@ ~]
      ::  scry for txs included in a validated block
      ^-  (unit (unit (z-map tx-id:t tx:t)))
      :-  ~
      %-  ~(get z-by txs.c.k)
      (from-b58:hash:t bid.pole)
    ::
        [%block-transaction bid=@ tid=@ ~]
      ::  scry for a tx that has been included in a validated block
      ^-  (unit (unit tx:t))
      =/  tx-id  (from-b58:hash:t tid.pole)
      =/  block-id  (from-b58:hash:t bid.pole)
      =/  block-txs  (~(get z-by txs.c.k) block-id)
      ?~  block-txs  ~
      =/  maybe-tx  (~(get z-by u.block-txs) tx-id)
      ?~  maybe-tx  ~
      ``u.maybe-tx
    ::
        [%raw-transaction tid=@ ~]
      ::  scry for a raw-tx
      ^-  (unit (unit raw-tx:t))
      :-  ~
      (get-raw-tx:con (from-b58:hash:t tid.pole))
    ::
        [%heavy ~]
      ^-  (unit (unit (unit block-id:t)))
      ``heaviest-block.c.k
    ::
        [%heavy-n pag=@ ~]
      ^-  (unit (unit page:t))
      =/  num=(unit page-number:t)
        ((soft page-number:t) pag.pole)
      ?~  num
        ~
      =/  id=(unit block-id:t)
        (~(get z-by heaviest-chain.d.k) u.num)
      ?~  id
        [~ ~]
      `(bind (~(get z-by blocks.c.k) u.id) to-page:local-page:t)
    ::
        [%heaviest-chain ~]
      ^-  (unit (unit [page-number:t block-id:t]))
      ?~  highest=highest-block-height.d.k
        [~ ~]
      =/  block-id=(unit block-id:t)
        (~(get z-by heaviest-chain.d.k) u.highest)
      ?~  block-id
        [~ ~]
      %-  some
      %-  some
      [u.highest u.block-id]
    ::
        [%heaviest-chain-map ~]
      ^-  (unit (unit (z-map page-number:t block-id:t)))
      ``heaviest-chain.d.k
    ::
        [%heaviest-chain-blocks-range start=@ end=@ ~]
      ^-  (unit (unit (list [page-number:t block-id:t page:t (z-map tx-id:t tx:t)])))
      =/  start-height  ((soft page-number:t) start.pole)
      =/  end-height  ((soft page-number:t) end.pole)
      ?~  start-height  ~
      ?~  end-height  ~
      ::  ensure start <= end
      ?:  (gth u.start-height u.end-height)
        ``~
      ::  build list of blocks in range from heaviest chain
      =/  result=(list [page-number:t block-id:t page:t (z-map tx-id:t tx:t)])
        =/  height  u.start-height
        |-  ^-  (list [page-number:t block-id:t page:t (z-map tx-id:t tx:t)])
        ?:  (gth height u.end-height)
          ~
        ::  get block-id from heaviest chain
        =/  block-id=(unit block-id:t)
          (~(get z-by heaviest-chain.d.k) height)
        ?~  block-id
          $(height +(height))
        ::  get block data
        =/  local-block=(unit local-page:t)
          (~(get z-by blocks.c.k) u.block-id)
        ?~  local-block
          $(height +(height))
        ::  get transactions for this block
        =/  block-txs=(unit (z-map tx-id:t tx:t))
          (~(get z-by txs.c.k) u.block-id)
        =/  txs-map  ?~(block-txs ~ u.block-txs)
        ::  add to result list
        :-  [height u.block-id (to-page:local-page:t u.local-block) txs-map]
        $(height +(height))
      ``result
    ::
        [%desk-hash ~]
      ^-  (unit (unit (unit @uvI)))
      ``desk-hash.a.k
    ::
        [%mining-pubkeys ~]
      ^-  (unit (unit (list [m=@ pks=(list @t)])))
      =/  sigs=(list [m=@ pks=(list @t)])
        %-  ~(rep z-by v0-shares.m.k)
        |=  [[=sig:t *] l=(list [m=@ pks=(list @t)])]
        [(to-b58:sig:t sig) l]
      ``sigs
    ::
        [%balance bid=@ ~]
      ^-  (unit (unit (z-map nname:t nnote:t)))
      :-  ~
      %-  ~(get z-by balance.c.k)
      (from-b58:hash:t bid.pole)
    ::
        [%heaviest-block ~]
      ^-  (unit (unit page:t))
      ?~  heaviest-block.c.k
        [~ ~]
      =/  heaviest-block  (~(get z-by blocks.c.k) u.heaviest-block.c.k)
      ?~  heaviest-block  ~
      ``(to-page:local-page:t u.heaviest-block)
    ::
        [%current-balance ~]
      ^-  (unit (unit (z-map nname:t nnote:t)))
      ?~  heaviest-block.c.k
        [~ ~]
      ?.  (~(has z-by blocks.c.k) u.heaviest-block.c.k)
        [~ ~]
      :-  ~
      %-  ~(get z-by balance.c.k)
      u.heaviest-block.c.k
    ::
        [%balance-by-first-name first-name=@t ~]
      ^-  (unit (unit [page-number:t block-id:t (z-map nname:t nnote:t)]))
      =/  first-name=hash:t  (from-b58:hash:t first-name.pole)
      ?~  heaviest-block.c.k
        [~ ~]
      ?.  (~(has z-by blocks.c.k) u.heaviest-block.c.k)
        [~ ~]
      ?~  bal=(~(get z-by balance.c.k) u.heaviest-block.c.k)
        [~ ~]
      ?~  highest=highest-block-height.d.k
        [~ ~]
      %-  some
      %-  some
      :+  u.highest
        u.heaviest-block.c.k
      %-  ~(rep z-by u.bal)
      |=  [[k=nname:t v=nnote:t] bal=(z-map nname:t nnote:t)]
      ?.  =(~(first-name get:nnote:t v) first-name)
        bal
      (~(put z-by bal) k v)
    ::
        [%balance-by-pubkey key-b58=@t ~]
      ^-  (unit (unit [page-number:t block-id:t (z-map nname:t nnote:t)]))
      =/  pubkey=schnorr-pubkey:t  (from-b58:schnorr-pubkey:t key-b58.pole)
      ?~  heaviest-block.c.k
        [~ ~]
      ?.  (~(has z-by blocks.c.k) u.heaviest-block.c.k)
        [~ ~]
      ?~  bal=(~(get z-by balance.c.k) u.heaviest-block.c.k)
        [~ ~]
      ?~  highest=highest-block-height.d.k
        [~ ~]
      %-  some
      %-  some
      :+  u.highest
        u.heaviest-block.c.k
      %-  ~(rep z-by u.bal)
      |=  [[k=nname:t v=nnote:t] pub-bal=(z-map nname:t nnote:t)]
      ::  only include v0 notes; v1 notes use lock-roots
      ?.  ?=(^ -.v)
        pub-bal
      ?:  ?&  (~(has z-in pubkeys.sig.v) pubkey)
              |(=(1 m.sig.v) =(1 ~(wyt z-in pubkeys.sig.v)))
          ==
        (~(put z-by pub-bal) k v)
      pub-bal
    ::
        [%heavy-summary ~]
      ^-  (unit (unit [(each (z-set sig:t) (z-set hash:t)) (unit page-summary:t)]))
      ?~  heaviest-block.c.k
        ``[[%& ~(key z-by v0-shares.m.k)] ~]
      =/  heaviest-block  (~(get z-by blocks.c.k) u.heaviest-block.c.k)
      ?~  heaviest-block
        ``[[%& ~(key z-by v0-shares.m.k)] ~]
      ?~  highest-block-height.d.k
        ``[[%& ~(key z-by v0-shares.m.k)] ~]
      ::  before v1-phase: return v0-shares (sigs)
      ::  at or after v1-phase: return shares (hashes)
      =/  keys=(each (z-set sig:t) (z-set hash:t))
        ?:  (gte u.highest-block-height.d.k v1-phase:t)
          [%| ~(key z-by shares.m.k)]
        [%& ~(key z-by v0-shares.m.k)]
      ``[keys `(to-page-summary:page:t (to-page:local-page:t u.heaviest-block))]
    ::
        [%blocks-summary ~]
      ^-  (unit (unit (list [block-id:t page:t])))
      :-  ~
      :-  ~
      %~  tap  z-by
      ^-  (z-map block-id:t page:t)
      %-  ~(run z-by blocks.c.k)
      |=  lp=local-page:t
      ^-  page:t
      ?^  -.lp  lp(pow ~)
      lp(pow ~)
    ::
        [%tx-accepted tid-b58=@t ~]
      ^-  (unit (unit ?))
      =+  tid=(from-b58:hash:t tid-b58:pole)
      ``(~(has z-by raw-txs.c.k) tid)
    ==
  ::
  ++  poke
    |=  [wir=wire eny=@ our=@ux now=@da dat=*]
    ^-  [(list effect:dk) kernel-state:dk]
    |^
    =/  old-state  m.k
    =/  cause  ((soft cause:dk) dat)
    ?~  cause
      ~>  %slog.[1 [%leaf "Error: badly formatted cause, should never occur."]]
      ~&  ;;([thing=@t ver=@ type=@t] [-.dat +<.dat +>-.dat])
      =/  peer-id  (get-peer-id wir)
      ?~  peer-id
        `k
      ~>  %slog.[1 [leaf+"Peer-id found in wire of badly formatted cause, emitting %liar-peer"]]
      [[%liar-peer u.peer-id %invalid-fact]~ k]
    =/  cause  u.cause
    ::~&  "inner dumbnet cause: {<[-.cause -.+.cause]>}"
    =^  effs  k
      ?+    wir  ~|("Unsupported wire: {<wir>}" !!)
          [%poke src=?(%nc %timer %sys %miner %grpc) ver=@ *]
        ?-  -.cause
          %command  (handle-command now eny p.cause)
          %fact     (handle-fact wir eny our now p.cause)
        ==
      ::
         [%poke %libp2p ver=@ typ=?(%gossip %response) %peer-id =peer-id:dk *]
        ?>  ?=(%fact -.cause)
        (handle-fact wir eny our now p.cause)
      ==
    ::  possibly update candidate block for mining
    =^  candidate-changed  m.k  (update-candidate-block:min c.k now)
    :_  k
    ?.  candidate-changed  effs
    :_  effs
    =/  version=proof-version:sp
      (height-to-proof-version:con ~(height get:page:t candidate-block.m.k))
    =/  target  (~(got z-by targets.c.k) ~(parent get:page:t candidate-block.m.k))
    =/  commit  (block-commitment:page:t candidate-block.m.k)
    ?-  version
      %0  [%mine %0 commit target pow-len:t]
      %1  [%mine %1 commit target pow-len:t]
      %2  [%mine %2 commit target pow-len:t]
    ==
    ::
    ::  +heard-genesis-block: check if block is a genesis block and decide whether to keep it
    ++  heard-genesis-block
      |=  [wir=wire now=@da eny=@ pag=page:t]
      ^-  [(list effect:dk) kernel-state:dk]
      ?:  (check-duplicate-block ~(digest get:page:t pag))
        :: do nothing (idempotency), we already have block
        `k
      ::
      ?~  btc-data.c.k
        ~>  %slog.[1 'heard-genesis-block: Bitcoin block hash not set!']
        !!
      ?.  (check-genesis pag u.btc-data.c.k genesis-seal.c.k)
        ::  is not a genesis block, throw it out and inform the king. note this
        ::  must be a %liar effect since genesis blocks have no powork and are
        ::  thus cheap to make, so we cannot trust their block-id.
        [[(liar-effect wir %not-a-genesis-block)]~ k]
      ::  heard valid genesis block
      ~>  %slog.[0 leaf+"heard-genesis-block: Validated genesis block!"]
      (accept-block now eny pag *tx-acc:t)
    ::
    ++  heard-block
      |=  [wir=wire now=@da pag=page:t eny=@]
      ^-  [(list effect:dk) kernel-state:dk]
      ?:  =(*page-number:t ~(height get:page:t pag))
        ::  heard genesis block
        ~>  %slog.[0 leaf+"heard-block: Heard genesis block"]
        (heard-genesis-block wir now eny pag)
      ?~  heaviest-block.c.k
        =/  peer-id=(unit @)  (get-peer-id wir)
        ?~  peer-id
          ::  received block before genesis from source other than libp2p
          `k
        :_  k
        (missing-parent-effects ~(digest get:page:t pag) ~(height get:page:t pag) u.peer-id)
      ::  if we don't have parent and block claims to be heaviest
      ::  request ancestors to catch up or handle reorg
      ?.  (~(has z-by blocks.c.k) ~(parent get:page:t pag))
        ?:  %+  compare-heaviness:page:t  pag
            (~(got z-by blocks.c.k) u.heaviest-block.c.k)
          =/  peer-id=(unit @)  (get-peer-id wir)
          ?~  peer-id
            ~|("heard-block: Unsupported wire: {<wir>}" !!)
          :_  k
          (missing-parent-effects ~(digest get:page:t pag) ~(height get:page:t pag) u.peer-id)
        ::  received block, don't have parent, isn't heaviest, ignore.
        `k
      ::  yes, we have its parent
      ::
      ::  do we already have this block?
      ?:  (check-duplicate-block ~(digest get:page:t pag))
        :: do almost nothing (idempotency), we already have block
        :: however we *should* tell the runtime we have it
        ~>  %slog.[1 leaf+"heard-block: Duplicate block"]
        :_  k
        [%seen %block ~(digest get:page:t pag) ~]~
      ::
      ::  check to see if the .digest is valid. if it is not, we
      ::  emit a %liar-peer. if it is, then any further %liar effects
      ::  should be %liar-block-id. this tells the runtime that
      ::  anybody who sends us this block id is a liar
      ?.  (check-digest:page:t pag)
        ~>  %slog.[1 leaf+"heard-block: Digest is not valid"]
        :_  k
        [(liar-effect wir %page-digest-invalid)]~
      ::
      ::  since we know the digest is valid, we want to tell the runtime
      ::  to start tracking that block-id.
      =/  block-effs=(list effect:dk)
        =/  =(pole)  wir
        ?.  ?=([%poke %libp2p ver=@ typ=?(%gossip %response) %peer-id =peer-id:dk *] pole)
          ~
        [%track %add ~(digest get:page:t pag) peer-id.pole]~
      ::
      ::  %liar-block-id only says that anybody who sends us this
      ::  block-id is a liar, but it doesn't (and can't) include the
      ::  peer id. so it gets cross-referenced with the blocks being
      ::  tracked to know who to ban.
      ::
      ::  the crash case is when we get a bad block from the grpc driver or
      ::  from the kernel itself.
      ::
      =/  check-page-without-txs=(reason:dk ~)
        (validate-page-without-txs-da:con pag now)
      ?:  ?=(%.n -.check-page-without-txs)
        ::  block has bad data
        :_  k
        ::  the order here matters since we want to add the block to tracking
        ::  and then ban the peer who sent it. we do this instead of %liar-peer
        ::  since its possible for another poke to be processed after %track %add
        ::  but before %liar-block-id, so more peers may be added to tracking
        ::  before %liar-block-id is processed.
        ~&  >>  page-failed+check-page-without-txs
        %+  snoc  block-effs
        [%liar-block-id ~(digest get:page:t pag) +.check-page-without-txs]
      ::
      ?.  (check-pow pag)
        ~>  %slog.[1 leaf+"heard-block: Failed PoW check"]
        :_  k
        %+  snoc  block-effs
        [%liar-block-id ~(digest get:page:t pag) %failed-pow-check]
      ::
      ::  tell driver we have seen this block so don't send it back to the kernel again
      =.  block-effs
        [[%seen %block ~(digest get:page:t pag) `~(height get:page:t pag)] block-effs]
      ::  stop tracking block id as soon as we verify pow
      =.  block-effs
        %+  snoc  block-effs
        ^-  effect:dk
        [%track %remove ~(digest get:page:t pag)]
      =>  .(c.k `consensus-state:dk`c.k)  ::  tmi
      =^  missing-txs=(list tx-id:t)  c.k
        (add-pending-block:con pag)
      =.  d.k  (update-highest:der ~(height get:page:t pag))
      ?:  !=(missing-txs *(list tx-id:t))
        ~>  %slog.[0 'heard-block: Missing transactions, requesting from peers']
        ::  block has missing txs
        =.  block-effs
          %+  weld  block-effs
          %+  turn  missing-txs
          |=  =tx-id:t
          ^-  effect:dk
          [%request %raw-tx %by-id tx-id]
        :_  k
        ?:  %+  compare-heaviness:page:t  pag
            (~(got z-by blocks.c.k) (need heaviest-block.c.k))
          ~>  %slog.[0 'heard-block: Gossiping new heaviest block (transactions pending validation)']
          :-  [%gossip %0 %heard-block pag]
          block-effs
        block-effs
      ::
      ::  block has no missing transactions, so we check that its transactions
      ::  are valid
      (process-block-with-txs now eny pag block-effs)
    ::
    ::  +heard-elders: handle response to parent hashes request
    ++  heard-elders
      |=  [wir=wire now=@da oldest=page-number:t ids=(list block-id:t)]
      ^-  [(list effect:dk) kernel-state:dk]
      ::  extract peer ID from wire
      =/  peer-id=(unit @)  (get-peer-id wir)
      ?~  peer-id
        ~|("heard-elders: Unsupported wire: {<wir>}" !!)
      =/  ids-lent  (lent ids)
      ?:  (gth ids-lent 24)
        ~>  %slog.[1 'heard-elders: More than 24 parent hashes received']
        :_  k
        [[%liar-peer u.peer-id %more-than-24-parent-hashes]~]
      ?.  ?|  =(oldest *page-number:t)
              =(ids-lent 24)
          ==
        =/  log-message
          %-  crip
          "heard-elders: ".
          "Received parent hashes, but either oldest is genesis ".
          "or exactly 24 parent hashes were received ".
          "(expected less than 24 only if oldest is genesis)"
        ~>  %slog.[1 log-message]
        ::  either oldest is genesis OR we must have received exactly 24 ids
        :_  k
        [[%liar-peer u.peer-id %less-than-24-parent-hashes]~]
      ::
      =/  log-message
        %^  cat  3
          'heard-elders: Received elders starting at height '
        (rsh [3 2] (scot %ui oldest))
      ~>  %slog.[0 log-message]
      ::  find highest block we have in the ancestor list
      =/  latest-known=(unit [=block-id:t =page-number:t])
        =/  height  (dec (add oldest ids-lent))
        |-
        ?~  ids  ~
        ?:  =(height 0)  ~
        ?:  (~(has z-by blocks.c.k) i.ids)
          `[i.ids height]
        $(ids t.ids, height (dec height))
      ?~  latest-known
        ?:  =(oldest *page-number:t)
          ?:  =(~ heaviest-block.c.k)
            ::  request genesis block because we don't have it yet
            :_  k
            [%request %block %by-height *page-number:t]~
          ::  if we have differing genesis blocks, liar
          ~>  %slog.[1 'heard-elders: Received bad response, differing genesis blocks']
          :_  k
          [[%liar-peer u.peer-id %differing-genesis]~]
        ::  request elders of oldest ancestor to catch up faster
        ::  hashes are ordered newest>oldest
        =/  last-id  (rear ids)
        :: extra log to clarify that this is a deep re-org.
        :: we need to handle this case but we hope to never see this
        =/  log-message
          %+  rap  3
          :~  'heard-elders: (DEEP REORG) Requesting oldest ancestor for block '
              (to-b58:hash:t last-id)
              ' at height '
              (rsh [3 2] (scot %ui oldest))
          ==
        ~>  %slog.[0 log-message]
        :_  k
        (missing-parent-effects last-id oldest u.peer-id)
      =/  print-var
        %^  cat  3
          %-  crip
          "heard-elders: Processed elders and found intersection: ".
          "requesting next block at height "
        (rsh [3 2] (scot %ui +(page-number.u.latest-known)))
      ~>  %slog.[0 print-var]
      ::  request next block after our highest known block
      ::  this will trigger either catchup or reorg from this point
      :_  k
      [%request %block %by-height +(page-number.u.latest-known)]~
    ::
    ++  check-duplicate-block
      |=  digest=block-id:t
      ?|  (~(has z-by blocks.c.k) digest)
          (~(has z-by pending-blocks.c.k) digest)
      ==
    ::
    ++  check-genesis
     |=  [pag=page:t btc-hash=(unit btc-hash:t) =genesis-seal:t]
     ^-  ?
     =/  check-digest  (check-digest:page:t pag)
     =/  check-pow-hash=?
      ?.  check-pow-flag:t
         ::  this case only happens during testing
         ::~&  "skipping pow hash check for {(trip (to-b58:hash:t ~(digest get:page:t pag)))}"
         %.y
       %-  check-target:mine
       :_  ~(target get:page:t pag)
       (proof-to-pow:zeke (need ~(pow get:page:t pag)))
     =/  check-pow-valid=?  (check-pow pag)
     ::
     ::  check if timestamp is in base field, this will anchor subsequent timestamp checks
     ::  since child block timestamps have to be within a certain range of the most recent
     ::  N blocks.
     =/  check-timestamp=?  (based:zeke ~(timestamp get:page:t pag))
     =/  check-txs=?  =(~(tx-ids get:page:t pag) *(z-set tx-id:t))
     =/  check-epoch=?  =(~(epoch-counter get:page:t pag) *@)
     =/  check-target=?  =(~(target get:page:t pag) genesis-target:t)
     =/  check-work=?  =(~(accumulated-work get:page:t pag) (compute-work:page:t genesis-target:t))
     =/  cb=coinbase-split:t  ~(coinbase get:page:t pag)
     ?>  ?=(%0 -.cb)
     =/  check-coinbase=?  =(+.cb *(z-map sig:t @))
     =/  check-height=?  =(~(height get:page:t pag) *page-number:t)
     =/  check-btc-hash=?
       ?~  btc-hash
         ~>  %slog.[0 'check-genesis: Not checking btc hash when validating genesis block']
         %.y
       =(~(parent get:page:t pag) (hash:btc-hash:t u.btc-hash))
     ::
     ::  check that the message matches what's in the seal
     =/  check-msg=?
       ?~  genesis-seal
         ~>  %slog.[1 'check-genesis: Genesis seal not set, cannot check genesis block']  !!
       =((hash:page-msg:t ~(msg get:page:t pag)) msg-hash.u.genesis-seal)
     ~&  :*  check-digest+check-digest
             check-pow-hash+check-pow-hash
             check-pow-valid+check-pow-valid
             check-timestamp+check-timestamp
             check-txs+check-txs
             check-epoch+check-epoch
             check-target+check-target
             check-work+check-work
             check-coinbase+check-coinbase
             check-height+check-height
             check-msg+check-msg
             check-btc-hash+check-btc-hash
         ==
     ?&  check-digest
         check-pow-hash
         check-pow-valid
         check-timestamp
         check-txs
         check-epoch
         check-target
         check-work
         check-coinbase
         check-height
         check-msg
         check-btc-hash
     ==
    ++  check-pow
      |=  pag=page:t
      ^-  ?
      ?.  check-pow-flag:t
        ~>  %slog.[1 'check-pow: check-pow-flag is off, skipping pow check']
        ::  this case only happens during testing
        %.y
      =/  pow  ~(pow get:page:t pag)
      ?~  pow
        %.n
      ::
      ::  validate that powork puzzle in the proof is correct.
      ?&  (check-pow-puzzle u.pow pag)
          ::
          ::  validate the powork. this is done separately since the
          ::  other checks are much cheaper.
          (verify:nv u.pow ~ eny)
      ==
    ::
    ++  check-pow-puzzle
      |=  [pow=proof:sp pag=page:t]
      ^-  ?
      ?:  =((lent objects.pow) 0)
        %.n
      =/  puzzle  (snag 0 objects.pow)
      ?.  ?=(%puzzle -.puzzle)
        %.n
      ?&  =((block-commitment:page:t pag) commitment.puzzle)
          =(pow-len:t len.puzzle)
      ==
    ::
    ++  heard-tx
      |=  [wir=wire now=@da raw=raw-tx:t eny=@]
      ^-  [(list effect:dk) kernel-state:dk]
      ~>  %slog.[0 'heard-tx: Received raw transaction']
      =/  id-b58  (to-b58:hash:t ~(id get:raw-tx:t raw))
      ~>  %slog.[0 (cat 3 'heard-tx: Raw transaction id: ' id-b58)]
      ::
      ::  check if we already have raw-tx
      ?:  (has-raw-tx:con ~(id get:raw-tx:t raw))
        :: do almost nothing (idempotency), we already have it
        :: but do tell the runtime we've already seen it
        =/  log-message
          %^  cat  3
           'heard-tx: Transaction id already seen: '
          id-b58
        ~>  %slog.[1 log-message]
        :_  k
        [%seen %tx ~(id get:raw-tx:t raw)]~
      ::
      ::  check if the raw-tx contents are in base field
      ?.  (based:raw-tx:t raw)
        :_  k
        [(liar-effect wir %raw-tx-not-based)]~
      ::
      ::  check tx-id. this is faster than calling validate:raw-tx (which also checks the id)
      ::  so we do it first
      =/  computed-id=hash:t  (compute-id:raw-tx:t raw)
      ?.  =(computed-id ~(id get:raw-tx:t raw))
        =/  log-message
          ;:  (cury cat 3)
            'heard-tx: Invalid transaction id: '
            id-b58
            ', expected: '
            (to-b58:hash:t computed-id)
          ==
        ~>  %slog.[1 log-message]
        :_  k
        [(liar-effect wir %tx-id-invalid)]~
      ::
      ::  check if raw-tx is part of a pending block
      ::
      ?:  (needed-by-block:con ~(id get:raw-tx:t raw))
        ::  pending blocks are waiting on tx
        ?.  (validate:raw-tx:t raw)
          ::  raw-tx doesn't validate.
          ::  remove blocks containing bad tx from pending state. note that since
          ::  we already checked that the id of the transaction was valid, we
          ::  won't accidentally throw out a block that contained a valid tx-id
          ::  just because we received a tx that claimed the same id as the valid
          ::  one.
          =/  tx-pending-blocks  (~(get z-ju blocks-needed-by.c.k) ~(id get:raw-tx:t raw))
          =.  c.k
            %-  ~(rep z-in tx-pending-blocks)
            |=  [id=block-id:t c=_c.k]
            =.  c.k  c
            (reject-pending-block:con id)
          ::
          ~>  %slog.[1 'heard-tx: Pending blocks waiting on invalid transaction!']
          :_  k
          [(liar-effect wir %page-pending-raw-tx-invalid) ~]
        =^  work  c.k  (add-raw-tx:con raw)
        ~>  %slog.[0 'heard-tx: Processing ready blocks']
        (process-ready-blocks now eny work raw)
      ::  no pending blocks waiting on tx
      ::
      ::  check if any inputs are absent in heaviest balance
      ?.  (inputs-in-heaviest-balance:con raw)
        ::  input(s) in tx not in balance, discard tx
        ~>  %slog.[1 'heard-tx: Inputs not in heaviest balance, discarding transaction']
        `k
      ::  all inputs in balance
      ::
      ::  check if any inputs are in spent-by
      ?:  (inputs-spent:con raw)
        ::  inputs present in spent-by, discard tx
        ~>  %slog.[1 'heard-tx: Inputs present in spent-by, discarding transaction']
        `k
      ::  inputs not present in spent-by
      ?.  (validate:raw-tx:t raw)
        ::  raw-tx doesn't validate.
        ~>  %slog.[1 'heard-tx: Transaction invalid, discarding']
        :_  k
        [(liar-effect wir %tx-inputs-not-in-spent-by-and-invalid)]~
      ::
      =^  work  c.k
        (add-raw-tx:con raw)
      :: no blocks were depending on this so work should be empty
      ?>  =(~ work)
      ::
      ~>  %slog.[0 'heard-tx: Heard new valid transaction']
      :-  ~[[%seen %tx ~(id get:raw-tx:t raw)] [%gossip %0 %heard-tx raw]]
      k
    ::
    ::  +process-ready-blocks: process blocks no longer waitings on txs
    ++  process-ready-blocks
      |=  [now=@da eny=@ work=(list block-id:t) =raw-tx:t]
      ^-  [(list effect:dk) kernel-state:dk]
      ::  .work contains block-ids for blocks that no longer have any
      ::  missing transactions
      =^  eff  k
        %+  roll  work
        |=  [bid=block-id:t effs=(list effect:dk) k=_k]
        =.  ^k  k
        ::  process the block, skipping the steps that we know its already
        ::  done by the fact that it was in pending-blocks.c.k
        =^  new-effs  k
          %:  process-block-with-txs
            now  eny
            page:(~(got z-by pending-blocks.c.k) bid)
            :: if the block is bad, then tell the driver we dont want to see it
            :: again
            ~[[%seen %block bid ~]]
          ==
        ::  remove the block from pending blocks. at this point, its either
        ::  been discarded by the kernel or lives in the consensus state
        [(weld new-effs effs) k]
      ::
      eff^k
    ::
    ::
    ::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::
    ::  the remaining arms are used by both %heard-tx and %heard-block
    ::
    ::  +process-block-with-txs: process a block that we have all txs for
    ::
    ::    this is called along the codepath for both %heard-block and +heard-tx,
    ::    since once we hear the last transaction we're waiting for in a given
    ::    block, we immediately try to validate it. the genesis block does _not_
    ::    go through here.
    ::
    ::    bad-block-effs are effects which are passed through and emitted
    ::    only if the block is bad. If the block is good then ++accept-block
    ::    emits effects and bad-block-effs is ignored.
    ++  process-block-with-txs
      |=  [now=@da eny=@ pag=page:t bad-block-effs=(list effect:dk)]
      ^-  [(list effect:dk) kernel-state:dk]
      =/  digest-b58  (to-b58:hash:t ~(digest get:page:t pag))
      ::
      ::  if we do have all raw-txs, check if pag validates
      ::  (i.e. transactions are valid and size isnt too big)
      =/  new-transfers=(reason:dk tx-acc:t)
        (validate-page-with-txs:con pag)
      ?-    -.new-transfers
          %.y
        (accept-block now eny pag +.new-transfers)
        ::
          %.n
        =/  log-message
          %^  cat  3
            'process-block-with-txs: Block did not validate. Reason: '
          p.new-transfers
        ~>  %slog.[0 log-message]
        ::  did not validate, so we throw the block out and stop
        ::  tracking it
        =.  c.k  (reject-pending-block:con ~(digest get:page:t pag))
        [bad-block-effs k]
      ==
    ::
    ::  +accept-block: update kernel state with new valid block.
    ++  accept-block
      |=  [now=@da eny=@ pag=page:t acc=tx-acc:t]
      ^-  [(list effect:dk) kernel-state:dk]
      ::
      ::  page is validated, update consensus and derived state
      =.  c.k  (accept-page:con pag acc now)
      =/  print-var
        =/  pow-print=@t
          ?:  check-pow-flag:t
            =/  pow  ~(pow get:page:t pag)
            ?>  ?=(^ pow)
            %+  rap  3
            :~  ' with proof version '  (rsh [3 2] (scot %ui version.u.pow))
            ==
          '. Skipping pow check because check-pow-flag was disabled'
        %-  trip
        ^-  @t
        %+  rap  3
        :~  'accept-block: '
            'block '  (to-b58:hash:t ~(digest get:page:t pag))
            ' added to validated blocks at '  (rsh [3 2] (scot %ui ~(height get:page:t pag)))
            pow-print
        ==
      ~>  %slog.[0 %leaf^print-var]
      =/  effs=(list effect:dk)
        ::  request block N+1 on each peer's heaviest chain
        :+  [%request %block %by-height +(~(height get:page:t pag))]
          ::  tell driver we've seen this block so don't process it again
          [%seen %block ~(digest get:page:t pag) `~(height get:page:t pag)]
        ~
      ::
      =/  old-heavy  heaviest-block.c.k
      =.  c.k  (update-heaviest:con pag)
      ::
      =/  is-new-heaviest=?  !=(old-heavy heaviest-block.c.k)
      ::  if block is the new heaviest block, gossip it to peers
      =?  effs  is-new-heaviest
        ~>  %slog.[0 'accept-block: New heaviest block!']
        =/  span=span-effect:dk
          :+  %span  %new-heaviest-chain
          ~['block_height'^n+~(height get:page:t pag) 'heaviest_block_digest'^s+(to-b58:hash:t ~(digest get:page:t pag))]
        :*  [%gossip %0 %heard-block pag]
            span
            effs
        ==
      ::  case (a): block validated but not new heaviest - it's on a side chain
      =?  effs  !is-new-heaviest
          :_  effs
          :+  %span  %orphaned-block
          :~  'block_id'^s+(to-b58:hash:t ~(digest get:page:t pag))
              'block_height'^n+~(height get:page:t pag)
              'event_type'^s+'side-chain-orphan'
          ==
      ::
      =/  is-reorg=?
        ?~  old-heavy  %.n  ::  first block after genesis, not a reorg
        &(is-new-heaviest !=(~(parent get:page:t pag) u.old-heavy))
      ::  case (b): new heaviest block - check if it's a reorganization
      =?  effs  is-reorg
        ?~  old-heavy  effs
        ::  reorganization detected - previous heaviest block is now orphaned
        =/  orphaned-block-span=span-effect:dk
          :+  %span  %orphaned-block
          :~  'block_id'^s+(to-b58:hash:t u.old-heavy)
              'new_heaviest_block'^s+(to-b58:hash:t ~(digest get:page:t pag))
              'new_height'^n+~(height get:page:t pag)
              'event_type'^s+'reorg-orphan'
          ==
        =/  reorg-span=span-effect:dk
          :+  %span  %chain-reorg
          :~  'block_id'^s+(to-b58:hash:t u.old-heavy)
              'new_heaviest_height'^n+~(height get:page:t pag)
              'event_type'^s+'reorg'
          ==
        [orphaned-block-span reorg-span effs]
      ::
      ::  Garbage collect pending blocks and excluded transactions.
      ::  Garbage collection only runs when we receive a new heaviest
      ::  block, since that's when the block height advances and we can
      ::  determine what's expired. Pending blocks are removed based on
      ::  elapsed heaviest blocks since they were heard. Excluded txs are
      ::  removed based on the same criteria with the added check that they
      ::  they aren't spent in the current heaviest chain.
      =?  c.k  is-new-heaviest
        (garbage-collect:con retain.a.k)
      ::
      ::  if new block is heaviest, regossip txs that haven't been garbage collected
      =?  effs  is-new-heaviest
        %-  ~(rep z-in excluded-txs.c.k)
        |=  [=tx-id:t effs=_effs]
        [[%gossip %0 %heard-tx (got-raw-tx:con tx-id)] effs]
      ::  regossip block transactions if mining
      =.  effs  (weld (regossip-block-txs-effects pag) effs)
      ::
      ::  tell the miner about the new block
      =.  m.k  (heard-new-block:min c.k now)
      ::
      ::  update derived state
      =.  d.k  (update:der c.k pag)
      ?.  =(old-heavy heaviest-block.c.k)
        =^  mining-effs  k  do-mine
        =.  effs  (weld mining-effs effs)
        effs^k
      ::
      effs^k
    ::
    ::  +liar-effect: produce the appropriate liar effect
    ::
    ::    this only produces the `%liar-peer` effect. the other possibilities
    ::    are receiving a bad block or tx via the grpc driver or from within
    ::    the miner module or +do-genesis. in this case we just emit a
    ::    warning and crash, since that means there's a bug.
    ++  liar-effect
      |=  [wir=wire r=term]
      ^-  effect:dk
      ?+    wir  ~|("liar-effect: Bad wire for liar effect! {<wir>}" !!)
          [%poke %libp2p ver=@ typ=?(%gossip %response) %peer-id id=@ *]
        [%liar-peer (need (get-peer-id wir)) r]
      ::
          [%poke %grpc ver=@ *]
        ~|  'liar-effect: ATTN: received a bad block or tx via grpc driver'
        !!
      ::
          [%poke %miner *]
        ::  this indicates that the mining module built a bad block and then
        ::  told the kernel about it. alternatively, +do-genesis produced
        ::  a bad genesis block. this should never happen, it indicates
        ::  a serious bug otherwise.
        ~|  'liar-effect: ATTN: miner or +do-genesis produced a bad block!'
        !!
      ::
          [%poke %sys *]
        ?:  =(%not-a-genesis-block r)
          ~|  'liar-effect: ATTN: received a bad genesis block! check pow params and jamfile'
          !!
        ~|  'liar-effect: ATTN: received an unknown bad poke!'
        !!
      ==
    ::
    ++  get-peer-id
      |=  wir=wire
      ^-  (unit @)
      =/  =(pole)  wir
      ?.  ?=([%poke %libp2p ver=@ typ=?(%gossip %response) %peer-id id=@ *] pole)
        ~
      (some id.pole)
    ::
    ++  handle-command
      |=  [now=@da eny=@ =command:dk]
      ^-  [(list effect:dk) kernel-state:dk]
      ~>  %slog.[0 (cat 3 'handle-command: ' -.command)]
      ::  ~&  "handling command: {<-.command>}"
      ?:  &(?=(init-only-command:dk -.command) !init.a.k)
        ::  kernel no longer in init phase, can't do init-only command
        ~>  %slog.[1 'handle-command: Kernel no longer in init phase, cannot do init-only command']
        `k
      ?:  &(?!(?=(init-command:dk -.command)) init.a.k)
        ::  kernel in init phase, can't perform non-init command
        ~>  %slog.[1 'handle-command: Kernel is in init phase, cannot do non-init command']
        `k
      |^
      ?-  -.command
          %born
        ::  We leave this string interpolation in because %born only happens once on boot
        ~&  constants+constants.k
        (do-born eny)
      ::
          %pow
        do-pow
      ::
          %set-mining-key
        do-set-mining-key
      ::
          %set-mining-key-advanced
        do-set-mining-key-advanced
      ::
          %enable-mining
        do-enable-mining
      ::
          %timer
        do-timer
      ::
          %set-genesis-seal
        =.  c.k  (set-genesis-seal:con p.command)
        `k
      ::
          %genesis
        do-genesis
      ::
          %btc-data
        do-btc-data
      ::
      ::  !!! COMMANDS BELOW ARE ONLY FOR TESTING. NEVER CALL IF RUNNING MAINNET !!!
      ::
          %set-constants
        `k(constants p.command)
      ==
      ::
      ++  do-born
        |=  eny=@
        ^-  [(list effect:dk) kernel-state:dk]
        ?>  ?=([%born *] command)
        ::  once born command is registered, the init phase is over
        ::  note state update won't be registered unless poke is successful.
        =.  k  k(init.a %.n)
        :: do we have any blocks?
        ?~  heaviest-block.c.k
          ::  no, request genesis block
          ?~  btc-data.c.k
            ~>  %slog.[1 'do-born: No genesis parent btc block hash set, crashing']
            !!
          ::  requesting any genesis block, keeping first one we see.
          ::  we do not request blocks by id so we can only request height 0
          ::  blocks and throw out ones we aren't expecting
          ~>  %slog.[0 'do-born: Requesting genesis block']
          :_  k
          [%request %block %by-height *page-number:t]~
        :: yes, so get height N of heaviest block and request the block
        :: of height N+1
        :: Also emit %seen for the heaviest block so our cache can start to update
        =/  height=page-number:t
          +(~(height get:local-page:t (~(got z-by blocks.c.k) u.heaviest-block.c.k)))
        =/  born-effects=(list effect:dk)
          :~  [%request %block %by-height height]
              [%seen %block u.heaviest-block.c.k `height]
          ==
        =/  k=kernel-state:dk  k
        =^  mine-effects=(list effect:dk)  k
          do-mine
        ~>  %slog.[0 'do-born: Dumbnet born']
        :_  k
        (weld mine-effects born-effects)
      ::
      ++  do-pow
        ^-  [(list effect:dk) kernel-state:dk]
        ?>  ?=([%pow *] command)
        =/  commit=block-commitment:t
          (block-commitment:page:t candidate-block.m.k)
        ?.  =(bc.command commit)
          ~>  %slog.[1 'do-pow: Mined for wrong (old) block commitment']
          [~ k]
        ?:  %+  check-target:mine  dig.command
            (~(got z-by targets.c.k) ~(parent get:page:t candidate-block.m.k))
          =.  m.k  (set-pow:min prf.command)
          =.  m.k  set-digest:min
          =^  heard-block-effs  k  (heard-block /poke/miner now candidate-block.m.k eny)
          :_  k
          heard-block-effs
        [~ k]
      ::
      ++  do-set-mining-key
        ^-  [(list effect:dk) kernel-state:dk]
        ?>  ?=([%set-mining-key *] command)
        =/  pk=(unit schnorr-pubkey:t)
          (mole |.((from-b58:schnorr-pubkey:t v0.command)))
        =/  pkh=(unit hash:t)
          (mole |.((from-b58:hash:t v1.command)))
        ?~  pk
          ~>  %slog.[1 'do-set-mining-key: Invalid mining pubkey, exiting']
          [[%exit 1]~ k]
        ?~  pkh
          ~>  %slog.[1 'do-set-mining-key: Invalid mining pubkey, exiting']
          [[%exit 1]~ k]
        =/  =sig:t  (new:sig:t u.pk)
        =.  m.k  (set-v0-shares:min [sig 100]~)
        =.  m.k  (set-shares:min [u.pkh 100]~)
        `k
      ::
      ++  do-set-mining-key-advanced
        ^-  [(list effect:dk) kernel-state:dk]
        ?>  ?=([%set-mining-key-advanced *] command)
        ?:  (gth (lent v0.command) 2)
        ~>  %slog.[1 'do-set-mining-key-advanced: Coinbase split for more than two sigs not yet supported, exiting']
          [[%exit 1]~ k]
        ?:  (gth (lent v1.command) 2)
        ~>  %slog.[1 'do-set-mining-key-advanced: Coinbase split for more than two public-key hashes not yet supported, exiting']
          [[%exit 1]~ k]
        ?~  v0.command
        ~>  %slog.[1 'do-set-mining-key-advanced: Empty list of sigs, exiting.']
          [[%exit 1]~ k]
        ::
        ?~  v1.command
        ~>  %slog.[1 'do-set-mining-key-advanced: Empty list of public key hashes, exiting.']
          [[%exit 1]~ k]
        ::
        =/  [v0-shares=(list [sig:t @]) crash=?]
          %+  roll  `(list [@ @ (list @t)])`v0.command
          |=  $:  [s=@ m=@ ks=(list @t)]
                  shares=(list [sig:t @])
                  crash=_`?`%|
              ==
          =+  r=(mule |.((from-b58:sig:t m ks)))
          ?:  ?=(%| -.r)
            ((slog p.r) [~ %&])
          [[[p.r s] shares] crash]
        ?:  crash
          ~>  %slog.[1 'do-set-mining-key-advanced: Invalid public keys provided, exiting']
          [[%exit 1]~ k]
        =/  [shares=(list [hash:t @]) crash=?]
          %+  roll  `(list [@ @t])`v1.command
          |=  $:  [s=@ h=@t]
                  shares=(list [hash:t @])
                  crash=_`?`%|
              ==
          =+  r=(mule |.(=-(?:((based:hash:t -) - !!) (from-b58:hash:t h))))
          ?:  ?=(%| -.r)
            ((slog p.r) [~ %&])
          [[[p.r s] shares] crash]
        ?:  crash
          ~>  %slog.[1 'do-set-mining-key-advanced: Invalid public keys provided, exiting']
          [[%exit 1]~ k]
        =.  m.k  (set-v0-shares:min v0-shares)
        =.  m.k  (set-shares:min shares)
        `k
      ::
      ++  do-enable-mining
        ^-  [(list effect:dk) kernel-state:dk]
        ?>  ?=([%enable-mining *] command)
        ?.  p.command
          ::~&  >  'generation of candidate blocks disabled'
          =.  m.k  (set-mining:min p.command)
          `k
        ?:  no-keys-set:min
          ::  ~&  >
          ::      """
          ::      generation of candidate blocks has not been enabled because mining pubkey
          ::      is empty. set it with %set-mining-key then run %enable-mining again
          ::      """
          `k
        ?:  =(~ heaviest-block.c.k)
          ::~&  >
          ::    """
          ::    generation of candidate blocks enabled. candidate block will be generated
          ::    once a genesis block has been received.
          ::    """
          =.  m.k  (set-mining:min p.command)
          `k
        ::~&  >  'generation of candidate blocks enabled.'
        =.  m.k  (set-mining:min p.command)
        =.  m.k  (heard-new-block:min c.k now)
        `k
      ::
      ++  do-timer
        ::TODO post-dumbnet: only rerequest transactions a max of once/twice (maybe an admin param)
        ^-  [(list effect:dk) kernel-state:dk]
        ?>  ?=([%timer *] command)
        ?:  init.a.k
          ::  kernel in init phase, command ignored
          `k
        =/  effects=(list effect:dk)
          %+  turn  missing-tx-ids:con
          |=  =tx-id:t
          ^-  effect:dk
          [%request %raw-tx %by-id tx-id]
        ::
        ::  we always request the next heaviest block with each %timer event
        =/  heavy-height=page-number:t
          ?~  heaviest-block.c.k
            *page-number:t  ::  rerequest genesis block
          +(~(height get:local-page:t (~(got z-by blocks.c.k) u.heaviest-block.c.k)))
        =.  effects
          [[%request %block %by-height heavy-height] effects]
        =.  effects
          (weld regossip-candidate-block-txs-effects effects)
        effects^k
      ::
      ++  do-genesis
        ::  generate genesis block and sets it as candidate block
        ^-  [(list effect:dk) kernel-state:dk]
        ?>  ?=([%genesis *] command)
        ::  creating genesis block with template
        ~>  %slog.[0 'do-genesis: Creating genesis block with template']
        =/  =genesis-template:t
          (new:genesis-template:t p.command)
        =/  genesis-page=page:t
          (new-genesis:page:t genesis-template now)
        =.  candidate-block.m.k  genesis-page
        =.  c.k  (add-btc-data:con `btc-hash.p.command)
        `k
      ::
      ++  do-btc-data
        ^-  [(list effect:dk) kernel-state:dk]
        ?>  ?=([%btc-data *] command)
        =.  c.k  (add-btc-data:con p.command)
        `k
      --::+handle-command
    ::
    ++  handle-fact
      |=  [wir=wire eny=@ our=@ux now=@da =fact:dk]
      ^-  [(list effect:dk) kernel-state:dk]
      ~>  %slog.[0 (cat 3 'handle-fact: ' +<.fact)]
      ?:  init.a.k
        ::  kernel in init phase, fact ignored
        `k
      ?-    -.data.fact
          %heard-block
        (heard-block wir now p.data.fact eny)
      ::
          %heard-tx
        (heard-tx wir now p.data.fact eny)
      ::
          %heard-elders
        (heard-elders wir now p.data.fact)
      ==
      ::
      ++  do-mine
        ^-  [(list effect:dk) kernel-state:dk]
        ?.  mining.m.k
          `k
        ?:  no-keys-set:min
          ::~&  "cannot mine without first setting pubkey with %set-mining-key"
          `k
        =/  commit=block-commitment:t
          (block-commitment:page:t candidate-block.m.k)
        =/  target  ~(target get:page:t candidate-block.m.k)
        =/  proof-version  (height-to-proof-version:con ~(height get:page:t candidate-block.m.k))
        =/  mine-start
          ?-  proof-version
            %0  [%0 commit target pow-len:t]
            %1  [%1 commit target pow-len:t]
            %2  [%2 commit target pow-len:t]
          ==
        :_  k
        [%mine mine-start]~
      ::
      ::  only send a %elders request for reasonable heights
      ++  missing-parent-effects
        |=  [=block-id:t block-height=page-number:t peer-id=@]
        ^-  (list effect:dk)
        ?~  highest-block-height.d.k
          ~|  %missing-parent-genesis-case :: below assertion should never trip
          ?>  ?=(~ heaviest-block.c.k)
          =/  log-message
            %+  rap  3
            :~  'missing-parent-effects: '
                'No genesis block but heard block with id '
               (to-b58:hash:t block-id)
               ': requesting genesis block'
            ==
          ~>  %slog.[0 log-message]
          [%request %block %by-height 0]~ :: ask for the genesis block, we don't have it
        ?:  (gth block-height +(u.highest-block-height.d.k))
          ::  ask for next-heaviest block, too far up for elders
          =/  log-message
            %+  rap  3
            :~  'missing-parent-effects: '
                'Heard block '
                (to-b58:hash:t block-id)
                ' at height '
                (rsh [3 2] (scot %ui block-height))
                ' but we only have blocks up to height '
                (rsh [3 2] (scot %ui u.highest-block-height.d.k))
                ': requesting next highest block.'
            ==
          ~>  %slog.[0 log-message]
          [%request %block %by-height +(u.highest-block-height.d.k)]~ :: ask for the next block by height
        :: ask for elders
        =/  log-message
          %+  rap  3
          :~  'missing-parent-effects: '
              'Potential reorg: requesting elders for block '
              (to-b58:hash:t block-id)
              ' at height '
              (rsh [3 2] (scot %ui block-height))
          ==
        ~>  %slog.[0 log-message]
        [%request %block %elders block-id peer-id]~ :: ask for elders
    ::
    ::  only if mining: re-gossip transactions included in block when block is fully validated
    ::  precondition: all transactions for block are in raw-txs
    ++  regossip-block-txs-effects
      |=  =page:t
      ^-  (list effect:dk)
      ?:  no-keys-set:min  ~
      %-  ~(rep z-in ~(tx-ids get:page:t page))
      |=  [=tx-id:t effects=(list effect:dk)]
      ^-  (list effect:dk)
      =/  tx=raw-tx:t  raw-tx:(~(got z-by raw-txs.c.k) tx-id)
      =/  fec=effect:dk  [%gossip %0 %heard-tx tx]
      [fec effects]
    ::
    ::  only if mining: regossip transactions included in candidate block
    ++  regossip-candidate-block-txs-effects
      ^-  (list effect:dk)
      (regossip-block-txs-effects candidate-block.m.k)
    --::  +poke
  --::  +kernel
--
:: churny churn 1
