::  %= mutation, %~ door-arm call, and %* mutating call product types.
|%
++  main
  |=  s=[p=@ud q=@t]
  =/  door  |_  n=@ud
            ++  bump  +(n)
            --
  [!>(s(p 9)) !>(~(bump door 4)) !>(%*(. s p 1, q 'x'))]
--
