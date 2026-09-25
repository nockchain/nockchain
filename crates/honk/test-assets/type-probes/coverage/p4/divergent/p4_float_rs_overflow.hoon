::  p4 DIVERGENT (MISMATCH): an @rs literal whose magnitude overflows single
::  precision. hoonc and honk encode `.1e39` to different atoms (hatch float
::  literal parser, utils.rs ~3780-3830 round/overflow handling); found while
::  probing ast/hoon.rs BinaryFloat helpers, owned by the atom-parser package.
|%
++  main  !>(..main)
++  f  .1e39
--
