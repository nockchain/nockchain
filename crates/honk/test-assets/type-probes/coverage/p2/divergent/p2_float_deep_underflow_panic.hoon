::  p2 divergence: honk panics on an @rs literal far below the subnormal
::  range (lug utils.rs:3828 calls bex(q-1) with q > 128, which asserts at
::  utils.rs:4060; hoon-138 ++lug has no width limit)
|%
++  main  .1e-100
--
