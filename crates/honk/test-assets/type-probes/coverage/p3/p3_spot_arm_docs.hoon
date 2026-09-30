!:
::  p3: %dbug spots for arm bodies under doc comments (utils.rs
::  chumsky_spot_to_hoon_spot -> arm_body_start_from_header: ++/+$ headers,
::  $ arm name, doc markers with and without content, = body after docs)
|%
++  gen
  ::  arm-level docs
  ::
  ::  second paragraph
  ^-  @
  0
++  foo
  ::    the legacy z-set uses raw gor
  ::    so things stay consistent
  =/  one  17
  (add one one)
++  bar  ::  inline arm doc
  1
++  baz
  ::
  ::  after an empty doc line
  2
++  qux
  ::  doc then equals
  =+  x=3
  x
+$  mol
  ::  a mold
  @ud
++  $
  ::  buc arm
  4
++  zap
  ::no-space comment
  5
--
