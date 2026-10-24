::  ?: on a ?= that is statically always true: the else branch is
::  unreachable and vet bails mint-vain.
|%
++  main
  |=  x=@ud
  =/  y  %foo
  ?:  ?=(%foo y)
    1
  2
--
