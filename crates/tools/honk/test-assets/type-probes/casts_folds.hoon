::  Casts and compile-time folds: ^- ^+ ^* products and a ^~ constant fold
::  (seminoun/battery constant parity — the `^*([cube cube])` class).
|%
++  main
  |=  x=@
  =/  folded  ^~([1 2 3])
  [!>(^-(@u x)) !>(^+(folded folded)) !>(^*([%a %b]))]
--
