::  p2 divergence: honk panics on an @rd literal whose 5^e product is wider
::  than 128+p bits (lug utils.rs:3806-3808 casts the discarded low bits to
::  u128 and panics)
|%
++  main  .~1e300
--
