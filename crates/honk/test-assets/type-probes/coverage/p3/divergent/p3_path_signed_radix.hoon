::  p3 DIVERGENT: signed non-decimal path segment. hoonc renders /-0x10 as
::  '-16' (decimal); honk renders '-0x10' (utils.rs rend_with_rep 's' arm
::  recurses into the 'u' arm with the original radix letter).
|%
++  main
  |=  a=@
  !>(/-0x10)
--
