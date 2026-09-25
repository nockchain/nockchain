::  p2 divergence: a positive @rs literal with a nonnegative decimal exponent
::  gets its sign flipped (binaryfloat_mul utils.rs:4153 tests the mantissas
::  instead of =(s.a s.b), so it always takes the swr/fli branch)
|%
++  main  .1
--
