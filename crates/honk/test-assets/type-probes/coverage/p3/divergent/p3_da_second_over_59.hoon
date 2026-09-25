::  p3 DIVERGENT (HOONC-ONLY): hoon ++when parses seconds with dum:ag;
::  utils.rs absolute_date rejects s >= 60
|%
++  main
  |=  a=@
  !>(~2020.1.1..1.1.60)
--
