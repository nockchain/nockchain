::  wet gate whose formal sample is a fork of constants that the actual
::  constant misses entirely: ++redo's sint prunes every fork case, wec is
::  empty, and hoon-138 ++dear returns `~ so the sample loses its face; honk's
::  base-kind fallback re-admits the atom cases and keeps a=.
|%
++  wig
  |*  a=?(%foo %bar)
  1
++  main
  |=  [x=@ud b=?]
  !>((wig %baz))
--
