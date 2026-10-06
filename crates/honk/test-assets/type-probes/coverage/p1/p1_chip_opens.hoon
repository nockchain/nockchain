::  p1: hatch open() arms reached through honk's chip (a ?: condition):
::  %sgbr with feck on rock, cell and cord traces (~2097, ~2827), %ktdt
::  (~2080), %wtgl (~2546), %mccl (~2288-2314, one-item case ~2290), %sgls
::  (~2165).
|%
++  main
  |=  a=*
  :*  !>(?:(~|(%foo ?=(@ a)) a 0))
      !>(?:(~|([%foo a] ?=(@ a)) a 0))
      !>(?:(~|('foo' ?=(@ a)) a 0))
      !>(?:(^.(|=(b=? b) ?=(@ a)) a 0))
      !>(?:(?<(?=(^ a) ?=(@ a)) a 0))
      !>(?:(;:(|=([b=? c=?] &(b c)) ?=(@ a) ?=(@ a) ?=(@ a)) a 0))
      !>(?:(;:(|=(b=? b) ?=(@ a)) a 0))
      !>(?:(~+(?=(@ a)) a 0))
  ==
--
