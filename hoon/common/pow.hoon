/=  sp  /common/stark/prover
/=  np  /common/nock-prover
/=  *  /common/zeke
~%  %pow-lib  ..ut  ~
|%
::  +ai-pow-verify: consensus verifier for a version-%3 (%ai-pow) block's
::  recursive certificate. Branch (b): the Hoon body is a fail-safe `!!`; the
::  real implementation is the mandatory `~/ %ai-pow-verify` jet (crate
::  `ai-pow-jets`), which canonicalizes the block commitment (`BLAKE3(jam(..))`,
::  matching the miner), re-derives the canonical matrices from the protocol
::  seed, verifies the compact recursive certificate against the boot-injected
::  setup, and binds it to `[commit target]` (target = merge:bignum LE atom).
::  Lives in this shared lib (imported by both apps/dumbnet/inner.hoon as `mine`
::  and .../lib/consensus.hoon) under a `~% .. ..ut ~` root so the jet's hint
::  chain resolves — the kernel's `%dumb-inner` door can host no extra arm (its
::  `fort` mold fixes it to load/peek/poke) and its `|^`-nested arms fail cold
::  registration. `!!` is fail-safe: a missing/mis-chained jet crashes rather
::  than silently accepting. Called from +check-pow's `%ai-pow` branch.
++  ai-pow-verify
  ~/  %ai-pow-verify
  |=  [artifact=* commit=* target=@]
  ^-  ?
  !!
::
++  check-target
  |=  [proof-hash-atom=tip5-hash-atom target-bn=bignum:bignum]
  ^-  ?
  =/  target-atom=@  (merge:bignum target-bn)
  ?>  (lte proof-hash-atom max-tip5-atom:tip5)
  (lte proof-hash-atom target-atom)
::
++  prove-block  (cury prove-block-inner pow-len)
::
::  +prove-block-inner
++  prove-block-inner
  |=  prover-input:sp
  ^-  [proof:sp tip5-hash-atom]
  =/  =prove-result:sp
    ?-  version
      %0  (prove:np version header nonce pow-len)
      %1  (prove:np version header nonce pow-len)
      %2  (prove:np version header nonce pow-len)
    ==
  ?>  ?=(%& -.prove-result)
  =/  =proof:sp  p.prove-result
  =/  proof-hash=tip5-hash-atom  (proof-to-pow proof)
  [proof proof-hash]
--
