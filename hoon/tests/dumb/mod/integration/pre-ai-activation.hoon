::  tests/dumb/mod/integration/pre-ai-activation.hoon
::
::    Consensus-safety tests for the window BELOW ai-pow-activation-height
::    (016-logos). The invariant: a node running the dual-puzzle code at any
::    height h < ai-pow-activation-height must make byte-identical consensus
::    decisions to a pre-Logos (014-aletheia) node. Any divergence at an
::    activation-gated code path forks upgraded nodes from un-upgraded ones
::    before AI-PoW ever goes live. Each arm pins one gate's pre-activation
::    branch:
::      - proof-version:        %ai-pow (v4) blocks are REJECTED below activation
::                              (+proof-version-valid-at-height / validation),
::      - ZK difficulty regime: the ZK ASERT re-anchor switches EXACTLY at
::                              activation, so pre-activation targets use the
::                              unchanged Aletheia (regime-1) computation,
:::      - version recording:    pre-activation block versions are height-derived,
:::                              never stored in block-versions,
:::      - full chain:           a chain built entirely below activation accepts.
::    bc-pre-ai activates AI at height 8 with the ZK re-anchor phase at 8 and
::    anchor at 7, so heights 5..7 are post-asert (fund slots + ASERT targets)
::    yet strictly pre-AI-activation — the exact window a real mainnet node
::    occupies before block 114,300.
/=  helpers  /tests/dumb/helpers
/=  dcon  /apps/dumbnet/lib/consensus
/=  txe   /common/tx-engine
/=  *  /apps/dumbnet/lib/types
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
--
::
|%
::
::  ---- (a) version gate: %ai-pow (v4) rejected below activation ----
:::
::  +test-pre-ai-proof-version-rejects-v4: the +proof-version-valid-at-height
::    truth table across the activation boundary (8). A v4 (%ai-pow) artifact
::    is valid only at and after activation; the height-selected ZK version is
::    valid at every height. This keeps pre-activation consensus unchanged.
++  test-pre-ai-proof-version-rejects-v4
  =/  con  (initial-consensus-state-custom:h bc-pre-ai)
  =/  legacy-6
    (~(height-to-proof-version-legacy dcon con der bc-pre-ai) 6)
  =/  legacy-8
    (~(height-to-proof-version-legacy dcon con der bc-pre-ai) 8)
  ;:  weld
    ::  %4 rejected strictly below activation (heights 6, 7)
    (expect-eq !>(%.n) !>((~(proof-version-valid-at-height dcon con der bc-pre-ai) %4 6)))
    (expect-eq !>(%.n) !>((~(proof-version-valid-at-height dcon con der bc-pre-ai) %4 7)))
    ::  %4 accepted at and after activation (heights 8, 9)
    (expect-eq !>(%.y) !>((~(proof-version-valid-at-height dcon con der bc-pre-ai) %4 8)))
    (expect-eq !>(%.y) !>((~(proof-version-valid-at-height dcon con der bc-pre-ai) %4 9)))
    ::  the legacy ZK version stays valid on BOTH sides of the boundary
    (expect-eq !>(%.y) !>((~(proof-version-valid-at-height dcon con der bc-pre-ai) legacy-6 6)))
    (expect-eq !>(%.y) !>((~(proof-version-valid-at-height dcon con der bc-pre-ai) legacy-8 8)))
  ==
::
::  +test-pre-ai-v4-block-rejected-by-validation: a structurally well-formed
::    %ai-pow block below activation is rejected before certificate verification.
++  test-pre-ai-v4-block-rejected-by-validation
  =/  con  (initial-consensus-state-custom:h bc-pre-ai)
  =^  tip=page:t  con  (add-n-pages:h 6 con default-retain:h)
  =/  v4-page=page:t  (make-ai-pow-garbage-page:h tip)
  ::  the garbage page sits at height 7 (tip 6 + 1), strictly below activation 8
  =/  height-ok=?  =(7 ~(height get:page:t v4-page))
  =/  r  (~(validate-page-without-txs dcon con der bc-pre-ai) v4-page *@)
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
::  ---- (d) heaviness: pre-activation work is the unchanged ZK formula ----
::
::  +test-pre-ai-heaviness-uses-zk-normalizer: +block-compute-work for a
::    pre-activation block equals +compute-work of its target, NOT the shared
::    per-block work the dual-puzzle rule gives every block from activation on.
::    Pre-activation heaviness is therefore the same sum an un-upgraded node
::    computes, so no block already on the chain changes weight.
++  test-pre-ai-heaviness-uses-zk-normalizer
  =/  con  (initial-consensus-state-custom:h bc-pre-ai)
  =^  tip=page:t  con  (add-n-pages:h 6 con default-retain:h)
  =/  target  ~(target get:page:t tip)
  =/  actual=@   (merge:bignum:t (~(block-compute-work dcon con der bc-pre-ai) tip))
  =/  zk-work=@  (merge:bignum:t (compute-work:page:t target))
  =/  post-work=@  (merge:bignum:t (block-work-at:page:t 1.000 %dumb-zkpow target))
  ;:  weld
    (expect-eq !>(zk-work) !>(actual))
    ::  the ZK formula is unchanged across the boundary: same target, same work
    (expect-eq !>(zk-work) !>(post-work))
  ==
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
