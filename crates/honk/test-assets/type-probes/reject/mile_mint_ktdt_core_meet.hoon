::  control for mile_mull_ktdt_core_meet: the same ^. in a dry gate is
::  minted, so the literal core carries a full battery seminoun, unequal to
::  the played core's in both compilers; both take nest's slow path and
::  reject x=%foo against x=@.
|%
++  id  |*(a=* a)
++  dry
  |=  x=@
  ^.  id
  =>  |%  ++  y  1  --
  .(x %foo)
++  main
  |=  [a=@ b=?]
  !>((dry a))
--
