::  p3 DIVERGENT: @uw path segment. hoonc renders /0w1 as '0w1'; honk
::  renders '0wB' (utils.rs w_ne uses the RFC 4648 digit order A-Z a-z 0-9,
::  hoon ++ne:w is 0-9 a-z A-Z - ~).
|%
++  main
  |=  a=@
  !>(/0w1)
--
