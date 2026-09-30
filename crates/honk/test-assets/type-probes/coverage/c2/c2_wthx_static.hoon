::  c2: ?# static skin matching and dynamic skin formulas on legs:
::  base_match_static %flag/%atom true/false/unknown (mod.rs ~5071-5095),
::  cell_skin_match_static partial/unknown/short-circuit outcomes (~5129-5138),
::  skin_match_static %name and exact %leaf (~5148-5156), and
::  skin_test_formula %flag/%base/%cell/%name/%spec/%term (~5291-5352).
|%
+$  duo  [@ @]
++  main
  |=  $:  a=@
          b=?
          c=*
          d=[@ @]
          e=[* *]
          f=$@(@ [[@ @] @])
          g=%5
          h=[[@ @] *]
          k=[[@ @] @]
      ==
  :*  !>(?#(? b))
      !>(?#(? d))
      !>(?#(? a))
      !>(?#(? c))
      !>(?#(@ a))
      !>(?#(@ d))
      !>(?#(@ c))
      !>(?#(^ c))
      !>(?#([@ @] c))
      !>(?#([@ @] f))
      !>(?#([@ @] h))
      !>(?#([@ @] k))
      !>(?#(x=@ a))
      !>(?#(x=@ c))
      !>(?#(%5 g))
      !>(?#(%5 a))
      !>(?#(*[@ @] d))
      !>(?#(duo d))
  ==
--
