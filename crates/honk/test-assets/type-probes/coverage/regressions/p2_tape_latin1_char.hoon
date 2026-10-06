::  p2 regression (DIVERGENCES row 21): hoonc parses source bytes, so a
::  non-ASCII character in a wide tape is one element per UTF-8 byte ("é" is
::  [0xc3 0xa9]); honk used to keep one element holding the code point
|%
++  main  "é"
--
