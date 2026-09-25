::  p2 divergence: honk panics on @uv/@uw literals wider than 128 bits
::  (base32_number/base64_number feed base32_to_atom/base64_to_atom,
::  utils.rs:179-213, which accumulate in a u128 and expect no overflow)
|%
++  main
  :-  0v1.00000.00000.00000.00000.00000.00000
  0w1.00000.00000.00000.00000.00000
--
