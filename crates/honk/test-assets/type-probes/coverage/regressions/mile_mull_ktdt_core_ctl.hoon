::  control for mile_mull_ktdt_core_meet: the edit x=@ud survives nest's
::  slow-path meet against x=@, so both compilers accept.
|%
++  id  |*(a=* a)
++  wet
  |*  x=*
  ^.  id
  =>  |%  ++  y  1  --
  .(x 5)
++  main
  |=  [a=@ b=?]
  !>((wet a))
--
