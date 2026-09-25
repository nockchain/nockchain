::  p1: hoon-138 names a =foo=spec sample foo-<autoname> (+scad '='), but
::  hatch's spec parser (runes/buc.rs ~622) keeps just foo, so x-ud is unbound.
|%
++  main
  |=  =x=@ud
  !>(x-ud)
--
