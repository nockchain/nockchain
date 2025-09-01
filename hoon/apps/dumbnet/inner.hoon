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
    =.  k  ~>  %bout  (update-constants (check-checkpoints (state-n-to-5 arg)))
    =.  c.k  ~>  %bout  check-and-repair:con
    k
    ::  this arm should be renamed each state upgrade to state-n-to-[latest] and extended to loop through all upgrades
    ++  state-n-to-5
      |=  arg=load-kernel-state:dk
      ^-  kernel-state:dk
      ?.  ?=(%5 -.arg)
        ~>  %slog.[0 'load: State upgrade required']
        ?-  -.arg
            ::
          %0  $(arg (state-0-to-1 arg))
          %1  $(arg (state-1-to-2 arg))
          %2  $(arg (state-2-to-3 arg))
          %3  $(arg (state-3-to-4 arg))
          %4  $(arg (state-4-to-5 arg))
        ==
      arg
    ::  upgrade kernel state 4 to kernel state 5
    ++  state-4-to-5
    |=  arg=kernel-state-4:dk
    ^-  kernel-state-5:dk
    |^
      [%5 new-consensus a.arg m.arg d.arg constants.arg]
    ++  new-consensus
      ^-  consensus-state-5:dk
      ~>  %slog.[0 'load: This upgrade may take some time']
      =/  blocks-needed-by=(z-jug tx-id:t block-id:t)
        %-  ~(rep z-by blocks.c.arg)
        |=  [[=block-id:t pag=local-page:t] bnb=(z-jug tx-id:t block-id:t)]
        ^-  (z-jug tx-id:t block-id:t)
        %-  ~(rep z-in tx-ids.pag)
        |=  [=tx-id:t bnb=_bnb]
        ^-  (z-jug tx-id:t block-id:t)
        =+
          ?.  (~(has z-by raw-txs.c.arg) tx-id)
            ~>  %slog.[1 'load: Missing transaction in consensus state. Please alert the developers.']  ~
            ~
        (~(put z-ju bnb) tx-id block-id)
      ~>  %slog.[0 'load: Indexed blocks by transaction id']
      =/  rtx=(map tx-id:t *)  raw-txs.c.arg
      =/  bnb=(map tx-id:t *)  blocks-needed-by
      =/  excluded-map=(map tx-id:t *)  (~(dif z-by rtx) bnb)
      =/  excluded-txs=(z-set tx-id:t)  ~(key z-by excluded-map)
      =+
        ?:  =(*(z-set tx-id:t) excluded-txs)
          ~>  %slog.[0 'load: Consensus state is consistent']  ~
        :: this is only a concern at upgrade time. After the upgrade this is allowed to happen
        =/  log-message
          %-  crip
          "load: ".
          "There are transactions in consensus state which are not included in any block. ".
          "Please inform the developers."
        ~>  %slog.[1 log-message]  ~
      =/  [spent-by=(z-jug nname:t tx-id:t) raw-txs=(z-map tx-id:t [raw-tx:t @])]
        %-  ~(rep z-by raw-txs.c.arg)
        |=  [[=tx-id:t =raw-tx:t] [sb=(z-jug nname:t tx-id:t) rtx=(z-map tx-id:t [raw-tx:t @])]]
        ^-  [(z-jug nname:t tx-id:t) (z-map tx-id:t [raw-tx:t @])]
        =.  sb
          %-  ~(rep z-in (inputs-names:raw-tx:t raw-tx))
          |=  [=nname:t sb=_sb]
          (~(put z-ju sb) nname tx-id)
        =.  rtx  (~(put z-by rtx) tx-id [raw-tx 0])
        [sb rtx]
      ~>  %slog.[0 'load: Indexed transactions by spent notes']
      ~>  %slog.[0 'load: Upgrade state version 4 to version 5 complete']
      =|  pending-blocks=(z-map block-id:t [=page:t heard-at=@])
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
      =.  candidate-acc.m.arg  (new:tx-acc:t (~(get z-by balance.c.arg) u.heaviest-block.c.arg))
      =.  tx-ids.candidate-block.m.arg  ~
      [%4 c.arg p a.arg m.arg d.arg constants.arg]
    ::  upgrade kernel-state-2 to kernel-state-3
    ++  state-2-to-3
      |=  arg=kernel-state-2:dk
      ^-  kernel-state-3:dk
      ~>  %slog.[0 'load: State version 2 to version 3']
      =/  raw-txs=(z-map tx-id:t raw-tx:t)
        %-  ~(rep z-by txs.c.arg)
        |=  [[block-id:t m=(z-map tx-id:t tx:t)] n=(z-map tx-id:t raw-tx:t)]
        %-  ~(uni z-by n)
        %-  ~(run z-by m)
        |=  =tx:t
        ^-  raw-tx:t  -.tx
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
      =/  d  [*(unit page-number:t) heaviest-chain.d.arg]
      =.  d  (compute-highest blocks.c.arg pending-blocks.p.arg d constants.arg)
      [%1 c.arg p.arg a.arg m.arg d constants.arg]
    ::  compute the highest block (for the 0-1 upgrade)
    ++  compute-highest
      |=  $:  blocks=(z-map block-id:t local-page:t)
              pending=(z-map block-id:t local-page:t)
              derived-state=derived-state-1:dk
              constants=blockchain-constants:t
          ==
      =/  both  (~(uni z-by blocks) pending)
      =/  list  ~(tap z-by both)
      |-  ^-  derived-state-1:dk
      ?~  list  derived-state
      %=  $
        derived-state  (~(update-highest dumb-derived derived-state constants) height.q.i.list)
        list  t.list
      ==
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
        =.  pubkeys.m.nk  pubkeys.m.arg
        =.  shares.m.nk  shares.m.arg
        =.  init.a.k  init.a.arg
        nk
      arg
    --
  ::
  ::TODO make referentially transparent by requiring event number in the scry path
  ++  peek
    |=  arg=path
    ^-  (unit (unit *))
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
        [%transaction tid=@ ~]
      ::  scry for a tx that has been included in a validated block
      ::  TODO: fixme this is wrong, it returns a map of txs from a *block* id
      ^-  (unit (unit (z-map tx-id:t tx:t)))
      :-  ~
      %-  ~(get z-by txs.c.k)
      (from-b58:hash:t tid.pole)
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
        [%desk-hash ~]
      ^-  (unit (unit (unit @uvI)))
      ``desk-hash.a.k
    ::
        [%mining-pubkeys ~]
      ^-  (unit (unit (list [m=@ pks=(list @t)])))
      =/  locks=(list [m=@ pks=(list @t)])
        %-  ~(rep z-in pubkeys.m.k)
        |=  [=lock:t l=(list [m=@ pks=(list @t)])]
        [(to-b58:lock:t lock) l]
      ``locks
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
        [%heavy-summary ~]
      ^-  (unit (unit [(z-set lock:t) (unit page-summary:t)]))
      ?~  heaviest-block.c.k
        ``[pubkeys.m.k ~]
      =/  heaviest-block  (~(get z-by blocks.c.k) u.heaviest-block.c.k)
      :+  ~  ~
      :-  pubkeys.m.k
      ?~  heaviest-block
        ~
      `(to-page-summary:page:t (to-page:local-page:t u.heaviest-block))
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
      lp(pow ~)
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
      (height-to-proof-version:con height.candidate-block.m.k)
    =/  target  (~(got z-by targets.c.k) parent.candidate-block.m.k)
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
      ?:  (check-duplicate-block digest.pag)
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
      ?:  =(*page-number:t height.pag)
        ::  heard genesis block
        ~>  %slog.[0 leaf+"heard-block: Heard genesis block"]
        (heard-genesis-block wir now eny pag)
      ?~  heaviest-block.c.k
        =/  peer-id=(unit @)  (get-peer-id wir)
        ?~  peer-id
          ::  received block before genesis from source other than libp2p
          `k
        :_  k
        (missing-parent-effects digest.pag height.pag u.peer-id)
      ::  if we don't have parent and block claims to be heaviest
      ::  request ancestors to catch up or handle reorg
      ?.  (~(has z-by blocks.c.k) parent.pag)
        ?:  %+  compare-heaviness:page:t  pag
            (~(got z-by blocks.c.k) u.heaviest-block.c.k)
          =/  peer-id=(unit @)  (get-peer-id wir)
          ?~  peer-id
            ~|("heard-block: Unsupported wire: {<wir>}" !!)
          :_  k
          (missing-parent-effects digest.pag height.pag u.peer-id)
        ::  received block, don't have parent, isn't heaviest, ignore.
        `k
      ::  yes, we have its parent
      ::
      ::  do we already have this block?
      ?:  (check-duplicate-block digest.pag)
        :: do almost nothing (idempotency), we already have block
        :: however we *should* tell the runtime we have it
        ~>  %slog.[1 leaf+"heard-block: Duplicate block"]
        :_  k
        [%seen %block digest.pag ~]~
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
        [%track %add digest.pag peer-id.pole]~
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
        [%liar-block-id digest.pag +.check-page-without-txs]
      ::
      ?.  (check-pow pag)
        ~>  %slog.[1 leaf+"heard-block: Failed PoW check"]
        :_  k
        %+  snoc  block-effs
        [%liar-block-id digest.pag %failed-pow-check]
      ::
      ::  tell driver we have seen this block so don't send it back to the kernel again
      =.  block-effs
        [[%seen %block digest.pag `height.pag] block-effs]
      ::  stop tracking block id as soon as we verify pow
      =.  block-effs
        %+  snoc  block-effs
        ^-  effect:dk
        [%track %remove digest.pag]
      =>  .(c.k `consensus-state:dk`c.k)  ::  tmi
      =^  missing-txs=(list tx-id:t)  c.k
        (add-pending-block:con pag)
      =.  d.k  (update-highest:der height.pag)
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
         ::~&  "skipping pow hash check for {(trip (to-b58:hash:t digest.pag))}"
         %.y
       %-  check-target:mine
       :_  target.pag
       (proof-to-pow:zeke (need pow.pag))
     =/  check-pow-valid=?  (check-pow pag)
     ::
     ::  check if timestamp is in base field, this will anchor subsequent timestamp checks
     ::  since child block timestamps have to be within a certain range of the most recent
     ::  N blocks.
     =/  check-timestamp=?  (based:zeke timestamp.pag)
     =/  check-txs=?  =(tx-ids.pag *(z-set tx-id:t))
     =/  check-epoch=?  =(epoch-counter.pag *@)
     =/  check-target=?  =(target.pag genesis-target:t)
     =/  check-work=?  =(accumulated-work.pag (compute-work:page:t genesis-target:t))
     =/  check-coinbase=?  =(coinbase.pag *(z-map lock:t @))
     =/  check-height=?  =(height.pag *page-number:t)
     =/  check-btc-hash=?
       ?~  btc-hash
         ~>  %slog.[0 'check-genesis: Not checking btc hash when validating genesis block']
         %.y
       =(parent.pag (hash:btc-hash:t u.btc-hash))
     ::
     ::  check that the message matches what's in the seal
     =/  check-msg=?
       ?~  genesis-seal
         ~>  %slog.[1 'check-genesis: Genesis seal not set, cannot check genesis block']  !!
       =((hash:page-msg:t msg.pag) msg-hash.u.genesis-seal)
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
      ?~  pow.pag
        %.n
      ::
      ::  validate that powork puzzle in the proof is correct.
      ?&  (check-pow-puzzle u.pow.pag pag)
          ::
          ::  validate the powork. this is done separately since the
          ::  other checks are much cheaper.
          (verify:nv u.pow.pag ~ eny)
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
      =/  id-b58  (to-b58:hash:t id.raw)
      ~>  %slog.[0 (cat 3 'heard-tx: Raw transaction id: ' id-b58)]
      ::
      ::  check if we already have raw-tx
      ?:  (has-raw-tx:con id.raw)
        :: do almost nothing (idempotency), we already have it
        :: but do tell the runtime we've already seen it
        =/  log-message
          %^  cat  3
           'heard-tx: Transaction id already seen: '
          id-b58
        ~>  %slog.[1 log-message]
        :_  k
        [%seen %tx id.raw]~
      ::
      ::  check if the raw-tx contents are in base field
      ?.  (based:raw-tx:t raw)
        :_  k
        [(liar-effect wir %raw-tx-not-based)]~
      ::
      ::  check tx-id. this is faster than calling validate:raw-tx (which also checks the id)
      ::  so we do it first
      ?.  =((compute-id:raw-tx:t raw) id.raw)
        =/  log-message
          %^  cat  3
            'heard-tx: Invalid transaction id: '
        id-b58
        ~>  %slog.[1 log-message]
        :_  k
        [(liar-effect wir %tx-id-invalid)]~
      ::
      ::  check if raw-tx is part of a pending block
      ::
      ?:  (needed-by-block:con id.raw)
        ::  pending blocks are waiting on tx
        ?.  (validate:raw-tx:t raw)
          ::  raw-tx doesn't validate.
          ::  remove blocks containing bad tx from pending state. note that since
          ::  we already checked that the id of the transaction was valid, we
          ::  won't accidentally throw out a block that contained a valid tx-id
          ::  just because we received a tx that claimed the same id as the valid
          ::  one.
          =/  tx-pending-blocks  (~(get z-ju blocks-needed-by.c.k) id.raw)
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
      :-  ~[[%seen %tx id.raw] [%gossip %0 %heard-tx raw]]
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
      =/  digest-b58  (to-b58:hash:t digest.pag)
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
        =.  c.k  (reject-pending-block:con digest.pag)
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
            ?>  ?=(^ pow.pag)
            %+  rap  3
            :~  ' with proof version '  (rsh [3 2] (scot %ui version.u.pow.pag))
            ==
          '. Skipping pow check because check-pow-flag was disabled'
        %-  trip
        ^-  @t
        %+  rap  3
        :~  'accept-block: '
            'block '  (to-b58:hash:t digest.pag)
            ' added to validated blocks at '  (rsh [3 2] (scot %ui height.pag))
            pow-print
        ==
      ~>  %slog.[0 %leaf^print-var]
      =/  effs=(list effect:dk)
        ::  request block N+1 on each peer's heaviest chain
        :+  [%request %block %by-height +(height.pag)]
          ::  tell driver we've seen this block so don't process it again
          [%seen %block digest.pag `height.pag]
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
          ~['block_height'^n+height.pag 'heaviest_block_digest'^s+(to-b58:hash:t digest.pag)]
        :*  [%gossip %0 %heard-block pag]
            span
            effs
        ==
      ::  case (a): block validated but not new heaviest - it's on a side chain
      =?  effs  !is-new-heaviest
          :_  effs
          :+  %span  %orphaned-block
          :~  'block_id'^s+(to-b58:hash:t digest.pag)
              'block_height'^n+height.pag
              'event_type'^s+'side-chain-orphan'
          ==
      ::
      =/  is-reorg=?
        ?~  old-heavy  %.n  ::  first block after genesis, not a reorg
        &(is-new-heaviest !=(parent.pag u.old-heavy))
      ::  case (b): new heaviest block - check if it's a reorganization
      =?  effs  is-reorg
        ?~  old-heavy  effs
        ::  reorganization detected - previous heaviest block is now orphaned
        =/  orphaned-block-span=span-effect:dk
          :+  %span  %orphaned-block
          :~  'block_id'^s+(to-b58:hash:t u.old-heavy)
              'new_heaviest_block'^s+(to-b58:hash:t digest.pag)
              'new_height'^n+height.pag
              'event_type'^s+'reorg-orphan'
          ==
        =/  reorg-span=span-effect:dk
          :+  %span  %chain-reorg
          :~  'block_id'^s+(to-b58:hash:t u.old-heavy)
              'new_heaviest_height'^n+height.pag
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
          +(height:(~(got z-by blocks.c.k) u.heaviest-block.c.k))
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
            (~(got z-by targets.c.k) parent.candidate-block.m.k)
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
          (mole |.((from-b58:schnorr-pubkey:t p.command)))
        ?~  pk
          ~>  %slog.[1 'do-set-mining-key: Invalid mining pubkey, exiting']
          [[%exit 1]~ k]
        =/  =lock:t  (new:lock:t u.pk)
        =.  m.k  (set-pubkeys:min [lock]~)
        =.  m.k  (set-shares:min [lock 100]~)
        ::  ~&  >  "pubkeys.m set to {<pubkeys.m.k>}"
        ::  ~&  >  "shares.m set to {<shares.m.k>}"
        `k
      ::
      ++  do-set-mining-key-advanced
        ^-  [(list effect:dk) kernel-state:dk]
        ?>  ?=([%set-mining-key-advanced *] command)
        ?:  (gth (lent p.command) 2)
        ~>  %slog.[1 'do-set-mining-key-advanced: Coinbase split for more than two locks not yet supported, exiting']
          [[%exit 1]~ k]
        ?~  p.command
        ~>  %slog.[1 'do-set-mining-key-advanced: Empty list of locks, exiting.']
          [[%exit 1]~ k]
        ::
        =/  [keys=(list lock:t) shares=(list [lock:t @]) crash=?]
          %+  roll  `(list [@ @ (list @t)])`p.command
          |=  $:  [s=@ m=@ ks=(list @t)]
                  locks=(list lock:t)
                  shares=(list [lock:t @])
                  crash=_`?`%|
              ==
          =+  r=(mule |.((from-b58:lock:t m ks)))
          ?:  ?=(%| -.r)
            ((slog p.r) [~ ~ %&])
          [[p.r locks] [[p.r s] shares] crash]
        ?:  crash
          ~>  %slog.[1 'do-set-mining-key-advanced: Invalid public keys provided, exiting']
          [[%exit 1]~ k]
        =.  m.k  (set-pubkeys:min keys)
        =.  m.k  (set-shares:min shares)
        ::  ~&  >  "pubkeys.m set to {<pubkeys.m.k>}"
        ::  ~&  >  "shares.m set to {<shares.m.k>}"
        `k
      ::
      ++  do-enable-mining
        ^-  [(list effect:dk) kernel-state:dk]
        ?>  ?=([%enable-mining *] command)
        ?.  p.command
          ::~&  >  'generation of candidate blocks disabled'
          =.  m.k  (set-mining:min p.command)
          `k
        ?:  =(*(z-set lock:t) pubkeys.m.k)
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
          +(height:(~(got z-by blocks.c.k) u.heaviest-block.c.k))
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
        ?:  =(*(z-set lock:t) pubkeys.m.k)
          ::~&  "cannot mine without first setting pubkey with %set-mining-key"
          `k
        =/  commit=block-commitment:t
          (block-commitment:page:t candidate-block.m.k)
        =/  target  target.candidate-block.m.k
        =/  proof-version  (height-to-proof-version:con height.candidate-block.m.k)
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
      ?.  mining-pubkeys-set:min  ~
      %-  ~(rep z-in tx-ids.page)
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
