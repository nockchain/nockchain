::  c6 divergent (HOONC-ONLY): hoonc's +segments falls back to the literal
::  name `dbl--dash` when the hep-separated parse fails; honk splits on every
::  `-` and drops empty parts (pipeline.rs hyphen_segment_variants ~643), so it
::  only tries lib/dbl-dash.hoon and lib/dbl/dash.hoon.
/+  dbl--dash
`*`dd:dbl--dash
