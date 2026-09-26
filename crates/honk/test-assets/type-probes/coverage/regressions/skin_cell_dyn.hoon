::  Cell skin on a ref whose cell-ness is dynamic but whose halves match statically: ar gives flan([%3 %0 a] [1 &]) = [%3 %0 a].
|%
++  main
  |=  c=$@(@ [@ @])
  !>(?#([@ @] c))
--
