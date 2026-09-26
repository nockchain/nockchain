::  Fixed c3 divergence: lose of a nested cell skin on a faced sub-ref. hoon-138 ar +lose %cell strips a
::  %face ref ([%face *] $(ref q.ref)); honk's lose_cell_skin used to re-wrap the face, so
::  the else-branch type kept q=@ where hoonc has a bare @ (and the fork treap differs).
|%
++  main
  |=  c=$@(@ [p=* q=*])
  !>(?:(?#([@ ^] c) !! c))
--
