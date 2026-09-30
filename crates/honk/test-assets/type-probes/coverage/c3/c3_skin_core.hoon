::  c3: ?# skins on a %core ref under ?: (mod.rs gain_atom_skin/lose_atom_skin ~9803/~10093,
::  gain_leaf_skin/lose_leaf_skin ~9972/~10242, gain_cell_skin/lose_cell_skin %core ~9867/~10162 with
::  a void head or a non-%noun tail; the core-preserving tail is in divergent/).
|%
++  main
  |=  a=@
  =/  c  |.(a)
  :*  !>(?:(?#(@ c) !! c))
      !>(?:(?#(%5 c) !! c))
      !>(?:(?#([* @] c) c !!))
      !>(?:(?#([@ @] c) !! c))
  ==
--
