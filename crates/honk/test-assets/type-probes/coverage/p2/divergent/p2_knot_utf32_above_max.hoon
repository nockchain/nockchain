::  p2 divergence: ~-~110000. (above U+10FFFF) decodes to 0xfffd in honk
::  (and ~-~140000., lead byte 0xf5) but to the code points in hoonc;
::  decode_one_utf8 utils.rs:6038 and teff utils.rs:5988 substitute U+FFFD
|%
++  main  [~-~110000. ~-~140000.]
--
