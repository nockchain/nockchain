::  lose of ?#([@ *] core) under ?:. hoon-138 ar +lose %cell on a %core ref keeps the core only
::  for the term tail %noun; for the [%base %noun] tail it loses the tail from %noun, which is
::  %void, so the else branch is vain: mint-vain. (honk used to keep the %core and accept.)
|%
++  main
  |=  a=@
  =/  c  |.(a)
  ?:(?#([@ *] c) !! %foo)
--
