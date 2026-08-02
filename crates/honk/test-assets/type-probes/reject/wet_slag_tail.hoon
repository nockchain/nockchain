::  A claimed honk-only `fire-dry` failure, retained as a rejection-parity
::  regression after checking the actual artifact verdict. The canonical
::  hoon-138 compiler also emits no artifact: `?~ cs` specializes the wet
::  ++slag call to a nonempty list, whose dry loop can recall with an empty
::  tail. Both compilers must reject; hoonc's process exit code alone is not a
::  successful-build oracle.
|%
++  t
  |=  cs=tape
  ^-  tape
  ?~  cs  ~
  (slag 1 cs)
--
