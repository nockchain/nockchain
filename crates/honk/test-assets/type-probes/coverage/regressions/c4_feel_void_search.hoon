::  Companion to reject/c4_fond_void_tail: with a one-limb wing the void
::  search is the whole answer, so feel answers no and !@ takes its else
::  branch in both compilers.
|%
++  main
  |=  a=@
  =+  loop
  !@(zz 1 2)
++  loop  loop
--
