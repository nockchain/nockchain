::  c3: ?# skins on faced fork options and faced hint payloads under ?: (mod.rs gain_atom_skin %face
::  ~9804, lose_atom_skin %face ~10094, gain_leaf_skin/lose_leaf_skin %face ~9976/~10245,
::  gain_cell_skin %face ~9891 and lose_cell_skin %face ~10184 where the lost payload is void).
|%
+$  fac  x=@
+$  duf  x=[p=@ q=@]
++  main
  |=  [f=$@(x=@ [@ @]) g=$@(@ x=[p=@ q=@]) h=fac k=duf]
  :*  !>(?:(?#(@ f) f f))
      !>(?:(?#(%5 f) f f))
      !>(?:(?#(^ g) g g))
      !>(?:(?#(@ h) h !!))
      !>(?:(?#(%5 h) h h))
      !>(?:(?#(^ k) k !!))
  ==
--
