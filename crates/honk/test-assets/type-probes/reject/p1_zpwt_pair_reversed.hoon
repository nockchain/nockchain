::  p1: open %zpwt (utils.rs ~2695) accepts !?([130 140] x), which hoon-138
::  rejects: it requires q <= hoon-version <= p, so [130 140] never holds.
|%
++  main
  |=  a=@
  !>(!?([130 140] a))
--
