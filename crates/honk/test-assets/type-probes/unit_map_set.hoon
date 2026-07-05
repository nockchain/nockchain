::  Container doors: map/set/unit products through by/in door calls.
|%
++  main
  |=  [m=(map @tas @ud) s=(set @t)]
  [!>((~(get by m) %a)) !>((~(put in s) 'x')) !>((need (some 7)))]
--
