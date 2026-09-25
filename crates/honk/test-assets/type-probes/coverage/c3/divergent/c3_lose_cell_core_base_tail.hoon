::  c3 divergent: lose of ?#([@ *] core) under ?:. hoon-138 ar +lose %cell on a %core ref keeps the
::  core only for the term tail %noun; for the [%base %noun] tail it loses the tail from %noun, which
::  is %void, so the else branch is vain and hoonc rejects (mint-vain). honk's lose_cell_skin
::  (mod.rs ~10176) keeps the %core for Skin::Base(NounExpr) and accepts.
|%
++  main
  |=  a=@
  =/  c  |.(a)
  ?:(?#([@ *] c) !! %foo)
--
