::  a face on the pinned value hides its inner faces from find:
::  d does not resolve inside a=[b=@ c=@].
|%
++  main
  |=  x=@ud
  =/  a  [b=x c=x]
  d.a
--
