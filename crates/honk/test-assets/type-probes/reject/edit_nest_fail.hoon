::  %= edit whose replacement does not nest in the leg's type when the
::  product is cast back (^+ around the edit).
|%
++  main
  |=  x=@ud
  =/  a  [p=x q=x]
  ^+  a
  a(p [x x])
--
