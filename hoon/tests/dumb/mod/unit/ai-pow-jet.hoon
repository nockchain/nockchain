::  tests/dumb/mod/unit/ai-pow-jet.hoon
::
::    Validates the AI-PoW consensus verify jet (`~/ %ai-pow-verify` in
::    /common/pow, implemented by crate `ai-pow-jets`, Branch b: Hoon body is a
::    fail-safe `!!`, the Rust jet is the real impl).
::
/=  helpers  /tests/dumb/helpers
/=  txe  /common/tx-engine
/=  mine  /common/pow
/=  *  /common/zeke
/=  *  /common/test
|%
++  bc-ai-pow-v1-provable
  %*  .  bc-ai-pow-provable:helpers
    v1-phase  1
  ==
++  h  ~(. helpers bc-ai-pow-provable:helpers)
++  h-v1  ~(. helpers bc-ai-pow-v1-provable)
++  t  ~(. txe bc-ai-pow-provable:helpers)
::
::  A hostile %ai-pow artifact must be rejected CLEANLY -- a liar-block-id, never
::  an admission or a crash. Which of the two rejections fires depends on the
::  fixture's target: a target outside the minable domain is caught by the
::  consensus target-domain gate before the certificate is ever verified, and an
::  artifact carrying a minable target reaches the jet. Both are correct, and
::  neither is what these tests are about. The jet path itself is proven by
::  +test-ai-pow-verify-jet-fires (a direct call) and by the ai_pow_accept_e2e
::  integration test, which grinds and admits a real block.
++  cleanly-rejected
  |=  effs=(list effect:h)
  ^-  ?
  ?|  (has-liar-cause %failed-pow-check effs)
      (has-liar-cause %ai-pow-target-outside-minable-domain effs)
      (has-liar-cause %pow-target-check-failed effs)
  ==
::
++  has-liar-cause
  |=  [expected=term effs=(list effect:h)]
  ^-  ?
  %+  lien  effs
  |=  e=effect:h
  ?&  ?=([%liar-block-id *] e)
      =(expected cause.e)
  ==
::
::  Invalid wire data has no trustworthy block ID. Peer-origin malformed
::  artifacts must therefore identify the sender rather than the claimed digest.
++  has-liar-peer-cause
  |=  [expected=term effs=(list effect:h)]
  ^-  ?
  %+  lien  effs
  |=  e=effect:h
  ?&  ?=([%liar-peer *] e)
      =(expected cause.e)
  ==
::
++  sample-ai-pow-cert
  |=  cert-len=@ud
  ^-  ai-pow-certificate:t
  =/  cert-data=@
    ?:  =(0 cert-len)  0
    (bex (dec (mul 8 cert-len)))
  =/  cert=ai-pow-certificate:t  *ai-pow-certificate:t
  cert(version 1, certificate [%bytes cert-len cert-data])
::
  ++  make-malformed-non-ai-page
  |=  parent=page:t
  ^-  page:t
  =/  pag=page:t  (make-empty-page:h parent)
  =.  pag
    ?^  -.pag
      pag
    pag(pow `[%not-a-proof 1])
  ?^  -.pag
    pag(digest (compute-digest:page:t pag))
  pag(digest (compute-digest:page:t pag))
::  A legacy proof mold admits %3 even though that tag is reserved for AI. The
::  retained digest is deliberately stale: hashing this representation crashes.
++  make-legacy-v3-proof-page
  |=  parent=page:t
  ^-  page:t
  =/  pag=page:t  (prove-page:h-v1 (make-empty-page:h-v1 parent))
  =/  prf=proof
    (need ((soft proof) (need ~(pow get:page:t pag))))
  =.  pag
    ?^  -.pag
      pag
    pag(pow `[%3 objects.prf hashes.prf read-index.prf])
  pag
::
::
  ++  make-oversized-ai-page
  |=  parent=page:t
  ^-  page:t
  =/  pag=page:t  (make-empty-page:h parent)
  =/  cert=ai-pow-certificate:t  (sample-ai-pow-cert 4)
  =.  pag
    ?^  -.pag
      pag
    pag(pow `[%ai-pow [1.048.577 0] cert])
  ?^  -.pag
    pag(digest (compute-digest:page:t pag))
  pag(digest (compute-digest:page:t pag))
::
::  Unit: call the jet directly with a deliberately-undecodable %ai-pow artifact.
::  The jet decodes first, fails, and returns %.n — WITHOUT needing the boot
::  setup. A clean %.n proves the `~%`/`~/` hint chain matches the hot state and
::  the jet executes; a mis-chained hint would run the stub `!!` and crash.
++  test-ai-pow-verify-jet-fires
  ^-  tang
  =/  result=?  (ai-pow-verify:mine [%ai-pow 0 0] 0 0)
  (expect-eq !>(%.n) !>(result))
::
::  Integration: a height-1 page carrying a garbage %ai-pow pow travels the live
::  consensus path. It passes +validate-page-without-txs (version %3 valid at
::  height >= ai-pow-activation-height=0; target = parent epoch target since
::  pre-zk-asert; digest via the belt-safe +hashable-digest %ai-pow fix) and
::  reaches +check-pow, whose %ai-pow branch calls `ai-pow-verify:mine`. The jet
::  decode-fails -> %.n, so +heard-block emits %liar-block-id %failed-pow-check.
::  This exercises BOTH the jet firing in-consensus AND the digest fix (without
::  it, +make-ai-pow-garbage-page crashes building the block).
++  test-ai-pow-block-rejected
  ^-  tang
  =+  [nockchain genesis]=init-nockchain:h
  =/  block1=page:t  (make-ai-pow-garbage-page:h genesis)
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) block1)
  =/  rejected=?  (cleanly-rejected effs)
  (expect-eq !>(%.y) !>(rejected))
::
++  test-malformed-non-ai-pow-cleanly-rejected
  ^-  tang
  =+  [nockchain genesis]=init-nockchain:h
  =/  block1=page:t  (make-malformed-non-ai-page genesis)
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) block1)
  =/  rejected=?
    ?|  (has-liar-cause %pow-target-check-failed effs)
        (has-liar-cause %failed-pow-check effs)
    ==
  (expect-eq !>(%.y) !>(rejected))
::
++  test-oversized-ai-pow-artifact-cleanly-rejected
  ^-  tang
  =+  [nockchain genesis]=init-nockchain:h
  =/  block1=page:t  (make-oversized-ai-page genesis)
  =^  effs=(list effect:h)  nockchain
    (~(heard-block k-by:h nockchain) block1)
  =/  rejected=?  (cleanly-rejected effs)
  (expect-eq !>(%.y) !>(rejected))
::
::  A peer can encode %3 in the legacy proof mold. It must be rejected before
::  the undefined proof hash can process the untrusted page digest.
++  test-legacy-v3-proof-cleanly-rejected
  ^-  tang
  =+  [nockchain genesis]=init-nockchain:h-v1
  =/  block1=page:t  (make-legacy-v3-proof-page genesis)
  =/  =cause:h-v1  [%fact %0 %heard-block block1]
  =^  effs=(list effect:h-v1)  nockchain
    (pok-on-wire:h-v1 libp2p-gossip-wire:h-v1 cause nockchain)
  =/  rejected=?  (has-liar-peer-cause %legacy-v3-proof-artifact effs)
  (expect-eq !>(%.y) !>(rejected))
--
