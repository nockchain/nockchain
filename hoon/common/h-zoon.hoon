::  common/h-zoon/hoon
::
::    deterministic treap containers for digest-shaped keys.
::    h-map and h-set keep zoon tree semantics while comparing
::    existing digest limbs to avoid repeated tip5 hashing.
::    keys are either one tip5 digest or a non-singleton list of tip5 digests.
::
::    h-map and h-set use %hmap and %hset empty leaves. this gives the hoon
::    guard a real boundary from legacy z-map and z-set nouns. callers cross
::    the boundary with zh-* and hz-* helpers, never by treating one tree kind
::    as the other.
::
::    the algebraic contract for these containers and the seven h-zoon jets
::    (gor-hip, mor-hip, zh-molt, zh-silt, zh-milt, zh-balmilt, zh-jult) is
::    written down in docs/H-ZOON-TEST-PLAN.md under "Algebraic Contract".
::    when landing a new h-* operation or jet, update the contract there and
::    add the corresponding randomized coverage in tests/zoon/mod/h-zoon and
::    tests/dumb/mod/unit/h-zoon-consensus so reviewers can grep for the law
::    a change must preserve.
::
/=  *  /common/zoon
~%  %h-zoon  ..stark-engine-jet-hook:z  ~
|%
::
+|  %types
+$  hashed  $^(noun-digests:z noun-digest:tip5:z)
:: NOTE: empty-checking with ?~ does not work due to tagged empty leaves. Use ?@.
++  hmap-tree
  |$  [item]
  $~  %hmap
  $|  $@(%hmap [n=item l=(hmap-tree item) r=(hmap-tree item)])
  |=  a=$@(%hmap [n=* l=(hmap-tree) r=(hmap-tree)])
  |-  ^-  ?
  ?@  a  =(%hmap a)
  ?&  $(a l.a)
      $(a r.a)
  ==
::
++  hset-tree
  |$  [item]
  $~  %hset
  $|  $@(%hset [n=item l=(hset-tree item) r=(hset-tree item)])
  |=  a=$@(%hset [n=* l=(hset-tree) r=(hset-tree)])
  |-  ^-  ?
  ?@  a  =(%hset a)
  ?&  $(a l.a)
      $(a r.a)
  ==
::
+|  %map
++  h-map
  |$  [key value]                                       ::  table
  $|  (hmap-tree (pair key value))
  |=(a=(hmap-tree (pair hashed *)) ?@(a =(%hmap a) ~(apt h-by a)))
::
++  h-by                                                  ::  h-map engine
  ~/  %h-by
  =|  a=(hmap-tree (pair hashed *))  ::  (h-map)
  |@
  ++  all                                               ::  logical AND
    ~/  %all
    |*  b=$-(* ?)
    |-  ^-  ?
    ?@  a
      &
    ?&((b q.n.a) $(a l.a) $(a r.a))
  ::
  ++  any                                               ::  logical OR
    ~/  %any
    |*  b=$-(* ?)
    |-  ^-  ?
    ?@  a
      |
    ?|((b q.n.a) $(a l.a) $(a r.a))
  ::
  ++  bif                                               ::  splits a h-by b
    ~/  %bif
    |*  b=*
    |-  ^+  [l=a r=a]
    ?@  a
      [%hmap %hmap]
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
    ?@  a
      %hmap
    ?.  =(b p.n.a)
      ?:  (gor-hip b p.n.a)
        a(l $(a l.a))
      a(r $(a r.a))
    |-  ^-  [$?(%hmap _a)]
    ?@  l.a  r.a
    ?@  r.a  l.a
    ?:  (mor-hip p.n.l.a p.n.r.a)
      l.a(r $(l.a r.l.a))
    r.a(l $(r.a l.r.a))
  ::
  ++  dif                                               ::  difference
    ~/  %dif
    |*  b=_a
    |-  ^+  a
    ?@  b
      a
    =+  c=(bif p.n.b)
    ?>  ?=(^ c)
    =+  d=$(a l.c, b l.b)
    =+  e=$(a r.c, b r.b)
    |-  ^-  [$?(%hmap _a)]
    ?@  d  e
    ?@  e  d
    ?:  (mor-hip p.n.d p.n.e)
      d(r $(d r.d))
    e(l $(e l.e))
  ::
  ++  dig                                               ::  axis of b key
    ~/  %dig
    |=  b=*
    =+  c=1
    |-  ^-  (unit @)
    ?@  a  ~
    ?:  =(b p.n.a)  [~ u=(peg c 2)]
    ?:  (gor-hip b p.n.a)
      $(a l.a, c (peg c 6))
    $(a r.a, c (peg c 7))
  ::
  ++  apt                                               ::  check correctness
    =<  $
    =|  [l=(unit hashed) r=(unit hashed)]
    |.  ^-  ?
    ?@  a   =(%hmap a)
    =+  (checked-hashed p.n.a)
    ?&  ?~(l & &((gor-hip p.n.a u.l) !=(p.n.a u.l)))
        ?~(r & &((gor-hip u.r p.n.a) !=(u.r p.n.a)))
        ?@  l.a   &
        &((mor-hip p.n.a p.n.l.a) !=(p.n.a p.n.l.a) $(a l.a, l `p.n.a))
        ?@  r.a   &
        &((mor-hip p.n.a p.n.r.a) !=(p.n.a p.n.r.a) $(a r.a, r `p.n.a))
    ==
  ::
  ++  gas                                               ::  concatenate
    ~/  %gas
    |*  b=(list [p=* q=*])
    =>  .(b `(list _?>(?=(^ a) n.a))`b)
    |-  ^+  a
    ?@  b
      a
    $(b t.b, a (put p.i.b q.i.b))
  ::
  ++  get                                               ::  grab value h-by key
    ~/  %get
    |*  b=*
    =>  .(b `_?>(?=(^ a) p.n.a)`b)
    |-  ^-  (unit _?>(?=(^ a) q.n.a))
    ?@  a
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
    ?@  b
      %hmap
    ?@  a
      %hmap
    ?:  (mor-hip p.n.a p.n.b)
      ?:  =(p.n.b p.n.a)
        b(l $(a l.a, b l.b), r $(a r.a, b r.b))
      ?:  (gor-hip p.n.b p.n.a)
        %-  uni(a $(a l.a, r.b %hmap))  $(b r.b)
      %-  uni(a $(a r.a, l.b %hmap))  $(b l.b)
    ?:  =(p.n.a p.n.b)
      b(l $(b l.b, a l.a), r $(b r.b, a r.a))
    ?:  (gor-hip p.n.a p.n.b)
      %-  uni(a $(b l.b, r.a %hmap))  $(a r.a)
    %-  uni(a $(b r.b, l.a %hmap))  $(a l.a)
  ::
  ++  jab
    ~/  %jab
    |*  [key=_?>(?=(^ a) p.n.a) fun=$-(_?>(?=(^ a) q.n.a) _?>(?=(^ a) q.n.a))]
    ^+  a
    ::
    ?@  a  !!
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
    =+  (checked-hashed `hashed`b)
    |-  ^+  a
    ?@  a
      [[b c] %hmap %hmap]
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
    ?@  a  +<+.b
    $(a r.a, +<+.b $(a l.a, +<+.b (b n.a +<+.b)))
  ::
  ++  rib                                               ::  transform + product
    ~/  %rib
    |*  [b=* c=gate]
    |-  ^+  [b a]
    ?@  a  [b %hmap]
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
    ?@  a  a
    [n=[p=p.n.a q=(b q.n.a)] l=$(a l.a) r=$(a r.a)]
  ::
  ++  tap                                               ::  listify pairs
    =<  $
    =+  b=`(list _?>(?=(^ a) n.a))`~
    |.  ^+  b
    ?@  a
      b
    $(a r.a, b [n.a $(a l.a)])
  ::
  ++  uni                                               ::  union, merge
    ~/  %uni
    |*  b=_a
    |-  ^+  a
    ?@  b
      a
    ?@  a
      b
    ?:  =(p.n.b p.n.a)
      b(l $(a l.a, b l.b), r $(a r.a, b r.b))
    ?:  (mor-hip p.n.a p.n.b)
      ?:  (gor-hip p.n.b p.n.a)
        $(l.a $(a l.a, r.b %hmap), b r.b)
      $(r.a $(a r.a, l.b %hmap), b l.b)
    ?:  (gor-hip p.n.a p.n.b)
      $(l.b $(b l.b, r.a %hmap), a r.a)
    $(r.b $(b r.b, l.a %hmap), a l.a)
  ::
  ++  uno                                               ::  general union
    ~/  %uno
    |*  b=_a
    |*  meg=$-([* * *] *)
    |-  ^+  a
    ?@  b
      a
    ?@  a
      b
    ?:  =(p.n.b p.n.a)
      :+  [p.n.a `_?>(?=(^ a) q.n.a)`(meg p.n.a q.n.a q.n.b)]
        $(b l.b, a l.a)
      $(b r.b, a r.a)
    ?:  (mor-hip p.n.a p.n.b)
      ?:  (gor-hip p.n.b p.n.a)
        $(l.a $(a l.a, r.b %hmap), b r.b)
      $(r.a $(a r.a, l.b %hmap), b l.b)
    ?:  (gor-hip p.n.a p.n.b)
      $(l.b $(b l.b, r.a %hmap), a r.a)
    $(r.b $(b r.b, l.a %hmap), a l.a)
  ::
  ++  urn                                               ::  apply gate to nodes
    ~/  %urn
    |*  b=$-([* *] *)
    |-
    ?@  a  %hmap
    a(n n.a(q (b p.n.a q.n.a)), l $(a l.a), r $(a r.a))
  ::
  ++  wyt                                               ::  depth of h-map
    =<  $
    |.  ^-  @
    ?@(a 0 +((add $(a l.a) $(a r.a))))
  ::
  ++  key                                               ::  h-set of keys
    =+  b=`(h-set _?>(?=(^ a) p.n.a))`%hset
    |-  ^+  b
    ?@  a  b
    =.  b  (~(put h-in b) p.n.a)
    =.  b  $(a l.a, b b)
    $(a r.a, b b)
  ::
  ++  val                                               ::  list of vals
    =+  b=`(list _?>(?=(^ a) q.n.a))`~
    |-  ^+  b
    ?@  a   b
    $(a r.a, b [q.n.a $(a l.a)])
  --
+|  %set
++  h-set
  |$  [item]                                            ::  h-set
  $|  (hset-tree item)
  |=(a=(hset-tree item) ?@(a =(%hset a) ~(apt h-in a)))
::
++  h-in                                                  ::  h-set engine
  ~/  %h-in
  =|  a=(hset-tree hashed)  :: (h-set)
  |@
  ++  all                                               ::  logical AND
    ~/  %all
    |*  b=$-(* ?)
    |-  ^-  ?
    ?@  a
      &
    ?&((b n.a) $(a l.a) $(a r.a))
  ::
  ++  any                                               ::  logical OR
    ~/  %any
    |*  b=$-(* ?)
    |-  ^-  ?
    ?@  a
      |
    ?|((b n.a) $(a l.a) $(a r.a))
  ::
  ++  apt                                               ::  check correctness
    =<  $
    =|  [l=(unit hashed) r=(unit hashed)]
    |.  ^-  ?
    ?@  a   =(%hset a)
    =+  (checked-hashed n.a)
    ?&  ?~(l & &((gor-hip n.a u.l) !=(n.a u.l)))
        ?~(r & &((gor-hip u.r n.a) !=(u.r n.a)))
        ?@(l.a & ?&((mor-hip n.a n.l.a) !=(n.a n.l.a) $(a l.a, l `n.a)))
        ?@(r.a & ?&((mor-hip n.a n.r.a) !=(n.a n.r.a) $(a r.a, r `n.a)))
    ==
  ::
  ++  bif                                               ::  splits a by b
    ~/  %bif
    |*  b=*
    ^+  [l=a r=a]
    =<  +
    |-  ^+  a
    ?@  a
      [b %hset %hset]
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
    ?@  a
      %hset
    ?.  =(b n.a)
      ?:  (gor-hip b n.a)
        a(l $(a l.a))
      a(r $(a r.a))
    |-  ^-  [$?(%hset _a)]
    ?@  l.a  r.a
    ?@  r.a  l.a
    ?:  (mor-hip n.l.a n.r.a)
      l.a(r $(l.a r.l.a))
    r.a(l $(r.a l.r.a))
  ::
  ++  dif                                              ::  difference
    ~/  %dif
    |*  b=_a
    |-  ^+  a
    ?@  b
      a
    =+  c=(bif n.b)
    ?>  ?=(^ c)
    =+  d=$(a l.c, b l.b)
    =+  e=$(a r.c, b r.b)
    |-  ^-  [$?(%hset _a)]
    ?@  d  e
    ?@  e  d
    ?:  (mor-hip n.d n.e)
      d(r $(d r.d))
    e(l $(e l.e))
  ::
  ++  dig                                               ::  axis of a h-in b
    ~/  %dig
    |=  b=*
    =+  c=1
    |-  ^-  (unit @)
    ?@  a  ~
    ?:  =(b n.a)  [~ u=(peg c 2)]
    ?:  (gor-hip b n.a)
      $(a l.a, c (peg c 6))
    $(a r.a, c (peg c 7))
  ::
  ++  gas                                               ::  concatenate
    ~/  %gas
    |=  b=(list _?>(?=(^ a) n.a))
    |-  ^+  a
    ?@  b
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
    ::  if we used the real item type of _?^(a n.a !!) as the sample type,
    ::  then hoon would bunt it to create the default sample for the gate.
    ::
    ::  however, bunting that expression fails if :a is ~. if we wrap it
    ::  h-in a unit, the bunted unit doesn't include the bunted item type.
    ::
    ::  this way we can ensure type safety of :b without needing to perform
    ::  this failing bunt. it's a hack.
    ::
    %.  [~ b]
    |=  b=(unit _?>(?=(^ a) n.a))
    =>  .(b ?>(?=(^ b) u.b))
    |-  ^-  ?
    ?@  a
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
    ?@  b
      %hset
    ?@  a
      %hset
    ?:  =(n.b n.a)
      a(l $(a l.a, b l.b), r $(a r.a, b r.b))
    ?.  (mor-hip n.a n.b)
      $(a b, b a)
    ?:  (gor-hip n.b n.a)
      %-  uni(a $(a l.a, r.b %hset))  $(b r.b)
    %-  uni(a $(a r.a, l.b %hset))  $(b l.b)
  ::
  ++  put                                               ::  puts b h-in a, sorted
    ~/  %put
    |*  b=hashed
    =+  (checked-hashed `hashed`b)
    |-  ^+  a
    ?@  a
      [b %hset %hset]
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
    ?@  a  +<+.b
    $(a r.a, +<+.b $(a l.a, +<+.b (b n.a +<+.b)))
  ::
  ++  run                                               ::  apply gate to values
    ~/  %run
    |*  b=gate
    =+  c=`(h-set _?>(?=(^ a) (b n.a)))`%hset
    |-  ?@  a  c
    =.  c  (~(put h-in c) (b n.a))
    =.  c  $(a l.a, c c)
    $(a r.a, c c)
  ::
  ++  tap                                               ::  convert to list
    =<  $
    =+  b=`(list _?>(?=(^ a) n.a))`~
    |.  ^+  b
    ?@  a
      b
    $(a r.a, b [n.a $(a l.a)])
  ::
  ++  uni                                               ::  union
    ~/  %uni
    |*  b=_a
    ?:  =(a b)  a
    |-  ^+  a
    ?@  b
      a
    ?@  a
      b
    ?:  =(n.b n.a)
      b(l $(a l.a, b l.b), r $(a r.a, b r.b))
    ?:  (mor-hip n.a n.b)
      ?:  (gor-hip n.b n.a)
        $(l.a $(a l.a, r.b %hset), b r.b)
      $(r.a $(a r.a, l.b %hset), b l.b)
    ?:  (gor-hip n.a n.b)
      $(l.b $(b l.b, r.a %hset), a r.a)
    $(r.b $(b r.b, l.a %hset), a l.a)
  ::
  ++  wyt                                               ::  size of h-set
    =<  $
    |.  ^-  @
    ?@(a 0 +((add $(a l.a) $(a r.a))))
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
  ++  inner
    |*  b=*
    =>  .(b `_?>(?=(^ a) p.n.a)`b)
    ^-  _?>(?=(^ a) q.n.a)
    (~(gut h-by a) b `_?>(?=(^ a) q.n.a)`%hmap)
  ::
  ++  del
    |*  [b=* c=*]
    =>  .(b `_?>(?=(^ a) p.n.a)`b, c `_?>(?=(^ a) ?>(?=(^ q.n.a) p.n.q.n.a))`c)
    =+  d=(inner b)
    =+  e=(~(del h-by d) c)
    ?@  e
      (~(del h-by a) b)
    (~(put h-by a) b e)
  ::
  ++  get
    |*  [b=* c=*]
    =>  .(b `_?>(?=(^ a) p.n.a)`b, c `_?>(?=(^ a) ?>(?=(^ q.n.a) p.n.q.n.a))`c)
    ^-  (unit _?>(?=(^ a) ?>(?=(^ q.n.a) q.n.q.n.a)))
    (~(get h-by (inner b)) c)
  ::
  ++  got
    |*  [b=* c=*]
    (need (get b c))
  ::
  ++  gut
    |*  [b=* c=* d=*]
    (~(gut h-by (inner b)) c d)
  ::
  ++  has
    |*  [b=* c=*]
    !=(~ (get b c))
  ::
  ++  key
    |*  b=*
    ~(key h-by (inner b))
  ::
  ++  put
    |*  [b=* c=* d=*]
    =>  .(b `_?>(?=(^ a) p.n.a)`b, c `_?>(?=(^ a) ?>(?=(^ q.n.a) p.n.q.n.a))`c, d `_?>(?=(^ a) ?>(?=(^ q.n.a) q.n.q.n.a))`d)
    %+  ~(put h-by a)  b
    %.  [c d]
    %~  put  h-by
    (inner b)
  ::
  ++  tap
    ::NOTE  naive turn-based implementation find-errors ):
    =<  $
    =+  b=`_?>(?=(^ a) *(list [x=_p.n.a _?>(?=(^ q.n.a) [y=p v=q]:n.q.n.a)]))`~
    |.  ^+  b
    ?@  a
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
  =|  a=(hmap-tree (pair hashed (hset-tree hashed)))       ::  (h-jug)
  |@
  ++  del                                               ::  del key-set pair
    |*  [b=* c=*]
    =>  .(b `_?>(?=(^ a) p.n.a)`b, c `_?>(?=(^ a) ?>(?=(^ q.n.a) n.q.n.a))`c)
    ^+  a
    =+  d=(get b)
    =/  e=_?>(?=(^ a) q.n.a)  (~(del h-in d) c)
    ?@  e
      (~(del h-by a) b)
    (~(put h-by a) b e)
  ::
  ++  gas                                               ::  concatenate
    |*  b=(list [p=* q=*])
    =>  .(b `(list _?>(?=([[* ^] ^] a) [p=p q=n.q]:n.a))`b)
    |-  ^+  a
    ?@  b
      a
    $(b t.b, a (put p.i.b q.i.b))
  ::
  ++  get                                               ::  gets h-set by key
    |*  b=*
    =>  .(b `_?>(?=(^ a) p.n.a)`b)
    =+  c=(~(get h-by a) b)
    ?~(c `_?>(?=(^ a) q.n.a)`%hset u.c)
  ::
  ++  has                                               ::  existence check
    |*  [b=* c=*]
    ^-  ?
    (~(has h-in (get b)) c)
  ::
  ++  put                                               ::  add key-h-set pair
    |*  [b=* c=*]
    =>  .(b `_?>(?=(^ a) p.n.a)`b, c `_?>(?=(^ a) ?>(?=(^ q.n.a) n.q.n.a))`c)
    ^+  a
    =+  d=(get b)
    =/  e=_?>(?=(^ a) q.n.a)  (~(put h-in d) c)
    |-  ^+  a
    ?@  a
      [[b e] %hmap %hmap]
    ?:  =(b p.n.a)
      ?:  =(e q.n.a)
        a
      a(n [b e])
    ?:  (gor-hip b p.n.a)
      =+  f=$(a l.a)
      ?>  ?=(^ f)
      ?:  (mor-hip p.n.a p.n.f)
        a(l f)
      f(r a(l r.f))
    =+  f=$(a r.a)
    ?>  ?=(^ f)
    ?:  (mor-hip p.n.a p.n.f)
      a(r f)
    f(l a(r l.f))
  --
::
+|  %ordering
::  +gor-hip: pre-hashed tip order.
::
::    this is h-zoon's key order. a key is normalized to a digest list; a
::    single digest acts like a one-item list. each digest compares limbs
::    [4 3 2 1 0], then equal prefixes continue to the next digest. equal
::    lists return %.n and an equal-prefix shorter list loses.
::
::    there is no dor fallback here. equality means the digest keys collide,
::    and h-zoon treats digest collision resistance as a consensus assumption.
::
++  gor-hip
  ~/  %gor-hip
  |=  [a=hashed b=hashed]
  ^-  ?
  (gor-digests (hashed-to-digests a) (hashed-to-digests b))
::  +mor-hip: pre-hashed priority order.
::
::    this is h-zoon's treap priority order. it uses the same digest-list
::    rules as gor-hip, but compares limbs [0 1 2 3 4], matching double-tip
::    priority without hashing the key noun again.
::
++  mor-hip
  ~/  %mor-hip
  |=  [a=hashed b=hashed]
  ^-  ?
  (mor-digests (hashed-to-digests a) (hashed-to-digests b))
::
++  hashed-to-digests
  |=  a=hashed
  ^-  noun-digests:z
  ?:  ?=([@ @ @ @ @] a)
    [a ~]
  ?:  ?=([[@ @ @ @ @] ~] a)
    ~|  %hashed-singleton-list-forbidden  !!
  a
::
::  validate a `hashed` key without canonicalizing its noun shape.
::
::    `hashed-to-digests` defines the ordered domain used by `gor-hip`
::    and `mor-hip`: direct digest keys and non-singleton digest-list
::    keys are valid, singleton digest-list keys are ambiguous and crash.
::
::    comparator validation alone does not cover empty insertion or a
::    single-node `apt`, because neither path needs to compare two keys.
::    `h-map.put`, `h-set.put`, `h-map.apt`, and `h-set.apt` call this
::    arm at their root boundary before the key can enter or validate as
::    an h-container member.
::
::    return the original noun to preserve each caller's static key type
::    and exact key shape after validation.
++  checked-hashed
  |=  a=hashed
  ^-  hashed
  =+  (hashed-to-digests a)
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
    =+  b=`(hset-tree _?>(?=(^ a) i.a))`%hset
    (~(gas h-in b) a)
  ::
  ++  h-molt                                              :: h-map from pair
      |*  a=(list (pair))
      (~(gas h-by `(hmap-tree [p=_p.i.-.a q=_q.i.-.a])`%hmap) a)
  ::
  ++  h-malt                                              ::  h-map from list
  |*  a=(list)
  (h-molt `(list [p=_-<.a q=_->.a])`a)
  ::
  ++  zh-molt
  ~/  %zh-molt
  |*  a=(z-map hashed *)
  (h-molt ~(tap z-by a))
  ::
  ++  zh-jult
  ~/  %zh-jult
  |*  a=(z-jug hashed hashed)
  (zh-molt (~(run z-by a) zh-silt))
  ::
  ++  zh-milt
  ~/  %zh-milt
  |*  a=(z-mip hashed hashed hashed)
  (zh-molt (~(run z-by a) zh-molt))
  ::
  ::  balance migration is the only z-mip conversion that needs extra context.
  ::
  ::    blocks:   block-id -> page(parent-id, ...)
  ::    balance:  block-id -> full note-balance snapshot
  ::
  ::  the balance value at each block is a full z-map snapshot. snapshots near
  ::  each other usually share most of their source tree with the parent block,
  ::  because a block changes only a small number of notes. plain zh-milt cannot
  ::  see that history. it receives only a nested map and must treat every inner
  ::  balance root as a fresh full map unless the exact root noun repeats.
  ::
  ::  blocks is not part of this arm's hoon result. it exists only to give a jet
  ::  parent links for conversion order. the jet may read those links, convert
  ::  parents first, and derive child h-balances from z-map diffs.
  ::
  ::  hashed keys can also be digest lists, but block ids in this fast path must
  ::  be direct digests. any other valid key shape falls back because the output
  ::  must preserve exact key nouns, not only compact digest identity.
  ::
  ::  the fallback is deliberately still zh-milt, making this arm's hoon meaning
  ::  the consensus oracle. the jet must produce exactly the same noun as this
  ::  line.
  ++  zh-balmilt
  ~/  %zh-balmilt
  |*  [blocks=(z-map hashed *) balance=(z-mip hashed hashed hashed)]
  (zh-milt balance)
  ::
  ++  hz-molt
  |*  a=(h-map hashed *)
  (z-molt ~(tap h-by a))
  ::
  ++  zh-silt
  ~/  %zh-silt
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
