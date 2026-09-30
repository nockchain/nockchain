::  A ^- whose mold is a gate with its sample replaced by a %hold that fails
::  to play, against another gate: nest meets the mold's context with its
::  payload and repo of the hold fails.
|%
++  main
  =/  g  |=(a=@ a)
  =/  h  |=(a=@ +(a))
  ^-(_g(a =>(|.(zzz) $)) h)
--
