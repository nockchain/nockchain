::  Minimal reproduction of hoon-138's ++dvr and its transitive
::  dependencies, extracted from the layer-1 math core (lines 17-70).
::
::  This isolates the mint-nice failure in dvr without compiling the
::  full hoon-138.
::
|%
++  dec
  ~/  %dec
  |=  a=@
  ~_  leaf+"decrement-underflow"
  ?<  =(0 a)
  =+  b=0
  |-  ^-  @
  ?:  =(a +(b))  b
  $(b +(b))
::
++  sub
  ~/  %sub
  |=  [a=@ b=@]
  ~_  leaf+"subtract-underflow"
  ^-  @
  ?:  =(0 b)  a
  $(a (dec a), b (dec b))
::
++  lth
  ~/  %lth
  |=  [a=@ b=@]
  ^-  ?
  ?&  !=(a b)
      |-
      ?|  =(0 a)
          ?&  !=(0 b)
              $(a (dec a), b (dec b))
  ==  ==  ==
::
++  dvr
  ~/  %dvr
  |:  [a=`@`1 b=`@`1]
  ^-  [p=@ q=@]
  ~_  leaf+"divide-by-zero"
  ?<  =(0 b)
  =+  c=0
  |-
  ?:  (lth a b)  [c a]
  $(a (sub a b), c +(c))
--
