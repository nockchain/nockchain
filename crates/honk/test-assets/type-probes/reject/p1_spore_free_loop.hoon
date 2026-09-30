::  p1: spore (utils.rs ~440) on a %loop spec (`/foo`): no parser rule builds
::  a $$ recursion point, so the loop is free and hoon-138's ~(got by cox)
::  crashes; hatch's spore panics.
|%
++  main
  =/  foo  @
  !>(,[@ /foo])
--
