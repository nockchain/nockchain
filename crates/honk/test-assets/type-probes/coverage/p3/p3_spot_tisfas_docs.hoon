!:
::  p3: %dbug spots for =/ binders after anchoring docs (utils.rs
::  skip_plain_doc_before_equals_slash_start: trailing larg doc on the
::  previous =/ line and on a :- line, larg doc after =+ / after a blank
::  line / before a face=spec binder, a +link naming the binder followed
::  by an empty doc)
|%
++  main
  |=  a=@
  =/  b  1
  =/  c  2  ::    larg trailing doc
  =/  d  3
  =+  e=4
  ::    larg after tislus
  =/  f  5

  ::    larg after blank line
  =/  g  6
  ::    larg before a tuple binder
  =/  hi=[h=@ i=@]  [7 8]
  :-  b  ::    larg trailing on a colhep line
  =/  k  10
  ::  +j: names the binder
  ::
  =/  j  9
  [b c d e f g hi j k]
--
