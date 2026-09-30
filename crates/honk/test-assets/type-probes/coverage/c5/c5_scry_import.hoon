::  c5: formula_dag.rs import (~242) of a .^ type hint whose quoted noun
::  already exists as an unmaterialized folded constant, so interning hits it
::  and backfills its materialization (~163).
|%
++  main
  |=  c=*
  :*  ^~([138 %atom 0 0])
      .^(@ %cx /foo)
      .^(@ %cx /bar)
      !>(.^(@ud %cx /baz))
  ==
--
