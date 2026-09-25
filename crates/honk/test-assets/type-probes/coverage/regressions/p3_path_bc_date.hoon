::  p3 regression (was MISMATCH): BC @da path segment. hoonc renders /~1-.1.1 as '~1-.1.1';
::  honk renders '~0-.1.1' (utils.rs yore computes PIVOT - y for the BC era;
::  hoon ++yore uses +((sub 292.277.024.400 y))).
|%
++  main
  |=  a=@
  !>(/~1-.1.1)
--
