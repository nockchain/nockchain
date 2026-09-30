::  control for wet_rib_subject_key_arm: the outer call site's subject differs
::  from the inner one, so both compilers mull x=[@ud @ud] and reject.
|%
++  foo
  |*  x=@
  :-  .+(x)
  =>  ..foo
  (foo [0 0])
++  main
  |=  [a=@ b=?]
  (foo 0)
--
