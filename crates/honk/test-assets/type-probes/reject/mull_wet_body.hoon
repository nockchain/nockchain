::  wet gate whose body type-checks for the formal sample but not the
::  actual (cell passed where the body needs an atom): ++mull rejects.
|%
++  wig
  |*  a=@
  (dec a)
++  main
  |=  x=@ud
  (wig [x x])
--
