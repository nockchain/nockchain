::  This file aims to approach hoon.hoon, as the pieces necessary to run a live
::  ship with Ares as Serf are written. Required to run toddler.hoon as an Arvo.
::
!.
=>  %a50
~%  %a.50  ~  ~
|%
::
::  Types
::
+$  cord  @t
+$  knot  @ta
+$  term  @tas
+$  char  @t
+$  ship  @p
+$  life  @ud
+$  rift  @ud
+$  pass  @
+$  bloq  @
+$  step  _`@u`1
+$  bite  $@(bloq [=bloq =step])
+$  octs  [p=@ud q=@]
+$  dime  [p=@ta q=@]
+$  pint  [p=[p=@ q=@] q=[p=@ q=@]]
+$  spot  [p=path q=pint]
+$  mold  $~(* $-(* *))
++  unit  |$  [item]  $@(~ [~ u=item])
++  list  |$  [item]  $@(~ [i=item t=(list item)])
++  lest  |$  [item]  [i=item t=(list item)]
+$  tape  (list @tD)
+$  path  (list knot)
+$  coin  $~  [%$ %ud 0]
          $%  [%$ p=dime]
              [%blob p=*]
              [%many p=(list coin)]
          ==
+$  tone  $%  [%0 product=*]
              [%1 block=*]
              [%2 trace=(list [@ta *])]
          ==
++  pair
  |$  [head tail]
  [p=head q=tail]
++  trel
  |$  [first second third]
  [p=first q=second r=third]
++  qual
  |$  [first second third fourth]
  [p=first q=second r=third s=fourth]
+$  tank
  $+  tank
  $~  leaf/~
  $@  cord
  $%  [%leaf p=tape]
      [%palm p=(qual tape tape tape tape) q=(list tank)]
      [%rose p=(trel tape tape tape) q=(list tank)]
  ==
+$  tang  (list tank)
+$  toon  $%  [%0 p=*]
              [%1 p=*]
              [%2 p=tang]
          ==
++  tree  |$  [node]  $@(~ [n=node l=(tree node) r=(tree node)])
++  gate  $-(* *)
::
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
::  Basic arithmetic
::
++  add                                                 ::  unsigned addition
  ~/  %add
  |=  [a=@ b=@]
  ~>  %sham.%add
  ^-  @
  ?:  =(0 a)  b
  $(a (dec a), b +(b))
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
++  lte                                                 ::  less or equal
  ~/  %lte
  |=  [a=@ b=@]
  ~>  %sham.%lte
  |(=(a b) (lth a b))
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
++  mod                                                 ::  modulus
  ~/  %mod
  |:  [a=`@`1 b=`@`1]
  ~>  %sham.%mod
  ^-  @
  ?<  =(0 b)
  (sub a (mul b (div a b)))
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
::  Tree addressing
::
++  cap                                                 ::  index in head or tail
  ~/  %cap                                              
  |=  a=@
  ~>  %sham.%cap
  ^-  ?(%2 %3)
  ?-  a
    %2        %2
    %3        %3
    ?(%0 %1)  !!
    *         $(a (div a 2))
  ==
::
++  mas                                                 ::  axis within head/tail
  ~/  %mas
  |=  a=@
  ~>  %sham.%mas
  ^-  @
  ?-  a
    ?(%2 %3)  1
    ?(%0 %1)  !!
    *         (add (mod a 2) (mul $(a (div a 2)) 2))
  ==
::
::  List logic
::
++  snoc                                                ::  append to end of list
  |*  [a=(list) b=*]
  (weld a ^+(a [b]~))
::
++  flop                                                ::  reverse
  ~/  %flop
  |*  a=(list)
  ~>  %sham.%flop
  =>  .(a (homo a))
  ^+  a
  =+  b=`_a`~
  |-
  ?~  a  b
  $(a t.a, b [i.a b])
::
++  homo                                                ::  homogenize
  |*  a=(list)
  ^+  =<  $
    |@  ++  $  ?:(*? ~ [i=(snag 0 a) t=$])
    --
  a
::
++  lent                                                ::  length
  ~/  %lent
  |=  a=(list)
  ~>  %sham.%lent
  ^-  @
  =+  b=0
  |-
  ?~  a  b
  $(a t.a, b +(b))
::
++  reap                                                ::  replicate
  ~/  %reap
  |*  [a=@ b=*]
  |-  ^-  (list _b)
  ?~  a  ~
  [b $(a (dec a))]
::
++  roll                                                ::  left fold
  ~/  %roll
  |*  [a=(list) b=_=>(~ |=([* *] +<+))]
  |-  ^+  ,.+<+.b
  ?~  a
    +<+.b
  $(a t.a, b b(+<+ (b i.a +<+.b)))
::
++  scag                                                ::  prefix
  ~/  %scag
  |*  [a=@ b=(list)]
  |-  ^+  b
  ?:  |(?=(~ b) =(0 a))  ~
  [i.b $(b t.b, a (dec a))]
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
++  turn                                                ::  transform
  ~/  %turn
  |*  [a=(list) b=gate]
  =>  .(a (homo a))
  ^-  (list _?>(?=(^ a) (b i.a)))
  |-
  ?~  a  ~
  [i=(b i.a) t=$(a t.a)]
::
++  weld                                                ::  concatenate
  ~/  %weld
  |*  [a=(list) b=(list)]
  =>  .(a ^.(homo a), b ^.(homo b))
  |-  ^+  b
  ?~  a  b
  [i.a $(a t.a)]
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
::  Bit arithmetic
::
++  bex                                                 ::  binary exponent
  ~/  %bex
  |=  a=bloq
  ~>  %sham.%bex
  ^-  @
  ?:  =(0 a)  1
  (mul 2 $(a (dec a)))
++  can                                                 ::  assemble
  ~/  %can
  |=  [a=bloq b=(list [p=step q=@])]
  ~>  %sham.%can
  ^-  @
  ?~  b  0
  (add (end [a p.i.b] q.i.b) (lsh [a p.i.b] $(b t.b)))
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
++  end                                                 ::  tail
  ~/  %end
  |=  [a=bite b=@]
  ~>  %sham.%end
  =/  [=bloq =step]  ?^(a a [a *step])
  (mod b (bex (mul (bex bloq) step)))
::
++  fil                                                 ::  fill bloqstream
  ~/  %fil
  |=  [a=bloq b=step c=@]
  =|  n=@ud
  =.  c  (end a c)
  =/  d  c
  |-  ^-  @
  ?:  =(n b)
    (rsh a d)
  $(d (add c (lsh a d)), n +(n))
::
++  lsh                                                 ::  left-shift
  ~/  %lsh
  |=  [a=bite b=@]
  ~>  %sham.%lsh
  =/  [=bloq =step]  ?^(a a [a *step])
  (mul b (bex (mul (bex bloq) step)))
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
++  rap                                                 ::  assemble variable
  ~/  %rap
  |=  [a=bloq b=(list @)]
  ~>  %sham.%rap
  ^-  @
  ?~  b  0
  (cat a i.b $(b t.b))
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
++  rev                                                 ::  reverse block order
  ~/  %rev
  |=  [boz=bloq len=@ud dat=@]
  ~>  %sham.%rev
  ^-  @
  =.  dat  (end [boz len] dat)
  %+  lsh
    [boz (sub len (met boz dat))]
  (swp boz dat)
::
++  rip                                                 ::  disassemble
  ~/  %rip
  |=  [a=bite b=@]
  ~>  %sham.%rip
  ^-  (list @)
  ?:  =(0 b)  ~
  [(end a b) $(b (rsh a b))]
::
++  rsh                                                 ::  right-shift
  ~/  %rsh
  |=  [a=bite b=@]
  ~>  %sham.%rsh
  =/  [=bloq =step]  ?^(a a [a *step])
  (div b (bex (mul (bex bloq) step)))
::
++  swp                                                 ::  naive rev bloq order
  ~/  %swp
  |=  [a=bloq b=@]
  (rep a (flop (rip a b)))
::
++  xeb                                                 ::  binary logarithm
  ~/  %xeb
  |=  a=@
  ~>  %sham.%xeb
  ^-  @
  (met 0 a)
::
::  Modular arithmetic
::
++  fe                                                  ::  modulo bloq
  |_  a=bloq
  ++  dif                                               ::  difference
    |=([b=@ c=@] (sit (sub (add out (sit b)) (sit c))))
  ++  inv  |=(b=@ (sub (dec out) (sit b)))              ::  inverse
  ++  net  |=  b=@  ^-  @                               ::  flip byte endianness
           =>  .(b (sit b))
           ?:  (lte a 3)
             b
           =+  c=(dec a)
           %+  con
             (lsh c $(a c, b (cut c [0 1] b)))
           $(a c, b (cut c [1 1] b))
  ++  out  (bex (bex a))                                ::  mod value
  ++  rol  |=  [b=bloq c=@ d=@]  ^-  @                  ::  roll left
           =+  e=(sit d)
           =+  f=(bex (sub a b))
           =+  g=(mod c f)
           (sit (con (lsh [b g] e) (rsh [b (sub f g)] e)))
  ++  ror  |=  [b=bloq c=@ d=@]  ^-  @                  ::  roll right
           =+  e=(sit d)
           =+  f=(bex (sub a b))
           =+  g=(mod c f)
           (sit (con (rsh [b g] e) (lsh [b (sub f g)] e)))
  ++  sum  |=([b=@ c=@] (sit (add b c)))                ::  wrapping add
  ++  sit  |=(b=@ (end a b))                            ::  enforce modulo
  --
::
::  Bit logic
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
::  Hashes
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
::  Noun Ordering
::
++  dor                                                 ::  tree order
  ~/  %dor
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
::
++  gor                                                 ::  mug order
  ~/  %gor
  |=  [a=* b=*]
  ^-  ?
  =+  [c=(mug a) d=(mug b)]
  ?:  =(c d)
    (dor a b)
  (lth c d)
::
++  mor                                                 ::  more mug order
  ~/  %mor
  |=  [a=* b=*]
  ^-  ?
  =+  [c=(mug (mug a)) d=(mug (mug b))]
  ?:  =(c d)
    (dor a b)
  (lth c d)
::
::  Unsigned powers
::
++  pow                                                 ::  unsigned exponent
  ~/  %pow
  |=  [a=@ b=@]
  ?:  =(b 0)  1
  |-  ?:  =(b 1)  a
  =+  c=$(b (div b 2))
  =+  d=(mul c c)
  ?~  (dis b 1)  d  (mul d a)
::
::  Set logic
::
++  in
  ~/  %in
  =|  a=(tree)  :: (set)
  |@
  ++  apt
    =<  $
    ~/  %apt
    =|  [l=(unit) r=(unit)]
    |.  ^-  ?
    ?~  a   &
    ?&  ?~(l & (gor n.a u.l))
        ?~(r & (gor u.r n.a))
        ?~(l.a & ?&((mor n.a n.l.a) $(a l.a, l `n.a)))
        ?~(r.a & ?&((mor n.a n.r.a) $(a r.a, r `n.a)))
    ==
  ::
  ++  del
    ~/  %del
    |*  b=*
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
  ++  put
    ~/  %put
    |*  b=*
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
  --
::
::  Map logic
::
++  by
  ~/  %by
  =|  a=(tree (pair))  ::  (map)
  =*  node  ?>(?=(^ a) n.a)
  |@
  ++  del
    ~/  %del
    |*  b=*
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
    |.  ^-  ?
    ?~  a   &
    ?&  ?~(l & &((gor p.n.a u.l) !=(p.n.a u.l)))
        ?~(r & &((gor u.r p.n.a) !=(u.r p.n.a)))
        ?~  l.a   &
        &((mor p.n.a p.n.l.a) !=(p.n.a p.n.l.a) $(a l.a, l `p.n.a))
        ?~  r.a   &
        &((mor p.n.a p.n.r.a) !=(p.n.a p.n.r.a) $(a r.a, r `p.n.a))
    ==
  ::
  ++  get
    ~/  %get
    |*  b=*
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
    |-  ^+  a
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
  --
::
::  Jug logic
::
++  ju
  =|  a=(tree (pair * (tree)))  ::  (jug)
  |@
  ++  del
    |*  [b=* c=*]
    ^+  a
    =+  d=(get b)
    =+  e=(~(del in d) c)
    ?~  e
      (~(del by a) b)
    (~(put by a) b e)
  ::
  ++  get
    |*  b=*
    =+  c=(~(get by a) b)
    ?~(c ~ u.c)
  ::
  ++  put
    |*  [b=* c=*]
    ^+  a
    =+  d=(get b)
    (~(put by a) b (~(put in d) c))
  --
::
::  Parsing
::
++  ne
  |_  tig=@
  ++  d  (add tig '0')
  --
::
::  Atom printing
::
++  co
  !:
  ~%  %co  ..co  ~
  =<  |_  lot=coin
      ++  rend
        ^-  tape
        ~+
        ?.  ?=(%$ -.lot)  !!
        =+  [yed=(end 3 p.p.lot) hay=(cut 3 [1 1] p.p.lot)]
        |-  ^-  tape
        ?+    yed  !!
            %ud
          ::
          =/  gam=tape
            ((ox-co [10 3] |=(a=@ ~(d ne a))) q.p.lot)
          ?:  =(0 q.p.lot)
            ['0' ~]
          gam
        ==
      --
  =|  rep=tape
  |%
  ::  +em-co: format in numeric base
  ::
  ::    in .bas, format .min digits of .hol with .par
  ::
  ::    - .hol is processed least-significant digit first
  ::    - all available digits in .hol will be processed, but
  ::      .min digits can exceed the number available in .hol
  ::    - .par handles all accumulated output on each call,
  ::      and can edit it, prepend or append digits, &c
  ::    - until .hol is exhausted, .par's sample is [| digit output],
  ::      subsequently, it's [& 0 output]
  ::
  ++  em-co
    |=  [[bas=@ min=@] par=$-([? @ tape] tape)]
    |=  hol=@
    ^-  tape
    ?:  &(=(0 hol) =(0 min))
      rep
    =/  [dar=@ rad=@]  (dvr hol bas)
    %=  $
      min  ?:(=(0 min) 0 (dec min))
      hol  dar
      rep  (par =(0 dar) rad rep)
    ==
  ::
  ::  +ox-co: format '.'-separated digit sequences in numeric base
  ::
  ::    in .bas, format each digit of .hol with .dug,
  ::    with '.' separators every .gop digits.
  ::
  ::    - .hol is processed least-significant digit first
  ::    - .dug handles individual digits, output is prepended
  ::    - every segment but the last is zero-padded to .gop
  ::
  ++  ox-co
    |=  [[bas=@ gop=@] dug=$-(@ @)]
    %+  em-co
      [(pow bas gop) 0]
    |=  [top=? seg=@ res=tape]
    %+  weld
      ?:(top ~ `tape`['.' ~])
    %.  seg
    %+  em-co(rep res)
      [bas ?:(top 0 gop)]
    |=([? b=@ c=tape] [(dug b) c])
  --
::
::  Formatting functions
::
++  scow
  ~/  %scow
  |=  mol=dime
  ~>  %sham.%scow
  ~(rend co %$ mol)
--
