::  control for wet_rib_subject_key_trap: entering with $(x 0) makes the
::  redone core differ from the entry subject, so both compilers reach the
::  x=[@ud @ud] re-mull and reject.
|%
++  main
  |=  [a=@ b=?]
  =/  foo  |*(x=@ [.+(x) $(x [x x])])
  =>  foo
  $(x 0)
--
