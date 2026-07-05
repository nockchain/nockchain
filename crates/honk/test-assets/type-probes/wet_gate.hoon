::  Wet gate (|*) instantiated at two unrelated types: mull + wet fire and
::  the per-call product types.
|%
++  main
  |=  [a=@ud b=[p=@t q=?]]
  =/  dup  |*(x=* [x x])
  [!>((dup a)) !>((dup b))]
--
