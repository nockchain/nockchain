::  p2 regression (DIVERGENCES row 21): a tape character above U+00FF is
::  accepted as its UTF-8 bytes ([0xe2 0x82 0xac]); honk used to reject it
::  because its tape filter only admitted code points 0x80-0xff
|%
++  main  "€"
--
