::  c2 divergent: two +| chapters with the same name. hoon-138's parser
::  (++wisp) replaces the second chapter with [%$ [%eror "duplicate chapter"]];
::  hatch keeps a chapter map without that check, so the arm hoon that ends up
::  in the artifact's type differs. ^+ only plays the goal, so hoonc accepts.
|%
++  main
  ^+  |%  +|  %aa  ++  x  1  +|  %aa  ++  y  1  --
  !!
--
