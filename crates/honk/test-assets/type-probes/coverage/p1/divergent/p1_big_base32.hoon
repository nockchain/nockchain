::  p1: base32_to_atom (utils.rs ~199) accumulates in a u128 and panics past
::  128 bits; hoonc accepts arbitrarily large @uv literals.
|%
++  main
  |=  a=@
  !>(0v1.00000.00000.00000.00000.00000.00000)
--
