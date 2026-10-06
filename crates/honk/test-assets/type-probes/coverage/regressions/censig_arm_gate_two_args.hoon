::  Intended reading of the censig unit test: one door arg (+6), then call the arm's gate with two args.
|%
++  main
  |=  [a=@ b=?]
  =/  d
    |_  s=@
    ++  f  |=  [x=@ y=@]  [x y s]
    --
  [!>(~(f d 5)) !>((~(f d 5) 1 2)) !>((~(f d a) a 2))]
--
