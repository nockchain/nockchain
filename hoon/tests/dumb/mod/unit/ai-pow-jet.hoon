::  tests/dumb/mod/unit/ai-pow-jet.hoon
::
::    Validates that the AI-PoW consensus verify jet (`~/ %ai-pow-verify` in
::    /common/pow, implemented by crate `ai-pow-jets`) FIRES. The Hoon body of
::    `++ai-pow-verify` is a fail-safe `!!` (Branch b: the Rust jet is the real
::    implementation), so the ONLY way this arm returns a loobean instead of
::    crashing is if the jet's `~%`/`~/` hint chain matches the registered hot
::    state. We call it directly with a deliberately-undecodable `%ai-pow`
::    artifact: the jet decodes the artifact first, fails, and returns %.n
::    (rejected) — WITHOUT needing the boot-injected setup. A clean %.n therefore
::    proves the jet is wired and firing; a mis-chained hint would run the stub
::    `!!` and crash the test instead.
::
::    +check-pow (apps/dumbnet/inner.hoon) dispatches version-%3 (%ai-pow) blocks
::    to this same `ai-pow-verify:mine`, so a firing jet here means it fires in the
::    live consensus path too.
::
/=  mine  /common/pow
/=  *  /common/test
|%
++  test-ai-pow-verify-jet-fires
  ^-  tang
  ::  [artifact commit target]; artifact is garbage → jet returns %.n.
  =/  result=?  (ai-pow-verify:mine [%ai-pow 0 0] 0 0)
  (expect-eq !>(%.n) !>(result))
--
