::  Axis alias edited from a trap: $(a.x ..) tacks through the trap payload into the =* face tune (fund on +6 relative to that face); alias also passed as a gate sample.
|%
++  main
  |=  [a=@ b=?]
  =*  x  +6
  :-  !>  |-  ^-  [@ ?]
          ?:  =(0 a.x)  x
          $(a.x (dec a.x), b.x !b.x)
  !>  =/  g  |=(s=[a=@ b=?] [b.s a.s])
      (g x)
--
