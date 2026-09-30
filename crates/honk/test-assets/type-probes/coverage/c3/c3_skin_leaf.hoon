::  c3: gain_leaf_skin / lose_leaf_skin, %flag and %null base skins under ?: (mod.rs ~9924, ~10219, ~9694, ~10034):
::  noun, atom (constant hit and miss), cell, face, fork, hint and hold refs.
|%
+$  num  @ud
++  main
  |=  [a=* b=@ud c=? d=(unit @) e=(list @) f=$@(?(%x %y) [@ @]) g=[h=@ i=@] n=num]
  :*  !>(?:(?#(%5 a) a a))
      !>(?:(?#(%5 b) b b))
      !>(?:(?#(%foo a) a a))
      !>(?:(?#(%.y c) c c))
      !>(?:(?#(? c) c !!))
      !>(?:(?#(? a) a a))
      !>(?:(?#(~ d) d d))
      !>(?:(?#(~ e) e e))
      !>(?:(?#(%x f) f f))
      !>(?:(?#(%z f) !! f))
      !>(?:(?#(~ a) a a))
      !>(?:(?#(%7 g) !! g))
      !>(?:(?#(%7 h.g) g g))
      !>(?:(?#(%7 n) n n))
      !>(?:(?#(~ t.+.e) e e))
  ==
--
