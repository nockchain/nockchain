::  c6 (was divergence 28): hoonc parses the import block apart from the body,
::  so the body's %spot starts after the /- and /+ lines. honk blanks the
::  header before parsing the body (pipeline.rs parse_import_header). Also
::  exercises /- /+ resolution, face=suffix aliases, and the hyphen-to-slash
::  fallback (resolve_import, suffix_path_candidates).
/-  *kinds
/+  alias=math-extra, *star-lib,
    deep-nest-mod
|%
++  main  [`kind`%a (triple:alias 3) starred deepest:deep-nest-mod]
--
