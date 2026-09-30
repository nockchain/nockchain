::  c2: core nesting against fork goals where a losing core option may be
::  tried first: wet cores with unequal tomes (nest_core ~8994-8998; spots are
::  off under !. so equal wet cores can match), and a payload edit that breaks
::  the value's own context (nest_core ~8966-8975); plus ?# on an alias
::  (synthetic port) in play position, where chip keeps the subject (~9605),
::  and a ^~ fold of a sibling arm inside a wet core (lazy resolver, wet vet).
!.
|%
++  w0
  |=  b=?
  ^+  ?:(b |*(a=@ +(a)) |*(a=@ a))
  |*(a=@ a)
++  w1
  |=  b=?
  ^+  ?:(b |*(a=@ a) |*(a=@ +(a)))
  |*(a=@ a)
++  w2
  |=  b=?
  ^+  ?:(b |*(a=@ [a a]) |*(a=@ a))
  |*(a=@ a)
++  w3
  |=  b=?
  ^+  ?:(b |*(a=@ a) |*(a=@ [a a]))
  |*(a=@ a)
++  w4
  |=  b=?
  ^+  ?:(b |*(a=@ a) |*(a=@ a))
  |*(a=@ a)
++  w5
  |=  b=?
  ^+  ?:(b |*(a=@ a) |*(a=@ a))
  |*(a=@ a)
++  w6
  |=  b=?
  ^+  ?:(b |*(a=@ (add a 1)) |*(a=@ a))
  |*(a=@ a)
++  w7
  |=  b=?
  ^+  ?:(b |*(a=@ a) |*(a=@ (add a 1)))
  |*(a=@ a)
++  w8
  |=  b=?
  ^+  ?:(b |*(a=@ ?:(=(a 0) 1 a)) |*(a=@ a))
  |*(a=@ a)
++  w9
  |=  b=?
  ^+  ?:(b |*(a=@ a) |*(a=@ ?:(=(a 0) 1 a)))
  |*(a=@ a)
++  w10
  |=  b=?
  ^+  ?:(b |*(a=@ [a 0]) |*(a=@ a))
  |*(a=@ a)
++  w11
  |=  b=?
  ^+  ?:(b |*(a=@ a) |*(a=@ [a 0]))
  |*(a=@ a)
++  e0
  |=  b=?
  =/  g  |=(a=@ +(a))
  ^+  ?:(b g `[* *]`[0 0])
  g(a [1 2])
++  e1
  |=  b=?
  =/  g  |=(a=@ +(a))
  ^+  ?:(b `[* *]`[0 0] g)
  g(a [1 2])
++  e2
  |=  b=?
  =/  g  |=(a=@ [a a])
  ^+  ?:(b g `[* *]`[0 0])
  g(a [1 2])
++  e3
  |=  b=?
  =/  g  |=(a=@ [a a])
  ^+  ?:(b `[* *]`[0 0] g)
  g(a [1 2])
++  e4
  |=  b=?
  =/  g  |=(a=@ a)
  ^+  ?:(b g `[* *]`[0 0])
  g(a [1 2])
++  e5
  |=  b=?
  =/  g  |=(a=@ a)
  ^+  ?:(b `[* *]`[0 0] g)
  g(a [1 2])
++  e6
  |=  b=?
  =/  g  |=(a=@ (add a 1))
  ^+  ?:(b g `[* *]`[0 0])
  g(a [1 2])
++  e7
  |=  b=?
  =/  g  |=(a=@ (add a 1))
  ^+  ?:(b `[* *]`[0 0] g)
  g(a [1 2])
++  e8
  |=  b=?
  =/  g  |=(a=@ ?:(=(a 0) 1 a))
  ^+  ?:(b g `[* *]`[0 0])
  g(a [1 2])
++  e9
  |=  b=?
  =/  g  |=(a=@ ?:(=(a 0) 1 a))
  ^+  ?:(b `[* *]`[0 0] g)
  g(a [1 2])
++  e10
  |=  b=?
  =/  g  |=(a=@ [a 0])
  ^+  ?:(b g `[* *]`[0 0])
  g(a [1 2])
++  e11
  |=  b=?
  =/  g  |=(a=@ [a 0])
  ^+  ?:(b `[* *]`[0 0] g)
  g(a [1 2])
++  h0
  |=  b=?
  =/  g  |=(a=@ +(a))
  =/  h  |=(a=@ 7)
  ^+  ?:(b h `[* *]`[0 0])
  g(a [1 2])
++  h1
  |=  b=?
  =/  g  |=(a=@ +(a))
  =/  h  |=(a=@ 7)
  ^+  ?:(b `[* *]`[0 0] h)
  g(a [1 2])
++  h2
  |=  b=?
  =/  g  |=(a=@ [a a])
  =/  h  |=(a=@ 7)
  ^+  ?:(b h `[* *]`[0 0])
  g(a [1 2])
++  h3
  |=  b=?
  =/  g  |=(a=@ [a a])
  =/  h  |=(a=@ 7)
  ^+  ?:(b `[* *]`[0 0] h)
  g(a [1 2])
++  h4
  |=  b=?
  =/  g  |=(a=@ a)
  =/  h  |=(a=@ 7)
  ^+  ?:(b h `[* *]`[0 0])
  g(a [1 2])
++  h5
  |=  b=?
  =/  g  |=(a=@ a)
  =/  h  |=(a=@ 7)
  ^+  ?:(b `[* *]`[0 0] h)
  g(a [1 2])
++  h6
  |=  b=?
  =/  g  |=(a=@ (add a 1))
  =/  h  |=(a=@ 7)
  ^+  ?:(b h `[* *]`[0 0])
  g(a [1 2])
++  h7
  |=  b=?
  =/  g  |=(a=@ (add a 1))
  =/  h  |=(a=@ 7)
  ^+  ?:(b `[* *]`[0 0] h)
  g(a [1 2])
++  h8
  |=  b=?
  =/  g  |=(a=@ ?:(=(a 0) 1 a))
  =/  h  |=(a=@ 7)
  ^+  ?:(b h `[* *]`[0 0])
  g(a [1 2])
++  h9
  |=  b=?
  =/  g  |=(a=@ ?:(=(a 0) 1 a))
  =/  h  |=(a=@ 7)
  ^+  ?:(b `[* *]`[0 0] h)
  g(a [1 2])
++  h10
  |=  b=?
  =/  g  |=(a=@ [a 0])
  =/  h  |=(a=@ 7)
  ^+  ?:(b h `[* *]`[0 0])
  g(a [1 2])
++  h11
  |=  b=?
  =/  g  |=(a=@ [a 0])
  =/  h  |=(a=@ 7)
  ^+  ?:(b `[* *]`[0 0] h)
  g(a [1 2])
++  lazy
  |@
  ++  x  5
  ++  y  ^~(x)
  --
++  main
  |=  b=?
  =*  foo  [a=1 c=2]
  :*  !>(^+(?:(?#(@ a.foo) 1 2) 3))
      (w0 b)
      (w1 b)
      (w2 b)
      (w3 b)
      (w4 b)
      (w5 b)
      (w6 b)
      (w7 b)
      (w8 b)
      (w9 b)
      (w10 b)
      (w11 b)
      (e0 b)
      (e1 b)
      (e2 b)
      (e3 b)
      (e4 b)
      (e5 b)
      (e6 b)
      (e7 b)
      (e8 b)
      (e9 b)
      (e10 b)
      (e11 b)
      (h0 b)
      (h1 b)
      (h2 b)
      (h3 b)
      (h4 b)
      (h5 b)
      (h6 b)
      (h7 b)
      (h8 b)
      (h9 b)
      (h10 b)
      (h11 b)
      lazy
  ==
--
