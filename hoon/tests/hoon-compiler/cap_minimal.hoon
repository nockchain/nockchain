::  Minimal reproduction of hoon-138's ++cap and its transitive
::  dependencies, extracted from the layer-1 math/tree core.
::
::  This isolates the remaining boolean mismatch now seen at the
::  `++cap` arm in full hoon-138.
::
|%
++  dec
  ~/  %dec
  |=  a=@
  ~_  leaf+"decrement-underflow"
  ?<  =(0 a)
  =+  b=0
  |-
  ^-  @
  ?:  =(a +(b))
    b
  $(b +(b))
::
++  sub
  ~/  %sub
  |=  [a=@ b=@]
  ~_  leaf+"subtract-underflow"
  ^-  @
  ?:  =(0 b)
    a
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
  ?:  (lth a b)
    [c a]
  $(a (sub a b), c +(c))
::
++  div
  ~/  %div
  |:  [a=`@`1 b=`@`1]
  ^-  @
  -:(dvr a b)
::
++  cap
  ~/  %cap
  |=  a=@
  ^-  ?(%2 %3)
  ?-  a
    %2        %2
    %3        %3
    ?(%0 %1)  !!
    *         $(a (div a 2))
  ==
--
