::  Door +* aliases to axis wings (tstr wrapped around each arm body), pulled via two-arg cnsg; edits through the alias inside the arm.
|%
++  main
  |=  [a=@ b=?]
  =/  d
    |_  [a=@ b=?]
    +*  x  +6
        y  +<
    ++  f  x(a 5)
    ++  g  =.(a.y 7 [a b])
    ++  h  [x y]
    --
  [!>(~(f d a b)) !>(~(g d 1 |)) !>(~(h d a &))]
--
