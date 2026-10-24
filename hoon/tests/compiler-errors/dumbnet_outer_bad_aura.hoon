/=  dumb  /apps/dumbnet/inner
::  Based on open/hoon/apps/dumbnet/outer.hoon.
::
::  The real outer kernel expression is still below, but this demo first binds
::  text (`@t`) into a decimal atom aura (`@ud`).  Run with honk's `--vet`
::  flag to see the strict type/nest error at this binding.
=/  demo-block-height=@ud
  'height-is-text'
((moat:dumb |) inner:dumb)
