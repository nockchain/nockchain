::  c2 regression: two +| chapters with the same name. hoon-138's parser
::  (++wisp) makes the chapter [%$ [%eror "duplicate chapter: |aa"]]; hatch
::  once merged the two chapters. ^+ only plays the goal, so hoonc accepts.
|%
++  main
  ^+  |%  +|  %aa  ++  x  1  +|  %aa  ++  y  1  --
  !!
--
