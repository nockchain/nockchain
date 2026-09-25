::  Non-wing =* aliases (typed via kthp, autocons of wings, a call) take fund's synthetic mint branch in both compilers.
|%
++  main
  |=  [a=@ b=?]
  =*  t=@ud  +12
  =*  p  [+13 +12]
  =*  q  (add a 1)
  [!>(t) !>(p) !>(-.p) !>(q) !>([t p q])]
--
