::  c6 divergent (HOONC-ONLY): honk joins continuation lines into one clause
::  (pipeline.rs parse_leading_imports ~447-461), then strip_inline_comment
::  (~596) cuts everything after `::`, dropping `base`.
/+  util,  ::  a comment after the comma
    base
`*`seed:base
