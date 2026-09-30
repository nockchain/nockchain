::  c3: ?# skins on a self-referential hold (+$ r $@(@ r)) under ?: -- the per-skin hold guards
::  (mod.rs gain/lose_atom_skin ~9825/~10117, gain/lose_cell_skin ~9912/~10207,
::  gain/lose_leaf_skin ~9994/~10266) cut the repeat to %void.
|%
+$  r  $@(@ r)
++  main
  |=  a=r
  :*  !>(?:(?#(@ a) a !!))
      !>(?:(?#(^ a) !! a))
      !>(?:(?#(%5 a) a a))
  ==
--
