::  Minimal reproduction of hoon-138's ++mor and its transitive
::  dependencies, extracted from the layer-1 compare/math core.
::
::  This isolates the native `rest-loop` failure seen while compiling
::  the `mor` arm in full hoon-138.
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
--
