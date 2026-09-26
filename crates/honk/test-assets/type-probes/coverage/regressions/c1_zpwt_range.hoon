::  c1 divergence: !? with a version pair.  hoon-138 ++open %zpwt accepts
::  [p q] when (lte hoon-version p) and (gte hoon-version q), so [139 137]
::  admits 138 (hoon-138.hoon:8677).  hatch's open (utils.rs:2690) reads the
::  pair as [min max] and panics "hoon-version" for [139 137]; it would also
::  accept [137 139], which hoonc rejects.  Would cover write_zpwt_arg's
::  pair arm (mod.rs ~1341).
|%
++  main
  |=  a=@
  !?([139 137] a)
--
