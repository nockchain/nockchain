::  A wet arm whose body tests an alias to ^~(+4) with ?=, where +4 is an arm
::  formula in the battery of a core built in the body. Mulling the call
::  folds +4 through the ++laze thunk of ++mile, which mints the arm, so the
::  alias formula is [%1 formula] and ++cove crashes. Spots are off so the
::  fold is not wrapped in a %spot hint.
!.
|%
++  w
  |*  a=*
  =>  |%  ++  x  1  ++  y  2  --
  =*  z  ^~(+4)
  ?=(@ z)
++  main  (w 5)
--
