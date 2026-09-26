::  p3 DIVERGENT (MISMATCH): a tall hoon that starts with a signed path
::  segment. hatch used to treat any /- /+ /= /* /# /? /% line in tall
::  position as an import header and fail on the following -- (fixed with
::  divergence 28/30: runes/fas.rs import_header is only tried at the top of
::  the file). Both compilers now build it, but the knot renders as '-0x10' on
::  one side and '-16' on the other: divergence 19 (signed-radix path knots).
|%
++  main  /-0x10
--
