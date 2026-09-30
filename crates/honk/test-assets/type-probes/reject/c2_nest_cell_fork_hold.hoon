::  A ^- whose cell mold has a %hold head that fails to play (the arm of a
::  played, never minted trap), against a fork of cells: nest walks the
::  reference fork, then the cell heads, and repo of the hold fails.
|%
++  main
  |=  c=?
  =/  v  ?:(c [%a 5] [%b 6])
  ^-([_=>(|.(zzz) $) @] v)
--
