::  p1: open %zpwt (utils.rs ~2695) reads !?([p q] x) as min/max, but hoon-138
::  accepts when q <= hoon-version <= p (p is the upper bound).
|%
++  main
  |=  a=@
  !>(!?([140 130] a))
--
