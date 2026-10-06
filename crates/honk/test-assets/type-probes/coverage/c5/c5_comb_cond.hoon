::  c5: formula_dag comb/cond peepholes (formula_dag.rs comb ~491, cond ~527):
::  [0 a] composed with [2 [0 x] [0 y]], with non-slot and [0 0] eval
::  operands, crash formulas on either side, and ?: tests that fold.
::  `!=` pins the formulas of void-typed expressions, which `!>` cannot wrap.
|%
++  vd  !!
++  main
  |=  c=[p=* q=*]
  :*  !>(=>(c .*(p q)))
      !>(=>(c .*(- +)))
      !>(=>(c .*(p 0)))
      !=(=>(c .*(!! q)))
      !=(=>(c .*(p !!)))
      !=(=>(c !!))
      !=(=>(!! 5))
      !=(=>(!! ?:(& 1 2)))
      !>(=>(c .*(- .*(+ -))))
      !=(=>(c .*(p q)))
      !=(=+(d=vd ?:(?=(@ d) 1 2)))
      !=(=+(d=vd ?:(?=(^ d) 1 2)))
      !=(=+(d=`@`vd ?:(?=(^ d) 1 2)))
  ==
--
