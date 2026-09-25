::  p4 DIVERGENT (HOONC-ONLY): an @rh (half precision) literal with a huge
::  exponent. honk panics with "value too large for u128" at hatch
::  utils.rs:3808; hoonc builds it.
|%
++  main  !>(..main)
++  f  .~~1e5000
--
