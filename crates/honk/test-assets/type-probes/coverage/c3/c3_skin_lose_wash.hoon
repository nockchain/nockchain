::  c3: a wash skin in lose (mod.rs lose_skin_inner %wash). The head skin
::  gains %void on the cell head, so gain never reaches the wash tail, while
::  lose keeps the tail.
|%
++  main
  |=  x=[^ @]
  ?:(?#([@ ,] x) !! x)
--
