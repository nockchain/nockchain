::  hatch reek accepts %cncb with no changes (hoon-138 reek does not); show %_ with zero changes does not parse in either compiler.
|%
++  main
  |=  [a=@ b=?]
  =*  x  %_(+6)
  !>(x)
--
