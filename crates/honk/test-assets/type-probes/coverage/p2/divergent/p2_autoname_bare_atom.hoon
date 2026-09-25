::  p2 divergence: =/  =@  5  atom crashes honk; hoonc autonames a bare @
::  as %atom. variable_name_and_type (utils.rs:3378) relies on autoname,
::  which tests the aura for "$" (utils.rs:2999) while aura_text yields ""
|%
++  main
  =/  =@  5
  atom
--
