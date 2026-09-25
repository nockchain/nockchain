::  A ^- whose mold is a played core with an arm that fails to play,
::  against a core with the same chapters and arms: nest compares the arms
::  one by one and the failing arm is reached through left and right
::  subtrees of both treaps (the shapes of c2_battery_left_chapter).
|%
++  main
  =/  r
    |%
    +|  %as
    ++  x  1
    +|  %eu
    ++  y  1
    +|  %gg
    ++  z  1
    +|  %av
    ++  as  1
    ++  eu  1
    ++  gg  1
    ++  av  1
    --
  ^-  $_  =>  +
          |%
          +|  %as
          ++  x  1
          +|  %eu
          ++  y  1
          +|  %gg
          ++  z  1
          +|  %av
          ++  as  1
          ++  eu  1
          ++  gg  1
          ++  av  zzz
          --
  r
--
