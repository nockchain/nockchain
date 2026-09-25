::  c2 divergent: one arm name in two different +| chapters. hoon-138's parser
::  (++wisp) replaces the duplicate with [%eror "duplicate arm"]; hatch does
::  not, so the arm hoon in the artifact's type differs.
|%
++  main
  ^+  |%  +|  %aa  ++  x  1  +|  %bb  ++  x  2  --
  !!
--
