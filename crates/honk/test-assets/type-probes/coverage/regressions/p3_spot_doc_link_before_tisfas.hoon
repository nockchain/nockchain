!:
::  p3 regression (MISMATCH): under !:, a `::  +name:` doc line (also `$name`,
::  `.name`, `|name` links) between two =/ binders anchors hoonc's %dbug spot
::  for the second =/ at the doc line; honk once skipped it as a plain comment
::  and started the spot at the =/ (utils.rs
::  skip_plain_doc_before_equals_slash_start only kept a +link naming the
::  binder itself).
|%
++  main
  |=  a=@
  =/  e  4
  ::  +main: mentions the arm
  =/  f  5
  [e f]
--
