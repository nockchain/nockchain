::  %= with changes applied through a wing that only resolves synthetically
::  (the face sits at different axes across the fork branches), which
::  cannot take changes: the ++et door bails.
|%
++  main
  |=  x=@ud
  =/  c
    ?:  =(x 1)
      [a=1 2]
    [b=3 a=4 5]
  c(a 9)
--
