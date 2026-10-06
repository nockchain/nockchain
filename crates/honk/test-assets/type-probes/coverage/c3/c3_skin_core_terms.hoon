::  c3: cell skins with term tails on a core ref (mod.rs gain/lose_cell_skin
::  %core, is_noun_term_skin). A term tail other than noun refines the core as
::  a cell; losing @ from the payload leaves it, and the term tail noun keeps
::  the core.
|%
+$  any  *
++  main
  |=  a=@
  =/  c  |.(a)
  :-  ?:(?#([* any] c) c !!)
  ?:(?#([@ noun] c) !! c)
--
