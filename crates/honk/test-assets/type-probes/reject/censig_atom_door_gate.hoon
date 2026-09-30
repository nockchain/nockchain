::  Same shape as the censig unit test inside a probe gate: two cnsg args on a door whose sample is a bare atom (no +12 to edit).
|%
++  main
  |=  [a=@ b=?]
  =/  d
    |_  s=@
    ++  f  |=  [x=@ y=@]  [x y s]
    --
  !>(~(f d 1 2))
--
