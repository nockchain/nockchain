::  ^+ cast between cores with different batteries: the value core's
::  battery cannot nest in the example's.
|%
++  main
  |=  x=@ud
  =/  one
    |%
    ++  a  1
    --
  =/  two
    |%
    ++  a  1
    ++  b  2
    --
  ^+(two one)
--
