::  p2 divergence: ~-~d800. (a surrogate code point) decodes to 0xfffd in
::  honk but to 0xd800 in hoonc (++taft has no range check);
::  decode_one_utf8 utils.rs:6020 substitutes U+FFFD
|%
++  main  ~-~d800.
--
