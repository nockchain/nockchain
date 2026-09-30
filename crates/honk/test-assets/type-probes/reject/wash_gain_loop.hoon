::  A wash skin (,) as a ?: condition. hoon-138 ar:gain handles %wash with
::  $(ref (~(play ut ref) [%wing ~])), recursing on the same skin forever, so
::  hoonc never writes an artifact. honk rejects it with a gain-wash error.
|%
++  main
  |=  a=*
  ?:(?#(, a) 1 2)
--
