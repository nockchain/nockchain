::  p2 regression (DIVERGENCES row 21): ~-~110000. and ~-~140000. (above
::  U+10FFFF; the second has lead byte 0xf5) decode to their code points,
::  since ++teff maps every lead byte above 0xef to length 4; honk used to
::  substitute 0xfffd
|%
++  main  [~-~110000. ~-~140000.]
--
