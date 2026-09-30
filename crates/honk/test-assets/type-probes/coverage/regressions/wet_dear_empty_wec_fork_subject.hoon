::  forked actual sample: the %baz case misses every formal case (empty wec,
::  faceless in hoon-138) while the %foo case keeps a=.
|%
++  wig
  |*  a=?(%foo %bar)
  1
++  main
  |=  [x=@ud b=?]
  !>((wig ?:(b %baz %foo)))
--
