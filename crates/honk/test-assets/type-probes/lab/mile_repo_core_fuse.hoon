::  ++repo on a %core, reached from ++fuse (?=(^ core) narrowing) and from
::  ++nest sint (^-([* *] core)), with a plain payload and with a %hold
::  payload that plays to void; neither payload is literal %void.
|%
++  main
  |=  [a=@ b=?]
  =/  c  |%  ++  x  a  --
  =/  f  |=(@ !!)
  =/  d  =>  (f a)  |%  ++  y  1  --
  :*  ?:(?=(^ c) !>(c) !>(~))
      !>(`[* *]`c)
      ?:(?=(^ d) !>(d) !>(~))
      !>(`[* *]`d)
  ==
--
