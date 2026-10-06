::  p1: %loop specs (`/foo`, hoon-138 ++scad) in example (^- and a =/foo
::  sample), factory (a bare ,/mol), relative (the $@ side spore skips) and
::  autoname (=/foo); autoname %bcpm names a =$&(...) sample after its spec.
|%
++  $  5
++  buc  ^-(/$ 7)
++  main
  |=  a=@
  =/  foo  5
  =/  mol  $@(@ud [@ @])
  :*  buc
      ^-(/foo 6)
      !>(|=(=/foo foo))
      !>(,/mol)
      !>((,/mol 8))
      !>(,$@(@ /mol))
      !>((,$@(@ /mol) [1 2]))
      !>(|=(=$&(@ud |=(b=@ b)) ud))
  ==
--
