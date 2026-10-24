::  Prelude wet gates over lists: turn/roll/snag products (hold resolution
::  through the stdlib's recursive molds).
|%
++  main
  |=  l=(list @ud)
  [!>((turn l |=(n=@ud +(n)))) !>((roll l add)) !>((flop l))]
--
