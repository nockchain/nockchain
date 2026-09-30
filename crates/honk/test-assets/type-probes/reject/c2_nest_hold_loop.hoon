::  A ^- whose mold is the %hold of an arm that returns itself: repo of the
::  hold reaches the same hold with no cell in between, so nest answers no
::  (hoon-138's seg check) and the cast fails.
|%
++  main  ^-(_loop 5)
++  loop  loop
--
