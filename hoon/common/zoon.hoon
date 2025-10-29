::  /lib/zoon: vendored types from hoon.hoon
/=  z  /common/zeke
~%  %zoon  ..stark-engine-jet-hook:z  ~
|%
::
+|  %no-by-in
++  by  %do-not-use
++  in  %do-not-use
++  ju  %do-not-use
++  ja  %do-not-use
++  bi  %do-not-use
::
+|  %map
++  z-map
  |$  [key value]                                       ::  table
  $|  (tree (pair key value))
  |=(a=(tree (pair)) ?:(=(~ a) & ~(apt z-by a)))
::
++  z-by                                                  ::  z-map engine
  ~/  %z-by
  =|  a=(tree (pair))  ::  (z-map)
  |@
  ++  all                                               ::  logical AND
    ~/  %all
    |*  b=$-(* ?)
    |-  ^-  ?
    ?~  a
      &
    ?&((b q.n.a) $(a l.a) $(a r.a))
  ::
  ++  any                                               ::  logical OR
    ~/  %any
    |*  b=$-(* ?)
    |-  ^-  ?
    ?~  a
      |
    ?|((b q.n.a) $(a l.a) $(a r.a))
  ::
  ++  bif                                               ::  splits a z-by b
    ~/  %bif
    |*  b=*
    |-  ^+  [l=a r=a]
    ?~  a
      [~ ~]
    ?:  =(b p.n.a)
      +.a
    ?:  (gor-tip b p.n.a)
      =+  d=$(a l.a)
      ?>  ?=(^ d)
      [l.d a(l r.d)]
    =+  d=$(a r.a)
    ?>  ?=(^ d)
    [a(r l.d) r.d]
  ::
  ++  del                                               ::  delete at key b
    ~/  %del
    |*  b=*
    |-  ^+  a
    ?~  a
      ~
    ?.  =(b p.n.a)
      ?:  (gor-tip b p.n.a)
        a(l $(a l.a))
      a(r $(a r.a))
    |-  ^-  [$?(~ _a)]
    ?~  l.a  r.a
    ?~  r.a  l.a
    ?:  (mor-tip p.n.l.a p.n.r.a)
      l.a(r $(l.a r.l.a))
    r.a(l $(r.a l.r.a))
  ::
  ++  dif                                               ::  difference
    ~/  %dif
    |*  b=_a
    |-  ^+  a
    ?~  b
      a
    =+  c=(bif p.n.b)
    ?>  ?=(^ c)
    =+  d=$(a l.c, b l.b)
    =+  e=$(a r.c, b r.b)
    |-  ^-  [$?(~ _a)]
    ?~  d  e
    ?~  e  d
    ?:  (mor-tip p.n.d p.n.e)
      d(r $(d r.d))
    e(l $(e l.e))
  ::
  ++  dig                                               ::  axis of b key
    ~/  %dig
    |=  b=*
    =+  c=1
    |-  ^-  (unit @)
    ?~  a  ~
    ?:  =(b p.n.a)  [~ u=(peg c 2)]
    ?:  (gor-tip b p.n.a)
      $(a l.a, c (peg c 6))
    $(a r.a, c (peg c 7))
  ::
  ++  apt                                               ::  check correctness
    =<  $
    =|  [l=(unit) r=(unit)]
    |.  ^-  ?
    ?~  a   &
    ?&  ?~(l & &((gor-tip p.n.a u.l) !=(p.n.a u.l)))
        ?~(r & &((gor-tip u.r p.n.a) !=(u.r p.n.a)))
        ?~  l.a   &
        &((mor-tip p.n.a p.n.l.a) !=(p.n.a p.n.l.a) $(a l.a, l `p.n.a))
        ?~  r.a   &
        &((mor-tip p.n.a p.n.r.a) !=(p.n.a p.n.r.a) $(a r.a, r `p.n.a))
    ==
  ::
  ++  gas                                               ::  concatenate
    ~/  %gas
    |*  b=(list [p=* q=*])
    =>  .(b `(list _?>(?=(^ a) n.a))`b)
    |-  ^+  a
    ?~  b
      a
    $(b t.b, a (put p.i.b q.i.b))
  ::
  ++  get                                               ::  grab value z-by key
    ~/  %get
    |*  b=*
    =>  .(b `_?>(?=(^ a) p.n.a)`b)
    |-  ^-  (unit _?>(?=(^ a) q.n.a))
    ?~  a
      ~
    ?:  =(b p.n.a)
      (some q.n.a)
    ?:  (gor-tip b p.n.a)
      $(a l.a)
    $(a r.a)
  ::
  ++  got                                               ::  need value z-by key
    ~/  %got
    |*  b=*
    (need (get b))
  ::
  ++  gut                                               ::  fall value z-by key
    ~/  %gut
    |*  [b=* c=*]
    (fall (get b) c)
  ::
  ++  has                                               ::  key existence check
    ~/  %has
    |*  b=*
    !=(~ (get b))
  ::
  ++  int                                               ::  intersection
    ~/  %int
    |*  b=_a
    |-  ^+  a
    ?~  b
      ~
    ?~  a
      ~
    ?:  (mor-tip p.n.a p.n.b)
      ?:  =(p.n.b p.n.a)
        b(l $(a l.a, b l.b), r $(a r.a, b r.b))
      ?:  (gor-tip p.n.b p.n.a)
        %-  uni(a $(a l.a, r.b ~))  $(b r.b)
      %-  uni(a $(a r.a, l.b ~))  $(b l.b)
    ?:  =(p.n.a p.n.b)
      b(l $(b l.b, a l.a), r $(b r.b, a r.a))
    ?:  (gor-tip p.n.a p.n.b)
      %-  uni(a $(b l.b, r.a ~))  $(a r.a)
    %-  uni(a $(b r.b, l.a ~))  $(a l.a)
  ::
  ++  jab
    ~/  %jab
    |*  [key=_?>(?=(^ a) p.n.a) fun=$-(_?>(?=(^ a) q.n.a) _?>(?=(^ a) q.n.a))]
    ^+  a
    ::
    ?~  a  !!
    ::
    ?:  =(key p.n.a)
      a(q.n (fun q.n.a))
    ::
    ?:  (gor-tip key p.n.a)
      a(l $(a l.a))
    ::
    a(r $(a r.a))
  ::
  ++  mar                                               ::  add with validation
    ~/  %mar
    |*  [b=* c=(unit *)]
    ?~  c
      (del b)
    (put b u.c)
  ::
  ++  put                                               ::  adds key-value pair
    ~/  %put
    |*  [b=* c=*]
    |-  ^+  a
    ?~  a
      [[b c] ~ ~]
    ?:  =(b p.n.a)
      ?:  =(c q.n.a)
        a
      a(n [b c])
    ?:  (gor-tip b p.n.a)
      =+  d=$(a l.a)
      ?>  ?=(^ d)
      ?:  (mor-tip p.n.a p.n.d)
        a(l d)
      d(r a(l r.d))
    =+  d=$(a r.a)
    ?>  ?=(^ d)
    ?:  (mor-tip p.n.a p.n.d)
      a(r d)
    d(l a(r l.d))
  ::
  ++  rep                                               ::  reduce to product
    ~/  %rep
    |*  b=_=>(~ |=([* *] +<+))
    |-
    ?~  a  +<+.b
    $(a r.a, +<+.b $(a l.a, +<+.b (b n.a +<+.b)))
  ::
  ++  rib                                               ::  transform + product
    ~/  %rib
    |*  [b=* c=gate]
    |-  ^+  [b a]
    ?~  a  [b ~]
    =+  d=(c n.a b)
    =.  n.a  +.d
    =+  e=$(a l.a, b -.d)
    =+  f=$(a r.a, b -.e)
    [-.f a(l +.e, r +.f)]
  ::
  ++  run                                               ::  apply gate to values
    ~/  %run
    |*  b=gate
    |-
    ?~  a  a
    [n=[p=p.n.a q=(b q.n.a)] l=$(a l.a) r=$(a r.a)]
  ::
  ++  tap                                               ::  listify pairs
    =<  $
    =+  b=`(list _?>(?=(^ a) n.a))`~
    |.  ^+  b
    ?~  a
      b
    $(a r.a, b [n.a $(a l.a)])
  ::
  ++  uni                                               ::  union, merge
    ~/  %uni
    |*  b=_a
    |-  ^+  a
    ?~  b
      a
    ?~  a
      b
    ?:  =(p.n.b p.n.a)
      b(l $(a l.a, b l.b), r $(a r.a, b r.b))
    ?:  (mor-tip p.n.a p.n.b)
      ?:  (gor-tip p.n.b p.n.a)
        $(l.a $(a l.a, r.b ~), b r.b)
      $(r.a $(a r.a, l.b ~), b l.b)
    ?:  (gor-tip p.n.a p.n.b)
      $(l.b $(b l.b, r.a ~), a r.a)
    $(r.b $(b r.b, l.a ~), a l.a)
  ::
  ++  uno                                               ::  general union
    ~/  %uno
    |*  b=_a
    |*  meg=$-([* * *] *)
    |-  ^+  a
    ?~  b
      a
    ?~  a
      b
    ?:  =(p.n.b p.n.a)
      :+  [p.n.a `_?>(?=(^ a) q.n.a)`(meg p.n.a q.n.a q.n.b)]
        $(b l.b, a l.a)
      $(b r.b, a r.a)
    ?:  (mor-tip p.n.a p.n.b)
      ?:  (gor-tip p.n.b p.n.a)
        $(l.a $(a l.a, r.b ~), b r.b)
      $(r.a $(a r.a, l.b ~), b l.b)
    ?:  (gor-tip p.n.a p.n.b)
      $(l.b $(b l.b, r.a ~), a r.a)
    $(r.b $(b r.b, l.a ~), a l.a)
  ::
  ++  urn                                               ::  apply gate to nodes
    ~/  %urn
    |*  b=$-([* *] *)
    |-
    ?~  a  ~
    a(n n.a(q (b p.n.a q.n.a)), l $(a l.a), r $(a r.a))
  ::
  ++  wyt                                               ::  depth of z-map
    =<  $
    |.  ^-  @
    ?~(a 0 +((add $(a l.a) $(a r.a))))
  ::
  ++  key                                               ::  z-set of keys
    |-  ^-  (z-set _?>(?=(^ a) p.n.a))
    ?~  a  ~
    [p.n.a $(a l.a) $(a r.a)]
  ::
  ++  val                                               ::  list of vals
    =+  b=`(list _?>(?=(^ a) q.n.a))`~
    |-  ^+  b
    ?~  a   b
    $(a r.a, b [q.n.a $(a l.a)])
  --
+|  %set
++  z-set
  |$  [item]                                            ::  z-set
  $|  (tree item)
  |=(a=(tree) ?:(=(~ a) & ~(apt z-in a)))
::
++  z-in                                                  ::  z-set engine
  ~/  %z-in
  =|  a=(tree)  :: (z-set)
  |@
  ++  all                                               ::  logical AND
    ~/  %all
    |*  b=$-(* ?)
    |-  ^-  ?
    ?~  a
      &
    ?&((b n.a) $(a l.a) $(a r.a))
  ::
  ++  any                                               ::  logical OR
    ~/  %any
    |*  b=$-(* ?)
    |-  ^-  ?
    ?~  a
      |
    ?|((b n.a) $(a l.a) $(a r.a))
  ::
  ++  apt                                               ::  check correctness
    =<  $
    =|  [l=(unit) r=(unit)]
    |.  ^-  ?
    ?~  a   &
    ?&  ?~(l & &((gor-tip n.a u.l) !=(n.a u.l)))
        ?~(r & &((gor-tip u.r n.a) !=(u.r n.a)))
        ?~(l.a & ?&((mor-tip n.a n.l.a) !=(n.a n.l.a) $(a l.a, l `n.a)))
        ?~(r.a & ?&((mor-tip n.a n.r.a) !=(n.a n.r.a) $(a r.a, r `n.a)))
    ==
  ::
  ++  bif                                               ::  splits a by b
    ~/  %bif
    |*  b=*
    ^+  [l=a r=a]
    =<  +
    |-  ^+  a
    ?~  a
      [b ~ ~]
    ?:  =(b n.a)
      a
    ?:  (gor-tip b n.a)
      =+  c=$(a l.a)
      ?>  ?=(^ c)
      c(r a(l r.c))
    =+  c=$(a r.a)
    ?>  ?=(^ c)
    c(l a(r l.c))
  ::
  ++  del                                               ::  b without any a
    ~/  %del
    |*  b=*
    |-  ^+  a
    ?~  a
      ~
    ?.  =(b n.a)
      ?:  (gor-tip b n.a)
        a(l $(a l.a))
      a(r $(a r.a))
    |-  ^-  [$?(~ _a)]
    ?~  l.a  r.a
    ?~  r.a  l.a
    ?:  (mor-tip n.l.a n.r.a)
      l.a(r $(l.a r.l.a))
    r.a(l $(r.a l.r.a))
  ::
  ++  dif                                              ::  difference
    ~/  %dif
    |*  b=_a
    |-  ^+  a
    ?~  b
      a
    =+  c=(bif n.b)
    ?>  ?=(^ c)
    =+  d=$(a l.c, b l.b)
    =+  e=$(a r.c, b r.b)
    |-  ^-  [$?(~ _a)]
    ?~  d  e
    ?~  e  d
    ?:  (mor-tip n.d n.e)
      d(r $(d r.d))
    e(l $(e l.e))
  ::
  ++  dig                                               ::  axis of a z-in b
    ~/  %dig
    |=  b=*
    =+  c=1
    |-  ^-  (unit @)
    ?~  a  ~
    ?:  =(b n.a)  [~ u=(peg c 2)]
    ?:  (gor-tip b n.a)
      $(a l.a, c (peg c 6))
    $(a r.a, c (peg c 7))
  ::
  ++  gas                                               ::  concatenate
    ~/  %gas
    |=  b=(list _?>(?=(^ a) n.a))
    |-  ^+  a
    ?~  b
      a
    $(b t.b, a (put i.b))
  ::  +has: does :b exist z-in :a?
  ::
  ++  has
    ~/  %has
    |*  b=*
    ^-  ?
    ::    wrap extracted item type z-in a unit because bunting fails
    ::
    ::  If we used the real item type of _?^(a n.a !!) as the sample type,
    ::  then hoon would bunt it to create the default sample for the gate.
    ::
    ::  However, bunting that expression fails if :a is ~. If we wrap it
    ::  z-in a unit, the bunted unit doesn't include the bunted item type.
    ::
    ::  This way we can ensure type safety of :b without needing to perform
    ::  this failing bunt. It's a hack.
    ::
    %.  [~ b]
    |=  b=(unit _?>(?=(^ a) n.a))
    =>  .(b ?>(?=(^ b) u.b))
    |-  ^-  ?
    ?~  a
      |
    ?:  =(b n.a)
      &
    ?:  (gor-tip b n.a)
      $(a l.a)
    $(a r.a)
  ::
  ++  int                                               ::  intersection
    ~/  %int
    |*  b=_a
    |-  ^+  a
    ?~  b
      ~
    ?~  a
      ~
    ?.  (mor-tip n.a n.b)
      $(a b, b a)
    ?:  =(n.b n.a)
      a(l $(a l.a, b l.b), r $(a r.a, b r.b))
    ?:  (gor-tip n.b n.a)
      %-  uni(a $(a l.a, r.b ~))  $(b r.b)
    %-  uni(a $(a r.a, l.b ~))  $(b l.b)
  ::
  ++  put                                               ::  puts b z-in a, sorted
    ~/  %put
    |*  b=*
    |-  ^+  a
    ?~  a
      [b ~ ~]
    ?:  =(b n.a)
      a
    ?:  (gor-tip b n.a)
      =+  c=$(a l.a)
      ?>  ?=(^ c)
      ?:  (mor-tip n.a n.c)
        a(l c)
      c(r a(l r.c))
    =+  c=$(a r.a)
    ?>  ?=(^ c)
    ?:  (mor-tip n.a n.c)
      a(r c)
    c(l a(r l.c))
  ::
  ++  rep                                               ::  reduce to product
    ~/  %rep
    |*  b=_=>(~ |=([* *] +<+))
    |-
    ?~  a  +<+.b
    $(a r.a, +<+.b $(a l.a, +<+.b (b n.a +<+.b)))
  ::
  ++  run                                               ::  apply gate to values
    ~/  %run
    |*  b=gate
    =+  c=`(z-set _?>(?=(^ a) (b n.a)))`~
    |-  ?~  a  c
    =.  c  (~(put z-in c) (b n.a))
    =.  c  $(a l.a, c c)
    $(a r.a, c c)
  ::
  ++  tap                                               ::  convert to list
    =<  $
    =+  b=`(list _?>(?=(^ a) n.a))`~
    |.  ^+  b
    ?~  a
      b
    $(a r.a, b [n.a $(a l.a)])
  ::
  ++  uni                                               ::  union
    ~/  %uni
    |*  b=_a
    ?:  =(a b)  a
    |-  ^+  a
    ?~  b
      a
    ?~  a
      b
    ?:  =(n.b n.a)
      b(l $(a l.a, b l.b), r $(a r.a, b r.b))
    ?:  (mor-tip n.a n.b)
      ?:  (gor-tip n.b n.a)
        $(l.a $(a l.a, r.b ~), b r.b)
      $(r.a $(a r.a, l.b ~), b l.b)
    ?:  (gor-tip n.a n.b)
      $(l.b $(b l.b, r.a ~), a r.a)
    $(r.b $(b r.b, l.a ~), a l.a)
  ::
  ++  wyt                                               ::  size of z-set
    =<  $
    |.  ^-  @
    ?~(a 0 +((add $(a l.a) $(a r.a))))
  --
+|  %mip
::
++  z-mip                                                 ::  map of maps
  |$  [kex key value]
  (z-map kex (z-map key value))
::
++  z-bi                                                  ::  mip engine
  =|  a=(z-map * (z-map))
  |@
  ++  del
    |*  [b=* c=*]
    =+  d=(~(gut z-by a) b ~)
    =+  e=(~(del z-by d) c)
    ?~  e
      (~(del z-by a) b)
    (~(put z-by a) b e)
  ::
  ++  get
    |*  [b=* c=*]
    =>  .(b `_?>(?=(^ a) p.n.a)`b, c `_?>(?=(^ a) ?>(?=(^ q.n.a) p.n.q.n.a))`c)
    ^-  (unit _?>(?=(^ a) ?>(?=(^ q.n.a) q.n.q.n.a)))
    (~(get z-by (~(gut z-by a) b ~)) c)
  ::
  ++  got
    |*  [b=* c=*]
    (need (get b c))
  ::
  ++  gut
    |*  [b=* c=* d=*]
    (~(gut z-by (~(gut z-by a) b ~)) c d)
  ::
  ++  has
    |*  [b=* c=*]
    !=(~ (get b c))
  ::
  ++  key
    |*  b=*
    ~(key z-by (~(gut z-by a) b ~))
  ::
  ++  put
    |*  [b=* c=* d=*]
    %+  ~(put z-by a)  b
    %.  [c d]
    %~  put  z-by
    (~(gut z-by a) b ~)
  ::
  ++  tap
    ::NOTE  naive turn-based implementation find-errors ):
    =<  $
    =+  b=`_?>(?=(^ a) *(list [x=_p.n.a _?>(?=(^ q.n.a) [y=p v=q]:n.q.n.a)]))`~
    |.  ^+  b
    ?~  a
      b
    $(a r.a, b (welp (turn ~(tap z-by q.n.a) (lead p.n.a)) $(a l.a)))
  --
::
+|  %jug
::
++  z-jug
  |$  [key value]
  (z-map key (z-set value))
::
++  z-ju                                                ::  z-jug engine
  =|  a=(tree (pair * (tree)))                          ::  (z-jug)
  |@
  ++  del                                               ::  del key-set pair
    |*  [b=* c=*]
    ^+  a
    =+  d=(get b)
    =+  e=(~(del z-in d) c)
    ?~  e
      (~(del z-by a) b)
    (~(put z-by a) b e)
  ::
  ++  gas                                               ::  concatenate
    |*  b=(list [p=* q=*])
    =>  .(b `(list _?>(?=([[* ^] ^] a) [p=p q=n.q]:n.a))`b)
    |-  ^+  a
    ?~  b
      a
    $(b t.b, a (put p.i.b q.i.b))
  ::
  ++  get                                               ::  gets z-set by key
    |*  b=*
    =+  c=(~(get z-by a) b)
    ?~(c ~ u.c)
  ::
  ++  has                                               ::  existence check
    |*  [b=* c=*]
    ^-  ?
    (~(has z-in (get b)) c)
  ::
  ++  put                                               ::  add key-z-set pair
    |*  [b=* c=*]
    ^+  a
    =+  d=(get b)
    (~(put z-by a) b (~(put z-in d) c))
  --
::
+|  %ordering
::  +dor-tip: depth order.
::
::    Orders z-in ascending tree depth.
::
++  dor-tip
  ~/  %dor-tip
  |=  [a=* b=*]
  ^-  ?
  ?:  =(a b)  &
  ?.  ?=(@ a)
    ?:  ?=(@ b)  |
    ?:  =(-.a -.b)
      $(a +.a, b +.b)
    $(a -.a, b -.b)
  ?.  ?=(@ b)  &
  (lth a b)
::  +gor-tip: tip order.
::
::    Orders z-in ascending +tip hash order, collisions fall back to +dor.
::
++  gor-tip
  ~/  %gor-tip
  |=  [a=* b=*]
  ^-  ?
  =+  [c=(tip a) d=(tip b)]
  ?:  =(c d)
    (dor-tip a b)
  (lth-tip c d)
::  +mor-tip: mor tip order.
::
::    Orders z-in ascending double +tip hash order, collisions fall back to +dor.
::
++  mor-tip
  ~/  %mor-tip
  |=  [a=* b=*]
  ^-  ?
  =+  [c=(double-tip a) d=(double-tip b)]
  ?:  =(c d)
    (dor-tip a b)
  (lth-tip c d)
::
++  tip
  |=  a=*
  ^-  noun-digest:tip5:z
  (hash-noun-varlen:tip5:z a)
::
++  double-tip
  |=  a=*
  ^-  noun-digest:tip5:z
  =/  one  (tip a)
  (hash-ten-cell:tip5:z one one)
::
++  lth-tip
  |=  [a=noun-digest:tip5:z b=noun-digest:tip5:z]
  %+  lth
    (digest-to-atom:tip5:z a)
  (digest-to-atom:tip5:z b)
::
+|   %z-container-from-container
  ++  z-silt                                              :: z-set from list
    |*  a=(list)
    =+  b=*(tree _?>(?=(^ a) i.a))
    (~(gas z-in b) a)
  ::
  ++  z-molt                                              :: z-map from pair
      |*  a=(list (pair))
      (~(gas z-by `(tree [p=_p.i.-.a q=_q.i.-.a])`~) a)
  ::
  ++  z-malt                                              ::  z-map from list
  |*  a=(list)
  (z-molt `(list [p=_-<.a q=_->.a])`a)
--
