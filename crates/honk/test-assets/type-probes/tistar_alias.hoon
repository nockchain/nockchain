::  =* aliasing: the alias resolves to the underlying leg's type.
|%
++  main
  |=  s=[deep=[val=@ud] other=@t]
  =*  v  val.deep.s
  [!>(v) !>([v other.s])]
--
