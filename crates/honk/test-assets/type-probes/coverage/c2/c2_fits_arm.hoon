::  c2: mint_fits where the wing is an arm rather than a leg, so hoon-138
::  emits [%7 arm-formula fish-at-1] (mod.rs ~5028-5032); fish of a void
::  example type is [%1 |] (type_test_formula_on_axis_inner ~5229).
|%
++  five  5
++  duo  [1 2]
++  main
  |=  b=?
  :*  !>(?=(@ five))
      !>(?=(^ five))
      !>(?=([@ @] duo))
      !>(?=(~ five))
      !>(?=(%5 five))
      !>(?=(_!! b))
  ==
--
