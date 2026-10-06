::  regression (DIVERGENCES row 13): an @rs literal far below the subnormal
::  range; ++lug has no width limit (hatch lug used to call bex(q-1) with
::  q > 128 and panic).
|%
++  main  .1e-100
--
