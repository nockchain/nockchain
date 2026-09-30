::  A ^- whose mold is a fork with a %hold option that fails to play,
::  against a cell: nest tries each option and repo of the hold fails.
|%
++  main
  |=  c=?
  =/  v  [1 2]
  ^-(_?:(c 5 =>(|.(zzz) $)) v)
--
