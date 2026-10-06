::  c6: import block parsing and resolution for /= and /* (pipeline.rs
::  parse_leading_imports continuation lines (indented comments and
::  whitespace-only lines inside a clause), inline comments, blank and comment
::  lines between runes; resolve_import Raw and Bar), data leaves with trailing and
::  all-zero bytes, and dependency reuse by path and by content (honk.rs
::  compile_path caches)
/=  raw  /common/raw-thing  ::  inline comment after a raw import
/=  *  /common/star-raw

/=
    ::  an indented comment inside the continued clause
    deep
    
    /common/nested/deep
::  a comment line between import runes
/=  twice  /common/raw-thing
/=  copy  /common/raw-copy
/*  blob  %jam  /data/blob/jam
/*  zeros  %jam
    /data/zeros/jam
|%
++  main
  :*  !>(raw)
      !>(sr)
      !>(deep)
      !>(twice)
      !>(copy)
      !>(blob)
      !>(zeros)
  ==
--
