::  as wet_dear_empty_wec_atom, but the actual sample carries its own faces:
::  hoon-138 ++dear returns `~ for an empty wec, so the top-level q= face is
::  dropped too, while the faces inside a cell sample (redone with fresh face
::  stacks) survive.
|%
++  wig
  |*  a=?(%foo %bar)
  1
++  main
  |=  [x=@ud b=?]
  :-  !>((wig q=%baz))
  !>((wig r=[p=%baz q=x]))
--
