::  /lib/zoon: vendored types from hoon.hoon
/=  *  /common/zoon
~%  %h-zoon  ..stark-engine-jet-hook:z  ~
|%
::
+|  %no-by-in
++  by  %do-not-use
++  in  %do-not-use
++  ju  %do-not-use
++  ja  %do-not-use
++  bi  %do-not-use
+$  hashed
  $?  noun-digest:tip5:z
      noun-digests:z
  ==
::
+|  %map
++  h-map
  |$  [key value]                                       ::  table
  $|  (tree (pair key value))
  |=(a=(tree (pair hashed *)) ?:(=(~ a) & ~(apt h-by a)))
::
++  h-by                                                  ::  h-map engine
  ~/  %h-by
  =|  a=(tree (pair hashed *))  ::  (h-map)
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
  ++  bif                                               ::  splits a h-by b
    ~/  %bif
    |*  b=*
    |-  ^+  [l=a r=a]
    ?~  a
      [~ ~]
    ?:  =(b p.n.a)
      +.a
    ?:  (gor-hip b p.n.a)
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
      ?:  (gor-hip b p.n.a)
        a(l $(a l.a))
      a(r $(a r.a))
    |-  ^-  [$?(~ _a)]
    ?~  l.a  r.a
    ?~  r.a  l.a
    ?:  (mor-hip p.n.l.a p.n.r.a)
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
    ?:  (mor-hip p.n.d p.n.e)
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
    ?:  (gor-hip b p.n.a)
      $(a l.a, c (peg c 6))
    $(a r.a, c (peg c 7))
  ::
  ++  apt                                               ::  check correctness
    =<  $
    =|  [l=(unit hashed) r=(unit hashed)]
    |.  ^-  ?
    ?~  a   &
    ?&  ?~(l & &((gor-hip p.n.a u.l) !=(p.n.a u.l)))
        ?~(r & &((gor-hip u.r p.n.a) !=(u.r p.n.a)))
        ?~  l.a   &
        &((mor-hip p.n.a p.n.l.a) !=(p.n.a p.n.l.a) $(a l.a, l `p.n.a))
        ?~  r.a   &
        &((mor-hip p.n.a p.n.r.a) !=(p.n.a p.n.r.a) $(a r.a, r `p.n.a))
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
  ++  get                                               ::  grab value h-by key
    ~/  %get
    |*  b=*
    =>  .(b `_?>(?=(^ a) p.n.a)`b)
    |-  ^-  (unit _?>(?=(^ a) q.n.a))
    ?~  a
      ~
    ?:  =(b p.n.a)
      (some q.n.a)
    ?:  (gor-hip b p.n.a)
      $(a l.a)
    $(a r.a)
  ::
  ++  got                                               ::  need value h-by key
    ~/  %got
    |*  b=*
    (need (get b))
  ::
  ++  gut                                               ::  fall value h-by key
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
    ?:  (mor-hip p.n.a p.n.b)
      ?:  =(p.n.b p.n.a)
        b(l $(a l.a, b l.b), r $(a r.a, b r.b))
      ?:  (gor-hip p.n.b p.n.a)
        %-  uni(a $(a l.a, r.b ~))  $(b r.b)
      %-  uni(a $(a r.a, l.b ~))  $(b l.b)
    ?:  =(p.n.a p.n.b)
      b(l $(b l.b, a l.a), r $(b r.b, a r.a))
    ?:  (gor-hip p.n.a p.n.b)
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
    ?:  (gor-hip key p.n.a)
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
    ?:  (gor-hip b p.n.a)
      =+  d=$(a l.a)
      ?>  ?=(^ d)
      ?:  (mor-hip p.n.a p.n.d)
        a(l d)
      d(r a(l r.d))
    =+  d=$(a r.a)
    ?>  ?=(^ d)
    ?:  (mor-hip p.n.a p.n.d)
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
    ?:  (mor-hip p.n.a p.n.b)
      ?:  (gor-hip p.n.b p.n.a)
        $(l.a $(a l.a, r.b ~), b r.b)
      $(r.a $(a r.a, l.b ~), b l.b)
    ?:  (gor-hip p.n.a p.n.b)
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
    ?:  (mor-hip p.n.a p.n.b)
      ?:  (gor-hip p.n.b p.n.a)
        $(l.a $(a l.a, r.b ~), b r.b)
      $(r.a $(a r.a, l.b ~), b l.b)
    ?:  (gor-hip p.n.a p.n.b)
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
  ++  wyt                                               ::  depth of h-map
    =<  $
    |.  ^-  @
    ?~(a 0 +((add $(a l.a) $(a r.a))))
  ::
  ++  key                                               ::  h-set of keys
    |-  ^-  (h-set _?>(?=(^ a) p.n.a))
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
++  h-set
  |$  [item]                                            ::  h-set
  $|  (tree item)
  |=(a=(tree item) ?:(=(~ a) & ~(apt h-in a)))
::
++  h-in                                                  ::  h-set engine
  ~/  %h-in
  =|  a=(tree hashed)  :: (h-set)
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
    =|  [l=(unit hashed) r=(unit hashed)]
    |.  ^-  ?
    ?~  a   &
    ?&  ?~(l & &((gor-hip n.a u.l) !=(n.a u.l)))
        ?~(r & &((gor-hip u.r n.a) !=(u.r n.a)))
        ?~(l.a & ?&((mor-hip n.a n.l.a) !=(n.a n.l.a) $(a l.a, l `n.a)))
        ?~(r.a & ?&((mor-hip n.a n.r.a) !=(n.a n.r.a) $(a r.a, r `n.a)))
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
    ?:  (gor-hip b n.a)
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
      ?:  (gor-hip b n.a)
        a(l $(a l.a))
      a(r $(a r.a))
    |-  ^-  [$?(~ _a)]
    ?~  l.a  r.a
    ?~  r.a  l.a
    ?:  (mor-hip n.l.a n.r.a)
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
    ?:  (mor-hip n.d n.e)
      d(r $(d r.d))
    e(l $(e l.e))
  ::
  ++  dig                                               ::  axis of a h-in b
    ~/  %dig
    |=  b=*
    =+  c=1
    |-  ^-  (unit @)
    ?~  a  ~
    ?:  =(b n.a)  [~ u=(peg c 2)]
    ?:  (gor-hip b n.a)
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
  ::  +has: does :b exist h-in :a?
  ::
  ++  has
    ~/  %has
    |*  b=*
    ^-  ?
    ::    wrap extracted item type h-in a unit because bunting fails
    ::
    ::  If we used the real item type of _?^(a n.a !!) as the sample type,
    ::  then hoon would bunt it to create the default sample for the gate.
    ::
    ::  However, bunting that expression fails if :a is ~. If we wrap it
    ::  h-in a unit, the bunted unit doesn't include the bunted item type.
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
    ?:  (gor-hip b n.a)
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
    ?.  (mor-hip n.a n.b)
      $(a b, b a)
    ?:  =(n.b n.a)
      a(l $(a l.a, b l.b), r $(a r.a, b r.b))
    ?:  (gor-hip n.b n.a)
      %-  uni(a $(a l.a, r.b ~))  $(b r.b)
    %-  uni(a $(a r.a, l.b ~))  $(b l.b)
  ::
  ++  put                                               ::  puts b h-in a, sorted
    ~/  %put
    |*  b=hashed
    |-  ^+  a
    ?~  a
      [b ~ ~]
    ?:  =(b n.a)
      a
    ?:  (gor-hip b n.a)
      =+  c=$(a l.a)
      ?>  ?=(^ c)
      ?:  (mor-hip n.a n.c)
        a(l c)
      c(r a(l r.c))
    =+  c=$(a r.a)
    ?>  ?=(^ c)
    ?:  (mor-hip n.a n.c)
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
    =+  c=`(h-set _?>(?=(^ a) (b n.a)))`~
    |-  ?~  a  c
    =.  c  (~(put h-in c) (b n.a))
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
    ?:  (mor-hip n.a n.b)
      ?:  (gor-hip n.b n.a)
        $(l.a $(a l.a, r.b ~), b r.b)
      $(r.a $(a r.a, l.b ~), b l.b)
    ?:  (gor-hip n.a n.b)
      $(l.b $(b l.b, r.a ~), a r.a)
    $(r.b $(b r.b, l.a ~), a l.a)
  ::
  ++  wyt                                               ::  size of h-set
    =<  $
    |.  ^-  @
    ?~(a 0 +((add $(a l.a) $(a r.a))))
  --
+|  %mip
::
++  h-mip                                                 ::  map of maps
  |$  [kex key value]
  (h-map kex (h-map key value))
::
++  h-bi                                                  ::  mip engine
  =|  a=(h-map hashed (h-map hashed *))
  |@
  ++  del
    |*  [b=* c=*]
    =+  d=(~(gut h-by a) b ~)
    =+  e=(~(del h-by d) c)
    ?~  e
      (~(del h-by a) b)
    (~(put h-by a) b e)
  ::
  ++  get
    |*  [b=* c=*]
    =>  .(b `_?>(?=(^ a) p.n.a)`b, c `_?>(?=(^ a) ?>(?=(^ q.n.a) p.n.q.n.a))`c)
    ^-  (unit _?>(?=(^ a) ?>(?=(^ q.n.a) q.n.q.n.a)))
    (~(get h-by (~(gut h-by a) b ~)) c)
  ::
  ++  got
    |*  [b=* c=*]
    (need (get b c))
  ::
  ++  gut
    |*  [b=* c=* d=*]
    (~(gut h-by (~(gut h-by a) b ~)) c d)
  ::
  ++  has
    |*  [b=* c=*]
    !=(~ (get b c))
  ::
  ++  key
    |*  b=*
    ~(key h-by (~(gut h-by a) b ~))
  ::
  ++  put
    |*  [b=* c=* d=*]
    %+  ~(put h-by a)  b
    %.  [c d]
    %~  put  h-by
    (~(gut h-by a) b ~)
  ::
  ++  tap
    ::NOTE  naive turn-based implementation find-errors ):
    =<  $
    =+  b=`_?>(?=(^ a) *(list [x=_p.n.a _?>(?=(^ q.n.a) [y=p v=q]:n.q.n.a)]))`~
    |.  ^+  b
    ?~  a
      b
    $(a r.a, b (welp (turn ~(tap h-by q.n.a) (lead p.n.a)) $(a l.a)))
  --
::
+|  %jug
::
++  h-jug
  |$  [key value]
  (h-map key (h-set value))
::
++  h-ju                                                ::  h-jug engine
  =|  a=(tree (pair hashed (tree hashed *)))            ::  (h-jug)
  |@
  ++  del                                               ::  del key-set pair
    |*  [b=* c=*]
    ^+  a
    =+  d=(get b)
    =+  e=(~(del h-in d) c)
    ?~  e
      (~(del h-by a) b)
    (~(put h-by a) b e)
  ::
  ++  gas                                               ::  concatenate
    |*  b=(list [p=* q=*])
    =>  .(b `(list _?>(?=([[* ^] ^] a) [p=p q=n.q]:n.a))`b)
    |-  ^+  a
    ?~  b
      a
    $(b t.b, a (put p.i.b q.i.b))
  ::
  ++  get                                               ::  gets h-set by key
    |*  b=*
    =+  c=(~(get h-by a) b)
    ?~(c ~ u.c)
  ::
  ++  has                                               ::  existence check
    |*  [b=* c=*]
    ^-  ?
    (~(has h-in (get b)) c)
  ::
  ++  put                                               ::  add key-h-set pair
    |*  [b=* c=*]
    ^+  a
    =+  d=(get b)
    (~(put h-by a) b (~(put h-in d) c))
  --
::
+|  %ordering
::  +gor-hip: pre-hashed tip order.
::
::    Orders h-in ascending +tip hash order, collisions assumed not exist.
::
++  gor-hip
  ~/  %gor-hip
  |=  [a=hashed b=hashed]
  ^-  ?
  (gor-digests (hashed-to-digests a) (hashed-to-digests b))
::  +mor-hip: mor pre-hashed tip order.
::
::    Orders h-in ascending double +tip hash order, collisions assumed not exist.
::
++  mor-hip
  ~/  %mor-hip
  |=  [a=hashed b=hashed]
  ^-  ?
  (gor-digests (hashed-to-digests a) (hashed-to-digests b))
::
++  hashed-to-digests
  |=  a=hashed
  ^-  noun-digests:z
  ?:  ?=(@ a)
    ~
  ?:  ?=(@ -.a)
    [a ~]
  a
::
++  gor-digests
  |=  [a=noun-digests:z b=noun-digests:z]
  ^-  ?
  ?~  a  %.n
  ?~  b  %.y
  =+  c=(digest-to-atom:tip5:z i.a)
  =+  d=(digest-to-atom:tip5:z i.b)
  ?:  (gth c d)  %.y
  ?:  (lth c d)  %.n
  $(a t.a, b t.b)
::
++  rev-tip
  |=  a=noun-digest:tip5:z
  ^-  noun-digest:tip5:z
  =+  [b c d e f]=a
  [f e d c b]
::
++  mor-digests
  |=  [a=noun-digests:z b=noun-digests:z]
  ^-  ?
  ?~  a  %.n
  ?~  b  %.y
  =+  c=(digest-to-atom:tip5:z (rev-tip i.a))
  =+  d=(digest-to-atom:tip5:z (rev-tip i.b))
  ?:  (gth c d)  %.y
  ?:  (lth c d)  %.n
  $(a t.a, b t.b)
::
+|   %h-container-from-container
  ++  h-silt                                              :: h-set from list
    |*  a=(list)
    =+  b=*(tree _?>(?=(^ a) i.a))
    (~(gas h-in b) a)
  ::
  ++  h-molt                                              :: h-map from pair
      |*  a=(list (pair))
      (~(gas h-by `(tree [p=_p.i.-.a q=_q.i.-.a])`~) a)
  ::
  ++  h-malt                                              ::  h-map from list
  |*  a=(list)
  (h-molt `(list [p=_-<.a q=_->.a])`a)
  ::
  ++  zh-molt
  |*  a=(z-map hashed *)
  (h-molt ~(tap z-by a))
  ::
  ++  zh-jult
  |*  a=(z-jug hashed hashed)
  (zh-molt (~(run z-by a) zh-silt))
  ::
  ++  zh-milt
  |*  a=(z-mip hashed hashed hashed)
  (zh-molt (~(run z-by a) zh-molt))
  ::
  ++  hz-molt
  |*  a=(h-map hashed *)
  (z-molt ~(tap h-by a))
  ::
  ++  zh-silt
  |*  a=(z-set hashed)
  (h-silt ~(tap z-in a))
  ::
  ++  hz-silt
  |*  a=(h-set hashed)
  (z-silt ~(tap h-in a))
  ::
  ++  hz-jult
  |*  a=(h-jug hashed hashed)
  (hz-molt (~(run h-by a) hz-silt))
  ::
  ++  hz-milt
  |*  a=(h-mip hashed hashed hashed)
  (hz-molt (~(run h-by a) hz-molt))
--
