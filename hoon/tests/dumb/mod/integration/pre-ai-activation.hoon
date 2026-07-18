::  tests/dumb/mod/integration/pre-ai-activation.hoon
::
::    Consensus-safety tests for the window BELOW ai-pow-activation-height
::    (015-logos). The invariant: a node running the dual-puzzle code at any
::    height h < ai-pow-activation-height must make byte-identical consensus
::    decisions to a pre-Logos (014-aletheia) node. Any divergence at an
::    activation-gated code path forks upgraded nodes from un-upgraded ones
::    before AI-PoW ever goes live. Each arm pins one gate's pre-activation
::    branch:
::      - proof-version:        %ai-pow (v3) blocks are REJECTED below activation
::                              (+proof-version-valid-at-height / validation),
::      - ZK difficulty regime: the ZK ASERT re-anchor switches EXACTLY at
::                              activation, so pre-activation targets use the
::                              unchanged Aletheia (regime-1) computation,
::      - heaviness:            pre-activation work uses the ZK normalizer,
::      - coinbase:             pre-activation blocks pay the protocol fund, and
::                              an AI-fund coinbase is rejected by height,
::      - version recording:    pre-activation block versions are height-derived,
::                              never stored in block-versions,
::      - full chain:           a chain built entirely below activation accepts.
::    bc-pre-ai activates AI at height 8 with the ZK re-anchor phase at 8 and
::    anchor at 7, so heights 5..7 are post-asert (fund slots + ASERT targets)
::    yet strictly pre-AI-activation — the exact window a real mainnet node
::    occupies before block 114,300.
/=  helpers  /tests/dumb/helpers
/=  dcon  /apps/dumbnet/lib/consensus
/=  txe   /common/tx-engine
/=  *  /apps/dumbnet/lib/types
/=  *  /common/zoon
/=  *  /common/zeke
/=  *  /common/test
::
=>
|%
::  bc-pre-ai: AI-PoW activates at height 8; the ZK puzzle re-anchors at that
::    same height (phase.zk-asert-post-ai = 8, anchor = 7). v1/zk-asert activate
::    at height 5, so heights 5..7 are post-asert but pre-AI-activation.
++  bc-pre-ai
  %*  .  default-bc:helpers
    blocks-per-epoch               1.000.000
    v1-phase                       5
    phase.zk-asert                    5
    anchor-height.zk-asert            4
    anchor-target-atom.zk-asert       ^~((div max-tip5-atom:tip5 (bex 14)))
    ideal-block-time.zk-asert         150
    half-life.zk-asert                43.200
    anchor-min-timestamp.zk-asert     (add (time-in-secs:page:txe *@da) 1.200)
    ai-pow-activation-height          8
    phase.zk-asert-post-ai            8
    anchor-height.zk-asert-post-ai    7
  ==
--
::
|%
++  h  ~(. helpers bc-pre-ai)
++  t  ~(. txe bc-pre-ai)
::  +der: read-only extra arg for the consensus door (unused pre-activation —
::    regime-1 ASERT uses the hardcoded anchor, not the derived-state cache).
++  der  ^-  derived-state  *derived-state
::
::  +first-miner-pkh: the single miner pubkey hash the helpers seed blocks with.
++  first-miner-pkh
  ^-  hash:t
  (hash:schnorr-pubkey:t default-a-pt-1:helpers)
::
::  +with-coinbase-and-rehash: replace a v1 page's coinbase and recompute its
::    digest (mirrors the fund-split integration helper).
++  with-coinbase-and-rehash
  |=  [pag=page:t cb=(z-map hash:t coins:t)]
  ^-  page:t
  =/  with-cb=page:t
    ?^  -.pag  pag
    pag(coinbase cb)
  =/  d  (compute-digest:page:t with-cb)
  ?^  -.with-cb  with-cb(digest d)
  with-cb(digest d)
--
::
|%
::
::  ---- (a) version gate: %ai-pow (v3) rejected below activation ----
::
::  +test-pre-ai-proof-version-rejects-v3: the +proof-version-valid-at-height
::    truth table across the activation boundary (8). A v3 (%ai-pow) proof is
::    valid ONLY at/after activation; the legacy ZK version is valid at every
::    height. This is the exact gate an un-upgraded node also enforces (it has
::    no v3 concept), so upgraded and un-upgraded nodes agree on every
::    pre-activation block — no fork.
++  test-pre-ai-proof-version-rejects-v3
  =/  con  (initial-consensus-state-custom:h bc-pre-ai)
  =/  legacy-6
    (~(height-to-proof-version-legacy dcon con der bc-pre-ai) 6)
  =/  legacy-8
    (~(height-to-proof-version-legacy dcon con der bc-pre-ai) 8)
  ;:  weld
    ::  %3 rejected strictly below activation (heights 6, 7)
    (expect-eq !>(%.n) !>((~(proof-version-valid-at-height dcon con der bc-pre-ai) %3 6)))
    (expect-eq !>(%.n) !>((~(proof-version-valid-at-height dcon con der bc-pre-ai) %3 7)))
    ::  %3 accepted at and after activation (heights 8, 9)
    (expect-eq !>(%.y) !>((~(proof-version-valid-at-height dcon con der bc-pre-ai) %3 8)))
    (expect-eq !>(%.y) !>((~(proof-version-valid-at-height dcon con der bc-pre-ai) %3 9)))
    ::  the legacy ZK version stays valid on BOTH sides of the boundary
    (expect-eq !>(%.y) !>((~(proof-version-valid-at-height dcon con der bc-pre-ai) legacy-6 6)))
    (expect-eq !>(%.y) !>((~(proof-version-valid-at-height dcon con der bc-pre-ai) legacy-8 8)))
  ==
::
::  +test-pre-ai-v3-block-rejected-by-validation: end-to-end — a structurally
::    well-formed %ai-pow block at a pre-activation height (7 < 8) is rejected by
::    +validate-page-without-txs with %proof-version-invalid, BEFORE any
::    certificate check. This is the reason an un-upgraded node also produces
::    (unknown version), so the block is orphaned identically on both.
++  test-pre-ai-v3-block-rejected-by-validation
  =/  con  (initial-consensus-state-custom:h bc-pre-ai)
  =^  tip=page:t  con  (add-n-pages:h 6 con default-retain:h)
  =/  v3-page=page:t  (make-ai-pow-garbage-page:h tip)
  ::  the garbage page sits at height 7 (tip 6 + 1), strictly below activation 8
  =/  height-ok=?  =(7 ~(height get:page:t v3-page))
  =/  r  (~(validate-page-without-txs dcon con der bc-pre-ai) v3-page *@)
  =/  reject-term=term  ?:(?=(%.n -.r) p.r %accepted)
  ;:  weld
    (expect-eq !>(%.y) !>(height-ok))
    (expect-eq !>(%.n) !>(-.r))
    (expect-eq !>(%proof-version-invalid) !>(reject-term))
  ==
::
::  ---- (b)/(f) ZK difficulty regime: re-anchor exactly at activation ----
::
::  +test-pre-ai-zk-regime-switches-at-activation: the anti-split invariant for
::    difficulty on the REAL mainnet constants — the ZK ASERT re-anchor phase
::    equals ai-pow-activation-height and its anchor is activation-1. Because
::    +compute-target-zk-asert selects regime 1 (the unchanged Aletheia
::    zk-asert) for every child-height < phase.zk-asert-post-ai, this pin
::    guarantees pre-activation ZK targets are computed exactly as a pre-Logos
::    node computes them. If the phase drifted off activation, pre-activation
::    targets would diverge and upgraded/un-upgraded nodes would fork.
++  test-pre-ai-zk-regime-switches-at-activation
  =/  mainnet  *blockchain-constants:txe
  %+  weld
    (expect-eq !>(ai-pow-activation-height.mainnet) !>(phase.zk-asert-post-ai.mainnet))
  (expect-eq !>((dec ai-pow-activation-height.mainnet)) !>(anchor-height.zk-asert-post-ai.mainnet))
::
::  +test-pre-ai-target-uses-regime-1: a pre-activation child target is computed
::    under regime 1 — the unchanged Aletheia zk-asert, whose anchor is a
::    hardcoded protocol constant. Regime 2 (post-AI) instead reads the
::    lazily-populated zk-asert-post-ai anchor from the derived-state cache, which
::    is EMPTY here. +compute-target-zk-asert at a pre-activation height (7 < 8)
::    must therefore return a well-formed nonzero target WITHOUT touching that
::    cache; if the regime boundary were wrong it would instead crash on the empty
::    regime-2 cache. This pins that heights below activation take the regime-1
::    (pre-Logos) path.
++  test-pre-ai-target-uses-regime-1
  =/  con  (initial-consensus-state-custom:h bc-pre-ai)
  =^  tip=page:t  con  (add-n-pages:h 7 con default-retain:h)
  =/  parent-bid=block-id:t  ~(parent get:page:t tip)
  =/  target=@
    (merge:bignum:t (~(compute-target-zk-asert dcon con der bc-pre-ai) 7 parent-bid))
  (expect-eq !>(%.n) !>(=(0 target)))
::
::  ---- (d) heaviness: pre-activation work uses the ZK normalizer ----
::
::  +test-pre-ai-heaviness-uses-zk-normalizer: +block-compute-work for a
::    pre-activation block equals the ZK normalizer +compute-work of its target,
::    NOT the AI normalizer +compute-work-ai (which would scale the target by
::    2^64 and mis-weight fork choice). Pre-activation heaviness is therefore the
::    same sum an un-upgraded node computes.
++  test-pre-ai-heaviness-uses-zk-normalizer
  =/  con  (initial-consensus-state-custom:h bc-pre-ai)
  =^  tip=page:t  con  (add-n-pages:h 6 con default-retain:h)
  =/  target  ~(target get:page:t tip)
  =/  actual=@   (merge:bignum:t (~(block-compute-work dcon con der bc-pre-ai) tip))
  =/  zk-work=@  (merge:bignum:t (compute-work:page:t target))
  =/  ai-work=@  (merge:bignum:t (compute-work-ai:page:t target))
  ;:  weld
    (expect-eq !>(zk-work) !>(actual))
    ::  the two normalizers genuinely differ, so the equality above is meaningful
    (expect-eq !>(%.n) !>(=(zk-work ai-work)))
  ==
::
::  ---- (c) coinbase: pre-activation blocks pay the protocol fund ----
::
::  +test-pre-ai-coinbase-accepts-protocol-fund: a post-asert pre-activation ZK
::    block whose 20% slot pays +protocol-fund-address validates. The is-ai
::    dispatch in +check-fund-split is derived from the block's ZK proof version
::    (=> %.n), so the protocol fund is required by HEIGHT, with no AI fund in the
::    pre-activation world.
++  test-pre-ai-coinbase-accepts-protocol-fund
  =/  con  (initial-consensus-state-custom:h bc-pre-ai)
  =^  par=page:t  con  (add-n-pages:h 5 con default-retain:h)
  =/  base    (make-empty-page:h par)
  =/  emi     (emission-calc:coinbase:t 6)
  =/  fund    (div emi 5)
  =/  ok-cb=(z-map hash:t coins:t)
    %-  ~(gas z-by *(z-map hash:t coins:t))
    :~  [first-miner-pkh (sub emi fund)]
        [protocol-fund-address:t fund]
    ==
  =/  pag  (with-coinbase-and-rehash base ok-cb)
  =/  r=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der bc-pre-ai) pag)
  (expect-eq !>(%.y) !>(?=(%.y -.r)))
::
::  +test-pre-ai-coinbase-rejects-ai-fund: the same pre-activation block paying
::    the AI-Compute-Network fund (+ai-fund-address) instead of the protocol fund
::    is REJECTED (%improper-fund-split). Before activation no block may route its
::    fund share to the AI fund — an upgraded node rejects it exactly as an
::    un-upgraded node (which knows only the protocol fund) does.
++  test-pre-ai-coinbase-rejects-ai-fund
  =/  con  (initial-consensus-state-custom:h bc-pre-ai)
  =^  par=page:t  con  (add-n-pages:h 5 con default-retain:h)
  =/  base    (make-empty-page:h par)
  =/  emi     (emission-calc:coinbase:t 6)
  =/  fund    (div emi 5)
  =/  bad-cb=(z-map hash:t coins:t)
    %-  ~(gas z-by *(z-map hash:t coins:t))
    :~  [first-miner-pkh (sub emi fund)]
        [ai-fund-address:t fund]
    ==
  =/  pag  (with-coinbase-and-rehash base bad-cb)
  =/  r=(reason tx-acc:t)  (~(validate-page-with-txs dcon con der bc-pre-ai) pag)
  (expect-eq !>(%improper-fund-split) !>(?:(?=(%.y -.r) %accepted p.r)))
::
::  ---- (G6) version recording: pre-activation versions are height-derived ----
::
::  +test-pre-ai-accept-does-not-record-version: +accept-block records an
::    explicit proof version only for blocks at/after activation. A pre-activation
::    block's version is derived deterministically from its height, so upgraded
::    nodes never disagree with un-upgraded nodes on a pre-activation block's
::    version (there is no stored state to diverge on).
++  test-pre-ai-accept-does-not-record-version
  =/  con  (initial-consensus-state-custom:h bc-pre-ai)
  =^  tip=page:t  con  (add-n-pages:h 6 con default-retain:h)
  =/  tip-bid=block-id:t  ~(digest get:page:t tip)
  ::  no explicit version entry for the pre-activation block...
  =/  no-stored=?  =(~ (~(get h-by block-versions.con) tip-bid))
  ::  ...and its version resolves via the height-derived legacy map.
  =/  derived
    (~(block-id-to-proof-version dcon con der bc-pre-ai) tip-bid)
  =/  legacy
    (~(height-to-proof-version-legacy dcon con der bc-pre-ai) 6)
  %+  weld
    (expect-eq !>(%.y) !>(no-stored))
  (expect-eq !>(legacy) !>(derived))
::
::  ---- (G8) full pre-activation chain accepts and is heaviest ----
::
::  +test-pre-ai-full-chain-accepts: a chain built entirely below activation
::    (heights 1..7) accepts every block through +validate-page-with-txs and
::    settles on the height-7 tip as heaviest — the whole pre-activation flow is
::    Aletheia-equivalent, exercising regime-1 targets, the ZK normalizer, the
::    protocol-fund coinbase, and height-derived versions together.
++  test-pre-ai-full-chain-accepts
  =/  con  (initial-consensus-state-custom:h bc-pre-ai)
  =^  tip=page:t  con  (add-n-pages:h 7 con default-retain:h)
  =/  heaviest-bid=block-id:t  (need heaviest-block.con)
  %+  weld
    (expect-eq !>(7) !>(~(height get:page:t tip)))
  (expect-eq !>(heaviest-bid) !>(~(digest get:page:t tip)))
--
