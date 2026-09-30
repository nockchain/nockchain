::  c2: ^~ over a subject holding a self-referential %hold: bran re-enters
::  the same hold and blocks it (bran_canonical_semi_inner ~6620), leaving the
::  constant head foldable.
|%
++  foo  [%5 foo]
++  main
  |=  b=?
  =/  x  foo
  [^~(-.x) ^~(x) ^~(+<.x)]
--
