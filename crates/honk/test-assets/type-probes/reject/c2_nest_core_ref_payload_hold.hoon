::  Like c2_nest_core_payload_hold, one core deeper: the mold's edited
::  sample is itself an edited gate, so meeting the outer context with the
::  outer payload nests two gates, and the edited one is on the reference
::  side when its context meets its payload.
|%
++  main
  =/  g3  |=(b=@ b)
  =/  g2  |=(b=@ +(b))
  =/  g  |=(a=_g3 a)
  =/  h  |=(a=_g3 +(3))
  ^-(_g(a g2(b =>(|.(zzz) $))) h)
--
