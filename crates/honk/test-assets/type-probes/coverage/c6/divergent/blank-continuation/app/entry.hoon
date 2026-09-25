::  c6 divergent (HOONC-ONLY): an empty line ends honk's continued clause
::  (pipeline.rs parse_leading_imports ~447-460) and the indented `base` line
::  then ends the import block, so `base` is never imported. hoonc's gaw
::  accepts blank lines after the comma.
/+  util,

    base
`*`seed:base
