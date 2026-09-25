::  regression (DIVERGENCES rows 12, 13): float literals at the edges of
::  each width: positive values, overflow to +inf, and deep underflow.
|%
++  main
  :*  .1
      .~1
      .~~1
      .~~~1
      .1e39
      .~1e309
      .~~1e5000
      .~~~1e5000
      .1e-100
      .~1e300
      .~1e-400
      .~~~1e-5000
      .-1e-100
      .1e38
      .~1.7976931348623157e308
      .~~65504
  ==
--
