::  c6 divergent (MISMATCH): single-file form of the import-header spot bug.
::  honk ignores the /? Ford pin (pipeline.rs parse_leading_imports ~471), but
::  the body %spot starts at the /? line in honk and after the header in hoonc
::  (hatch utils.rs should_skip_outer ~11545 recognizes only /= /* /#).
/?  310
`*`[1 2]
