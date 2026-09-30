::  c2 regression: ^+ plays =<(x core) to a hold on the [%eror "duplicate
::  arm: +x"] arm (++whap) without expanding it, and `!!` mints without a
::  nest against that goal, so hoonc builds; honk once expanded the hold
::  there.
|%
++  main
  ^+  =<  x
      |%
      ++  x  1
      ++  x  2
      --
  !!
--
