::  c3: peek through non-gold cores (mod.rs go ~10945, peel ~12674): zinc/lead/iron cores read at the
::  sample, the context and the whole payload, plus peel(%both) while ?= searches past a zinc core.
|%
++  main
  |=  [x=$@(@ [@ @]) y=@]
  =/  z  ^&(|=(b=@ b))
  =/  l  ^?(|=(b=@ b))
  =/  i  ^|(|=(b=@ b))
  :*  !>(+6.z)
      !>(+7.z)
      !>(+3.z)
      !>(+6.l)
      !>(+7.l)
      !>(+7.i)
      !>(+6.i)
      !>(=>([^&(|=(b=@ b)) a=x] ?:(?=(@ a) a a)))
      !>(?:(=(y 0) y +2.+2.y))
  ==
--
