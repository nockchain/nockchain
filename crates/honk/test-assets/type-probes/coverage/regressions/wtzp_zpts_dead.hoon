::  ?!(p) under !=  (vet off) with gain(p) void: hoon-138 opens ?! to ?:,
::  drops the impossible branch and emits [11 [%toss p] [1 0]].
|%
++  main
  |=  a=@
  !=(?!(?=(^ a)))
--
