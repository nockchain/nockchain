::  self-recursive type alias with no base case. NOTE: as of 2026-07 BOTH
::  compilers reject this by native stack overflow (the expansion deepens
::  instead of repeating, so no rest-loop cycle-cut fires and the process
::  aborts) — the pairing still agrees (no artifact from either side), but
::  this probe is excluded from the in-process compiler_reject Rust test
::  (the overflow would abort the test harness) and its Bazel test is
::  tagged manual (how long the overflow takes is platform-dependent and
::  exceeds CI's small-test timeout on Linux runners).
|%
+$  aaa  aaa
++  main
  |=  x=@ud
  *aaa
--
