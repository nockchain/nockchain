/=  sp  /common/stark/prover
/=  np  /common/nock-prover
/=  *  /common/zeke
~%  %pow-lib  ..ut  ~
|%
::  +ai-pow-verify: consensus verifier for a version-%4 (%ai-pow) block's
::  recursive certificate. Branch (b): the Hoon body is a fail-safe `!!`; the
::  real implementation is the mandatory `~/ %ai-pow-verify-v2` jet (crate
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
  ::  The kernel selects rules from the block height, never the certificate.
  ::  Version the jet binding when changing its sample: old kernels/binaries
  ::  must hit the fail-safe body rather than misinterpret the new arguments.
  ~/  %ai-pow-verify-v2
  |=  [rules=?(%legacy %hardened) artifact=* commit=* target=@]
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
      %3  (prove:np version header nonce pow-len)
      %5  (prove:np version header nonce pow-len)
    ==
  ?>  ?=(%& -.prove-result)
  =/  =proof:sp  p.prove-result
  =/  proof-hash=tip5-hash-atom  (proof-to-pow proof)
  [proof proof-hash]
::
++  v5-nonce-pow
  |=  [header=noun-digest:tip5 nonce=noun-digest:tip5 pow-len=@]
  ^-  [object-zero=proof-data:sp digest=tip5-hash-atom]
  =/  object-zero=proof-data:sp
    (puzzle-proof-data:sp header nonce pow-len)
  =/  probe=proof:sp
    [%5 [object-zero ~] ~ 0]
  [object-zero (proof-to-pow:sp probe)]
--
