::  tests/dumb/mod/unit/ai-pow-jet.hoon
::
::    Validates the AI-PoW consensus verify jet (`~/ %ai-pow-verify` in
::    /common/pow, implemented by crate `ai-pow-jets`, Branch b: Hoon body is a
::    fail-safe `!!`, the Rust jet is the real impl).
::
/=  helpers  /tests/dumb/helpers
/=  txe  /common/tx-engine
/=  mine  /common/pow
/=  *  /common/test
|%
++  h  ~(. helpers bc-ai-pow-provable:helpers)
++  t  ~(. txe bc-ai-pow-provable:helpers)
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
  =/  rejected=?
    %+  lien  effs
    |=  e=effect:h
    ?&  ?=([%liar-block-id *] e)
        =(%failed-pow-check cause.e)
    ==
  (expect-eq !>(%.y) !>(rejected))
--
