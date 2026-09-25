::  p1: ~$ (%sgbc, profiler hit) is in hoon-138's rune table but hatch has no
::  parser for it, so open %sgbc (utils.rs ~2154) is unreachable from source.
|%
++  main
  |=  a=@
  !>(~$(%foo +(a)))
--
