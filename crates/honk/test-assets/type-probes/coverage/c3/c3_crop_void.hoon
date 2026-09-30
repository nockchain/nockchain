::  c3: ?= with a void pattern: fish is constant false, gain fuses to %void and lose crops
::  against a %void ref (mod.rs crop_inner ~10594).
|%
++  main
  |=  [a=@ b=[@ @]]
  :*  !>(?:(?=(!! a) !! a))
      !>(?:(?=(!! b) !! b))
  ==
--
