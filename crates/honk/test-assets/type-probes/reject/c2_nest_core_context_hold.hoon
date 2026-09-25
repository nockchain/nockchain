::  A ^- whose mold is a trap played over a subject that is a %hold failing
::  to play, against a trap: nest compares the gold contexts and repo of the
::  hold fails.
|%
++  main
  =/  r  |.(1)
  ^-(_=>(=>(|.(zzz) $) |.(1)) r)
--
