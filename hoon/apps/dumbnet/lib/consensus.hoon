/=  dk  /apps/dumbnet/lib/types
/=  sp  /common/stark/prover
/=  mine  /common/pow
/=  dumb-transact  /common/tx-engine
/=  asert  /apps/dumbnet/lib/asert
/=  *  /common/h-zoon
::
::  this library is where _every_ update to the consensus state
::  occurs, no matter how minor.
~%  %consensus  ..ut  ~
|_  [c=consensus-state:dk d=derived-state:dk =blockchain-constants:dumb-transact]
+*  t  ~(. dumb-transact blockchain-constants)
::
::  assert preconditions, provide reason for failure
++  apt
  ^-  (unit @tas)
  ?.  ~(apt h-by blocks-needed-by.c)  `%inapt-blocks-needed-by
  ?.  ~(apt h-in excluded-txs.c)  `%inapt-excluded-txs
  ?.  ~(apt h-by spent-by.c)  `%inapt-spent-by
  ?.  ~(apt h-by pending-blocks.c)  `%inapt-pending-blocks
  ?.  ~(apt h-by balance.c)  `%inapt-balance
  ?.  ~(apt h-by txs.c)  `%inapt-txs
  ::  these would take too long but a full semantic verification would include them
  ::?.  ~(apt h-by raw-txs.c)  `%inapt-raw-txs
  ::?.  ~(apt h-by blocks.c)  `%inapt-blocks
  ::?.  ~(apt h-by min-timestamps.c)  `%inapt-min-timestamps
  ::?.  ~(apt h-by epoch-start.c)  `%inapt-epoch-start
  ::?.  ~(apt h-by targets.c)  `%inapt-targets
  ?.  =(excluded-txs.c (~(int h-in excluded-txs.c) ~(key h-by raw-txs.c)))
    `%extra-excluded-txs
  ?.  =(*(h-set tx-id:t) (~(int h-in excluded-txs.c) ~(key h-by blocks-needed-by.c)))
    `%excluded-txs-arent
  ?.  =(excluded-txs.c (~(dif h-in ~(key h-by raw-txs.c)) ~(key h-by blocks-needed-by.c)))
    `%txs-fell-through-cracks
  ~
::
::  repair a bad state
++  repair
  ~/  %repair
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
    =/  rtx=(h-map tx-id:t *)  raw-txs.c
    =/  bnb=(h-map tx-id:t *)  blocks-needed-by.c
    c(excluded-txs ~(key h-by (~(dif h-by rtx) bnb)))
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
  ~/  %has-raw-tx
  |=  tid=tx-id:t
  ^-  ?
  (~(has h-by raw-txs.c) tid)
::
++  get-raw-tx
  ~/  %get-raw-tx
  |=  tid=tx-id:t
  ^-  (unit raw-tx:t)
  =/  tx  (~(get h-by raw-txs.c) tid)
  ?~  tx  ~  `raw-tx.u.tx
::
++  got-raw-tx
  ~/  %got-raw-tx
  |=  tid=tx-id:t
  ^-  raw-tx:t
  (need (get-raw-tx tid))
::
::  checkpointed digests for chain stability
::    phase-2 cutover of 014-aletheia pins both the ASERT anchor block
::    (height 65,499) and the first ASERT block (height 65,500) so any
::    competing block at either height is rejected network-wide. the
::    anchor digest is the same digest the phase-1 +find-anchor-min-ts
::    helper would have walked to; pinning it freezes the median-of-11
::    asert-anchor-min-timestamp now baked into blockchain-constants.
++  checkpointed-digests
  ^-  (z-map page-number:t hash:t)
  %-  ~(gas z-by *(z-map page-number:t hash:t))
  :~  [%65.500 (from-b58:hash:t '4dr8f3hWcQfgSMUrKRcNb1Z4nwzECbbUuqDYUp8G4WF6G5ocFXzPp2')]
      [%65.499 (from-b58:hash:t 'vYekzUpi6o95oA6qHfvcq9kVRzFMZLuUw33YxXQRqNCvBHwU7wys73')]
      [%16.128 (from-b58:hash:t 'ANjtb2YNFo3cAtLVkjkXXP2DJ2S5ZvByywpxgAa1UhxXM5f8YmiJLWX')]
      [%4.032 (from-b58:hash:t 'DhaVTgMz6CMy3ZG3vsci1z9U2Gg7WZL6y3g7bZzfJLUbus1rd8j4BQU')]
      [%2.448 (from-b58:hash:t '9EChUtcNJumW5DDYgS6UP5UHfHtD6vFH7HoSqjmTuWP2Px6JdpxaR23')]
      [%720 (from-b58:hash:t 'C4vJRnFNHCLHKHVRJGiYeoiYXS7CyTGrVk2ibEv95HQiZoxRvtr5SRQ')]
      [%144 (from-b58:hash:t '3rbqdep8HLqwwkW4YvZazVPYZpbqsFbqHCfEKGt13GVUUzA9ToDCsxT')]
      [%0 (from-b58:hash:t '7pR2bvzoMvfFcxXaHv4ERm8AgEnExcZLuEsjNgLkJziBkqBLidLg39Y')]
  ==
::
::  map a block heigh to a corresponding ZK proof version
::
::  Pre-Stage-6 behavior: this was the authoritative "what proof
::  version is legal at this height" oracle. After Stage 6, post-
::  activation heights accept either %2 (ZK) or %3 (AI); use
::  +proof-version-valid-at-height for the activation-aware
::  predicate. This arm retains the pre-activation deterministic
::  mapping and is used as the fallback for pre-activation
::  block-ids in +block-id-to-proof-version.
++  height-to-proof-version-legacy
  |=  height=page-number:t
  ^-  proof-version:sp
  ?:  (gte height proof-version-2-start)
    %2
  ?:  (gte height proof-version-1-start)
    %1
  %0
::  Alias kept temporarily for call-site compatibility while Stage 6
::  call-site updates land. Identical behavior to the legacy arm.
++  height-to-proof-version  height-to-proof-version-legacy
:: What block to start using proof version 2
++  proof-version-2-start  12.000
::  What block to start using proof version 1
++  proof-version-1-start  6.750
::
::  Stage 6 helpers: version <-> puzzle-type, per-block version lookup,
::  activation-aware predicate, same-type-ancestor walker.
::
::  +version-to-puzzle-type: maps a proof-version (the discriminator
::  shared by all proof shapes) to a puzzle-type tag.
::    %0/%1/%2 → %dumb-zkpow (ZK STARK PoW puzzle)
::    %3       → %ai-pow     (AI PoW puzzle)
::  Single source of truth for the version↔puzzle mapping. All
::  consumers MUST go through this arm.
++  version-to-puzzle-type
  |=  version=proof-version:sp
  ^-  ?(%dumb-zkpow %ai-pow)
  ?:  ?=(%3 version)  %ai-pow
  %dumb-zkpow
::
::  +pow-artifact-to-proof-version: page.pow is a generic persisted noun
::  so the AI certificate does not force hoonc to expand the recursive proof
::  mold in every page consumer. Recover the version discriminator locally.
++  pow-artifact-to-proof-version
  |=  pow=*
  ^-  proof-version:sp
  ?:  ?=([%ai-pow *] pow)
    %3
  =/  prf=proof:sp  (need ((soft proof:sp) pow))
  version.prf
::
::  +block-compute-work: a block's heaviness contribution, dispatched by
::  puzzle type. The ZK and AI puzzles use DIFFERENT work normalizers so
::  cross-puzzle fork choice is EQUAL-WEIGHT (a block at each puzzle's ASERT
::  anchor contributes identical work; see +compute-work-ai:page). This is the
::  single source of truth for a block's work — validation heaviness AND the
::  finalized block's stored accumulated-work MUST both use it. A powless block
::  defaults to the ZK normalizer (it fails the pow check regardless).
++  block-compute-work
  |=  pag=page:t
  ^-  bignum:bignum:t
  =/  target  ~(target get:page:t pag)
  ::  Cross-puzzle equal-weight AI normalization is active only post-asert
  ::  (it pairs with the per-puzzle ASERT regime). Pre-asert every block uses the
  ::  ZK normalizer — legacy behavior; AI blocks do not occur pre-asert on mainnet.
  ?.  (post-asert-activation:t ~(height get:page:t pag))
    (compute-work:page:t target)
  =/  pow-unit  ~(pow get:page:t pag)
  ?~  pow-unit  (compute-work:page:t target)
  ?:  =(%ai-pow (version-to-puzzle-type (pow-artifact-to-proof-version u.pow-unit)))
    (compute-work-ai:page:t target)
  (compute-work:page:t target)
::
::  +block-id-to-proof-version: returns the proof version of an
::  already-accepted block, given its block-id. Reads the
::  block-versions map first (post-activation blocks); falls back
::  to the deterministic height-derived map for pre-activation
::  block-ids (block-versions is post-activation-only — see
::  types.hoon consensus-state-10 doc).
++  block-id-to-proof-version
  |=  bid=block-id:t
  ^-  proof-version:sp
  =/  cached=(unit proof-version:sp)  (~(get h-by block-versions.c) bid)
  ?^  cached  u.cached
  =/  pag=local-page:t  (~(got h-by blocks.c) bid)
  (height-to-proof-version-legacy ~(height get:local-page:t pag))
::
::  +block-id-to-puzzle-type: composition. Consumers walking
::  ancestors should use this.
++  block-id-to-puzzle-type
  |=  bid=block-id:t
  ^-  ?(%dumb-zkpow %ai-pow)
  (version-to-puzzle-type (block-id-to-proof-version bid))
::
::  +proof-version-valid-at-height: Stage 6 replacement for the
::  height-equality check in +check-pow.
::    pre-activation: must equal the height-derived legacy ZK version
::    post-activation: legacy ZK version at this height OR %3 (AI)
::
::  The post-activation legacy branch lets fakenets (with their
::  low-height activation overrides) continue to use the height-
::  derived ZK version (%0/%1/%2) for ZK blocks instead of forcing
::  the latest %2. On mainnet (activation = 95000 > 12000), legacy
::  at post-activation height = %2 by definition; on fakenet
::  (activation = 2), legacy at height 2 = %0.
++  proof-version-valid-at-height
  |=  [version=proof-version:sp height=page-number:t]
  ^-  ?
  =/  legacy=proof-version:sp  (height-to-proof-version-legacy height)
  ?:  (gte height ai-pow-activation-height.blockchain-constants)
    ?|  ?=(%3 version)
        =(version legacy)
    ==
  =(version legacy)
::
::  +find-same-type-ancestor: starting at `start-bid` (a block-id in
::  blocks.c), walks parent edges and returns the FIRST block-id at
::  or below start-bid whose puzzle-type matches `target-type`.
::  Returns ~ if no such block exists within the bounded walk window
::  (cap = 2 * min-past-blocks * 2 = 44 global hops — §4.2 of the
::  Stage 6 design).
::
::  Used by S3 to route ASERT call sites: pass the candidate's
::  parent + the candidate's puzzle-type, get back the per-puzzle
::  parent-digest to feed to compute-target-*-asert.
::
::  When start-bid itself matches target-type, returns `start-bid.
::  This lets callers just say "find me the nearest %dumb-zkpow at
::  or above the heaviest" without an outer existence check.
++  find-same-type-ancestor
  |=  [start-bid=block-id:t target-type=?(%dumb-zkpow %ai-pow)]
  ^-  (unit block-id:t)
  =/  cap=@  (mul 2 (mul min-past-blocks:t 2))  ::  44
  =/  hops=@  0
  =/  cur-bid=block-id:t  start-bid
  |-
  =/  cur=local-page:t  (~(got h-by blocks.c) cur-bid)
  ?:  =(target-type (block-id-to-puzzle-type cur-bid))
    `cur-bid
  ?:  =(*page-number:t ~(height get:local-page:t cur))
    ~  ::  hit genesis without a same-type match
  ?:  (gte hops cap)
    ~  ::  exceeded the walk window
  $(cur-bid ~(parent get:local-page:t cur), hops +(hops))
::
::  +count-same-type-since-anchor: number of `target-type` blocks strictly above
::  `anchor-height` on the ancestry of `start-bid` (inclusive of start-bid). This
::  is the per-puzzle subchain distance from the ASERT anchor: a puzzle's ASERT
::  must retarget over ITS OWN blocks (each puzzle ideal 300s -> ~150s combined),
::  not over global height — otherwise both puzzles would drive the GLOBAL cadence
::  to their ideal and the chain would run at 2x the intended interval.
::
::    O(height - anchor-height) walk. Fine for the ASERT window; the fixed
::    mainnet anchor (height 95,000) makes this grow with the chain, so a
::    production deployment should cache a per-block subchain index (a local
::    derived quantity) instead. Correctness here is independent of that
::    optimization.
++  count-same-type-since-anchor
  |=  [start-bid=block-id:t target-type=?(%dumb-zkpow %ai-pow) anchor-height=@]
  ^-  @
  =/  count=@  0
  =/  cur-bid=block-id:t  start-bid
  |-
  =/  cur=local-page:t  (~(got h-by blocks.c) cur-bid)
  ?:  (lte ~(height get:local-page:t cur) anchor-height)
    count  ::  reached (or passed) the anchor level
  =?  count  =(target-type (block-id-to-puzzle-type cur-bid))  +(count)
  ?:  =(*page-number:t ~(height get:local-page:t cur))
    count  ::  genesis, no further ancestry
  $(cur-bid ~(parent get:local-page:t cur))
::
::  +set-genesis-seal: set .genesis-seal
++  set-genesis-seal
  ~/  %set-genesis-seal
  |=  [height=page-number:t msg-hash=@t]
  ^-  consensus-state:dk
  ~>  %slog.[0 'set-genesis-seal: Setting genesis seal']
  =/  seal  (new:genesis-seal:t height msg-hash)
  c(genesis-seal seal)
::
++  add-btc-data
  ~/  %add-btc-data
  |=  btc-hash=(unit btc-hash:t)
  ^-  consensus-state:dk
  ?:  =(~ btc-hash)
    ~>  %slog.[0 'add-btc-data: Not checking Bitcoin block hash for genesis block']
    c(btc-data `btc-hash)
  ~>  %slog.[0 'add-btc-data: Received Bitcoin block hash, waiting to hear Nockchain genesis block!']
  c(btc-data `btc-hash)
::
++  inputs-in-heaviest-balance
  ~/  %inputs-in-heaviest-balance
  |=  raw=raw-tx:t
  ^-  ?
  (inputs-in-balance raw get-cur-balance)
::
++  inputs-in-balance
  ~/  %inputs-in-balance
  |=  [raw=raw-tx:t balance=(h-map nname:t nnote:t)]
  ^-  ?
  ::  %.y: every input named by .raw is present in .balance
  ::  %.n: at least one input named by .raw is absent from .balance
  ::
  ::  probe the balance map directly for each of the tx's (few) inputs
  ::  (O(k log n)) rather than materializing the entire balance key-set and
  ::  diffing against it (O(n log n) plus n allocations). the input-set is
  ::  tiny; .balance is the full UTXO set, so the old key-set form did work
  ::  proportional to the whole chain's notes on every single transaction.
  %-  ~(all z-in ~(input-names get:raw-tx:t raw))
  |=  =nname:t
  (~(has h-by balance) nname)
::
++  get-cur-height
  ^-  page-number:t
  ~(height get:local-page:t (~(got h-by blocks.c) (need heaviest-block.c)))
::
++  get-cur-balance
  ^-  (h-map nname:t nnote:t)
  ?~  heaviest-block.c
    ~>  %slog.[1 'get-cur-balance: No known blocks, balance is empty']
    *(h-map nname:t nnote:t)
  =/  heaviest-page=local-page:t
    (~(got h-by blocks.c) u.heaviest-block.c)
  ?~  balance=(~(get h-by balance.c) u.heaviest-block.c)
    ?:  =(*page-number:t ~(height get:local-page:t heaviest-page))
      *(h-map nname:t nnote:t)
    ~|  'get-cur-balance: Missing balance for non-genesis heaviest block'
    !!
  u.balance
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
  ~/  %compute-target
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
  ~/  %compute-target-raw
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
::  +compute-target-zk-asert: aserti3-2d ZK puzzle target for a
::  post-zk-asert-activation block. Selects between two ZK ASERT
::  regimes based on .child-height:
::
::    [zk-asert.phase, zk-asert-post-ai.phase)   → zk-asert (150s ideal)
::    [zk-asert-post-ai.phase, +∞)               → zk-asert-post-ai (300s)
::
::  At and after ai-pow-activation-height the ZK puzzle re-anchors at
::  zk-asert-post-ai.anchor-height with the regime-2 anchor-target.
::  Each post-activation block walks back to ITS REGIME's anchor (not
::  the original zk-asert anchor) for the time-diff computation.
::
::  Caller must guarantee .child-height >= zk-asert.phase. .parent-digest
::  identifies the immediate parent — read from .min-timestamps for the
::  median-of-11.
++  compute-target-zk-asert
  |=  [child-height=@ parent-digest=block-id:t]
  ^-  bignum:bignum:t
  =/  is-post-ai-regime=?
    (gte child-height phase.zk-asert-post-ai.blockchain-constants)
  =/  params
    ?:  is-post-ai-regime
      zk-asert-post-ai.blockchain-constants
    zk-asert.blockchain-constants
  =/  parent-min-ts=@  (~(got h-by min-timestamps.c) parent-digest)
  ::  Anchor min-ts + target source priority:
  ::    1. blockchain-constants AsertParams value if non-zero
  ::       (phase-2-style hardcoded protocol constant)
  ::    2. derived-state cache (lazily populated at activation by
  ::       accept-block via populate-zk-asert-post-ai-anchor:der)
  ::    3. crash (cache must be populated post-activation)
  ::  Regime 1 (pre-AI) always uses the phase-2 hardcoded constant.
  ::  Regime 2 (post-AI) defaults to 0 placeholder + cache; can be
  ::  hardcoded later for code cleanliness.
  =/  anchor-min-ts=@
    ?.  =(0 anchor-min-timestamp.params)
      anchor-min-timestamp.params
    ?>  is-post-ai-regime
    ?~  cached-zk-asert-post-ai-anchor.d
      ~|  %zk-asert-post-ai-anchor-cache-empty
      !!
    min-ts.u.cached-zk-asert-post-ai-anchor.d
  =/  anchor-target=@
    ?.  =(0 anchor-target-atom.params)
      anchor-target-atom.params
    ?>  is-post-ai-regime
    ?~  cached-zk-asert-post-ai-anchor.d
      ~|  %zk-asert-post-ai-anchor-cache-empty
      !!
    target-atom.u.cached-zk-asert-post-ai-anchor.d
  ::  Regime 2 (post-AI) retargets over the ZK SUBCHAIN, mirroring the AI puzzle:
  ::  count only ZK blocks since the regime-2 anchor and feed that as virtual
  ::  heights (anchor index 0), so each puzzle targets its own ideal (300s) over
  ::  its own blocks -> ~150s combined. Regime 1 (pre-AI, single puzzle) keeps
  ::  global heights: there, global height == ZK-subchain length.
  =/  block-anchor-height=@  ?:(is-post-ai-regime 0 anchor-height.params)
  =/  block-height=@
    ?.  is-post-ai-regime  child-height
    +((count-same-type-since-anchor parent-digest %dumb-zkpow anchor-height.params))
  %-  chunk:bignum:t
  %-  compute-target:asert
  :*  anchor-target
      anchor-min-ts
      block-anchor-height
      parent-min-ts
      block-height
      ideal-block-time.params
      half-life.params
      max-target-atom:t
  ==
::
::  +compute-target-asert: legacy alias for +compute-target-zk-asert.
::  Existing callers haven't migrated to the per-puzzle API yet; this
::  wrapper preserves their semantics. New callers should use
::  +compute-target-zk-asert directly.
++  compute-target-asert
  ~/  %compute-target-asert
  |=  [child-height=@ parent-digest=block-id:t]
  ^-  bignum:bignum:t
  (compute-target-zk-asert child-height parent-digest)
::
::  +compute-target-ai-asert: aserti3-2d AI puzzle target for a
::  post-ai-pow-activation block. Single regime — ai-asert is the only
::  AI ASERT config. Same hardcoded-anchor pattern as compute-target-
::  zk-asert; reads its anchor params from ai-asert.blockchain-constants.
::
::  .parent-digest is the child's nearest AI ancestor (the most recent prior
::  %ai-pow block, via +find-same-type-ancestor at the call sites). Block
::  distance is measured over the AI subchain (+count-same-type-since-anchor),
::  so AI difficulty tracks AI-only cadence, not global block cadence.
++  compute-target-ai-asert
  |=  [child-height=@ parent-digest=block-id:t]
  ^-  bignum:bignum:t
  =/  params  ai-asert.blockchain-constants
  ::  Anchor sources, per-field: hardcoded constant (non-zero ⇒ in
  ::  use), else cache (if populated). Bootstrap: when EITHER
  ::  field has no source (no hardcoded AND cache empty), the AI
  ::  subchain has no usable anchor yet — degenerate to
  ::  anchor-target-atom (same value compute-target would produce
  ::  at the anchor with zero elapsed time). Keeps AI candidate
  ::  emission alive until the first AI block lands + populates
  ::  the cache.
  =/  use-hardcoded-min-ts=?  !=(0 anchor-min-timestamp.params)
  =/  use-hardcoded-target=?  !=(0 anchor-target-atom.params)
  =/  anchor-min-ts-opt=(unit @)
    ?:  use-hardcoded-min-ts  `anchor-min-timestamp.params
    ?~  cached-ai-asert-anchor.d  ~
    `min-ts.u.cached-ai-asert-anchor.d
  =/  anchor-target-opt=(unit @)
    ?:  use-hardcoded-target  `anchor-target-atom.params
    ?~  cached-ai-asert-anchor.d  ~
    `target-atom.u.cached-ai-asert-anchor.d
  ?~  anchor-min-ts-opt
    (chunk:bignum:t anchor-target-atom.params)
  ?~  anchor-target-opt
    (chunk:bignum:t anchor-target-atom.params)
  =/  parent-min-ts=@  (~(got h-by min-timestamps.c) parent-digest)
  ::  Retarget over the AI SUBCHAIN, not global height: the ASERT block distance
  ::  is the number of AI blocks between the anchor and this child (parent-digest
  ::  is the child's nearest AI ancestor, so child index = that count + 1). Feed
  ::  it to +compute-target as virtual heights (anchor index 0, child index =
  ::  count) while keeping the real timestamps — so the AI puzzle targets its own
  ::  ideal-block-time (300s) over its own blocks, giving ~150s combined with ZK.
  =/  blocks-since-anchor=@
    +((count-same-type-since-anchor parent-digest %ai-pow anchor-height.params))
  %-  chunk:bignum:t
  %-  compute-target:asert
  :*  u.anchor-target-opt
      u.anchor-min-ts-opt
      0
      parent-min-ts
      blocks-since-anchor
      ideal-block-time.params
      half-life.params
      max-target-atom:t
  ==
::
::  +build-ai-candidate: derive the AI-puzzle variant of a ZK candidate block.
::
::    Post-ai-activation the miner builds ONE candidate, targeted for the ZK
::    puzzle. The AI puzzle needs the SAME block bound to the AI ASERT target
::    instead: an AI certificate commits to the block commitment + target, so an
::    AI block must carry +compute-target-ai-asert (validation rejects any other
::    target as %page-target-invalid). This deterministically re-targets the
::    candidate — replace the target with the AI ASERT target (over the AI
::    subchain via +find-same-type-ancestor) and recompute accumulated-work with
::    the AI normalizer (+compute-work-ai, matching validation's +block-compute-
::    work). Emission (the %mine-ai effect) and +do-pow (reconstructing the block
::    from an AI solution) both call this on the same candidate + consensus
::    state, so the commitments match and the solved certificate validates.
++  build-ai-candidate
  |=  zk-cand=page:t
  ^-  page:t
  =/  candidate-height=@  ~(height get:page:t zk-cand)
  =/  parent-bid=block-id:t  ~(parent get:page:t zk-cand)
  =/  ai-parent=block-id:t
    =/  found=(unit block-id:t)  (find-same-type-ancestor parent-bid %ai-pow)
    ?~  found  parent-bid  ::  bootstrap: no AI ancestor yet -> degenerate anchor
    u.found
  =/  ai-target=bignum:bignum:t  (compute-target-ai-asert candidate-height ai-parent)
  =/  parent-work=@
    =/  par=page:t  (to-page:local-page:t (~(got h-by blocks.c) parent-bid))
    (merge:bignum:t ~(accumulated-work get:page:t par))
  =/  ai-work=bignum:bignum:t
    (chunk:bignum:t (add parent-work (merge:bignum:t (compute-work-ai:page:t ai-target))))
  =.  zk-cand
    ?^  -.zk-cand  zk-cand(target ai-target)  zk-cand(target ai-target)
  =.  zk-cand
    ?^  -.zk-cand  zk-cand(accumulated-work ai-work)  zk-cand(accumulated-work ai-work)
  =.  zk-cand
    ?^  -.zk-cand  zk-cand(digest (compute-digest:page:t zk-cand))  zk-cand(digest (compute-digest:page:t zk-cand))
  zk-cand
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
  ~/  %compute-epoch-duration
  |=  last-block=block-id:t
  ^-  @
  =/  prev-last-block=block-id:t
    (~(got h-by epoch-start.c) last-block)
  =/  epoch-start=@
    (~(got h-by min-timestamps.c) prev-last-block)
  =/  epoch-end=@
    (~(got h-by min-timestamps.c) last-block)
  ~|  "compute-epoch-duration: Time warp attack: Negative epoch duration"
  (sub epoch-end epoch-start)
::
::  +check-size: check on page size, requires all raw-tx
++  check-size
  ~/  %check-size
  |=  pag=page:t
  ^-  ?
  %+  lte
    %+  add
      (compute-size-without-txs:page:t pag)
    (txs-size-by-id:page:t pag got-raw-tx)
  max-block-size:t
::
++  accept-page
  ~/  %accept-page
  |=  [pag=page:t acc=tx-acc:t now=@da]
  ^-  consensus-state:dk
  ::  update balance
  ::
  =?  balance.c  !=(*(h-map nname:t nnote:t) balance.acc)
    ::  if balance.acc is empty, this would still add the following to balance.c,
    ::  so we do it conditionally.
    (~(put h-by balance.c) ~(digest get:page:t pag) balance.acc)
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
    (~(put h-bi bal) ~(digest get:page:t pag) ~(name get:nnote:t coinbase) coinbase)
  ::  update txs
  ::
  =.  txs.c
    %-  ~(rep h-by txs.acc)
    |=  [[=tx-id:t =tx:t] txs=_txs.c]
    (~(put h-bi txs) ~(digest get:page:t pag) tx-id tx)
  ::
  ::  update epoch map. the first block-id in an epoch maps to its parent,
  ::  and each subsequent block maps to the same block-id as the first. this is helpful
  ::  bookkeeping to avoid a length pointer chase of parent of parent of...
  ::  when reaching the end of an epoch and needing to compute its length.
  =.  epoch-start.c
    ?:  =(*page-number:t ~(height get:page:t pag))
      ::  genesis block is also considered the last block of the "0th" epoch.
      (~(put h-by epoch-start.c) ~(digest get:page:t pag) ~(digest get:page:t pag))
    ?:  =(0 ~(epoch-counter get:page:t pag))
      (~(put h-by epoch-start.c) ~(digest get:page:t pag) ~(parent get:page:t pag))
    %-  ~(put h-by epoch-start.c)
    :-  ~(digest get:page:t pag)
    (~(got h-by epoch-start.c) ~(parent get:page:t pag))
  =.  min-timestamps.c  (update-min-timestamps now pag)
  ::
  =.  targets.c
    ?:  (post-asert-activation:t ~(height get:page:t pag))
      ::  post-asert-activation: store pag's own aserti3-2d target. validation and
      ::  the miner compute ASERT fresh via +compute-target-asert rather
      ::  than reading this map, so we only populate it for debugging and to
      ::  keep the map shape consistent across the activation boundary.
      %-  ~(put h-by targets.c)
      :-  ~(digest get:page:t pag)
      (compute-target-asert ~(height get:page:t pag) ~(parent get:page:t pag))
    ?:  =(+(~(epoch-counter get:page:t pag)) blocks-per-epoch:t)
      ::  last block of an epoch means update to target
      %-  ~(put h-by targets.c)
      :-  ~(digest get:page:t pag)
      (compute-target ~(digest get:page:t pag) ~(target get:page:t pag))
    ?:  =(~(height get:page:t pag) *page-number:t)  ::  genesis block
      %-  ~(put h-by targets.c)
      [~(digest get:page:t pag) ~(target get:page:t pag)]
    ::  target remains the same throughout an epoch
    %-  ~(put h-by targets.c)
    :-  ~(digest get:page:t pag)
    (~(got h-by targets.c) ~(parent get:page:t pag))
  ::  note we do not update heaviest-block here, since that is conditional
  ::  and the effects emitted depend on whether we do it.
  ?:  (~(has h-by pending-blocks.c) ~(digest get:page:t pag))
    (accept-pending-block ~(digest get:page:t pag))
  (accept-block pag)
::
::  +validate-page-without-txs-da: helper for urbit time
++  validate-page-without-txs-da
  ~/  %validate-page-without-txs-da
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
  ~/  %validate-page-without-txs
  |=  [pag=page:t now-secs=@]
  ^-  (reason:dk ~)
  ::  Version check: pow is always verified (the no-pow testing path
  ::  was removed — see below). A powless block fails the `need`,
  ::  which is correct: every accepted block must carry a proof.
  ?.  %+  proof-version-valid-at-height
        (pow-artifact-to-proof-version (need ~(pow get:page:t pag)))
      ~(height get:page:t pag)
    ~&  [%proof-version-invalid ~(height get:page:t pag)]
    [%.n %proof-version-invalid]
  =/  par=page:t  (to-page:local-page:t (~(got h-by blocks.c) ~(parent get:page:t pag)))
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
    =/  pow  (need ~(pow get:page:t pag))
    ?:  ?=([%ai-pow *] pow)
      ::  Defer the AI-PoW work-meets-target check to +check-pow, whose
      ::  `%ai-pow` branch runs `++ai-pow-verify` (the recursive-certificate
      ::  jet), which binds the certificate to `(block-commitment .. target)`
      ::  and so subsumes this cheap target gate. Deferral is sound: this arm
      ::  (+validate-page-without-txs) doc-excludes "validating the powork",
      ::  and its sole caller +heard-block runs +check-pow immediately after,
      ::  admitting a block to consensus state ONLY once BOTH pass. Returning
      ::  %.n here would instead reject every %ai-pow block, valid included.
      %.y
    =/  prf=proof:sp  (need ((soft proof:sp) pow))
    %-  check-target:mine
    :_  ~(target get:page:t pag)
    (proof-to-pow:t prf)
  ?.  check-pow-hash
    [%.n %pow-target-check-failed]
  ::
  =/  check-timestamp=?
    ?&  %+  gte  ~(timestamp get:page:t pag)
        (~(got h-by min-timestamps.c) ~(parent get:page:t pag))
      ::
        (lte ~(timestamp get:page:t pag) (add now-secs max-future-timestamp:t))
    ==
  ?.  check-timestamp
    [%.n %page-timestamp-invalid]
  ::
  ::  check height
  ?.  =(~(height get:page:t pag) +(~(height get:page:t par)))
    [%.n %page-height-invalid]
  ::
  ::  check target — Stage 6: dispatch by puzzle-type. ZK and AI
  ::  blocks each use their own ASERT with the per-puzzle parent
  ::  found via +find-same-type-ancestor. Pre-asert-activation
  ::  falls back to the epoch-stored target (unchanged).
  =/  expected-target
    ?:  (post-asert-activation:t ~(height get:page:t pag))
      ::  powless block defaults to %dumb-zkpow (it will fail the pow
      ::  check regardless); avoids a crash on the flag-off path.
      =/  block-puzzle-type=?(%dumb-zkpow %ai-pow)
        =/  pow-unit  ~(pow get:page:t pag)
        ?~  pow-unit  %dumb-zkpow
        (version-to-puzzle-type (pow-artifact-to-proof-version u.pow-unit))
      =/  same-type-parent=block-id:t
        =/  found=(unit block-id:t)
          (find-same-type-ancestor ~(parent get:page:t pag) block-puzzle-type)
        ?~  found  ~(parent get:page:t pag)  ::  bootstrap: degenerate
        u.found
      ?:  =(%dumb-zkpow block-puzzle-type)
        (compute-target-zk-asert ~(height get:page:t pag) same-type-parent)
      (compute-target-ai-asert ~(height get:page:t pag) same-type-parent)
    (~(got h-by targets.c) ~(parent get:page:t pag))
  ?.  =(~(target get:page:t pag) expected-target)
    [%.n %page-target-invalid]
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
    (merge:bignum:t (block-compute-work pag))
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
  ~/  %validate-page-with-txs
  |=  pag=page:t
  ^-  (reason:dk tx-acc:t)
  =/  digest-b58=cord  (to-b58:hash:t ~(digest get:page:t pag))
  ?.  (check-size pag)
    ~>  %slog.[1 (cat 3 'validate-page-with-txs: Block too large: ' digest-b58)]
    [%.n %block-too-large]
  =/  tx-id-list=(list tx-id:t)
    ~(tap z-in ~(tx-ids get:page:t pag))
  =/  raw-tx-list=(list (unit raw-tx:t))
    (turn tx-id-list |=(=tx-id:t (get-raw-tx tx-id)))
  :: initialize balance transfer accumulator with parent block's balance
  =/  acc=tx-acc:t
    %+  new:tx-acc:t
      (~(get h-by balance.c) ~(parent get:page:t pag))
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
  =/  emission=coins:t
    (emission-calc:coinbase:t ~(height get:page:t pag))
  =/  emission-and-fees=coins:t  (add emission fees.u.balance-transfer)
  ?.  =(emission-and-fees total-split)
    [%.n %improper-split]
  ::
  ::  Phase-gated v1 coinbase entry count. The +based:coinbase-split:v1
  ::  parser allows up to `max-coinbase-split + 1` entries to admit the
  ::  fund slot post-asert-activation, but pre-activation v1 blocks
  ::  (v1-phase <= height < zk-asert-phase) carry no fund slot and must
  ::  continue to cap at `max-coinbase-split` entries — matching the
  ::  legacy v0 rule. Without this gate, a miner could pre-activation
  ::  emit a 3-entry v1 coinbase that this branch accepts and stricter
  ::  implementations reject (consensus split). See
  ::  docs/2026-05-01-MR2545-EMISSIONS-REVIEW.md P1 #1.
  =/  height=page-number:t  ~(height get:page:t pag)
  ?:  ?&  ?=([%1 *] cb)
          (pre-asert-activation:t height)
          (gth ~(wyt z-by +.cb) max-coinbase-split.blockchain-constants)
      ==
    [%.n %coinbase-split-pre-activation-too-many]
  ::
  ::  Post-activation (014-aletheia): coinbase must split 80/20 between
  ::  the miner and the consensus-known fund address.
  ?:  (post-asert-activation:t height)
    ?.  (check-fund-split cb emission)
      [%.n %improper-fund-split]
    ~>  %slog.[0 (cat 3 'validate-page-with-txs: Block validated: ' digest-b58)]
    [%.y u.balance-transfer]
  ~>  %slog.[0 (cat 3 'validate-page-with-txs: Block validated: ' digest-b58)]
  [%.y u.balance-transfer]
::
::  +update-heaviest: set new heaviest block if it is so
++  update-heaviest
  ~/  %update-heaviest
  |=  pag=page:t
  ^-  consensus-state:dk
  =/  digest-b58=cord  (to-b58:hash:t ~(digest get:page:t pag))
  ?:  =(~ heaviest-block.c)
    :: if we have no heaviest block, this must be genesis block.
    ~|  "update-heaviest: Received non-genesis block before genesis block"
    ?>  =(*page-number:t ~(height get:page:t pag))
    c(heaviest-block (some ~(digest get:page:t pag)))
  ::  > rather than >= since we take the first heaviest block we've heard
  ?:  %+  compare-heaviness:page:t  pag
      (~(got h-by blocks.c) (need heaviest-block.c))
    c(heaviest-block (some ~(digest get:page:t pag)))
  c
::
::  +check-fund-split: validate that a post-asert-activation coinbase pays
::  the consensus-known fund address exactly floor(emission/5) atoms.
::
::    The total-split-equals-(emission+fees) check has already passed
::    by the time this is called (see line ~515 above), and ++based on
::    the v1 coinbase-split caps total entries at max-coinbase-split+1.
::    So we only need to verify that:
::      (a) the split is v1 (post-asert-activation = post-v1-phase),
::      (b) the fund-address slot exists,
::      (c) that slot's coins equal exactly floor(emission/5).
::    The miner side is then `emission - fund-coins + fees`,
::    distributed across however many miner outputs the miner chose
::    (1 or 2; partner mode supported per 014-aletheia).
::
::    Post-cap special case (height > tail-end): when emission == 0 the
::    expected fund share is 0, but +based:coinbase-split:v1 rejects
::    zero-coin entries — so the only valid representation is fund-slot
::    *absent*, with all fees flowing to miner-side outputs. See
::    docs/2026-05-01-MR2545-EMISSIONS-REVIEW.md P1 #2.
++  check-fund-split
  ~/  %check-fund-split
  |=  [cb=coinbase-split:t emission=coins:t]
  ^-  ?
  ?.  ?=([%1 *] cb)  %.n
  =/  expected-fund-coins=coins:t  (div emission 5)
  =/  fund-coins=(unit coins:t)
    (~(get z-by +.cb) fund-address:t)
  ?:  =(0 expected-fund-coins)
    =(~ fund-coins)
  ?~  fund-coins  %.n
  =(u.fund-coins expected-fund-coins)
::
::  +get-elders: get list of ancestor block IDs up to 24 deep
::  (ordered newest->oldest)
++  get-elders
  ~/  %get-elders
  |=  [d=derived-state:dk bid=block-id:t]
  ^-  (unit [page-number:t (list block-id:t)])
  =/  block  (~(get h-by blocks.c) bid)
  ?~  block
    ~
  =/  unit-height=(unit page-number:t)
    ?~  heaviest-block.c  `0
    =/  heaviest-block  (~(get h-by blocks.c) u.heaviest-block.c)
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
::  +update-min-timestamps: sets the median-of-11 timestamp of a new
::    block, keyed by its digest. Stage 6 semantics: median of the
::    most recent min-past-blocks timestamps whose blocks are the
::    SAME puzzle-type as the new block, walked back from pag's
::    parent edge, skipping wrong-type hops. The new block's own
::    timestamp is always included as the first entry (matches
::    pre-Stage-6 convention).
::
::    Pre-activation invariant: every walk hop's puzzle-type is
::    %dumb-zkpow (height-derived legacy fallback in
::    +block-id-to-proof-version), pag-type is also %dumb-zkpow.
::    Filter is a no-op; output is bit-identical to the pre-Stage-6
::    walker. This is the compat anchor — verified end-to-end by
::    the pre-activation fakenet smoke + mainnet sync (S5).
::
::    Bounded walk: cap of 2 * min-past-blocks * 2 = 44 global hops
::    to prevent unbounded walks when one puzzle stalls for many
::    blocks of the other puzzle. On cap exit with fewer than
::    min-past-blocks collected, we take the median of what we
::    have (or fall back to pag's own timestamp if nothing else
::    matched — pag is always included).
::
++  update-min-timestamps
  ~/  %update-min-timestamps
  |=  [now=@da pag=page:t]
  ^-  (h-map block-id:t @)
  ::  Determine the new block's puzzle-type from its proof version.
  ::  A powless block (genesis, and any block where pow failed to
  ::  decode) is %dumb-zkpow by definition: genesis is pre-activation
  ::  and AI blocks cannot exist there. This guard restores the
  ::  pre-Stage-6 walker's tolerance — the old walker never read pow
  ::  at all, so it never crashed on a powless accepted block.
  =/  pag-type=?(%dumb-zkpow %ai-pow)
    =/  pow-unit  ~(pow get:page:t pag)
    ?~  pow-unit  %dumb-zkpow
    (version-to-puzzle-type (pow-artifact-to-proof-version u.pow-unit))
  =/  min-timestamp=@
    ::  collect up to N=min-past-blocks same-type timestamps,
    ::  starting with pag itself and walking parent edges.
    =|  prev-timestamps=(list @)
    ::  pag is always type-matching (it IS pag-type); seed with its
    ::  timestamp and one collected count.
    =.  prev-timestamps  [~(timestamp get:page:t pag) prev-timestamps]
    =/  collected=@  1
    =/  hops=@  0
    =/  cap=@  (mul 2 (mul min-past-blocks:t 2))  ::  44
    =/  cur-bid=block-id:t  ~(parent get:page:t pag)
    =/  cur-height=@  ~(height get:page:t pag)
    |-
    ?:  =(collected min-past-blocks:t)
      ::  collected enough; take median
      (median:t prev-timestamps)
    ?:  =(*page-number:t cur-height)
      ::  reached genesis; take median of what we have
      (median:t prev-timestamps)
    ?:  (gte hops cap)
      ::  exceeded walk window; take median of what we have
      (median:t prev-timestamps)
    =/  cur-lp=local-page:t  (~(got h-by blocks.c) cur-bid)
    =/  cur-page=page:t  (to-page:local-page:t cur-lp)
    =/  cur-type=?(%dumb-zkpow %ai-pow)
      (block-id-to-puzzle-type cur-bid)
    =?  prev-timestamps  =(cur-type pag-type)
      [~(timestamp get:page:t cur-page) prev-timestamps]
    =?  collected  =(cur-type pag-type)
      +(collected)
    %=  $
      hops        +(hops)
      cur-bid     ~(parent get:local-page:t cur-lp)
      cur-height  ~(height get:local-page:t cur-lp)
    ==
  ::
  (~(put h-by min-timestamps.c) ~(digest get:page:t pag) min-timestamp)
::
::::  pending block and tx functionality
::
::
::  Accept a block which has been fully validated and is not pending
++  accept-block
  ~/  %accept-block
  |=  pag=page:t
  ^-  consensus-state:dk
  ?<  (~(has h-by blocks.c) ~(digest get:page:t pag))
  ?<  (~(has h-by pending-blocks.c) ~(digest get:page:t pag))
  =.  blocks.c  (~(put h-by blocks.c) ~(digest get:page:t pag) (to-local-page:page:t pag))
  ::  Stage 6: populate block-versions for post-activation blocks.
  ::  Pre-activation block-ids are NOT inserted; their version is
  ::  derived deterministically from height by +block-id-to-proof-version.
  =?  block-versions.c
      (gte ~(height get:page:t pag) ai-pow-activation-height.blockchain-constants)
    %+  ~(put h-by block-versions.c)
      ~(digest get:page:t pag)
    (pow-artifact-to-proof-version (need ~(pow get:page:t pag)))
  %-  ~(rep z-in ~(tx-ids get:page:t pag))
  |=  [=tx-id:t c=_c]
  =.  blocks-needed-by.c  (~(put h-ju blocks-needed-by.c) tx-id ~(digest get:page:t pag))
  =.  excluded-txs.c  (~(del h-in excluded-txs.c) tx-id)
  c
::
::  add a block which is waiting on transactions to pending state.
::  If we have all transactions, a null set will be returned and
::  state will not be changed
++  add-pending-block
  ~/  %add-pending-block
  |=  pag=page:t
  ^-  [(list tx-id:t) consensus-state:dk]
  ?<  (~(has h-by blocks.c) ~(digest get:page:t pag))
  ?<  (~(has h-by pending-blocks.c) ~(digest get:page:t pag))
  =/  needed=(h-set tx-id:t)
    %-  ~(rep z-in ~(tx-ids get:page:t pag))
    |=  [=tx-id:t needed=(h-set tx-id:t)]
    ?.  (~(has h-by raw-txs.c) tx-id)
      (~(put h-in needed) tx-id)
    needed
  ?:  =(*(h-set tx-id:t) needed)
    [~ c] :: not missing any transactions
  =.  pending-blocks.c  (~(put h-by pending-blocks.c) ~(digest get:page:t pag) [pag get-cur-height])
  =.  c
    %-  ~(rep z-in ~(tx-ids get:page:t pag))
    |=  [=tx-id:t c=_c]
    =.  blocks-needed-by.c  (~(put h-ju blocks-needed-by.c) tx-id ~(digest get:page:t pag))
    =.  excluded-txs.c  (~(del h-in excluded-txs.c) tx-id)
    c
  [~(tap h-in needed) c]
::
::  reject a pending block
++  reject-pending-block
  ~/  %reject-pending-block
  |=  =block-id:t
  ^-  consensus-state:dk
  ::  block must be pending
  ?<  (~(has h-by blocks.c) block-id)
  =/  pag  page:(~(got h-by pending-blocks.c) block-id)
  =.  c
    %-  ~(rep z-in ~(tx-ids get:page:t pag))
    |=  [=tx-id:t c=_c]
    =.  blocks-needed-by.c  (~(del h-ju blocks-needed-by.c) tx-id ~(digest get:page:t pag))
    =?  excluded-txs.c
        ?&  ?!((~(has h-by blocks-needed-by.c) tx-id))  ::  not in blocks-needed-by
            (~(has h-by raw-txs.c) tx-id)               ::  but in raw-txs
        ==
      (~(put h-in excluded-txs.c) tx-id)
    c
  =.  pending-blocks.c  (~(del h-by pending-blocks.c) ~(digest get:page:t pag))
  c
::
::  missing transaction ids from pending blocks
++  missing-tx-ids
  ^-  (list tx-id:t)
  %~  tap  h-in
  ^-  (h-set tx-id:t)
  %-  ~(rep h-by pending-blocks.c)
  |=  [[block-id:t pag=page:t *] all=(h-set tx-id:t)]
  ^-  (h-set tx-id:t)
  %-  ~(rep z-in ~(tx-ids get:page:t pag))
  |=  [=tx-id:t all=_all]
  ?.  (~(has h-by raw-txs.c) tx-id)
    (~(put h-in all) tx-id)
  all
::
::  move a block from pending-blocks to blocks
++  accept-pending-block
  ~/  %accept-pending-block
  |=  =block-id:t
  ^-  consensus-state:dk
  ::  block must be pending
  ?<  (~(has h-by blocks.c) block-id)
  =/  pag  page:(~(got h-by pending-blocks.c) block-id)
  =.  pending-blocks.c  (~(del h-by pending-blocks.c) ~(digest get:page:t pag))
  =.  blocks.c  (~(put h-by blocks.c) block-id (to-local-page:page:t pag))
  c
::
::  list of pending blocks which are lower than the minimum retention height
++  dropable-pending-blocks
  ~/  %dropable-pending-blocks
  |=  retain=(unit @)
  ^-  (list block-id:t)
  ?~  retain
    ~
  ?~  heaviest-block.c  ~
  =/  pag=page:t  (to-page:local-page:t (~(got h-by blocks.c) u.heaviest-block.c))
  =/  height  ~(height get:page:t pag)
  ?:  (lth height u.retain)
    ~
  =/  min-height  (sub height u.retain)
  %-  ~(rep h-by pending-blocks.c)
  |=  [[=block-id:t =page:t heard-at=@] dropable=(list block-id:t)]
  ?:  (lte heard-at min-height)
    [block-id dropable]
  dropable
::
::  drop all dropable blocks
++  drop-dropable-blocks
  ~/  %drop-dropable-blocks
  |=  retain=(unit @)
  %+  roll  (dropable-pending-blocks retain)
  |=  [=block-id:t con=_c]
  =.  c  con
  (reject-pending-block block-id)
::
::  Are the inputs already spent by another transaction we know of?
++  inputs-spent
  ~/  %inputs-spent
  |=  =raw-tx:t
  ^-  ?
  =/  input-names=(h-set nname:t)
    (zh-silt ~(input-names get:raw-tx:t raw-tx))
  %-  ~(any h-in input-names)
  ~(has h-by spent-by.c)
::
::  Is the transaction needed by a block?
++  needed-by-block
  ~/  %needed-by-block
  |=  =tx-id:t
  ^-  ?
  (~(has h-by blocks-needed-by.c) tx-id)
::
::  add an already-validated raw transaction, producing a list of blocks ready to validate
++  add-raw-tx
  ~/  %add-raw-tx
  |=  =raw-tx:t
  ^-  [(list block-id:t) consensus-state:dk]
  =/  =tx-id:t  ~(id get:raw-tx:t raw-tx)
  ?<  (~(has h-by raw-txs.c) tx-id)
  =.  raw-txs.c  (~(put h-by raw-txs.c) tx-id [raw-tx get-cur-height])
  =/  input-names=(z-set nname:t)  ~(input-names get:raw-tx:t raw-tx)
  =.  spent-by.c
    %-  ~(rep z-in input-names)
    |=  [=nname:t sb=_spent-by.c]
    (~(put h-ju sb) nname tx-id)
  =/  bnb  (~(get h-ju blocks-needed-by.c) tx-id)
  ?:  =(*(h-set block-id:t) bnb)
    =.  excluded-txs.c  (~(put h-in excluded-txs.c) tx-id)
    [~ c]
  =/  ready-blocks=(list block-id:t)
    %-  ~(rep h-in bnb)
    |=  [=block-id:t ready=(list block-id:t)]
    =/  pending  (~(get h-by pending-blocks.c) block-id)
    ?~  pending  ready
    =/  pag  page.u.pending
    =/  needed
      %-  ~(rep z-in ~(tx-ids get:page:t pag))
      |=  [=tx-id:t needed=(h-set tx-id:t)]
      ^-  (h-set tx-id:t)
      ?.  (~(has h-by raw-txs.c) tx-id)
        (~(put h-in needed) tx-id)
      needed
    ::  if the block is ready, add it to the ready list
    ?:  =(*(h-set tx-id:t) needed)
      [block-id ready]
    ready
  [ready-blocks c]
::
::  drop a transaction. This will crash if any block needs the transaction
++  drop-tx
  ~/  %drop-tx
  |=  =tx-id:t
  ^-  consensus-state:dk
  ?<  (~(has h-by blocks-needed-by.c) tx-id)
  ?>  (~(has h-in excluded-txs.c) tx-id)
  =/  raw-tx  raw-tx:(~(got h-by raw-txs.c) tx-id)
  =.  raw-txs.c  (~(del h-by raw-txs.c) tx-id)
  =.  excluded-txs.c  (~(del h-in excluded-txs.c) tx-id)
  =.  spent-by.c
    %-  ~(rep z-in ~(input-names get:raw-tx:t raw-tx))
    |=  [=nname:t sb=_spent-by.c]
    (~(del h-ju sb) nname ~(id get:raw-tx:t raw-tx))
  c
::
::  transactions which may be dropped (excluded and lower than minimum retention height)
++  dropable-txs
  ~/  %dropable-txs
  |=  retain=(unit @)
  ^-  (h-set tx-id:t)
  ?~  heaviest-block.c  *(h-set tx-id:t)
  ?:  =(*(h-set tx-id:t) excluded-txs.c)
    *(h-set tx-id:t)
  =/  height  ~(height get:local-page:t (~(got h-by blocks.c) u.heaviest-block.c))
  ::  Hoist the heaviest balance map out of the per-tx fold: it is
  ::  loop-invariant (depends only on balance.c / heaviest-block.c, neither
  ::  mutated here). Each tx then probes the map directly for its handful of
  ::  inputs, so the spent-fold is O(|excluded-txs| * k * log |balance|) with
  ::  k tiny -- and, crucially, we never materialize the full balance key-set
  ::  (no O(|balance|) per-GC allocation). |excluded-txs| is also typically
  ::  tiny. Same predicate as the per-tx (inputs-in-heaviest-balance raw).
  =/  cur-balance=(h-map nname:t nnote:t)  get-cur-balance
  =/  spent=(h-set tx-id:t)
    %-  ~(rep h-in excluded-txs.c)
    |=  [=tx-id:t spent=(h-set tx-id:t)]
    ^-  (h-set tx-id:t)
    =/  raw-tx  raw-tx:(~(got h-by raw-txs.c) tx-id)
    ?.  (inputs-in-balance raw-tx cur-balance)
      (~(put h-in spent) tx-id)
    spent
  ?~  retain  spent
  ?:  (lth height u.retain)  spent
  =/  min-height  (sub height u.retain)
  %-  ~(rep h-in excluded-txs.c)
  |=  [=tx-id:t dropable=_spent]
  =/  [=raw-tx:t heard-at=@]  (~(got h-by raw-txs.c) tx-id)
  ?:  (lte heard-at min-height)
    (~(put h-in dropable) tx-id)
  dropable
::
::  drop all dropable transactions
++  drop-dropable-txs
  ~/  %drop-dropable-txs
  |=  retain=(unit @)
  ^-  consensus-state:dk
  %-  ~(rep h-in (dropable-txs retain))
  |=  [=tx-id:t con=_c]
  =.  c  con
  (drop-tx tx-id)
::
::  garbage-collect state
++  garbage-collect
  ~/  %garbage-collect
  |=  retain=(unit @)
  ^-  consensus-state:dk
  ::  Excluded txs are GC'd on a much shorter window than pending blocks
  ::  (decoupled): keep at most min(retain, 4) blocks of excluded-tx
  ::  history -- 4 blocks if admin configured never-drop (~) -- so
  ::  |excluded-txs| stays bounded and the dropable-txs spent-fold does
  ::  not blow up. Pending blocks keep the full `retain`.
  =/  tx-retain=(unit @)
    ?~  retain  `4
    `(min u.retain 4)
  ~>  %slog.[0 (cat 3 'garbage-collect: excluded-txs count ' (rsh [3 2] (scot %ui ~(wyt h-in excluded-txs.c))))]
  =.  c  (drop-dropable-blocks retain)
  (drop-dropable-txs tx-retain)
--
