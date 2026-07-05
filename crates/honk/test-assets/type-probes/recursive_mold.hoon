::  Recursive mold: %hold construction, bunt, and leg access through the
::  lazily-expanded type.
|%
++  main
  |=  t=tree
  [!>(*tree) !>(?@(t t l.t))]
+$  tree  $@(~ [n=@ud l=tree r=tree])
--
