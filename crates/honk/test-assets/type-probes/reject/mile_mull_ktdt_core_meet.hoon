::  ^. duplicates one core literal into a play (via the wet id) and a mull
::  (the wet caller's body, re-checked at the call site).  hoon-138 builds
::  the played core with *seminoun and the mulled one with a laze battery,
::  so nest takes the slow path and meet(context, payload) rejects x=%foo
::  against x=@.  honk gives both cores the same blocked seminoun.
|%
++  id  |*(a=* a)
++  wet
  |*  x=*
  ^.  id
  =>  |%  ++  y  1  --
  .(x %foo)
++  main
  |=  [a=@ b=?]
  !>((wet a))
--
