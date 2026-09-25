::  p1: a namespaced sail attribute name (mane a_b) does not parse in hatch's
::  sail attribute parser, so open %xray's attribute TagSpace arm (utils.rs
::  ~2614) is unreachable; hoonc accepts it.
|%
++  main
  |=  a=@
  !>(;foo(baz_qux "x");)
--
