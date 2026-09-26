::  c6: a /* import of a .hoon file. hoonc picks a node's kind from the file
::  name (+is-hoon), not the rune, so lib/foo.hoon is compiled as Hoon under
::  the face `x` (pipeline.rs import_kind_for).
/*  x  %hoon  /lib/foo/hoon
`*`x
