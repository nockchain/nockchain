::  c3: crop/fuse with a %core subject (mod.rs crop_inner %core ~10639, crop_sint ~10684):
::  ?= with fork and face refs on a core leg, and a ^* gate-mold skin on a gate.
|%
++  main
  |=  a=@
  =/  c  |.(a)
  =/  g  |=(b=@ b)
  :*  !>(?:(?=(?(%a %b) c) !! c))
      !>(?:(?=(x=@ c) !! c))
      !>(?:(?#(^*($-(@ @)) g) !! g))
  ==
--
