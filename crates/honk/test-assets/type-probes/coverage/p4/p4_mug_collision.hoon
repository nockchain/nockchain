::  p4: treap order of a battery whose arm names have equal mugs. %ccck and
::  %floi (also %esxt/%gqcl) collide under +mug, so when the nested `|%`
::  arm body is encoded, hatch utils.rs map_put_mug falls through
::  gor_mug/mor_mug to `dor` (~12853-12903) to place the arms. The file
::  core's own battery is encoded by honk, so the colliding arms sit in a
::  nested core that is stored raw as an arm body.
|%
++  main  !>(..main)
++  inner
  |%
  ++  ccck  1
  ++  floi  2
  ++  esxt  3
  ++  gqcl  4
  --
--
