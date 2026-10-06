::  p1: wide $&(spec gate) in hoon position; hatch's bucpam_wide
::  (runes/buc.rs ~355) omits the ( ) delimiters, so it never parses.
|%
++  main
  |=  a=*
  !>($&(@ |=(b=* ?@(b b 0))))
--
