::  p4 DIVERGENT (HOONC-ONLY): an @rd literal that overflows double
::  precision. honk panics with "value too large for u128" at hatch
::  utils.rs:3808 (`end_big(..).to_u128().expect(..)` in the float rounding
::  path); hoonc builds it.
|%
++  main  !>(..main)
++  f  .~1e309
--
