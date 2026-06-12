::  Minimal extraction from hoon-138 `++ff`/`++sea` around the current
::  `native mint: fire core` failure in the `sum:si` path.
::
::  Focuses on the nested `?:` flow and `=+ q=:(sum:si (sun:si e) me -1)`.
::
|%
++  ff
  |_  [[w=@u p=@u b=@s] r=@]
  ++  me  (dif:si (dif:si --1 b) (sun:si p))
  ++  sea
    |=  [a=@]
    =+  [f=(cut 0 [0 p] a) e=(cut 0 [p w] a)]
    =+  s=(sig a)
    ?:  =(e 0)
      ?:  =(f 0)  [%f s --0 0]  [%f s me f]
    ?:  =(e (fil 0 w 1))
      ?:  =(f 0)  [%i s]  [%n ~]
    =+  q=:(sum:si (sun:si e) me -1)
    =+  r=(add f (bex p))
    [%f s q r]
  ++  sig
    |=  [a=@]
    ^-  ?
    =(0 (cut 0 [(add p w) 1] a))
  --
--
