!:
::  p3 DIVERGENT (MISMATCH): under !:, a `::  +name:` doc line (also `$name`,
::  `.name`, `|name` links) between two =/ binders anchors hoonc's %dbug spot
::  for the second =/ at the doc line [6 3]; honk skips it as a plain comment
::  and starts the spot at the =/ [7 3] (utils.rs
::  skip_plain_doc_before_equals_slash_start only keeps a +link naming the
::  binder itself).
|%
++  main
  |=  a=@
  =/  e  4
  ::  +main: mentions the arm
  =/  f  5
  [e f]
--
