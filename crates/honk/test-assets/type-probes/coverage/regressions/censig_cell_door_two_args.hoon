::  Corrected censig: door sample is a pair, so ~(f d 1 2) edits +12/+13 of the door (wing [[%| 0 ~] [%& 12]] then [[%| 0 ~] [%& 13]]).
|%
++  main
  |=  [a=@ b=?]
  =/  d
    |_  [s=@ t=@]
    ++  f  |=  [x=@ y=@]  [x y s t]
    --
  [!>(~(f d 1 2)) !>((~(f d 1 2) 3 4)) !>((~(f d a 2) a 4))]
--
