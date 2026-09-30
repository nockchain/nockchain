::  p1: open %xray (~2585) with a namespaced tag (Mane TagSpace, ~2591) and a
::  plain attribute list (~2604-2636).
|%
++  main
  |=  a=@
  :-  !>(;foo_bar;)
  !>(;foo(baz "x", qux "y{<a>}");)
--
