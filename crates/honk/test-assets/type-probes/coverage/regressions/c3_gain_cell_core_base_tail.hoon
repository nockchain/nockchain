::  Fixed c3 divergence: gain of ?#(^ core) under ?:. hoon-138 ar +gain %cell on a %core ref keeps the core
::  only when the tail skin is the term %noun (=(%noun ^skin.skin)); ^ is [%cell [%base %noun] [%base %noun]],
::  so hoonc gives [%cell payload %noun]. honk's gain_cell_skin used to test for
::  Skin::Base(NounExpr) instead and keep the %core.
|%
++  main
  |=  a=@
  =/  c  |.(a)
  !>(?:(?#(^ c) c !!))
--
