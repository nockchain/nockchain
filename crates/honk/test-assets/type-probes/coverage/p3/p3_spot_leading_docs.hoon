!:
::  p3: %dbug spots for expressions that start at a doc block
::  (utils.rs non_doc_start_after_leading_doc_span: larg (4-space) doc
::  lines, blank doc lines, walk-back over equally indented larg lines;
::  skip_plain_doc_before_equals_slash_start: plain doc, +link naming the
::  binder, larg doc, blank line between =/ binders)
|%
++  main
  |=  a=@
  =/  b  1
  ::  plain doc before a sibling =/
  =/  c  2
  ::    larg doc one
  ::    larg doc two
  =/  d  3
  ::
  ::    larg after blank doc
  =/  e  4
  ::  +f: names the binder
  =/  f  5
  ::  + plus then space
  =/  g  6

  ::  plain after blank line
  =/  h  7
  :-  a
  ::    trailing larg doc
  ::  short
  [b c d e f g h]
--
