::  ?= products (cube bools) and ?& chained refinement threading.
|%
++  main
  |=  n=$@(~ [p=@ q=$@(~ [r=@ s=@])])
  [!>(?=(~ n)) !>(?&(?=(^ n) ?=(^ q.n) =(r.q.n 1)))]
--
