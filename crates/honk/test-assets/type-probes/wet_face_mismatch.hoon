::  wet-gate call with face-mismatched actuals: ++redo must refurbish the
::  actual sample's faces (p/q) with the formal's (wid/dat), nesting the
::  formal faces OUTSIDE the actual's own (hoon-138 ++dear/++done), or the
::  vet-time ++mull find of wid.msg fails. Regression for the redo
::  face-order bug (honk rejected zose's make-k via hmc/hmac).
|%
++  wham
  |*  [key=[wid=@ dat=@] msg=[wid=@ dat=@]]
  (add wid.msg dat.key)
++  main
  |=  x=@ud
  =/  oct  [p=4 q=x]
  !>((wham [2 x] oct))
--
