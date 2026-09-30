::  Fixed c3 divergence: gain of ?#([* noun] core) under ?:. The tail skin is the term %noun, so hoon-138
::  ar +gain keeps [%core payload coil]; honk's gain_cell_skin used to keep the core only for
::  Skin::Base(NounExpr) and instead gain the tail as a %spec of the `noun` mold, giving a %cell.
|%
++  main
  |=  a=@
  =/  c  |.(a)
  !>(?:(?#([* noun] c) c !!))
--
