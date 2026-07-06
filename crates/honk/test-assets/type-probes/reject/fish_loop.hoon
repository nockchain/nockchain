::  ?= against a recursive mold: the fish descent re-enters the same
::  %hold and bails fish-loop.
|%
++  main
  |=  x=*
  ?=((list @) x)
--
