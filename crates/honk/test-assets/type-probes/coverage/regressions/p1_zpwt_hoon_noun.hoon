::  p1: !?(138 x) opens fine (utils.rs ~2690), but hatch's hoon_to_noun
::  (zpwt_arg_to_noun, utils.rs ~13717) encodes the version as [%atom '138']
::  instead of 138, so the arm hoon embedded in the core type differs.
|%
++  main
  |=  a=@
  !>(!?(138 a))
--
