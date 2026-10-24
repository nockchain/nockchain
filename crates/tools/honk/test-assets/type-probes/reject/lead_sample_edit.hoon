::  %= edit of a lead core's sample: lead grants no write access to the
::  payload, so the edit peek is blocked.
|%
++  main
  |=  x=@ud
  =/  k  ^?(dec)
  k(+< 5)
--
