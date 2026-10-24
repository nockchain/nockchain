::  This file is a combination of tiny.hoon and naive.hoon from urbit/urbit
::  (but with sham jets). Required to run azimuth-pill.hoon as an Arvo.
::
!.
!=
:: begin tiny.hoon
=>
=>  %a50
~%  %a.50  ~  ~
|%
::  Types
::
+$  ship  @p
+$  life  @ud
+$  rift  @ud
+$  pass  @
+$  bloq  @
+$  step  _`@u`1
+$  bite  $@(bloq [=bloq =step])
+$  octs  [p=@ud q=@]
+$  mold  $~(* $-(* *))
++  unit  |$  [item]  $@(~ [~ u=item])
++  list  |$  [item]  $@(~ [i=item t=(list item)])
++  lest  |$  [item]  [i=item t=(list item)]
++  tree  |$  [node]  $@(~ [n=node l=(tree node) r=(tree node)])
++  pair  |$  [head tail]  [p=head q=tail]
++  map
  |$  [key value]
  $|  (tree (pair key value))
  |=(a=(tree (pair)) ?:(=(~ a) & ~(apt by a)))
::
++  set
  |$  [item]
  $|  (tree item)
  |=(a=(tree) ?:(=(~ a) & ~(apt in a)))
::
++  jug   |$  [key value]  (map key (set value))
::
::  Bits
::
++  dec                                                 ::  decrement
  ~/  %dec
  |=  a=@
  ~>  %sham.%dec
  ~_  leaf+"decrement-underflow"
  ?<  =(0 a)
  =+  b=0
  |-  ^-  @
  ?:  =(a +(b))  b
  $(b +(b))
::
++  add                                                 ::  plus
  ~/  %add
  |=  [a=@ b=@]
  ~>  %sham.%add
  ^-  @
  ?:  =(0 a)  b
  $(a (dec a), b +(b))
::
++  sub                                                 ::  subtract
  ~/  %sub
  |=  [a=@ b=@]
  ~>  %sham.%sub
  ~_  leaf+"subtract-underflow"
  ::  difference
  ^-  @
  ?:  =(0 b)  a
  $(a (dec a), b (dec b))
::
++  mul                                                 ::  multiply
  ~/  %mul
  |:  [a=`@`1 b=`@`1]
  ~>  %sham.%mul
  ^-  @
  =+  c=0
  |-
  ?:  =(0 a)  c
  $(a (dec a), c (add b c))
::
++  div                                                 ::  divide
  ~/  %div
  |:  [a=`@`1 b=`@`1]
  ~>  %sham.%div
  ^-  @
  ~_  leaf+"divide-by-zero"
  ?<  =(0 b)
  =+  c=0
  |-
  ?:  (lth a b)  c
  $(a (sub a b), c +(c))
::
++  dvr                                                 ::  divide w/remainder
  ~/  %dvr
  |:  [a=`@`1 b=`@`1]
  ~>  %sham.%dvr
  ^-  [p=@ q=@]
  [(div a b) (mod a b)]
::
++  mod                                                 ::  modulus
  ~/  %mod
  |:  [a=`@`1 b=`@`1]
  ~>  %sham.%mod
  ^-  @
  ?<  =(0 b)
  (sub a (mul b (div a b)))
::
++  bex                                                 ::  binary exponent
  ~/  %bex
  |=  a=bloq
  ~>  %sham.%bex
  ^-  @
  ?:  =(0 a)  1
  (mul 2 $(a (dec a)))
::
++  lsh                                                 ::  left-shift
  ~/  %lsh
  |=  [a=bite b=@]
  ~>  %sham.%lsh
  =/  [=bloq =step]  ?^(a a [a *step])
  (mul b (bex (mul (bex bloq) step)))
::
++  rsh                                                 ::  right-shift
  ~/  %rsh
  |=  [a=bite b=@]
  ~>  %sham.%rsh
  =/  [=bloq =step]  ?^(a a [a *step])
  (div b (bex (mul (bex bloq) step)))
::
++  con                                                 ::  binary or
  ~/  %con
  |=  [a=@ b=@]
  ~>  %sham.%con
  =+  [c=0 d=0]
  |-  ^-  @
  ?:  ?&(=(0 a) =(0 b))  d
  %=  $
    a   (rsh 0 a)
    b   (rsh 0 b)
    c   +(c)
    d   %+  add  d
          %+  lsh  [0 c]
          ?&  =(0 (end 0 a))
              =(0 (end 0 b))
          ==
  ==
::
++  dis                                                 ::  binary and
  ~/  %dis
  |=  [a=@ b=@]
  ~>  %sham.%dis
  =|  [c=@ d=@]
  |-  ^-  @
  ?:  ?|(=(0 a) =(0 b))  d
  %=  $
    a   (rsh 0 a)
    b   (rsh 0 b)
    c   +(c)
    d   %+  add  d
          %+  lsh  [0 c]
          ?|  =(0 (end 0 a))
              =(0 (end 0 b))
          ==
  ==
::
++  mix                                                 ::  binary xor
  ~/  %mix
  |=  [a=@ b=@]
  ~>  %sham.%mix
  ^-  @
  =+  [c=0 d=0]
  |-
  ?:  ?&(=(0 a) =(0 b))  d
  %=  $
    a   (rsh 0 a)
    b   (rsh 0 b)
    c   +(c)
    d   (add d (lsh [0 c] =((end 0 a) (end 0 b))))
  ==
::
++  lth                                                 ::  less
  ~/  %lth
  |=  [a=@ b=@]
  ~>  %sham.%lth
  ^-  ?
  ?&  !=(a b)
      |-
      ?|  =(0 a)
          ?&  !=(0 b)
              $(a (dec a), b (dec b))
  ==  ==  ==
::
++  lte                                                 ::  less or equal
  ~/  %lte
  |=  [a=@ b=@]
  ~>  %sham.%lte
  |(=(a b) (lth a b))
::
++  gte                                                 ::  greater or equal
  ~/  %gte
  |=  [a=@ b=@]
  ~>  %sham.%gte
  ^-  ?
  !(lth a b)
::
++  gth                                                 ::  greater
  ~/  %gth
  |=  [a=@ b=@]
  ~>  %sham.%gth
  ^-  ?
  !(lte a b)
::
++  swp                                                 ::  naive rev bloq order
  ~/  %swp
  |=  [a=bloq b=@]
  (rep a (flop (rip a b)))
::
++  met                                                 ::  measure
  ~/  %met
  |=  [a=bloq b=@]
  ~>  %sham.%met
  ^-  @
  =+  c=0
  |-
  ?:  =(0 b)  c
  $(b (rsh a b), c +(c))
::
++  end                                                 ::  tail
  ~/  %end
  |=  [a=bite b=@]
  ~>  %sham.%end
  =/  [=bloq =step]  ?^(a a [a *step])
  (mod b (bex (mul (bex bloq) step)))
::
++  cat                                                 ::  concatenate
  ~/  %cat
  |=  [a=bloq b=@ c=@]
  ~>  %sham.%cat
  (add (lsh [a (met a b)] c) b)
::
++  cut                                                 ::  slice
  ~/  %cut
  |=  [a=bloq [b=step c=step] d=@]
  ~>  %sham.%cut
  (end [a c] (rsh [a b] d))
::
++  can                                                 ::  assemble
  ~/  %can
  |=  [a=bloq b=(list [p=step q=@])]
  ~>  %sham.%can
  ^-  @
  ?~  b  0
  (add (end [a p.i.b] q.i.b) (lsh [a p.i.b] $(b t.b)))
::
++  cad                                                 ::  assemble specific
  ~/  %cad
  |=  [a=bloq b=(list [p=step q=@])]
  ~>  %sham.%cad
  ^-  [=step @]
  :_  (can a b)
  |-
  ?~  b
    0
  (add p.i.b $(b t.b))
::
++  rep                                                 ::  assemble fixed
  ~/  %rep
  |=  [a=bite b=(list @)]
  ~>  %sham.%rep
  =/  [=bloq =step]  ?^(a a [a *step])
  =|  i=@ud
  |-  ^-  @
  ?~  b   0
  %+  add  $(i +(i), b t.b)
  (lsh [bloq (mul step i)] (end [bloq step] i.b))
::
++  rip                                                 ::  disassemble
  ~/  %rip
  |=  [a=bite b=@]
  ~>  %sham.%rip
  ^-  (list @)
  ?:  =(0 b)  ~
  [(end a b) $(b (rsh a b))]
::
::
::  Lists
::
++  lent                                                ::  length
  ~/  %lent
  |=  a=(list)
  ^-  @
  =+  b=0
  |-
  ?~  a  b
  $(a t.a, b +(b))
::
++  slag                                                ::  suffix
  ~/  %slag
  |*  [a=@ b=(list)]
  |-  ^+  b
  ?:  =(0 a)  b
  ?~  b  ~
  $(b t.b, a (dec a))
::
++  snag                                                ::  index
  ~/  %snag
  |*  [a=@ b=(list)]
  |-  ^+  ?>(?=(^ b) i.b)
  ?~  b
    ~_  leaf+"snag-fail"
    !!
  ?:  =(0 a)  i.b
  $(b t.b, a (dec a))
::
++  homo                                                ::  homogenize
  |*  a=(list)
  ^+  =<  $
    |@  ++  $  ?:(*? ~ [i=(snag 0 a) t=$])
    --
  a
::
++  flop                                                ::  reverse
  ~/  %flop
  |*  a=(list)
  =>  .(a (homo a))
  ^+  a
  =+  b=`_a`~
  |-
  ?~  a  b
  $(a t.a, b [i.a b])
::
++  welp                                                ::  concatenate
  ~/  %welp
  =|  [* *]
  |@
  ++  $
    ?~  +<-
      +<-(. +<+)
    +<-(+ $(+<- +<->))
  --
::
++  reap                                                ::  replicate
  ~/  %reap
  |*  [a=@ b=*]
  |-  ^-  (list _b)
  ?~  a  ~
  [b $(a (dec a))]
::
::  Modular arithmetic
::
++  fe                                                  ::  modulo bloq
  |_  a=bloq
  ++  rol  |=  [b=bloq c=@ d=@]  ^-  @                  ::  roll left
           =+  e=(sit d)
           =+  f=(bex (sub a b))
           =+  g=(mod c f)
           (sit (con (lsh [b g] e) (rsh [b (sub f g)] e)))
  ++  sum  |=([b=@ c=@] (sit (add b c)))                ::  wrapping add
  ++  sit  |=(b=@ (end a b))                            ::  enforce modulo
  --
::
::  Hashes
::
++  muk                                                 ::  standard murmur3
  ~%  %muk  ..muk  ~
  =+  ~(. fe 5)
  |=  [syd=@ len=@ key=@]
  =.  syd      (end 5 syd)
  =/  pad      (sub len (met 3 key))
  =/  data     (welp (rip 3 key) (reap pad 0))
  =/  nblocks  (div len 4)  ::  intentionally off-by-one
  =/  h1  syd
  =+  [c1=0xcc9e.2d51 c2=0x1b87.3593]
  =/  blocks  (rip 5 key)
  =/  i  nblocks
  =.  h1  =/  hi  h1  |-
    ?:  =(0 i)  hi
    =/  k1  (snag (sub nblocks i) blocks)  ::  negative array index
    =.  k1  (sit (mul k1 c1))
    =.  k1  (rol 0 15 k1)
    =.  k1  (sit (mul k1 c2))
    =.  hi  (mix hi k1)
    =.  hi  (rol 0 13 hi)
    =.  hi  (sum (sit (mul hi 5)) 0xe654.6b64)
    $(i (dec i))
  =/  tail  (slag (mul 4 nblocks) data)
  =/  k1    0
  =/  tlen  (dis len 3)
  =.  h1
    ?+  tlen  h1  ::  fallthrough switch
      %3  =.  k1  (mix k1 (lsh [0 16] (snag 2 tail)))
          =.  k1  (mix k1 (lsh [0 8] (snag 1 tail)))
          =.  k1  (mix k1 (snag 0 tail))
          =.  k1  (sit (mul k1 c1))
          =.  k1  (rol 0 15 k1)
          =.  k1  (sit (mul k1 c2))
          (mix h1 k1)
      %2  =.  k1  (mix k1 (lsh [0 8] (snag 1 tail)))
          =.  k1  (mix k1 (snag 0 tail))
          =.  k1  (sit (mul k1 c1))
          =.  k1  (rol 0 15 k1)
          =.  k1  (sit (mul k1 c2))
          (mix h1 k1)
      %1  =.  k1  (mix k1 (snag 0 tail))
          =.  k1  (sit (mul k1 c1))
          =.  k1  (rol 0 15 k1)
          =.  k1  (sit (mul k1 c2))
          (mix h1 k1)
    ==
  =.  h1  (mix h1 len)
  |^  (fmix32 h1)
  ++  fmix32
    |=  h=@
    =.  h  (mix h (rsh [0 16] h))
    =.  h  (sit (mul h 0x85eb.ca6b))
    =.  h  (mix h (rsh [0 13] h))
    =.  h  (sit (mul h 0xc2b2.ae35))
    =.  h  (mix h (rsh [0 16] h))
    h
  --
::
++  mug                                                 ::  mug with murmur3
  ~/  %mug
  |=  a=*
  ~>  %sham.%mug
  |^  ?@  a  (mum 0xcafe.babe 0x7fff a)
      =/  b  (cat 5 $(a -.a) $(a +.a))
      (mum 0xdead.beef 0xfffe b)
  ::
  ++  mum
    |=  [syd=@uxF fal=@F key=@]
    =/  wyd  (met 3 key)
    =|  i=@ud
    |-  ^-  @F
    ?:  =(8 i)  fal
    =/  haz=@F  (muk syd wyd key)
    =/  ham=@F  (mix (rsh [0 31] haz) (end [0 31] haz))
    ?.(=(0 ham) ham $(i +(i), syd +(syd)))
  --
::
++  gor                                                 ::  mug order
  ~/  %gor
  |=  [a=* b=*]
  ~>  %sham.%gor
  ^-  ?
  =+  [c=(mug a) d=(mug b)]
  ?:  =(c d)
    (dor a b)
  (lth c d)
::
++  mor                                                 ::  more mug order
  ~/  %mor
  |=  [a=* b=*]
  ~>  %sham.%mor
  ^-  ?
  =+  [c=(mug (mug a)) d=(mug (mug b))]
  ?:  =(c d)
    (dor a b)
  (lth c d)
::
++  dor                                                 ::  tree order
  ~/  %dor
  |=  [a=* b=*]
  ~>  %sham.%dor
  ^-  ?
  ?:  =(a b)  &
  ?.  ?=(@ a)
    ?:  ?=(@ b)  |
    ?:  =(-.a -.b)
      $(a +.a, b +.b)
    $(a -.a, b -.b)
  ?.  ?=(@ b)  &
  (lth a b)
::
++  por                                                 ::  parent order
  ~/  %por
  |=  [a=@p b=@p]
  ~>  %sham.%por
  ^-  ?
  ?:  =(a b)  &
  =|  i=@
  |-
  ?:  =(i 2)
    ::  second two bytes
    (lte a b)
  ::  first two bytes
  =+  [c=(end 3 a) d=(end 3 b)]
  ?:  =(c d)
    $(a (rsh 3 a), b (rsh 3 b), i +(i))
  (lth c d)
::
::  Maps
::
++  by
  ~/  %by
  =|  a=(tree (pair))  ::  (map)
  ~>  %sham.%by
  =*  node  ?>(?=(^ a) n.a)
  |@
  ++  get
    ~/  %get
    |*  b=*
    ~>  %sham.%get
    =>  .(b `_?>(?=(^ a) p.n.a)`b)
    |-  ^-  (unit _?>(?=(^ a) q.n.a))
    ?~  a
      ~
    ?:  =(b p.n.a)
      `q.n.a
    ?:  (gor b p.n.a)
      $(a l.a)
    $(a r.a)
  ::
  ++  put
    ~/  %put
    |*  [b=* c=*]
    ~>  %sham.%put
    |-  ^+  a
    ~>  %sham.%putt
    ?~  a
      [[b c] ~ ~]
    ?:  =(b p.n.a)
      ?:  =(c q.n.a)
        a
      a(n [b c])
    ?:  (gor b p.n.a)
      =+  d=$(a l.a)
      ?>  ?=(^ d)
      ?:  (mor p.n.a p.n.d)
        a(l d)
      d(r a(l r.d))
    =+  d=$(a r.a)
    ?>  ?=(^ d)
    ?:  (mor p.n.a p.n.d)
      a(r d)
    d(l a(r l.d))
  ::
  ++  del
    ~/  %del
    |*  b=*
    ~>  %sham.%del
    |-  ^+  a
    ?~  a
      ~
    ?.  =(b p.n.a)
      ?:  (gor b p.n.a)
        a(l $(a l.a))
      a(r $(a r.a))
    |-  ^-  [$?(~ _a)]
    ?~  l.a  r.a
    ?~  r.a  l.a
    ?:  (mor p.n.l.a p.n.r.a)
      l.a(r $(l.a r.l.a))
    r.a(l $(r.a l.r.a))
  ::
  ++  apt
    =<  $
    ~/  %apt
    =|  [l=(unit) r=(unit)]
    ~>  %sham.%apt
    |.  ^-  ?
    ?~  a   &
    ?&  ?~(l & &((gor p.n.a u.l) !=(p.n.a u.l)))
        ?~(r & &((gor u.r p.n.a) !=(u.r p.n.a)))
        ?~  l.a   &
        &((mor p.n.a p.n.l.a) !=(p.n.a p.n.l.a) $(a l.a, l `p.n.a))
        ?~  r.a   &
        &((mor p.n.a p.n.r.a) !=(p.n.a p.n.r.a) $(a r.a, r `p.n.a))
    ==
  --
::
++  on                                                  ::  ordered map
  ~/  %on
  |*  [key=mold val=mold]
  ~>  %sham.%on
  =>  |%
      +$  item  [key=key val=val]
      --
  ::
  ~%  %comp  +>+  ~
  |=  compare=$-([key key] ?)
  ~%  %core    +  ~
  |%
  ::
  ++  apt
    ~/  %apt
    |=  a=(tree item)
    ~>  %sham.%apt
    =|  [l=(unit key) r=(unit key)]
    |-  ^-  ?
    ?~  a  %.y
    ?&  ?~(l %.y (compare key.n.a u.l))
        ?~(r %.y (compare u.r key.n.a))
        ?~(l.a %.y &((mor key.n.a key.n.l.a) $(a l.a, l `key.n.a)))
        ?~(r.a %.y &((mor key.n.a key.n.r.a) $(a r.a, r `key.n.a)))
    ==
  ::
  ++  get
    ~/  %get
    |=  [a=(tree item) b=key]
    ~>  %sham.%get
    ^-  (unit val)
    ?~  a  ~
    ?:  =(b key.n.a)
      `val.n.a
    ?:  (compare b key.n.a)
      $(a l.a)
    $(a r.a)
  ::
  ++  has
    ~/  %has
    |=  [a=(tree item) b=key]
    ~>  %sham.%has
    ^-  ?
    !=(~ (get a b))
  ::
  ++  put
    ~/  %put
    |=  [a=(tree item) =key =val]
    ~>  %sham.%put
    ^-  (tree item)
    ?~  a  [n=[key val] l=~ r=~]
    ?:  =(key.n.a key)  a(val.n val)
    ?:  (compare key key.n.a)
      =/  l  $(a l.a)
      ?>  ?=(^ l)
      ?:  (mor key.n.a key.n.l)
        a(l l)
      l(r a(l r.l))
    =/  r  $(a r.a)
    ?>  ?=(^ r)
    ?:  (mor key.n.a key.n.r)
      a(r r)
    r(l a(r l.r))
  --
::
::  Sets
::
++  in
  ~/  %in
  =|  a=(tree)  :: (set)
  ~>  %sham.%in
  |@
  ++  put
    ~/  %put
    |*  b=*
    ~>  %sham.%put
    |-  ^+  a
    ?~  a
      [b ~ ~]
    ?:  =(b n.a)
      a
    ?:  (gor b n.a)
      =+  c=$(a l.a)
      ?>  ?=(^ c)
      ?:  (mor n.a n.c)
        a(l c)
      c(r a(l r.c))
    =+  c=$(a r.a)
    ?>  ?=(^ c)
    ?:  (mor n.a n.c)
      a(r c)
    c(l a(r l.c))
  ::
  ++  del
    ~/  %del
    |*  b=*
    ~>  %sham.%del
    |-  ^+  a
    ?~  a
      ~
    ?.  =(b n.a)
      ?:  (gor b n.a)
        a(l $(a l.a))
      a(r $(a r.a))
    |-  ^-  [$?(~ _a)]
    ?~  l.a  r.a
    ?~  r.a  l.a
    ?:  (mor n.l.a n.r.a)
      l.a(r $(l.a r.l.a))
    r.a(l $(r.a l.r.a))
  ::
  ++  apt
    =<  $
    ~/  %apt
    =|  [l=(unit) r=(unit)]
    ~>  %sham.%apt
    |.  ^-  ?
    ?~  a   &
    ?&  ?~(l & (gor n.a u.l))
        ?~(r & (gor u.r n.a))
        ?~(l.a & ?&((mor n.a n.l.a) $(a l.a, l `n.a)))
        ?~(r.a & ?&((mor n.a n.r.a) $(a r.a, r `n.a)))
    ==
  --
::
::  Jugs
::
++  ju
  =|  a=(tree (pair * (tree)))  ::  (jug)
  |@
  ++  get
    |*  b=*
    =+  c=(~(get by a) b)
    ?~(c ~ u.c)
  ::
  ++  del
    |*  [b=* c=*]
    ^+  a
    =+  d=(get b)
    =+  e=(~(del in d) c)
    ?~  e
      (~(del by a) b)
    (~(put by a) b e)
  ::
  ++  put
    |*  [b=* c=*]
    ^+  a
    =+  d=(get b)
    (~(put by a) b (~(put in d) c))
  --
--
::
::  Begin naive.hoon
::
=>
::  Laconic bit
::
=|  lac=?
::  Constants
::
|%
::  Transfers on L1 to this address count as depositing to L2
::
++  deposit-address  0x1111.1111.1111.1111.1111.1111.1111.1111.1111.1111
++  log-names
  |%
  ::  Generated with (keccak-256:keccak:crypto (as-octs:mimes:html name))
  ::
  ::  OwnerChanged(uint32,address)
  ++  owner-changed
    0x16d0.f539.d49c.6cad.822b.767a.9445.bfb1.
      cf7e.a6f2.a6c2.b120.a7ea.4cc7.660d.8fda
  ::
  ::  Activated(uint32)
  ++  activated
    0xe74c.0380.9d07.69e1.b1f7.06cc.8414.258c.
      d1f3.b6fe.020c.d15d.0165.c210.ba50.3a0f
  ::
  ::  Spawned(uint32,uint32)
  ++  spawned
    0xb2d3.a6e7.a339.f5c8.ff96.265e.2f03.a010.
      a854.1070.f374.4a24.7090.9644.1508.1546
  ::
  ::  OwnershipTransferred(address,address)
  ++  ownership-transferred
    0x8be0.079c.5316.5914.1344.cd1f.d0a4.f284.
      1949.7f97.22a3.daaf.e3b4.186f.6b64.57e0
  ::
  ::  EscapeRequested(uint32,uint32)
  ++  escape-requested
    0xb4d4.850b.8f21.8218.141c.5665.cba3.79e5.
      3e9b.b015.b51e.8d93.4be7.0210.aead.874a
  ::
  ::  EscapeCanceled(uint32,uint32)
  ++  escape-canceled
    0xd653.bb0e.0bb7.ce83.93e6.24d9.8fbf.17cd.
      a590.2c83.28ed.0cd0.9988.f368.90d9.932a
  ::
  ::  EscapeAccepted(uint32,uint32)
  ++  escape-accepted
    0x7e44.7c9b.1bda.4b17.4b07.96e1.00bf.7f34.
      ebf3.6dbb.7fe6.6549.0b1b.fce6.246a.9da5
  ::
  ::  LostSponsor(uint32,uint32)
  ++  lost-sponsor
    0xd770.4f9a.2519.3dbd.0b0c.b4a8.09fe.ffff.
      a7f1.9d1a.ae88.17a7.1346.c194.4482.10d5
  ::
  ::  ChangedKeys(uint32,bytes32,bytes32,uint32,uint32)
  ++  changed-keys
    0xaa10.e7a0.117d.4323.f1d9.9d63.0ec1.69be.
      bb3a.988e.8957.70e3.5198.7e01.ff54.23d5
  ::
  ::  BrokeContinuity(uint32,uint32)
  ++  broke-continuity
    0x2929.4799.f1c2.1a37.ef83.8e15.f79d.d91b.
      cee2.df99.d63c.d1c1.8ac9.68b1.2951.4e6e
  ::
  ::  ChangedSpawnProxy(uint32,address)
  ++  changed-spawn-proxy
    0x9027.36af.7b3c.efe1.0d9e.840a.ed0d.687e.
      35c8.4095.122b.2505.1a20.ead8.866f.006d
  ::
  ::  ChangedTransferProxy(uint32,address)
  ++  changed-transfer-proxy
    0xcfe3.69b7.197e.7f0c.f067.93ae.2472.a9b1.
      3583.fecb.ed2f.78df.a14d.1f10.796b.847c
  ::
  ::  ChangedManagementProxy(uint32,address)
  ++  changed-management-proxy
    0xab9c.9327.cffd.2acc.168f.afed.be06.139f.
      5f55.cb84.c761.df05.e051.1c25.1e2e.e9bf
  ::
  ::  ChangedVotingProxy(uint32,address)
  ++  changed-voting-proxy
    0xcbd6.269e.c714.57f2.c7b1.a227.74f2.46f6.
      c5a2.eae3.795e.d730.0db5.1768.0c61.c805
  ::
  ::  ChangedDns(string,string,string)
  ++  changed-dns
    0xfafd.04ad.e1da.ae2e.1fdb.0fc1.cc6a.899f.
      d424.063e.d5c9.2120.e67e.0730.53b9.4898
  ::
  ::  ApprovalForAll(address,address,bool)
  ++  approval-for-all
    0x1730.7eab.39ab.6107.e889.9845.ad3d.59bd.
      9653.f200.f220.9204.89ca.2b59.3769.6c31
  --
--  =>
::  Types
|%
::  ethereum address, 20 bytes.
::
+$  address   @ux
+$  nonce     @ud
+$  dominion  ?(%l1 %l2 %spawn)
+$  keys      [=life suite=@ud auth=@ crypt=@]
++  orm       ((on ship point) por)
++  point
  $:  ::  domain
      ::
      =dominion
      ::
      ::  ownership
      ::
      $=  own
      $:  owner=[=address =nonce]
          spawn-proxy=[=address =nonce]
          management-proxy=[=address =nonce]
          voting-proxy=[=address =nonce]
          transfer-proxy=[=address =nonce]
      ==
      ::
      ::  networking
      ::
      $=  net
      $:  rift=@ud
          =keys
          sponsor=[has=? who=@p]
          escape=(unit @p)
      ==
  ==
::
++  diff
  $%  [%nonce =ship =proxy =nonce]
      [%tx =raw-tx err=(unit @tas)]
      [%operator owner=address operator=address approved=?]
      [%dns domains=(list @t)]
      $:  %point  =ship
          $%  [%rift =rift]
              [%keys =keys]
              [%sponsor sponsor=(unit @p)]
              [%escape to=(unit @p)]
              [%owner =address]
              [%spawn-proxy =address]
              [%management-proxy =address]
              [%voting-proxy =address]
              [%transfer-proxy =address]
              [%dominion =dominion]
  ==  ==  ==
::
+$  state
  $:  %0
      =points
      =operators
      dns=(list @t)
  ==
+$  points     (tree [ship point])
+$  operators  (jug address address)
+$  effects    (list diff)
+$  proxy      ?(%own %spawn %manage %vote %transfer)
+$  roll       (list raw-tx)
+$  raw-tx     [sig=@ raw=octs =tx]
+$  tx         [from=[=ship =proxy] skim-tx]
+$  skim-tx
  $%  [%transfer-point =address reset=?]
      [%spawn =ship =address]
      [%configure-keys encrypt=@ auth=@ crypto-suite=@ breach=?]
      [%escape parent=ship]
      [%cancel-escape parent=ship]
      [%adopt =ship]
      [%reject =ship]
      [%detach =ship]
      [%set-management-proxy =address]
      [%set-spawn-proxy =address]
      [%set-transfer-proxy =address]
  ==
::
+$  event-log
  $:  address=@ux
      data=@ux
      topics=(lest @ux)
  ==
+$  input
  $:  block=@ud
  $%  [%bat batch=@]
      [%log =event-log]
  ==  ==
::  ECDSA verifier.
::
::  Must keccak `dat` and recover the ethereum address which signed.
::  Must not crash.  `v` will normally be between 0 and 3; if it is not,
::  should produce null.
::
+$  verifier  $-([dat=octs v=@ r=@ s=@] (unit address))
--  =>
::
|%
++  debug
  |*  [meg=@t *]
  ?:  lac
    +<+
  ~>  %slog.[0 meg]
  +<+
::
++  parse-roll
  |=  batch=@
  =|  =roll
  =|  pos=@ud
  =/  las  (met 0 batch)
  |-  ^+  roll
  ?:  (gte pos las)
    (flop roll)
  =/  parse-result  (parse-raw-tx pos batch)
  ::  Parsing failed, abort batch
  ::
  ?~  parse-result
    (debug %parse-failed ~)
  =^  =raw-tx  pos  u.parse-result
  $(roll [raw-tx roll])
::
++  parse-raw-tx
  |=  [pos=@ud batch=@]
  ^-  (unit [raw-tx pos=@ud])
  |^
  =^  sig  pos  (take 3 65)
  =/  res=(unit [=tx pos=@ud])  parse-tx
  ?~  res  ~
  =/  dif  (sub pos.u.res pos)
  =/  len  =>((dvr dif 8) ?>(=(0 q) p))
  :-  ~  :_  pos.u.res
  [sig [len (cut 0 [pos dif] batch)] tx.u.res]
  ::
  ++  parse-tx
    ^-  (unit [tx pos=@ud])
    =^  from-proxy=@      pos  (take 0 3)
    ?.  ?=(?(%0 %1 %2 %3 %4) from-proxy)  (debug %bad-proxy ~)
    =/  =proxy
      ?-  from-proxy
        %0  %own
        %1  %spawn
        %2  %manage
        %3  %vote
        %4  %transfer
      ==
    =^  pad               pos  (take 0 5)
    =^  from-ship=ship    pos  (take 3 4)
    =-  ?~  res
          ~
        `[[[from-ship proxy] skim-tx.u.res] pos.u.res]
    ^-  res=(unit [=skim-tx pos=@ud])
    =^  op   pos  (take 0 7)
    ?+    op  (debug %strange-opcode ~)
        %0
      =^  reset=@         pos  (take 0)
      =^  =address        pos  (take 3 20)
      `[[%transfer-point address =(0 reset)] pos]
    ::
        %1
      =^  pad=@     pos  (take 0)
      =^  =ship     pos  (take 3 4)
      =^  =address  pos  (take 3 20)
      `[[%spawn ship address] pos]
    ::
        %2
      =^  breach=@        pos  (take 0)
      =^  encrypt=@       pos  (take 3 32)
      =^  auth=@          pos  (take 3 32)
      =^  crypto-suite=@  pos  (take 3 4)
      `[[%configure-keys encrypt auth crypto-suite =(0 breach)] pos]
    ::
        %3   =^(res pos take-ship `[[%escape res] pos])
        %4   =^(res pos take-ship `[[%cancel-escape res] pos])
        %5   =^(res pos take-ship `[[%adopt res] pos])
        %6   =^(res pos take-ship `[[%reject res] pos])
        %7   =^(res pos take-ship `[[%detach res] pos])
        %8   =^(res pos take-address `[[%set-management-proxy res] pos])
        %9   =^(res pos take-address `[[%set-spawn-proxy res] pos])
        %10  =^(res pos take-address `[[%set-transfer-proxy res] pos])
    ==
  ::
  ::  Take a bite
  ::
  ++  take
    |=  =bite
    ^-  [@ @ud]
    =/  =step
      ?@  bite  (bex bite)
      (mul step.bite (bex bloq.bite))
    [(cut 0 [pos step] batch) (add pos step)]
  ::  Encode ship and address
  ::
  ++  take-address
    ^-  [address @ud]
    =^  pad=@     pos  (take 0)
    =^  =address  pos  (take 3 20)
    [address pos]
  ::  Encode escape-related txs
  ::
  ++  take-ship
    ^-  [ship @ud]
    =^  pad=@       pos  (take 0)
    =^  other=ship  pos  (take 3 4)
    [other pos]
  --
::
++  proxy-from-point
  |=  [=proxy point]
  ^-  [=address =nonce]
  ?-  proxy
    %own       owner.own
    %spawn     spawn-proxy.own
    %manage    management-proxy.own
    %vote      voting-proxy.own
    %transfer  transfer-proxy.own
  ==
::
++  verify-sig-and-nonce
  |=  [=verifier chain-t=@t =state =raw-tx]
  ^-  ?
  |^
  =/  point  (get-point state ship.from.tx.raw-tx)
  ?>  ?=(^ point)  ::  we never parse more than four bytes for a ship
  =/  need=[=address =nonce]
    (proxy-from-point proxy.from.tx.raw-tx u.point)
  ::  We include a domain separator to avoid letting signatures be
  ::  accidentally reused with other applications.  We include the name
  ::  UrbitID, a signature format version number, and the EIP-155 chain
  ::  ID.
  ::
  ::  We also include a nonce so that a transaction cannot be
  ::  rebroadcast.
  ::
  =/  prepared-data=octs
    %:  cad  3
      14^'UrbitIDV1Chain'
      (met 3 chain-t)^chain-t
      1^':'
      4^nonce.need
      raw.raw-tx
      ~
    ==
  ::  Wallets which support personal_sign include this preamble to avoid
  ::  letting personal_sign be used to sign ethereum transactions
  ::
  =/  signed-data=octs
    =/  len  (ud-to-ascii p.prepared-data)
    %:  cad  3
      26^'\19Ethereum Signed Message:\0a'
      (met 3 len)^len
      prepared-data
      ~
    ==
  =/  dress  (verify-sig sig.raw-tx signed-data)
  ?~  dress
    |
  =(address.need u.dress)
  ::  Verify signature and produce signer address
  ::
  ++  verify-sig
    |=  [sig=@ txdata=octs]
    ^-  (unit address)
    |^
    ::  Reversed of the usual r-s-v order because Ethereum integers are
    ::  big-endian
    ::
    =^  v  sig  (take 3)
    =^  s  sig  (take 3 32)
    =^  r  sig  (take 3 32)
    ::  In Ethereum, v is generally 27 + recid, and verifier expects a
    ::  recid.  Old versions of geth used 0 + recid, so most software
    ::  now supports either format.  See:
    ::
    ::  https://github.com/ethereum/go-ethereum/issues/2053
    ::
    =?  v  (gte v 27)  (sub v 27)
    (verifier txdata v r s)
    ::
    ++  take
      |=  =bite
      [(end bite sig) (rsh bite sig)]
    --
  --
::  ASCII-decimal encode
::
++  ud-to-ascii
  |=  n=@ud
  ?~  n  '0'
  =|  l=(list @)
  |-  ^-  @t
  ?~  n  (rep 3 l)
  =+  (dvr n 10)
  $(n p, l [(add '0' q) l])
::
++  ship-rank
  |=  =ship
  ^-  ?(%0 %1 %2 %3 %4)
  ?:  (lth ship 0x100)                    %0
  ?:  (lth ship 0x1.0000)                 %1
  ?:  (lth ship 0x1.0000.0000)            %2
  ?:  (lth ship 0x1.0000.0000.0000.0000)  %3
  %4
::
++  sein                                          ::  autoboss
  |=  who=ship
  ^-  ship
  =/  mir  (ship-rank who)
  ?-  mir
    %0  who
    %1  (end 3 who)
    %2  (end 4 who)
    %3  (end 5 who)
    %4  (end 4 who)
  ==
::
::  Produces null only if ship is not a galaxy, star, or planet
::
++  get-point
  |=  [=state =ship]
  ^-  (unit point)
  =/  existing  (get:orm points.state ship)
  ?^  existing
    `u.existing
  =|  =point
  =.  who.sponsor.net.point  (sein ship)
  ?+    (ship-rank ship)  (debug %strange-point ~)
      %0  `point(dominion %l1)
      ?(%1 %2)
    =/  existing-parent  $(ship (sein ship))
    ?~  existing-parent  ~
    :-  ~
    %=    point
        dominion
      ?-  dominion.u.existing-parent
        %l1     %l1
        %l2     %l2
        %spawn  %l2
      ==
    ==
  ==
--  =>
|%
:: Receive log from L1 transaction
::
++  receive-log
  |=  [=state log=event-log]
  ^-  [effects ^state]
  =*  log-name  i.topics.log
  ?:  =(log-name activated:log-names)              `state
  ?:  =(log-name spawned:log-names)                `state
  ?:  =(log-name ownership-transferred:log-names)  `state
  ?:  =(log-name changed-dns:log-names)
    ?>  ?=(~ t.topics.log)
    =/  words  (rip 8 data.log)
    ::  This is only true if each domain is <= 32 bytes
    ::
    ?.  ?=([c=@ @ b=@ @ a=@ @ @ @ @ ~] words)  `state
    =*  one  &5.words
    =*  two  &3.words
    =*  tri  &1.words
    =/  domains  ~[(swp 3 one) (swp 3 two) (swp 3 tri)]
    :-  [%dns domains]~
    state(dns domains)
  ::
  ?:  =(log-name approval-for-all:log-names)
    ?>  ?=([@ @ ~] t.topics.log)
    =*  owner     i.t.topics.log
    =*  operator  i.t.t.topics.log
    =/  approved  !=(0 data.log)
    :-  [%operator owner operator approved]~
    =-  state(operators -)
    ?:  approved
      (~(put ju operators.state) owner operator)
    (~(del ju operators.state) owner operator)
  ::
  ::  The rest of the logs modify a particular ship, specified in the
  ::  second topic.  We fetch it, and insert the modification back into
  ::  our state.
  ::
  ?>  ?=([@ *] t.topics.log)
  =*  ship=@  i.t.topics.log
  =/  the-point  (get-point state ship)
  ?>  ?=(^ the-point)
  =*  point  u.the-point
  ::
  ::  Important to fully no-op on failure so we don't insert an entry
  ::  into points.state
  ::
  =-  ?~  res
        `state
      =/  p  (put:orm points.state ship new-point.u.res)
      [effects.u.res state(points p)]
  ^-  res=(unit [=effects new-point=^point])
  ::
  ?:  =(log-name changed-spawn-proxy:log-names)
    ?.  ?=(%l1 -.point)  ~
    ?>  ?=([@ ~] t.t.topics.log)
    =*  to  i.t.t.topics.log
    ::  Depositing to L2 is represented by a spawn proxy change on L1,
    ::  but it doesn't change the actual spawn proxy.
    ::
    ?:  =(deposit-address to)
      :+  ~  [%point ship %dominion %spawn]~
      point(dominion %spawn)
    :+  ~  [%point ship %spawn-proxy to]~
    point(address.spawn-proxy.own to)
  ::
  ?:  =(log-name escape-accepted:log-names)
    ?>  ?=([@ ~] t.t.topics.log)
    =*  parent=@  i.t.t.topics.log
    =/  parent-point  (get-point state parent)
    ?>  ?=(^ parent-point)
    ?:  ?=(%l2 -.u.parent-point)  ~
    :+  ~  [%point ship %sponsor `parent]~
    point(escape.net ~, sponsor.net [%& parent])
  ::
  ?:  =(log-name lost-sponsor:log-names)
    ?>  ?=([@ ~] t.t.topics.log)
    =*  parent=@  i.t.t.topics.log
    ::  If the sponsor we lost was not our actual sponsor, we didn't
    ::  actually lose anything.
    ::
    ?.  =(parent who.sponsor.net.point)  ~
    ::
    =/  parent-point  (get-point state parent)
    ?>  ?=(^ parent-point)
    ::
    ::  We can detach even if the child is on L2, as long as the parent
    ::  is on L1.
    ::
    ?:  ?=(%l2 -.u.parent-point)  ~
    :+  ~  [%point ship %sponsor ~]~
    point(has.sponsor.net %|)
  ::
  ::  The rest can be done by any ship on L1, even if their spawn proxy
  ::  is set to L2
  ::
  ?:  ?=(%l2 -.point)  ~
  ::
  ?:  =(log-name escape-requested:log-names)
    ?>  ?=([@ ~] t.t.topics.log)
    =*  parent=@  i.t.t.topics.log
    =/  parent-point  (get-point state parent)
    ?>  ?=(^ parent-point)
    :+  ~  [%point ship %escape `parent]~
    point(escape.net `parent)
  ::
  ?:  =(log-name escape-canceled:log-names)
    ?>  ?=([@ ~] t.t.topics.log)
    =*  parent=@  i.t.t.topics.log
    =/  parent-point  (get-point state parent)
    ?>  ?=(^ parent-point)
    :+  ~  [%point ship %escape ~]~
    point(escape.net ~)
  ::
  ?:  =(log-name broke-continuity:log-names)
    ?>  ?=(~ t.t.topics.log)
    =*  rift=@  data.log
    :+  ~  [%point ship %rift rift]~
    point(rift.net rift)
  ::
  ?:  =(log-name changed-keys:log-names)
    ?>  ?=(~ t.t.topics.log)
    =/  =keys
      :*  life=(cut 8 [0 1] data.log)
          suite=(cut 8 [1 1] data.log)
          auth=(cut 8 [2 1] data.log)
          crypt=(cut 8 [3 1] data.log)
      ==
    :+  ~  [%point ship %keys keys]~
    point(keys.net keys)
  ::
  ?:  =(log-name owner-changed:log-names)
    ?>  ?=([@ ~] t.t.topics.log)
    =*  to  i.t.t.topics.log
    ::  Depositing to L2 is represented by an ownership change on L1,
    ::  but it doesn't change who actually owns the ship.
    ::
    ?:  =(deposit-address to)
      :+  ~  [%point ship %dominion %l2]~
      point(dominion %l2)
    :+  ~  [%point ship %owner to]~
    point(address.owner.own to)
  ::
  ?:  =(log-name changed-transfer-proxy:log-names)
    ?>  ?=([@ ~] t.t.topics.log)
    =*  to  i.t.t.topics.log
    :: XX
    :+  ~  [%point ship %transfer-proxy to]~
    point(address.transfer-proxy.own to)
  ::
  ?:  =(log-name changed-management-proxy:log-names)
    ?>  ?=([@ ~] t.t.topics.log)
    =*  to  i.t.t.topics.log
    :+  ~  [%point ship %management-proxy to]~
    point(address.management-proxy.own to)
  ::
  ?:  =(log-name changed-voting-proxy:log-names)
    ?>  ?=([@ ~] t.t.topics.log)
    =*  to  i.t.t.topics.log
    :+  ~  [%point ship %voting-proxy to]~
    point(address.voting-proxy.own to)
  ::
  (debug %unknown-log ~)
::
::  Receive batch of L2 transactions
::
++  receive-batch
  |=  [=verifier chain-id=@ud =state batch=@]
  =/  chain-t  (ud-to-ascii chain-id)
  =/  =roll  (parse-roll batch)
  |-  ^-  [effects ^state]
  ?~  roll
    [~ state]
  ::  Verify signature, else skip tx
  ::
  ?.  (verify-sig-and-nonce verifier chain-t state i.roll)
    %+  debug  %l2-sig-failed
    =^  effects  state  $(roll t.roll)
    :_  state
    [[%tx i.roll `%sig-or-nonce-failed] effects]
  ::  Increment nonce, even if it later fails
  ::
  =^  effects-1  points.state  (increment-nonce state from.tx.i.roll)
  ::  Process tx
  ::
  =^  effects-2  state
    =/  tx-result=(unit [=effects =^state])  (receive-tx state tx.i.roll)
    ?~  tx-result
      %+  debug  %l2-tx-failed
      [[%tx i.roll `%tx-failed]~ state]
    [[[%tx i.roll ~] effects.u.tx-result] state.u.tx-result]
  =^  effects-3  state  $(roll t.roll)
  [:(welp effects-1 effects-2 effects-3) state]
::
++  increment-nonce
  |=  [=state =ship =proxy]
  =/  point  (get-point state ship)
  ?>  ?=(^ point)  ::  we only parsed 4 bytes
  =*  own  own.u.point
  =^  nonce  u.point
    ?-    proxy
        %own
      :-  nonce.owner.own
      u.point(nonce.owner.own +(nonce.owner.own))
    ::
        %spawn
      :-  nonce.spawn-proxy.own
      u.point(nonce.spawn-proxy.own +(nonce.spawn-proxy.own))
    ::
        %manage
      :-  nonce.management-proxy.own
      u.point(nonce.management-proxy.own +(nonce.management-proxy.own))
    ::
        %vote
      :-  nonce.voting-proxy.own
      u.point(nonce.voting-proxy.own +(nonce.voting-proxy.own))
    ::
        %transfer
      :-  nonce.transfer-proxy.own
      u.point(nonce.transfer-proxy.own +(nonce.transfer-proxy.own))
    ==
  ::
  :-  [%nonce ship proxy nonce]~
  (put:orm points.state ship u.point)
::
::  Receive an individual L2 transaction
::
++  receive-tx
  |=  [=state =tx]
  |^
  ^-  (unit [effects ^state])
  ?-    +<.tx
      %spawn            (process-spawn +>.tx)
      %transfer-point   (w-point process-transfer-point ship.from.tx +>.tx)
      %configure-keys   (w-point process-configure-keys ship.from.tx +>.tx)
      %escape           (w-point-esc process-escape ship.from.tx +>.tx)
      %cancel-escape    (w-point-esc process-cancel-escape ship.from.tx +>.tx)
      %adopt            (w-point-esc process-adopt ship.tx +>.tx)
      %reject           (w-point-esc process-reject ship.tx +>.tx)
      %detach           (w-point-esc process-detach ship.tx +>.tx)
      %set-spawn-proxy
    (w-point-spawn process-set-spawn-proxy ship.from.tx +>.tx)
  ::
      %set-transfer-proxy
    (w-point process-set-transfer-proxy ship.from.tx +>.tx)
  ::
      %set-management-proxy
    (w-point process-set-management-proxy ship.from.tx +>.tx)
  ==
  ::
  ++  w-point
    |*  [fun=$-([ship point *] (unit [effects point])) =ship rest=*]
    ^-  (unit [effects ^state])
    =/  point  (get-point state ship)
    ?~  point  (debug %strange-ship ~)
    ?.  ?=(%l2 -.u.point)  (debug %ship-not-on-l2 ~)
    ::  Important to fully no-op on failure so we don't insert an entry
    ::  into points.state
    ::
    =/  res=(unit [=effects new-point=^point])  (fun u.point rest)
    ?~  res
      ~
    `[effects.u.res state(points (put:orm points.state ship new-point.u.res))]
  ::
  ++  w-point-esc
    |*  [fun=$-([ship point *] (unit [effects point])) =ship rest=*]
    ^-  (unit [effects ^state])
    =/  point  (get-point state ship)
    ?~  point  (debug %strange-ship ~)
    =/  res=(unit [=effects new-point=^point])  (fun u.point rest)
    ?~  res
      ~
    `[effects.u.res state(points (put:orm points.state ship new-point.u.res))]
  ::
  ++  w-point-spawn
    |*  [fun=$-([ship point *] (unit [effects point])) =ship rest=*]
    ^-  (unit [effects ^state])
    =/  point  (get-point state ship)
    ?~  point  (debug %strange-ship ~)
    ?:  ?=(%l1 -.u.point)  (debug %ship-on-l2 ~)
    =/  res=(unit [=effects new-point=^point])  (fun u.point rest)
    ?~  res
      ~
    `[effects.u.res state(points (put:orm points.state ship new-point.u.res))]
  ::
  ++  process-transfer-point
    |=  [=point to=address reset=?]
    =*  ship  ship.from.tx
    ::  Assert from owner or transfer prxoy
    ::
    ?.  |(=(%own proxy.from.tx) =(%transfer proxy.from.tx))
      (debug %bad-permission ~)
    ::  Execute transfer
    ::
    =/  effects-1
      ~[[%point ship %owner to] [%point ship %transfer-proxy *address]]
    =:  address.owner.own.point           to
        address.transfer-proxy.own.point  *address
      ==
    ::  Execute reset if requested
    ::
    ?.  reset
      `[effects-1 point]
    ::
    =^  effects-2  net.point
      ?:  =([0 0 0] +.keys.net.point)
        `net.point
      =/  =keys  [+(life.keys.net.point) 0 0 0]
      :-  [%point ship %keys keys]~
      [rift.net.point keys sponsor.net.point escape.net.point]
    =^  effects-3  rift.net.point
      ?:  =(0 life.keys.net.point)
        `rift.net.point
      :-  [%point ship %rift +(rift.net.point)]~
      +(rift.net.point)
    =/  effects-4
      :~  [%point ship %spawn-proxy *address]
          [%point ship %management-proxy *address]
          [%point ship %voting-proxy *address]
          [%point ship %transfer-proxy *address]
      ==
    =:  address.spawn-proxy.own.point       *address
        address.management-proxy.own.point  *address
        address.voting-proxy.own.point      *address
        address.transfer-proxy.own.point    *address
      ==
    `[:(welp effects-1 effects-2 effects-3 effects-4) point]
  ::
  ++  process-spawn
    |=  [=ship to=address]
    ^-  (unit [effects ^state])
    =/  parent=^ship  (sein ship)
    ::  Assert parent is on L2
    ::
    =/  parent-point  (get-point state parent)
    ?~  parent-point  ~
    ?.  ?=(?(%l2 %spawn) -.u.parent-point)  ~
    ::  Assert from owner or spawn proxy
    ::
    ?.  ?&  =(parent ship.from.tx)
            |(=(%own proxy.from.tx) =(%spawn proxy.from.tx))
        ==
      (debug %bad-permission ~)
    ::  Assert child not already spawned
    ::
    ?^  (get:orm points.state ship)  (debug %spawn-exists ~)
    ::  Assert one-level-down
    ::
    ?.  =(+((ship-rank parent)) (ship-rank ship))  (debug %bad-rank ~)
    ::
    =/  [=effects new-point=point]
      =/  point=(unit point)  (get-point state ship)
      ?>  ?=(^ point)  ::  only parsed 4 bytes
      ::  If spawning to self, just do it
      ::
      ?:  ?|  ?&  =(%own proxy.from.tx)
                  =(to address.owner.own.u.parent-point)
              ==
              ?&  =(%spawn proxy.from.tx)
                  =(to address.spawn-proxy.own.u.parent-point)
              ==
          ==
        :-  ~[[%point ship %dominion %l2] [%point ship %owner to]]
        u.point(address.owner.own to)
      ::  Else spawn to parent and set transfer proxy
      ::
      :-  :~  [%point ship %dominion %l2]
              [%point ship %owner address.owner.own.u.parent-point]
              [%point ship %transfer-proxy to]
          ==
      %=  u.point
        address.owner.own           address.owner.own.u.parent-point
        address.transfer-proxy.own  to
      ==
    `[effects state(points (put:orm points.state ship new-point))]
  ::
  ++  process-configure-keys
    |=  [=point crypt=@ auth=@ suite=@ breach=?]
    =*  ship  ship.from.tx
    ::
    ?.  |(=(%own proxy.from.tx) =(%manage proxy.from.tx))
      (debug %bad-permission ~)
    ::
    =^  rift-effects  rift.net.point
      ?.  breach
        `rift.net.point
      [[%point ship %rift +(rift.net.point)]~ +(rift.net.point)]
    ::
    =^  keys-effects  keys.net.point
      ?:  =(+.keys.net.point [suite auth crypt])
        `keys.net.point
      =/  =keys
        [+(life.keys.net.point) suite auth crypt]
      [[%point ship %keys keys]~ keys]
    ::
    `[(welp rift-effects keys-effects) point]
  ::
  ++  process-escape
    |=  [=point parent=ship]
    =*  ship  ship.from.tx
    ?.  |(=(%own proxy.from.tx) =(%manage proxy.from.tx))
      (debug %bad-permission ~)
    ::
    ?.  =(+((ship-rank parent)) (ship-rank ship))
      (debug %bad-rank ~)
    ::
    :+  ~  [%point ship %escape `parent]~
    point(escape.net `parent)
  ::
  ++  process-cancel-escape
    |=  [=point parent=ship]
    =*  ship  ship.from.tx
    ?.  |(=(%own proxy.from.tx) =(%manage proxy.from.tx))
      (debug %bad-permission ~)
    ::
    :+  ~  [%point ship %escape ~]~
    point(escape.net ~)
  ::
  ++  process-adopt
    |=  [=point =ship]
    =*  parent  ship.from.tx
    ?.  |(=(%own proxy.from.tx) =(%manage proxy.from.tx))
      (debug %bad-permission ~)
    ::
    ?.  =(escape.net.point `parent)  (debug %no-adopt ~)
    :+  ~  [%point ship %sponsor `parent]~
    point(escape.net ~, sponsor.net [%& parent])
  ::
  ++  process-reject
    |=  [=point =ship]
    =*  parent  ship.from.tx
    ?.  |(=(%own proxy.from.tx) =(%manage proxy.from.tx))
      (debug %bad-permission ~)
    ::
    ?.  =(escape.net.point `parent)  (debug %no-reject ~)
    :+  ~  [%point ship %escape ~]~
    point(escape.net ~)
  ::
  ++  process-detach
    |=  [=point =ship]
    =*  parent  ship.from.tx
    ?.  |(=(%own proxy.from.tx) =(%manage proxy.from.tx))
      (debug %bad-permission ~)
    ::
    ?.  =(who.sponsor.net.point parent)  (debug %no-detach ~)
    :+  ~  [%point ship %sponsor ~]~
    point(has.sponsor.net %|)
  ::
  ++  process-set-management-proxy
    |=  [=point =address]
    ?.  |(=(%own proxy.from.tx) =(%manage proxy.from.tx))
      (debug %bad-permission ~)
    ::
    :+  ~  [%point ship.from.tx %management-proxy address]~
    point(address.management-proxy.own address)
  ::
  ++  process-set-spawn-proxy
    |=  [=point =address]
    ?.  |(=(%own proxy.from.tx) =(%spawn proxy.from.tx))
      (debug %bad-permission ~)
    ::
    ?:  (gte (ship-rank ship.from.tx) 2)
      (debug %spawn-proxy-planet ~)
    ::
    :+  ~  [%point ship.from.tx %spawn-proxy address]~
    point(address.spawn-proxy.own address)
  ::
  ++  process-set-transfer-proxy
    |=  [=point =address]
    ?.  |(=(%own proxy.from.tx) =(%transfer proxy.from.tx))
      (debug %bad-permission ~)
    ::
    :+  ~  [%point ship.from.tx %transfer-proxy address]~
    point(address.transfer-proxy.own address)
  --
--
::
::  State transition function
::
|=  [=verifier chain-id=@ud =state =input]
^-  [effects ^state]
?:  ?=(%log +<.input)
  :: Received log from L1 transaction
  ::
  (receive-log state event-log.input)
::  Received L2 batch
::
::  %+  debug  %batch
(receive-batch verifier chain-id state batch.input)
