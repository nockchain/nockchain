/=  dk  /apps/dumbnet/lib/types
/=  sp  /common/stark/prover
/=  mine  /common/pow
/=  dumb-transact  /common/tx-engine
/=  *  /common/zoon
::
::  this library is where _every_ update to the consensus state
::  occurs, no matter how minor.
|_  [c=consensus-state:dk =blockchain-constants:dumb-transact]
+*  t  ~(. dumb-transact blockchain-constants)
::
::  assert preconditions, provide reason for failure
++  apt
  ^-  (unit @tas)
  ?.  ~(apt z-by blocks-needed-by.c)  `%inapt-blocks-needed-by
  ?.  ~(apt z-in excluded-txs.c)  `%inapt-excluded-txs
  ?.  ~(apt z-by spent-by.c)  `%inapt-spent-by
  ?.  ~(apt z-by pending-blocks.c)  `%inapt-pending-blocks
  ?.  ~(apt z-by balance.c)  `%inapt-balance
  ?.  ~(apt z-by txs.c)  `%inapt-txs
  ::  these would take too long but a full semantic verification would include them
  ::?.  ~(apt z-by raw-txs.c)  `%inapt-raw-txs
  ::?.  ~(apt z-by blocks.c)  `%inapt-blocks
  ::?.  ~(apt z-by min-timestamps.c)  `%inapt-min-timestamps
  ::?.  ~(apt z-by epoch-start.c)  `%inapt-epoch-start
  ::?.  ~(apt z-by targets.c)  `%inapt-targets
  ?.  =(excluded-txs.c (~(int z-in excluded-txs.c) ~(key z-by raw-txs.c)))
    `%extra-excluded-txs
  ?.  =(*(z-set tx-id:t) (~(int z-in excluded-txs.c) ~(key z-by blocks-needed-by.c)))
    `%excluded-txs-arent
  ?.  =(excluded-txs.c (~(dif z-in ~(key z-by raw-txs.c)) ~(key z-by blocks-needed-by.c)))
    `%txs-fell-through-cracks
  ~
::
::  repair a bad state
++  repair
  |=  reason=@tas
  ~&  [%repair reason]
  |-  ^-  consensus-state:dk
  ?+  reason  ~|  [%cannot-repair reason]  !!
      %extra-included-txs
    $(reason %txs-fell-through-cracks)
  ::
      %excluded-txs-arent
    $(reason %txs-fell-through-cracks)
  ::
      %txs-fell-through-cracks
    =/  rtx=(z-map tx-id:t *)  raw-txs.c
    =/  bnb=(z-map tx-id:t *)  blocks-needed-by.c
    c(excluded-txs ~(key z-by (~(dif z-by rtx) bnb)))
  ==
::
::  check for bad state, repair if necessary
++  check-and-repair
  |-  ^-  consensus-state:dk
  =/  reason  apt
  ?~  reason  c
  $(c (repair u.reason))
::
++  has-raw-tx
  |=  tid=tx-id:t
  ^-  ?
  (~(has z-by raw-txs.c) tid)
::
++  get-raw-tx
  |=  tid=tx-id:t
  ^-  (unit raw-tx:t)
  =/  tx  (~(get z-by raw-txs.c) tid)
  ?~  tx  ~  `raw-tx.u.tx
::
++  got-raw-tx
  |=  tid=tx-id:t
  ^-  raw-tx:t
  (need (get-raw-tx tid))
::
::  checkpointed digests for chain stability
++  checkpointed-digests
  ^-  (z-map page-number:t hash:t)
  %-  ~(gas z-by *(z-map page-number:t hash:t))
  :~  [%16.128 (from-b58:hash:t 'ANjtb2YNFo3cAtLVkjkXXP2DJ2S5ZvByywpxgAa1UhxXM5f8YmiJLWX')]
      [%4.032 (from-b58:hash:t 'DhaVTgMz6CMy3ZG3vsci1z9U2Gg7WZL6y3g7bZzfJLUbus1rd8j4BQU')]
      [%2.448 (from-b58:hash:t '9EChUtcNJumW5DDYgS6UP5UHfHtD6vFH7HoSqjmTuWP2Px6JdpxaR23')]
      [%720 (from-b58:hash:t 'C4vJRnFNHCLHKHVRJGiYeoiYXS7CyTGrVk2ibEv95HQiZoxRvtr5SRQ')]
      [%144 (from-b58:hash:t '3rbqdep8HLqwwkW4YvZazVPYZpbqsFbqHCfEKGt13GVUUzA9ToDCsxT')]
      [%0 (from-b58:hash:t '7pR2bvzoMvfFcxXaHv4ERm8AgEnExcZLuEsjNgLkJziBkqBLidLg39Y')]
  ==
::
::  map a block heigh to a corresponding proof version
++  height-to-proof-version
  |=  height=page-number:t
  ^-  proof-version:sp
  ?:  (gte height proof-version-2-start)
    %2
  ?:  (gte height proof-version-1-start)
    %1
  %0
:: What block to start using proof version 2
++  proof-version-2-start  12.000
::  What block to start using proof version 1
++  proof-version-1-start  6.750
::
::  +set-genesis-seal: set .genesis-seal
++  set-genesis-seal
  |=  [height=page-number:t msg-hash=@t]
  ^-  consensus-state:dk
  ~>  %slog.[0 'set-genesis-seal: Setting genesis seal']
  =/  seal  (new:genesis-seal:t height msg-hash)
  c(genesis-seal seal)
::
++  add-btc-data
  |=  btc-hash=(unit btc-hash:t)
  ^-  consensus-state:dk
  ?:  =(~ btc-hash)
    ~>  %slog.[0 'add-btc-data: Not checking Bitcoin block hash for genesis block']
    c(btc-data `btc-hash)
  ~>  %slog.[0 'add-btc-data: Received Bitcoin block hash, waiting to hear Nockchain genesis block!']
  c(btc-data `btc-hash)
::
++  inputs-in-heaviest-balance
  |=  raw=raw-tx:t
  ^-  ?
  (inputs-in-balance raw get-cur-balance-names)
::
++  inputs-in-balance
  |=  [raw=raw-tx:t balance=(z-set nname:t)]
  ^-  ?
  ::  set of inputs required by tx that are not in balance
  =/  in-balance=(z-set nname:t)
    (~(dif z-in ~(input-names get:raw-tx:t raw)) balance)
  ::  %.y: all inputs in .raw are in balance
  ::  %.n: input(s) in .raw not in balance
  =(*(z-set nname:t) in-balance)
::
++  get-cur-height
  ^-  page-number:t
  ~(height get:local-page:t (~(got z-by blocks.c) (need heaviest-block.c)))
::
++  get-cur-balance
  ^-  (z-map nname:t nnote:t)
  ?~  heaviest-block.c
    ~>  %slog.[1 'get-cur-balance: No known blocks, balance is empty']
    *(z-map nname:t nnote:t)
  (~(got z-by balance.c) u.heaviest-block.c)
::
++  get-cur-balance-names
  ^-  (z-set nname:t)
  ~(key z-by get-cur-balance)
::
::
::  +compute-target: find the new target
::
::    this is supposed to be mathematically identical to
::    https://github.com/bitcoin/bitcoin/blob/master/src/pow.cpp
::
::    note that this works differently from what you might expect.
::    we/bitcoin compute "target" where the larger the number is,
::    the easier the block is to find. difficulty is just a human
::    friendly form to read target in. that's why this appears
::    backwards, where e.g. an epoch that takes 2x as long as the
::    desired duration results in doubling the target.
++  compute-target
  |=  [bid=block-id:t prev-target=bignum:bignum:t]
  ^-  bignum:bignum:t
  (compute-target-raw (compute-epoch-duration bid) prev-target)
::
::  +compute-target-raw: helper for +compute-target
::
::    makes it easier for unit tests. we currently do not use
::    bignum arithmetic due to lack of testing and it not yet
::    being necessary. once consensus logic starts being run
::    in the zkvm, we will need to change to bignum arithmetic.
++  compute-target-raw
  |=  [epoch-dur=@ prev-target-bn=bignum:bignum:t]
  ^-  bignum:bignum:t
  =/  prev-target-atom=@  (merge:bignum:t prev-target-bn)
  =/  capped-epoch-dur=@
    ?:  (lth epoch-dur quarter-ted:t)
      quarter-ted:t
    ?:  (gth epoch-dur quadruple-ted:t)
      quadruple-ted:t
    epoch-dur
  =/  next-target-atom=@
    %+  div
      (mul prev-target-atom capped-epoch-dur)
    target-epoch-duration:t
  =/  next-target-bn=bignum:bignum:t
    ?:  (gth next-target-atom max-target-atom:t)
      max-target:t
    (chunk:bignum:t next-target-atom)
  ?:  =(prev-target-atom next-target-atom)
    next-target-bn
  ~>  %slog.[0 (cat 3 'compute-target: Previous target: ' (rsh [3 2] (scot %ui prev-target-atom)))]
  ~>  %slog.[0 (cat 3 'compute-target: New target: ' (rsh [3 2] (scot %ui next-target-atom)))]
  next-target-bn
::
::  +compute-epoch-duration: computes the duration of an epoch in seconds
::
::    to mitigate certain types of "time warp" attacks, the timestamp we mark
::    as the end of an epoch is the median time of the last 11 blocks in the
::    epoch. this also happens to be the min timestamp for the first block
::    in the following epoch, which is already kept track of in
::    .min-timestamps, where the value at a given block-id is the min
::    timestamp of block that has that block-id as its parent. thus
::    the duration of a given epoch is the difference between the minimum timestamp
::    of the first block of the next epoch and the first block of the current
::    epoch.
++  compute-epoch-duration
  |=  last-block=block-id:t
  ^-  @
  =/  prev-last-block=block-id:t
    (~(got z-by epoch-start.c) last-block)
  =/  epoch-start=@
    (~(got z-by min-timestamps.c) prev-last-block)
  =/  epoch-end=@
    (~(got z-by min-timestamps.c) last-block)
  ~|  "compute-epoch-duration: Time warp attack: Negative epoch duration"
  (sub epoch-end epoch-start)
::
::  +check-size: check on page size, requires all raw-tx
++  check-size
  |=  pag=page:t
  ^-  ?
  %+  lte
    %+  add
      (compute-size-without-txs:page:t pag)
    (txs-size-by-id:page:t pag got-raw-tx)
  max-block-size:t
::
++  accept-page
  |=  [pag=page:t acc=tx-acc:t now=@da]
  ^-  consensus-state:dk
  ::  update balance
  ::
  =?  balance.c  !=(~ balance.acc)
    ::  if balance.acc is empty, this would still add the following to balance.c,
    ::  so we do it conditionally.
    (~(put z-by balance.c) ~(digest get:page:t pag) balance.acc)
  =/  cb=coinbase-split:t  ~(coinbase get:page:t pag)
  =/  height=page-number:t  ~(height get:page:t pag)
  =/  coinbases=(list coinbase:t)
    ?-  -.cb
      %0
        ::  v0 coinbase only allowed before v1-phase
        ?:  (gte height v1-phase.blockchain-constants)
          ~|  %v0-coinbase-after-cutoff  !!
        %+  turn  ~(tap z-in ~(key z-by +.cb))
        |=  =sig:t
        (new:v0:coinbase:t pag sig)
      %1
        ::  v1 coinbase only allowed at or after v1-phase
        ?:  (lth height v1-phase.blockchain-constants)
          ~|  %v1-coinbase-before-activation  !!
        %+  turn  ~(tap z-in ~(key z-by +.cb))
        |=  h=hash:t
        (new:coinbase:t pag (~(put z-in *(z-set hash:t)) h))
    ==
  =.  balance.c
    %+  roll  coinbases
    |=  [=coinbase:t bal=_balance.c]
    (~(put z-bi bal) ~(digest get:page:t pag) ~(name get:nnote:t coinbase) coinbase)
  ::  update txs
  ::
  =.  txs.c
    %-  ~(rep z-by txs.acc)
    |=  [[=tx-id:t =tx:t] txs=_txs.c]
    (~(put z-bi txs) ~(digest get:page:t pag) tx-id tx)
  ::
  ::  update epoch map. the first block-id in an epoch maps to its parent,
  ::  and each subsequent block maps to the same block-id as the first. this is helpful
  ::  bookkeeping to avoid a length pointer chase of parent of parent of...
  ::  when reaching the end of an epoch and needing to compute its length.
  =.  epoch-start.c
    ?:  =(*page-number:t ~(height get:page:t pag))
      ::  genesis block is also considered the last block of the "0th" epoch.
      (~(put z-by epoch-start.c) ~(digest get:page:t pag) ~(digest get:page:t pag))
    ?:  =(0 ~(epoch-counter get:page:t pag))
      (~(put z-by epoch-start.c) ~(digest get:page:t pag) ~(parent get:page:t pag))
    %-  ~(put z-by epoch-start.c)
    :-  ~(digest get:page:t pag)
    (~(got z-by epoch-start.c) ~(parent get:page:t pag))
  =.  min-timestamps.c  (update-min-timestamps now pag)
  ::
  =.  targets.c
    ?:  =(+(~(epoch-counter get:page:t pag)) blocks-per-epoch:t)
      ::  last block of an epoch means update to target
      %-  ~(put z-by targets.c)
      :-  ~(digest get:page:t pag)
      (compute-target ~(digest get:page:t pag) ~(target get:page:t pag))
    ?:  =(~(height get:page:t pag) *page-number:t)  ::  genesis block
      %-  ~(put z-by targets.c)
      [~(digest get:page:t pag) ~(target get:page:t pag)]
    ::  target remains the same throughout an epoch
    %-  ~(put z-by targets.c)
    :-  ~(digest get:page:t pag)
    (~(got z-by targets.c) ~(parent get:page:t pag))
  ::  note we do not update heaviest-block here, since that is conditional
  ::  and the effects emitted depend on whether we do it.
  ?:  (~(has z-by pending-blocks.c) ~(digest get:page:t pag))
    (accept-pending-block ~(digest get:page:t pag))
  (accept-block pag)
::
::  +validate-page-without-txs-da: helper for urbit time
++  validate-page-without-txs-da
  |=  [pag=page:t now=@da]
  (validate-page-without-txs pag (time-in-secs:page:t now))
::
::  +validate-page-without-txs: with parent, without raw-txs
::
::    performs every check that can be done on a page when you
::    know its parent, except for validating the powork or digest,
::    but don't have all of the raw-txs. not to be performed on
::    genesis block, which has its own check. this check should
::    be performed before adding a block to pending state.
++  validate-page-without-txs
  |=  [pag=page:t now-secs=@]
  ^-  (reason:dk ~)
  =/  version  (height-to-proof-version ~(height get:page:t pag))
  =/  version-check=?
    ?.  check-pow-flag:t
      %.y
    =(version version:(need ~(pow get:page:t pag)))
  ?.  version-check
    ~&  [%expected-vs-actual version version:(need ~(pow get:page:t pag))]
    [%.n %proof-version-invalid]
  =/  par=page:t  (to-page:local-page:t (~(got z-by blocks.c) ~(parent get:page:t pag)))
  ::  this is already checked in +heard-block but is done here again
  ::  to avoid a footgun
  ?.  (check-digest:page:t pag)
    [%.n %page-digest-invalid-2]
  ::
  =/  check-epoch-counter=?
    ?&  (lth ~(epoch-counter get:page:t pag) blocks-per-epoch:t)
      ?|  ?&  =(0 ~(epoch-counter get:page:t pag))
              ::  epoch-counter is zero-indexed so we decrement
              =(~(epoch-counter get:page:t par) (dec blocks-per-epoch:t))
          ==  :: start of an epoch
          ::  counter is one greater than its parent's counter.
          =(~(epoch-counter get:page:t pag) +(~(epoch-counter get:page:t par)))
      ==
    ==
  ?.  check-epoch-counter
    [%.n %page-epoch-invalid]
  ::
  =/  check-pow-hash=?
    ?.  check-pow-flag:t
      ::  this case only happens during testing
      ::~&  "skipping pow hash check for {(trip (to-b58:hash:t ~(digest get:page:t pag)))}"
      %.y
    %-  check-target:mine
    :_  ~(target get:page:t pag)
    (proof-to-pow:t (need ~(pow get:page:t pag)))
  ?.  check-pow-hash
    [%.n %pow-target-check-failed]
  ::
  =/  check-timestamp=?
    ?&  %+  gte  ~(timestamp get:page:t pag)
        (~(got z-by min-timestamps.c) ~(parent get:page:t pag))
      ::
        (lte ~(timestamp get:page:t pag) (add now-secs max-future-timestamp:t))
    ==
  ?.  check-timestamp
    [%.n %page-timestamp-invalid]
  ::
  ::  check target
  ?.  =(~(target get:page:t pag) (~(got z-by targets.c) ~(parent get:page:t pag)))
    [%.n %page-target-invalid]
  ::
  ::  check height
  ?.  =(~(height get:page:t pag) +(~(height get:page:t par)))
    [%.n %page-height-invalid]
  ::
  ::  check if digest matches checkpointed history, skip check if fakenet
  ?~  genesis-seal.c
    ~>  %slog.[1 'validate-page-without-txs: Fatal error: Genesis seal not set!']
    [%.n %genesis-seal-not-set]
  ?.  ?|  !=(realnet-genesis-msg:dk msg-hash.u.genesis-seal.c)
          ?!((~(has z-by checkpointed-digests) ~(height get:page:t pag)))
          =(~(digest get:page:t pag) (~(got z-by checkpointed-digests) ~(height get:page:t pag)))
      ==
    ~>  %slog.[1 'validate-page-without-txs: Checkpoint match failed']
    [%.n %checkpoint-match-failed]
  ::
  =/  check-heaviness=?
    .=  ~(accumulated-work get:page:t pag)
    %-  chunk:bignum:t
    %+  add
      (merge:bignum:t ~(accumulated-work get:page:t par))
    (merge:bignum:t (compute-work:page:t ~(target get:page:t pag)))
  ?.  check-heaviness
    [%.n %page-heaviness-invalid]
  ::
  =/  check-based-coinbase-split=?
    (based:coinbase-split:t ~(coinbase get:page:t pag))
  ?.  check-based-coinbase-split
    [%.n %coinbase-split-not-based]
  =/  check-msg-length=?
    (lth (lent ~(msg get:page:t pag)) 20)
  ?.  check-msg-length
    [%.n %msg-too-large]
  =/  check-msg-valid=?
    (validate:page-msg:t ~(msg get:page:t pag))
  ?.  check-msg-valid
    [%.n %msg-not-valid]
  ::
  [%.y ~]
::
::  +validate-page-with-txs: to be run after all txs gathered
::
::    note that this does _not_ repeat earlier validation steps,
::    namely that done by +validate-page-withouts-txs and checking
::    the powork. it returns ~ if any of the checks fail, and
::    a $tx-acc otherwise, which is the datum needed to add the
::    page to consensus state.
++  validate-page-with-txs
  |=  pag=page:t
  ^-  (reason:dk tx-acc:t)
  =/  digest-b58=cord  (to-b58:hash:t ~(digest get:page:t pag))
  ?.  (check-size pag)
    ~>  %slog.[1 (cat 3 'validate-page-with-txs: Block too large: ' digest-b58)]
    [%.n %block-too-large]
  =/  raw-tx-set=(z-set (unit raw-tx:t))
    (~(run z-in ~(tx-ids get:page:t pag)) |=(=tx-id:t (get-raw-tx tx-id)))
  =/  raw-tx-list=(list (unit raw-tx:t))  ~(tap z-in raw-tx-set)
  :: initialize balance transfer accumulator with parent block's balance
  =/  acc=tx-acc:t
    %+  new:tx-acc:t
      (~(get z-by balance.c) ~(parent get:page:t pag))
    ~(height get:page:t pag)
  ::
  ::  test to see that the input notes for all transactions
  ::  exist in the parent block's balance, that they are not
  ::  over- or underspent, and that the resulting
  ::  output notes are valid as well. a lot is going
  ::  on here - this is a load-bearing chunk of code in the
  ::  transaction engine.
  ::
  =/  balance-transfer=(unit tx-acc:t)
    |-
    ?~  raw-tx-list
      (some acc)
    ?~  i.raw-tx-list
      $(raw-tx-list t.raw-tx-list)
    =/  new-acc=(reason:dk tx-acc:t)
      (process:tx-acc:t acc u.i.raw-tx-list)
    ?.  ?=(%.y -.new-acc)
      =/  tx-id-b58=cord  (to-b58:hash:t (compute-id:raw-tx:t u.i.raw-tx-list))
      ~>  %slog.[1 (cat 3 'validate-page-with-txs: tx failed: ' tx-id-b58)]
      ~>  %slog.[1 (cat 3 'reason: ' +.new-acc)]
      ~  :: tx failed to process
    $(acc +.new-acc, raw-tx-list t.raw-tx-list)
  ::
  ?~  balance-transfer
    ::  balance transfer failed
    ~>  %slog.[1 (cat 3 'validate-page-with-txs: Block invalid: ' digest-b58)]
    [%.n %balance-transfer-failed]
  ::
  ::  check that the coinbase split adds up to emission+fees
  =/  cb=coinbase-split:t  ~(coinbase get:page:t pag)
  =/  total-split=coins:t
    ?-  -.cb
      %0  %+  roll  ~(val z-by +.cb)
          |=([c=coins:t s=coins:t] (add c s))
      %1  %+  roll  ~(val z-by +.cb)
          |=([c=coins:t s=coins:t] (add c s))
    ==
  =/  emission-and-fees=coins:t
    (add (emission-calc:coinbase:t ~(height get:page:t pag)) fees.u.balance-transfer)
  ?.  =(emission-and-fees total-split)
    [%.n %improper-split]
  ~>  %slog.[0 (cat 3 'validate-page-with-txs: Block validated: ' digest-b58)]
  [%.y u.balance-transfer]
::
::  +update-heaviest: set new heaviest block if it is so
++  update-heaviest
  |=  pag=page:t
  ^-  consensus-state:dk
  =/  digest-b58=cord  (to-b58:hash:t ~(digest get:page:t pag))
  =/  log-message
    %+  rap  3
    :~  'update-heaviest: '
        'Checking if block '
        digest-b58
        ' is heaviest'
    ==
  ~>  %slog.[0 log-message]
  ?:  =(~ heaviest-block.c)
    :: if we have no heaviest block, this must be genesis block.
    ~|  "update-heaviest: Received non-genesis block before genesis block"
    ?>  =(*page-number:t ~(height get:page:t pag))
    c(heaviest-block (some ~(digest get:page:t pag)))
  ::  > rather than >= since we take the first heaviest block we've heard
  ?:  %+  compare-heaviness:page:t  pag
      (~(got z-by blocks.c) (need heaviest-block.c))
    =/  log-message
      %+  rap  3
      :~  'update-heaviest: '
          'Block '
          digest-b58
          ' is new heaviest block'
      ==
    ~>  %slog.[0 log-message]
    c(heaviest-block (some ~(digest get:page:t pag)))
  =/  log-message
    %+  rap  3
    :~  'update-heaviest: '
        'Block '
        digest-b58
        ' is NOT new heaviest block'
    ==
  ~>  %slog.[0 log-message]
  c
::
::  +get-elders: get list of ancestor block IDs up to 24 deep
::  (ordered newest->oldest)
++  get-elders
  |=  [d=derived-state:dk bid=block-id:t]
  ^-  (unit [page-number:t (list block-id:t)])
  =/  block  (~(get z-by blocks.c) bid)
  ?~  block
    ~
  =/  unit-height=(unit page-number:t)
    ?~  heaviest-block.c  `0
    =/  heaviest-block  (~(get z-by blocks.c) u.heaviest-block.c)
    ?~  heaviest-block  ~
    `(min ~(height get:local-page:t u.heaviest-block) ~(height get:local-page:t u.block))
  ?~  unit-height  ~
  =/  height  u.unit-height
  =/  bid-at-height=(unit block-id:t)  (~(get z-by heaviest-chain.d) height)
  ?~  bid-at-height  ~
  =/  ids=(list block-id:t)  [u.bid-at-height ~]
  =/  count  1
  |-
  ?:  =(height *page-number:t)  `[height (flop ids)] :: genesis block
  ?:  =(24 count)  `[height (flop ids)] :: 24 blocks
  =/  prev-height=page-number:t  (dec height)
  =/  prev-id=(unit block-id:t)  (~(get z-by heaviest-chain.d) prev-height)
  ?~  prev-id
    ::  if prev-id is null, something is wrong
    ~
  $(height prev-height, ids [u.prev-id ids], count +(count))
::
::  +update-min-timestamps: sets min timestamp of children of .id
::
++  update-min-timestamps
  |=  [now=@da pag=page:t]
  ^-  (z-map block-id:t @)
  =/  min-timestamp=@
    ::  get timestamps of up to N=min-past-blocks prior blocks.
    =|  prev-timestamps=(list @)
    =/  b=@  (dec min-past-blocks:t)  :: iteration counter
    =/  cur-block=page:t  pag
    |-
    =.  prev-timestamps  [~(timestamp get:page:t cur-block) prev-timestamps]
    ?:  ?|  =(0 b)  :: we've looked back +min-past-blocks blocks
            ::
            :: we've reached genesis block
            =(*page-number:t ~(height get:page:t cur-block))
        ==
      ::  return median of timestamps
      (median:t prev-timestamps)
    %=  $
      b          (dec b)
      cur-block  (to-page:local-page:t (~(got z-by blocks.c) ~(parent get:page:t cur-block)))
    ==
  ::
  (~(put z-by min-timestamps.c) ~(digest get:page:t pag) min-timestamp)
::
::::  pending block and tx functionality
::
::
::  Accept a block which has been fully validated and is not pending
++  accept-block
  |=  pag=page:t
  ^-  consensus-state:dk
  ?<  (~(has z-by blocks.c) ~(digest get:page:t pag))
  ?<  (~(has z-by pending-blocks.c) ~(digest get:page:t pag))
  =.  blocks.c  (~(put z-by blocks.c) ~(digest get:page:t pag) (to-local-page:page:t pag))
  %-  ~(rep z-in ~(tx-ids get:page:t pag))
  |=  [=tx-id:t c=_c]
  =.  blocks-needed-by.c  (~(put z-ju blocks-needed-by.c) tx-id ~(digest get:page:t pag))
  =.  excluded-txs.c  (~(del z-in excluded-txs.c) tx-id)
  c
::
::  add a block which is waiting on transactions to pending state.
::  If we have all transactions, a null set will be returned and
::  state will not be changed
++  add-pending-block
  |=  pag=page:t
  ^-  [(list tx-id:t) consensus-state:dk]
  ?<  (~(has z-by blocks.c) ~(digest get:page:t pag))
  ?<  (~(has z-by pending-blocks.c) ~(digest get:page:t pag))
  =/  needed=(z-set tx-id:t)
    %-  ~(rep z-in ~(tx-ids get:page:t pag))
    |=  [=tx-id:t needed=(z-set tx-id:t)]
    ?.  (~(has z-by raw-txs.c) tx-id)
      (~(put z-in needed) tx-id)
    needed
  ?:  =(*(z-set tx-id:t) needed)
    [~ c] :: not missing any transactions
  =.  pending-blocks.c  (~(put z-by pending-blocks.c) ~(digest get:page:t pag) [pag get-cur-height])
  =.  c
    %-  ~(rep z-in ~(tx-ids get:page:t pag))
    |=  [=tx-id:t c=_c]
    =.  blocks-needed-by.c  (~(put z-ju blocks-needed-by.c) tx-id ~(digest get:page:t pag))
    =.  excluded-txs.c  (~(del z-in excluded-txs.c) tx-id)
    c
  [~(tap z-in needed) c]
::
::  reject a pending block
++  reject-pending-block
  |=  =block-id:t
  ^-  consensus-state:dk
  ::  block must be pending
  ?<  (~(has z-by blocks.c) block-id)
  =/  pag  page:(~(got z-by pending-blocks.c) block-id)
  =.  c
    %-  ~(rep z-by ~(tx-ids get:page:t pag))
    |=  [=tx-id:t c=_c]
    =.  blocks-needed-by.c  (~(del z-ju blocks-needed-by.c) tx-id ~(digest get:page:t pag))
    =?  excluded-txs.c
        ?&  ?!((~(has z-by blocks-needed-by.c) tx-id))  ::  not in blocks-needed-by
            (~(has z-by raw-txs.c) tx-id)               ::  but in raw-txs
        ==
      (~(put z-in excluded-txs.c) tx-id)
    c
  =.  pending-blocks.c  (~(del z-by pending-blocks.c) ~(digest get:page:t pag))
  c
::
::  missing transaction ids from pending blocks
++  missing-tx-ids
  ^-  (list tx-id:t)
  %~  tap  z-in
  ^-  (z-set tx-id:t)
  %-  ~(rep z-by pending-blocks.c)
  |=  [[block-id:t pag=page:t *] all=(z-set tx-id:t)]
  ^-  (z-set tx-id:t)
  %-  ~(rep z-in ~(tx-ids get:page:t pag))
  |=  [=tx-id:t all=_all]
  ?.  (~(has z-by raw-txs.c) tx-id)
    (~(put z-in all) tx-id)
  all
::
::  move a block from pending-blocks to blocks
++  accept-pending-block
  |=  =block-id:t
  ^-  consensus-state:dk
  ::  block must be pending
  ?<  (~(has z-by blocks.c) block-id)
  =/  pag  page:(~(got z-by pending-blocks.c) block-id)
  =.  pending-blocks.c  (~(del z-by pending-blocks.c) ~(digest get:page:t pag))
  =.  blocks.c  (~(put z-by blocks.c) block-id (to-local-page:page:t pag))
  c
::
::  list of pending blocks which are lower than the minimum retention height
++  dropable-pending-blocks
  |=  retain=(unit @)
  ^-  (list block-id:t)
  ?~  retain
    ~
  ?~  heaviest-block.c  ~
  =/  pag=page:t  (to-page:local-page:t (~(got z-by blocks.c) u.heaviest-block.c))
  =/  height  ~(height get:page:t pag)
  ?:  (lth height u.retain)
    ~
  =/  min-height  (sub height u.retain)
  %-  ~(rep z-by pending-blocks.c)
  |=  [[=block-id:t =page:t heard-at=@] dropable=(list block-id:t)]
  ?:  (lte heard-at min-height)
    [block-id dropable]
  dropable
::
::  drop all dropable blocks
++  drop-dropable-blocks
  |=  retain=(unit @)
  %+  roll  (dropable-pending-blocks retain)
  |=  [=block-id:t con=_c]
  =.  c  con
  (reject-pending-block block-id)
::
::  Are the inputs already spent by another transaction we know of?
++  inputs-spent
  |=  =raw-tx:t
  ^-  ?
  =/  input-names=(z-set nname:t)
    ~(input-names get:raw-tx:t raw-tx)
  %-  ~(any z-in input-names)
  ~(has z-by spent-by.c)
::
::  Is the transaction needed by a block?
++  needed-by-block
  |=  =tx-id:t
  ^-  ?
  (~(has z-by blocks-needed-by.c) tx-id)
::
::  add an already-validated raw transaction, producing a list of blocks ready to validate
++  add-raw-tx
  |=  =raw-tx:t
  ^-  [(list block-id:t) consensus-state:dk]
  =/  =tx-id:t  ~(id get:raw-tx:t raw-tx)
  ?<  (~(has z-by raw-txs.c) tx-id)
  =.  raw-txs.c  (~(put z-by raw-txs.c) tx-id [raw-tx get-cur-height])
  =/  input-names=(z-set nname:t)  ~(input-names get:raw-tx:t raw-tx)
  =.  spent-by.c
    %-  ~(rep z-in input-names)
    |=  [=nname:t sb=_spent-by.c]
    (~(put z-ju sb) nname tx-id)
  =/  bnb  (~(get z-ju blocks-needed-by.c) tx-id)
  ?:  =(*(z-set block-id:t) bnb)
    =.  excluded-txs.c  (~(put z-in excluded-txs.c) tx-id)
    [~ c]
  =/  ready-blocks=(list block-id:t)
    %-  ~(rep z-in bnb)
    |=  [=block-id:t ready=(list block-id:t)]
    =/  pending  (~(get z-by pending-blocks.c) block-id)
    ?~  pending  ready
    =/  pag  page.u.pending
    =/  needed
      %-  ~(rep z-in ~(tx-ids get:page:t pag))
      |=  [=tx-id:t needed=(z-set tx-id:t)]
      ^-  (z-set tx-id:t)
      ?.  (~(has z-by raw-txs.c) tx-id)
        (~(put z-in needed) tx-id)
      needed
    ::  if the block is ready, add it to the ready list
    ?:  =(*(z-set tx-id:t) needed)
      [block-id ready]
    ready
  [ready-blocks c]
::
::  drop a transaction. This will crash if any block needs the transaction
++  drop-tx
  |=  =tx-id:t
  ^-  consensus-state:dk
  ?<  (~(has z-by blocks-needed-by.c) tx-id)
  ?>  (~(has z-in excluded-txs.c) tx-id)
  =/  raw-tx  raw-tx:(~(got z-by raw-txs.c) tx-id)
  =.  raw-txs.c  (~(del z-by raw-txs.c) tx-id)
  =.  excluded-txs.c  (~(del z-in excluded-txs.c) tx-id)
  =.  spent-by.c
    %-  ~(rep z-in ~(input-names get:raw-tx:t raw-tx))
    |=  [=nname:t sb=_spent-by.c]
    (~(del z-ju sb) nname ~(id get:raw-tx:t raw-tx))
  c
::
::  transactions which may be dropped (excluded and lower than minimum retention height)
++  dropable-txs
  |=  retain=(unit @)
  ^-  (z-set tx-id:t)
  ?~  heaviest-block.c  ~
  =/  height  ~(height get:local-page:t (~(got z-by blocks.c) u.heaviest-block.c))
  =/  spent=(z-set tx-id:t)
    %-  ~(rep z-in excluded-txs.c)
    |=  [=tx-id:t spent=(z-set tx-id:t)]
    ^-  (z-set tx-id:t)
    =/  raw-tx  raw-tx:(~(got z-by raw-txs.c) tx-id)
    ?.  (inputs-in-heaviest-balance raw-tx)
      (~(put z-in spent) tx-id)
    spent
  ?~  retain  spent
  ?:  (lth height u.retain)  spent
  =/  min-height  (sub height u.retain)
  %-  ~(rep z-in excluded-txs.c)
  |=  [=tx-id:t dropable=_spent]
  =/  [=raw-tx:t heard-at=@]  (~(got z-by raw-txs.c) tx-id)
  ?:  (lte heard-at min-height)
    (~(put z-in dropable) tx-id)
  dropable
::
::  drop all dropable transactions
++  drop-dropable-txs
  |=  retain=(unit @)
  ^-  consensus-state:dk
  %-  ~(rep z-in (dropable-txs retain))
  |=  [=tx-id:t con=_c]
  =.  c  con
  (drop-tx tx-id)
::
::  garbage-collect state
++  garbage-collect
  |=  retain=(unit @)
  ^-  consensus-state:dk
  =.  c  (drop-dropable-blocks retain)
  (drop-dropable-txs retain)
--
