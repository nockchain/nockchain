::  c5: type IR forms through the native interner and noun boundary: named
::  molds (hints), faces, forks, and gold/iron/lead/zinc cores (ty.rs,
::  intern.rs); value_dag.rs children (~181) via a ^~ fold that takes a
::  fragment of a known atom
|%
+$  nn  *
+$  mm  @ud
+$  pr  [a=@ b=@]
++  zc  ^&(|=(a=@ a))
++  ic  ^|(|=(a=@ a))
++  lc  ^?(|=(a=@ a))
++  main
  |=  [b=? c=*]
  :*  !>(*nn)
      !>(*mm)
      !>(*pr)
      !>(`nn`c)
      !>(zc)
      !>(?:(b zc zc))
      !>(?:(b zc ic))
      !>(?:(b lc ic))
      !>(=/(z zc z))
      !>([zc ic lc])
      !>(^~(.*(5 [0 2])))
      !>(=/(k 5 ^~(.*(k [0 2]))))
      !>(=/(k [5 6] ^~(.*(k [0 5]))))
      !>(=/(p *pr =.(a.p 7 p)))
  ==
--
