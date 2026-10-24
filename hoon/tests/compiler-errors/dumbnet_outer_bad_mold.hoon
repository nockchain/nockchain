/=  dumb  /apps/dumbnet/inner
::  Based on open/hoon/apps/dumbnet/outer.hoon.
::
::  The real outer kernel expression produces the dumbnet kernel core.  This
::  demo intentionally claims that core has the mold `(list @ud)`.  Run with
::  honk's `--vet` flag to see the strict type/nest error at this binding.
=/  demo-kernel=(list @ud)
  ((moat:dumb |) inner:dumb)
demo-kernel
