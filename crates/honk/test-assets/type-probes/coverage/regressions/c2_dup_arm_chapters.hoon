::  c2 regression: one arm name in two different +| chapters. hoon-138's
::  parser (++wisp) replaces the one in the earlier chapter with [%eror
::  "duplicate arm: +x"]; hatch once kept both.
|%
++  main
  ^+  |%  +|  %aa  ++  x  1  +|  %bb  ++  x  2  --
  !!
--
