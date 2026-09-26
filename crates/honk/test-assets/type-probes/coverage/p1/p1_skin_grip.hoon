::  p1: flay (~2739), grip (~2846) and half (~2916) through p=q and ^= with
::  cell and nested-name skins over :_ :- :^ :~ :* products.
|%
++  main
  |=  a=@
  :*  !>([b c]=:_(1 2))
      !>([b c]=:-(1 2))
      !>([b c]=:^(1 2 3 4))
      !>([b c]=:~(1 2 3))
      !>([b c]=:*([1 2]))
      !>([b c]=:*(1 2 3))
      !>(^=(^=(d=* e) 5))
  ==
--
