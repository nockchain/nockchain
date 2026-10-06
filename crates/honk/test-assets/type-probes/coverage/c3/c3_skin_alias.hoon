::  c3: cool with a synthetic port (mod.rs ~9652): ?= on a =* alias of a computed (non-wing) hoon
::  leaves the subject unrefined in both branches.
|%
++  main
  |=  a=$@(@ [@ @])
  =*  x  [a a]
  =*  y  ?@(a a 0)
  :*  !>(?:(?=([@ @] x) x x))
      !>(?:(?=(%5 y) y a))
  ==
--
