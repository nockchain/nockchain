::  c4: tune faces in ++fond. ++twin of two synthetic ports from one =* alias
::  reached through a fork of wet-gate products (find.rs ~630-649, with
::  compose_axis_formula ~402), a limb after a synthetic port (find.rs ~126),
::  and a =, bridge that mints to void in one fork branch (find.rs ~456) next
::  to one that resolves (find.rs ~461).
|%
++  f
  |*  q=*
  =*  w  [q q]
  .
++  main
  |=  [a=@ b=?]
  =/  s  ?:(b (f `@ud`a) (f `@ux`a))
  =/  t
    ?:  b
      =,  !!
      .
    =,  [p=1 q=2]
    .
  :*  !>(w.s)
      !>(-.w.s)
      !>(p.t)
  ==
--
