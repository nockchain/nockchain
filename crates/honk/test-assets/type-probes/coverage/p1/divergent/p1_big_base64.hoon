::  p1: base64_to_atom (utils.rs ~179) accumulates in a u128 and panics past
::  128 bits; hoonc accepts arbitrarily large @uw literals.
|%
++  main
  |=  a=@
  !>(0w1.00000.00000.00000.00000.00000.00000)
--
