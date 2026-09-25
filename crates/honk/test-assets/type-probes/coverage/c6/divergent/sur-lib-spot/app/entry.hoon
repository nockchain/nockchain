::  c6 divergent (MISMATCH): the entry's body %spot starts at the first /- or
::  /+ line in honk but after the import block in hoonc. hatch utils.rs
::  should_skip_outer (~11545) only drops the outer spot after /= /* /#.
::  Also exercises honk's Sur/Lib resolution, face=suffix aliases, and the
::  hyphen-to-slash fallback (pipeline.rs resolve_import, suffix_path_candidates).
/-  *kinds
/+  alias=math-extra, *star-lib,
    deep-nest-mod
|%
++  main  [`kind`%a (triple:alias 3) starred deepest:deep-nest-mod]
--
