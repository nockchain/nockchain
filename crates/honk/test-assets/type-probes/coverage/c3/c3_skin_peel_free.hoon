::  c3: ?# cell skins on iron, zinc and lead cores. The skin test peeks the
::  payload with %free, which ++peel opens on every non-gold core.
|%
++  main
  |=  a=@
  =/  i  ^|(|.(a))
  =/  z  ^&(|.(a))
  =/  l  ^?(|.(a))
  :*  ?:(?#([* @] i) i !!)
      ?:(?#([* @] z) z !!)
      ?:(?#([* @] l) l !!)
  ==
--
