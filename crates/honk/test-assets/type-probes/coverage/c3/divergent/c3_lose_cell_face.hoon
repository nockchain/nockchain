::  c3 divergent: lose of a nested cell skin on a faced sub-ref. hoon-138 ar +lose %cell strips a
::  %face ref ([%face *] $(ref q.ref)); honk's lose_cell_skin (mod.rs ~10184) re-wraps the face, so
::  the else-branch type keeps q=@ where hoonc has a bare @ (and the fork treap differs).
|%
++  main
  |=  c=$@(@ [p=* q=*])
  !>(?:(?#([@ ^] c) !! c))
--
