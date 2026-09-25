::  p4 DIVERGENT (MISMATCH): positive float literals encode negative in honk
::  (`.1` -> 0xbf80.0000, `.~1` -> 0xbff0...). hatch utils.rs
::  binaryfloat_mul (~4152) repeats the `ma == 0 || mb == 0` test where
::  hoon's +mul:fl has `?:  =(s.a s.b)  (mul:m a b)`, so every finite product
::  (the literal's mantissa times 10^e) goes through `fli` and flips sign.
|%
++  main  !>(..main)
++  g  .1
++  f  .~1
++  h  .~~1
--
