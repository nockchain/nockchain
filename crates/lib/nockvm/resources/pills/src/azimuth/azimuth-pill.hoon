::  A small pill that runs a snasphot of azimuth state against a few tens of
::  thousands of logs.  Requires naive-cradle.hoon,
::  mainnet.azimuth-snapshot, and log.jam from this scaffolding directory.
::
/+  naive=naive-cradle, orig-naive=naive, ethereum, dice
/*  snap  %azimuth-snapshot  /lib/mainnet/azimuth-snapshot
/*  logs  %jam  /lib/logs/jam
!.
=/  processed-logs
  =/  net   (get-network:dice %mainnet)
  =/  logs  ((list event-log:rpc:ethereum) (cue logs))
  |-  ^-  (list ^input:orig-naive)
  ?~  logs
    ~
  ?~  mined.i.logs
    $(logs t.logs)
  :-  :-  block-number.u.mined.i.logs
      ?:  =(azimuth.net address.i.logs)
        =/  data  (data-to-hex:dice data.i.logs)
        =/  =event-log:orig-naive
          [address.i.logs data topics.i.logs]
        [%log event-log]
      ?~  input.u.mined.i.logs
        [%bat *@]
      [%bat u.input.u.mined.i.logs]
  $(logs t.logs)
=/  core
  =>  ~
  !=
  =/  crad
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
  =>
  |%
  +$  card  (cask)
  ++  cask  |$  [a]  (pair mark a)
  +$  knot  @ta
  ++  list  |$  [item]  $@(~ [i=item t=(list item)])
  +$  mark  @tas
  +$  ovum  [=wire =card]
  ++  pair  |$  [head tail]  [p=head q=tail]
  +$  path  (list knot)
  +$  wire  path
  ++  verifier  |=([[@ @] @ @ @] `0x123)
  --  =>
  ::
  =|  naive=*
  =|  snap=*
  |%
  ++  load  !!
  ++  peek  _~
  ++  wish  !!
  ++  poke
    |=  [now=@da ovo=ovum]
    ^-  ^
    :: ~>  %slog.[0 'got']
    :: ~>  %slog.[0 -.card.ovo]
    ?:  =(%naive -.card.ovo)
      ~>  %slog.[0 'storing naive formula']
      `..poke(naive +.card.ovo)
    ?:  =(%snap -.card.ovo)
      ~>  %slog.[0 'storing azimuth snapshot']
      `..poke(snap +.card.ovo)
    ?:  =(%logs -.card.ovo)
      ~>  %slog.[0 'running logs']
      =/  logs  +.card.ovo
      =|  n=@
      =.  snap
        =/  nave  .*(0 naive)
        !.
        |-
        =+  ?:  =(0 (mod:crad n 1.000))
              ~>  %slog.[0 'ran a thousand logs']
              ~
            ~
        ?~  logs
          snap
        =.  snap
          ::  (naive verifier 1 snap i.logs)
          +:.*([nave verifier snap -.logs] [9 2 10 [6 [0 6] [1 1] [0 14] [0 15]] 0 2])
        $(logs +.logs, n +(n))
      ~>  %slog.[0 'done']
      `..poke
    =/  fec  [//term/1 %blit [%put "effect"] [%nel ~] ~]
    [[fec ~] ..poke]
  --
  ::
  |=  [now=@da ovo=ovum]
  ^-  *
  .(+> +:(poke now ovo))
::
|%
++  aeon
  ^-  *
  =>  *[arvo=* epic=*]
  !=
  =+  [arvo epic]=.*(epic arvo)
  |-  ^-  *
  ?@  epic  arvo
  %=  $
    epic  +.epic
    arvo  .*([arvo -.epic] [%9 2 %10 [6 %0 3] %0 2])
  ==
--
!.
:+  %pill  %baby
:_  [~ ~]
:~  aeon
    =>  *[arvo-formula=^ installed=^ tale=*]
    !=(=+(.*(0 arvo-formula) [installed tale]))
    core
    .*(0 core)
    [*@da / %naive naive]
    [*@da / %snap nas.snap]
    [*@da / %logs processed-logs]
==
