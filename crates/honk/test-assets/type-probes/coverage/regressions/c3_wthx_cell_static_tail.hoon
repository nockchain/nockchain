::  Fixed c3 divergence: ?# cell skin with a dynamic head and a statically-true tail. hoon-138 ar +fish
::  folds (flan head [%1 &]) to head; honk's skin_test_formula used to build the pair with
::  formula_arena.and, which left [%6 head [%1 0] [%1 1]]. Formula-only mismatch.
|%
++  main
  |=  a=*
  ?#([@ *] a)
--
