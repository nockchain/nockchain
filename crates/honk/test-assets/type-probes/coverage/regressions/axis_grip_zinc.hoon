::  ^= cell-skin +5 read into a zinc (^&) core's payload: %read may see the
::  sample but not the context.
|%
++  main
  |=  [a=@ b=?]
  !>(^=([p q] ^&(|=(x=@ x))))
--
