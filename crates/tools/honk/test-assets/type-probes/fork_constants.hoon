::  Fork construction and dedup: nested ?: over cube constants builds %fork
::  sets whose treap layout (mug order) is pinned byte-exactly.
|%
++  main
  |=  [a=? b=?]
  [!>(?:(a %x ?:(b %y %z))) !>(?:(a 1 ?:(b 2 ?:(a 3 1))))]
--
