::  tests/dumb/mod/unit/state-10-to-11.hoon
::
::    Coverage for the kernel state-10 -> state-11 (Logos) migration, and in
::    particular its activation guard.
::
::    +state-10-to-11 refuses to migrate a node whose locally-synced chain tip
::    has already reached +ai-pow-activation-height, because the branch-local
::    per-puzzle ASERT lineage the dual-puzzle regime needs cannot be
::    reconstructed after the fact:
::
::      ~|  %state-10-post-ai-lineage-cannot-be-reconstructed
::      ?>  migration-safe
::
::    That guard is conditional on runtime chain state (highest-block-height vs
::    ai-pow-activation-height), unlike every prior migration, which either
::    succeeds or fails deterministically for every node. Its failure mode is
::    therefore only reachable for the subset of nodes that cross the activation
::    height before upgrading -- a case no single upgrade smoke test surfaces.
::    These arms pin both sides of the conditional, plus the happy-path field
::    initialization, by driving the real +load through +load:inner:dumb (the
::    same entry the runtime uses when a new kernel is swapped in over old
::    state) -- the idiom /tests/dumb/mod/unit/h-zoon-consensus uses for the
::    state-8 -> state-11 path.
::
/=  dumb      /apps/dumbnet/inner
/=  helpers   /tests/dumb/helpers
/=  tx-engine  /common/tx-engine
/=  *         /apps/dumbnet/lib/types
/=  *         /common/h-zoon
/=  *         /common/test
|%
::  Mainnet-default ai-pow-activation-height. A bunt kernel-state-10 carries the
::  blockchain-constants $~ defaults (so the ASERT phase relationships +load
::  asserts are already valid, and this is the activation height the migration
::  guard compares a node's tip against). Hardcoded rather than read off the
::  constants noun to keep this a pure unit test; the four arms are
::  self-checking -- any drift in the default flips one of them (a tip that was
::  below activation would start crashing, or one at activation would stop).
++  activation  ^-  @  114.300
::
::  A bunt kernel-state-10 with a chosen highest-block-height (the node's synced
::  tip). Constants stay at the $~ mainnet defaults.
++  ks10-with-tip
  |=  tip=(unit @)
  ^-  kernel-state-10
  =/  base=kernel-state-10  *kernel-state-10
  base(highest-block-height.d tip)
::
::  HAPPY PATH 1 — a fresh/un-synced node (no tip) migrates cleanly to %11 with
::  empty lineage caches. This is the branch the guard waves through via `?~`.
++  test-state-10-to-11-migrates-fresh-node
  ^-  tang
  =/  loaded=kernel-state  (load:inner:dumb (ks10-with-tip ~))
  %+  expect-eq  !>([%11 0 0])
  !>  :*  -.loaded
          ~(wyt h-by block-versions.c.loaded)
          ~(wyt h-by puzzle-asert-states.d.loaded)
      ==
::
::  HAPPY PATH 2 — a node synced to exactly one block below activation still
::  migrates. Reaching %11 proves it passed the reconstruction guard.
++  test-state-10-to-11-migrates-tip-below-activation
  ^-  tang
  =/  loaded=kernel-state  (load:inner:dumb (ks10-with-tip `(dec activation)))
  (expect-eq !>(%11) !>(-.loaded))
::
::  GUARD — a node whose tip is exactly at activation must refuse to load, with
::  the reconstruction reason. This is the case the user-facing risk is about:
::  a node that failed to upgrade before its chain crossed 114,300.
++  test-state-10-to-11-refuses-tip-at-activation
  ^-  tang
  %+  expect-fail
    |.((load:inner:dumb (ks10-with-tip `activation)))
  `"state-10-post-ai-lineage-cannot-be-reconstructed"
::
::  GUARD — same, for a tip well past activation.
++  test-state-10-to-11-refuses-tip-past-activation
  ^-  tang
  %+  expect-fail
    |.((load:inner:dumb (ks10-with-tip `(add activation 100.000))))
  `"state-10-post-ai-lineage-cannot-be-reconstructed"
--
