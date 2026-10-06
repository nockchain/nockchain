::  p1: hatch open() arms reached through honk's mull fallback while a called
::  wet gate is checked: %sgcn with hooks (~2120-2141), %sgls (~2165), %mccl
::  (~2288), %wtgl (~2546).
|%
++  wig
  |*  a=*
  ~%  %wig  +>  ==  %hook  +<  ==
  :*  ~+(a)
      ;:(|*([b=* c=*] [b c]) a a a)
      ?<(?=(~ a) a)
  ==
++  main
  |=  x=@ud
  [!>((wig x)) !>((wig [x x]))]
--
