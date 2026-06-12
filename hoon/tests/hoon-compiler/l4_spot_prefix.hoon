=>
|%
++  hoon-version  +
--
=>
|%
::    unsigned arithmetic
+|  %math
++  add
  ~/  %add
  ::    unsigned addition
  ::
  ::  a: augend
  ::  b: addend
  |=  [a=@ b=@]
  ::  sum
  ^-  @
  ?:  =(0 a)  b
  $(a (dec a), b +(b))
::
++  dec
  ~/  %dec
  ::    unsigned decrement by one.
  |=  a=@
  ~_  leaf+"decrement-underflow"
  ?<  =(0 a)
  =+  b=0
  ::  decremented integer
  |-  ^-  @
  ?:  =(a +(b))  b
  $(b +(b))
::
++  div
  ~/  %div
  ::    unsigned divide
  ::
  ::  a: dividend
  ::  b: divisor
  |:  [a=`@`1 b=`@`1]
  ::  quotient
  ^-  @
  -:(dvr a b)
::
++  dvr
  ~/  %dvr
  ::    unsigned divide with remainder
  ::
  ::  a: dividend
  ::  b: divisor
  |:  [a=`@`1 b=`@`1]
  ::  p: quotient
  ::  q: remainder
  ^-  [p=@ q=@]
  ~_  leaf+"divide-by-zero"
  ?<  =(0 b)
  =+  c=0
  |-
  ?:  (lth a b)  [c a]
  $(a (sub a b), c +(c))
::
++  gte
  ~/  %gte
  ::    unsigned greater than or equals
  ::
  ::  returns whether {a >= b}.
  ::
  ::  a: left hand operand (todo: name)
  ::  b: right hand operand
  |=  [a=@ b=@]
  ::  greater than or equal to?
  ^-  ?
  !(lth a b)
::
++  gth
  ~/  %gth
  ::    unsigned greater than
  ::
  ::  returns whether {a > b}
  ::
  ::  a: left hand operand (todo: name)
  ::  b: right hand operand
  |=  [a=@ b=@]
  ::  greater than?
  ^-  ?
  !(lte a b)
::
++  lte
  ~/  %lte
  ::    unsigned less than or equals
  ::
  ::  returns whether {a >= b}.
  ::
  ::  a: left hand operand (todo: name)
  ::  b: right hand operand
  |=  [a=@ b=@]
  ::  less than or equal to?
  |(=(a b) (lth a b))
::
++  lth
  ~/  %lth
  ::    unsigned less than
  ::
  ::  a: left hand operand (todo: name)
  ::  b: right hand operand
  |=  [a=@ b=@]
  ::  less than?
  ^-  ?
  ?&  !=(a b)
      |-
      ?|  =(0 a)
          ?&  !=(0 b)
              $(a (dec a), b (dec b))
  ==  ==  ==
::
++  max
  ~/  %max
  ::    unsigned maximum
  |=  [a=@ b=@]
  ::  the maximum
  ^-  @
  ?:  (gth a b)  a
  b
::
++  min
  ~/  %min
  ::    unsigned minimum
  |=  [a=@ b=@]
  ::  the minimum
  ^-  @
  ?:  (lth a b)  a
  b
::
++  mod
  ~/  %mod
  ::    unsigned modulus
  ::
  ::  a: dividend
  ::  b: divisor
  |:  [a=`@`1 b=`@`1]
  ::  the remainder
  ^-  @
  +:(dvr a b)
::
++  mul
  ~/  %mul
  ::    unsigned multiplication
  ::
  ::  a: multiplicand
  ::  b: multiplier
  |:  [a=`@`1 b=`@`1]
  ::  product
  ^-  @
  =+  c=0
  |-
  ?:  =(0 a)  c
  $(a (dec a), c (add b c))
::
++  sub
  ~/  %sub
  ::    unsigned subtraction
  ::
  ::  a: minuend
  ::  b: subtrahend
  |=  [a=@ b=@]
  ~_  leaf+"subtract-underflow"
  ::  difference
  ^-  @
  ?:  =(0 b)  a
  $(a (dec a), b (dec b))
::
::    tree addressing
+|  %tree
++  cap
  ~/  %cap
  ::    tree head
  ::
  ::  tests whether an `a` is in the head or tail of a noun. produces %2 if it
  ::  is within the head, or %3 if it is within the tail.
  |=  a=@
  ^-  ?(%2 %3)
  ?-  a
    %2        %2
    %3        %3
    ?(%0 %1)  !!
    *         $(a (div a 2))
  ==
::
++  mas
  ~/  %mas
  ::    axis within head/tail
  ::
  ::  computes the axis of `a` within either the head or tail of a noun
  ::  (depends whether `a` lies within the the head or tail).
  |=  a=@
  ^-  @
  ?-  a
    ?(%2 %3)  1
    ?(%0 %1)  !!
    *         (add (mod a 2) (mul $(a (div a 2)) 2))
  ==
::
++  peg
  ~/  %peg
  ::    axis within axis
  ::
  ::  computes the axis of {b} within axis {a}.
  |=  [a=@ b=@]
  ?<  =(0 a)
  ?<  =(0 b)
  ::  a composed axis
  ^-  @
  ?-  b
    %1  a
    %2  (mul a 2)
    %3  +((mul a 2))
    *   (add (mod b 2) (mul $(b (div b 2)) 2))
  ==
::
::  #  %containers
::
::    the most basic of data types
+|  %containers
::
+$  bite
  ::    atom slice specifier
  ::
  $@(bloq [=bloq =step])
::
+$  bloq
  ::    blocksize
  ::
  ::  a blocksize is the power of 2 size of an atom. ie, 3 is a byte as 2^3 is
  ::  8 bits.
  @
::
++  each
  |$  [this that]
  ::    either {a} or {b}, defaulting to {a}.
  ::
  ::  mold generator: produces a discriminated fork between two types,
  ::  defaulting to {a}.
  ::
  $%  [%| p=that]
      [%& p=this]
  ==
::
+$  gate
  ::    function
  ::
  ::  a core with one arm, `$`--the empty name--which transforms a sample noun
  ::  into a product noun. If used dryly as a type, the subject must have a
  ::  sample type of `*`.
  $-(* *)
::
++  list
  |$  [item]
  ::    null-terminated list
  ::
  ::  mold generator: produces a mold of a null-terminated list of the
  ::  homogeneous type {a}.
  ::
  $@(~ [i=item t=(list item)])
::
++  lone
  |$  [item]
  ::    single item tuple
  ::
  ::  mold generator: puts the face of `p` on the passed in mold.
  ::
  p=item
::
++  lest
  |$  [item]
  ::    null-terminated non-empty list
  ::
  ::  mold generator: produces a mold of a null-terminated list of the
  ::  homogeneous type {a} with at least one element.
  [i=item t=(list item)]
::
+$  mold
  ::    normalizing gate
  ::
  ::  a gate that accepts any noun, and validates its shape, producing the
  ::  input if it fits or a default value if it doesn't.
  ::
  ::  examples: * @ud ,[p=time q=?(%a %b)]
  $~(* $-(* *))
::
++  pair
  |$  [head tail]
  ::    dual tuple
  ::
  ::  mold generator: produces a tuple of the two types passed in.
  ::
  ::  a: first type, labeled {p}
  ::  b: second type, labeled {q}
  ::
  [p=head q=tail]
::
++  pole
  |$  [item]
  ::    faceless list
  ::
  ::  like ++list, but without the faces {i} and {t}.
  ::
  $@(~ [item (pole item)])
::
++  qual
  |$  [first second third fourth]
  ::    quadruple tuple
  ::
  ::  mold generator: produces a tuple of the four types passed in.
  ::
  [p=first q=second r=third s=fourth]
::
++  quip
  |$  [item state]
  ::    pair of list of first and second
  ::
  ::  a common pattern in hoon code is to return a ++list of changes, along with
  ::  a new state.
  ::
  ::  a: type of list item
  ::  b: type of returned state
  ::
  [(list item) state]
::
++  step
  ::    atom size or offset, in bloqs
  ::
  _`@u`1
::
++  trap
  |$  [product]
  ::    a core with one arm `$`
  ::
  _|?($:product)
::
++  tree
  |$  [node]
  ::    tree mold generator
  ::
  ::  a `++tree` can be empty, or contain a node of a type and
  ::  left/right sub `++tree` of the same type. pretty-printed with `{}`.
  ::
  $@(~ [n=node l=(tree node) r=(tree node)])
::
++  trel
  |$  [first second third]
  ::    triple tuple
  ::
  ::  mold generator: produces a tuple of the three types passed in.
  ::
  [p=first q=second r=third]
::
++  unit
  |$  [item]
  ::    maybe
  ::
  ::  mold generator: either `~` or `[~ u=a]` where `a` is the
  ::  type that was passed in.
  ::
  $@(~ [~ u=item])
--
=>
|%
::    2a: unit logic
+|  %unit-logc
::
++  biff                                                ::  apply
  |*  [a=(unit) b=$-(* (unit))]
  ?~  a  ~
  (b u.a)
::
++  bind                                                ::  argue
  |*  [a=(unit) b=gate]
  ?~  a  ~
  [~ u=(b u.a)]
::
++  bond                                                ::  replace
  |*  a=(trap)
  |*  b=(unit)
  ?~  b  $:a
  u.b
::
++  both                                                ::  all the above
  |*  [a=(unit) b=(unit)]
  ?~  a  ~
  ?~  b  ~
  [~ u=[u.a u.b]]
::
++  clap                                                ::  combine
  |*  [a=(unit) b=(unit) c=_=>(~ |=(^ +<-))]
  ?~  a  b
  ?~  b  a
  [~ u=(c u.a u.b)]
::
++  clef                                                ::  compose
  |*  [a=(unit) b=(unit) c=_=>(~ |=(^ `+<-))]
  ?~  a  ~
  ?~  b  ~
  (c u.a u.b)
::
++  drop                                                ::  enlist
  |*  a=(unit)
  ?~  a  ~
  [i=u.a t=~]
::
++  fall                                                ::  default
  |*  [a=(unit) b=*]
  ?~(a b u.a)
::
++  flit                                                ::  make filter
  |*  a=$-(* ?)
  |*  b=*
  ?.((a b) ~ [~ u=b])
::
++  hunt                                                ::  first of units
  |*  [ord=$-(^ ?) a=(unit) b=(unit)]
  ^-  %-  unit
      $?  _?>(?=(^ a) u.a)
          _?>(?=(^ b) u.b)
      ==
  ?~  a  b
  ?~  b  a
  ?:((ord u.a u.b) a b)
::
++  lift                                                ::  lift mold (fmap)
  |*  a=mold                                            ::  flipped
  |*  b=(unit)                                          ::  curried
  (bind b a)                                            ::  bind
::
++  mate                                                ::  choose
  |*  [a=(unit) b=(unit)]
  ?~  b  a
  ?~  a  b
  ?.(=(u.a u.b) ~>(%mean.'mate' !!) a)
::
++  need                                                ::  demand
  ~/  %need
  |*  a=(unit)
  ?~  a  ~>(%mean.'need' !!)
  u.a
::
++  some                                                ::  lift (pure)
  |*  a=*
  [~ u=a]
::
::    2b: list logic
+|  %list-logic
::  +snoc: append an element to the end of a list
::
++  snoc
  |*  [a=(list) b=*]
  (weld a ^+(a [b]~))
::
::  +lure: List pURE
++  lure
  |*  a=*
  [i=a t=~]
::
++  fand                                                ::  all indices
  ~/  %fand
  |=  [nedl=(list) hstk=(list)]
  =|  i=@ud
  =|  fnd=(list @ud)
  |-  ^+  fnd
  =+  [n=nedl h=hstk]
  |-
  ?:  |(?=(~ n) ?=(~ h))
    (flop fnd)
  ?:  =(i.n i.h)
    ?~  t.n
      ^$(i +(i), hstk +.hstk, fnd [i fnd])
    $(n t.n, h t.h)
  ^$(i +(i), hstk +.hstk)
::
++  find                                                ::  first index
  ~/  %find
  |=  [nedl=(list) hstk=(list)]
  =|  i=@ud
  |-   ^-  (unit @ud)
  =+  [n=nedl h=hstk]
  |-
  ?:  |(?=(~ n) ?=(~ h))
     ~
  ?:  =(i.n i.h)
    ?~  t.n
      `i
    $(n t.n, h t.h)
  ^$(i +(i), hstk +.hstk)
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
++  gulf                                                ::  range inclusive
  |=  [a=@ b=@]
  ?>  (lte a b)
  |-  ^-  (list @)
  ?:(=(a +(b)) ~ [a $(a +(a))])
::
++  homo                                                ::  homogenize
  |*  a=(list)
  ^+  =<  $
    |@  ++  $  ?:(*? ~ [i=(snag 0 a) t=$])
    --
  a
::  +join: construct a new list, placing .sep between every pair in .lit
::
++  join
  |*  [sep=* lit=(list)]
  =.  sep  `_?>(?=(^ lit) i.lit)`sep
  ?~  lit  ~
  =|  out=(list _?>(?=(^ lit) i.lit))
  |-  ^+  out
  ?~  t.lit
    (flop [i.lit out])
  $(out [sep i.lit out], lit t.lit)
::
::  +bake: convert wet gate to dry gate by specifying argument mold
::
++  bake
  |*  [f=gate a=mold]
  |=  arg=a
  (f arg)
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
++  levy
  ~/  %levy                                             ::  all of
  |*  [a=(list) b=$-(* ?)]
  |-  ^-  ?
  ?~  a  &
  ?.  (b i.a)  |
  $(a t.a)
::
++  lien                                                ::  some of
  ~/  %lien
  |*  [a=(list) b=$-(* ?)]
  |-  ^-  ?
  ?~  a  |
  ?:  (b i.a)  &
  $(a t.a)
::
++  limo                                                ::  listify
  |*  a=*
  ^+  =<  $
    |@  ++  $  ?~(a ~ ?:(*? [i=-.a t=$] $(a +.a)))
    --
  a
::
++  murn                                                ::  maybe transform
  ~/  %murn
  |*  [a=(list) b=$-(* (unit))]
  =>  .(a (homo a))
  |-  ^-  (list _?>(?=(^ a) (need (b i.a))))
  ?~  a  ~
  =/  c  (b i.a)
  ?~  c  $(a t.a)
  [+.c $(a t.a)]
::
++  oust                                                ::  remove
  ~/  %oust
  |*  [[a=@ b=@] c=(list)]
  (weld (scag +<-< c) (slag (add +<-< +<->) c))
::
++  reap                                                ::  replicate
  ~/  %reap
  |*  [a=@ b=*]
  |-  ^-  (list _b)
  ?~  a  ~
  [b $(a (dec a))]
::
++  rear                                                ::  last item of list
  ~/  %rear
  |*  a=(list)
  ^-  _?>(?=(^ a) i.a)
  ?>  ?=(^ a)
  ?:  =(~ t.a)  i.a  ::NOTE  avoiding tmi
  $(a t.a)
::
++  reel                                                ::  right fold
  ~/  %reel
  |*  [a=(list) b=_=>(~ |=([* *] +<+))]
  |-  ^+  ,.+<+.b
  ?~  a
    +<+.b
  (b i.a $(a t.a))
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
++  skid                                                ::  separate
  ~/  %skid
  |*  [a=(list) b=$-(* ?)]
  |-  ^+  [p=a q=a]
  ?~  a  [~ ~]
  =+  c=$(a t.a)
  ?:((b i.a) [[i.a p.c] q.c] [p.c [i.a q.c]])
::
++  skim                                                ::  only
  ~/  %skim
  |*  [a=(list) b=$-(* ?)]
  |-
  ^+  a
  ?~  a  ~
  ?:((b i.a) [i.a $(a t.a)] $(a t.a))
::
++  skip                                                ::  except
  ~/  %skip
  |*  [a=(list) b=$-(* ?)]
  |-
  ^+  a
  ?~  a  ~
  ?:((b i.a) $(a t.a) [i.a $(a t.a)])
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
++  snip                                                ::  drop tail off list
  ~/  %snip
  |*  a=(list)
  ^+  a
  ?~  a  ~
  ?:  =(~ t.a)  ~
  [i.a $(a t.a)]
::
++  sort  !.                                            ::  quicksort
  ~/  %sort
  |*  [a=(list) b=$-([* *] ?)]
  =>  .(a ^.(homo a))
  |-  ^+  a
  ?~  a  ~
  =+  s=(skid t.a |:(c=i.a (b c i.a)))
  %+  weld
    $(a p.s)
  ^+  t.a
  [i.a $(a q.s)]
::
++  spin                                                ::  stateful turn
  ::
  ::  a: list
  ::  b: state
  ::  c: gate from list-item and state to product and new state
  ~/  %spin
  |*  [a=(list) b=* c=_|=(^ [** +<+])]
  =>  .(c `$-([_?>(?=(^ a) i.a) _b] [_-:(c) _b])`c)
  =/  acc=(list _-:(c))  ~
  ::  transformed list and updated state
  |-  ^-  (pair _acc _b)
  ?~  a
    [(flop acc) b]
  =^  res  b  (c i.a b)
  $(acc [res acc], a t.a)
::
++  spun                                                ::  internal spin
  ::
  ::  a: list
  ::  b: gate from list-item and state to product and new state
  ~/  %spun
  |*  [a=(list) b=_|=(^ [** +<+])]
  ::  transformed list
  p:(spin a +<+.b b)
::
++  swag                                                ::  slice
  |*  [[a=@ b=@] c=(list)]
  (scag +<-> (slag +<-< c))
::  +turn: transform each value of list :a using the function :b
::
++  turn
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
++  snap                                               ::  replace item
  ~/  %snap
  |*  [a=(list) b=@ c=*]
  ^+  a
  (weld (scag b a) [c (slag +(b) a)])
::
++  into                                               ::  insert item
  ~/  %into
  |*  [a=(list) b=@ c=*]
  ^+  a
  (weld (scag b a) [c (slag b a)])
::
++  welp                                                ::  faceless weld
  ~/  %welp
  |*  [* *]
  ?~  +<-
    +<-(. +<+)
  +<-(+ $(+<- +<->))
::
++  zing                                                ::  promote
  ~/  %zing
  |*  *
  ?~  +<
    +<
  (welp +<- $(+< +<+))
::
::    2c: bit arithmetic
+|  %bit-arithmetic
::
++  bex                                                 ::  binary exponent
  ~/  %bex
  |=  a=bloq
  ^-  @
  ?:  =(0 a)  1
  (mul 2 $(a (dec a)))
::
++  can                                                 ::  assemble
  ~/  %can
  |=  [a=bloq b=(list [p=step q=@])]
  ^-  @
  ?~  b  0
  (add (end [a p.i.b] q.i.b) (lsh [a p.i.b] $(b t.b)))
::
++  cat                                                 ::  concatenate
  ~/  %cat
  |=  [a=bloq b=@ c=@]
  (add (lsh [a (met a b)] c) b)
::
++  cut                                                 ::  slice
  ~/  %cut
  |=  [a=bloq [b=step c=step] d=@]
  (end [a c] (rsh [a b] d))
::
++  end                                                 ::  tail
  ~/  %end
  |=  [a=bite b=@]
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
  =/  [=bloq =step]  ?^(a a [a *step])
  (mul b (bex (mul (bex bloq) step)))
::
++  met                                                 ::  measure
  ~/  %met
  |=  [a=bloq b=@]
  ^-  @
  =+  c=0
  |-
  ?:  =(0 b)  c
  $(b (rsh a b), c +(c))
::
++  rap                                                 ::  assemble variable
  ~/  %rap
  |=  [a=bloq b=(list @)]
  ^-  @
  ?~  b  0
  (cat a i.b $(b t.b))
::
++  rep                                                 ::  assemble fixed
  ~/  %rep
  |=  [a=bite b=(list @)]
  =/  [=bloq =step]  ?^(a a [a *step])
  =|  i=@ud
  |-  ^-  @
  ?~  b   0
  %+  add  $(i +(i), b t.b)
  (lsh [bloq (mul step i)] (end [bloq step] i.b))
::
++  rev
  ::    reverses block order, accounting for leading zeroes
  ::
  ::  boz: block size
  ::  len: size of dat, in boz
  ::  dat: data to flip
  ~/  %rev
  |=  [boz=bloq len=@ud dat=@]
  ^-  @
  =.  dat  (end [boz len] dat)
  %+  lsh
    [boz (sub len (met boz dat))]
  (swp boz dat)
::
++  rip                                                 ::  disassemble
  ~/  %rip
  |=  [a=bite b=@]
  ^-  (list @)
  ?:  =(0 b)  ~
  [(end a b) $(b (rsh a b))]
::
++  rsh                                                 ::  right-shift
  ~/  %rsh
  |=  [a=bite b=@]
  =/  [=bloq =step]  ?^(a a [a *step])
  (div b (bex (mul (bex bloq) step)))
::
++  run                                                 ::  +turn into atom
  ~/  %run
  |=  [a=bite b=@ c=$-(@ @)]
  (rep a (turn (rip a b) c))
::
++  rut                                                 ::  +turn into list
  ~/  %rut
  |*  [a=bite b=@ c=$-(@ *)]
  (turn (rip a b) c)
::
++  sew                                                 ::  stitch into
  ~/  %sew
  |=  [a=bloq [b=step c=step d=@] e=@]
  ^-  @
  %+  add
    (can a b^e c^d ~)
  =/  f  [a (add b c)]
  (lsh f (rsh f e))
::
++  swp                                                 ::  naive rev bloq order
  ~/  %swp
  |=  [a=bloq b=@]
  (rep a (flop (rip a b)))
::
++  xeb                                                 ::  binary logarithm
  ~/  %xeb
  |=  a=@
  ^-  @
  (met 0 a)
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
::    2d: bit logic
+|  %bit-logic
::
++  con                                                 ::  binary or
  ~/  %con
  |=  [a=@ b=@]
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
++  not  |=  [a=bloq b=@ c=@]                           ::  binary not (sized)
  (mix c (dec (bex (mul b (bex a)))))
::
::    2e: insecure hashing
+|  %insecure-hashing
::
++  muk                                                 ::  standard murmur3
  ~%  %muk  ..muk  ~
  =+  ~(. fe 5)
  |=  [syd=@ len=@ key=@]
  =.  syd      (end 5 syd)
  =/  pad      (sub len (met 3 key))
  =/  data     (weld (rip 3 key) (reap pad 0))
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
::                                                      ::
::    2f: noun ordering
+|  %noun-ordering
::
::  +aor: alphabetical order
::
::    Orders atoms before cells, and atoms in ascending LSB order.
::
++  aor
  ~/  %aor
  |=  [a=* b=*]
  ^-  ?
  ?:  =(a b)  &
  ?.  ?=(@ a)
    ?:  ?=(@ b)  |
    ?:  =(-.a -.b)
      $(a +.a, b +.b)
    $(a -.a, b -.b)
  ?.  ?=(@ b)  &
  |-
  =+  [c=(end 3 a) d=(end 3 b)]
  ?:  =(c d)
    $(a (rsh 3 a), b (rsh 3 b))
  (lth c d)
::  +dor: depth order
::
::    Orders in ascending tree depth.
::
++  dor
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
::  +gor: mug order
::
::    Orders in ascending +mug hash order, collisions fall back to +dor.
::
++  gor
  ~/  %gor
  |=  [a=* b=*]
  ^-  ?
  =+  [c=(mug a) d=(mug b)]
  ?:  =(c d)
    (dor a b)
  (lth c d)
::  +mor: (more) mug order
::
::    Orders in ascending double +mug hash order, collisions fall back to +dor.
::
++  mor
  ~/  %mor
  |=  [a=* b=*]
  ^-  ?
  =+  [c=(mug (mug a)) d=(mug (mug b))]
  ?:  =(c d)
    (dor a b)
  (lth c d)
::
::    2g: unsigned powers
+|  %unsigned-powers
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
++  sqt                                                 ::  unsigned sqrt/rem
  ~/  %sqt
  |=  a=@  ^-  [p=@ q=@]
  ?~  a  [0 0]
  =+  [q=(div (dec (xeb a)) 2) r=0]
  =-  [-.b (sub a +.b)]
  ^=  b  |-
  =+  s=(add r (bex q))
  =+  t=(mul s s)
  ?:  =(q 0)
    ?:((lte t a) [s t] [r (mul r r)])
  ?:  (lte t a)
    $(r s, q (dec q))
  $(q (dec q))
::
::    2h: set logic
+|  %set-logic
::
++  in                                                  ::  set engine
  ~/  %in
  =|  a=(tree)  :: (set)
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
    ~/  %apt
    =|  [l=(unit) r=(unit)]
    |.  ^-  ?
    ?~  a   &
    ?&  ?~(l & &((gor n.a u.l) !=(n.a u.l)))
        ?~(r & &((gor u.r n.a) !=(u.r n.a)))
        ?~(l.a & ?&((mor n.a n.l.a) !=(n.a n.l.a) $(a l.a, l `n.a)))
        ?~(r.a & ?&((mor n.a n.r.a) !=(n.a n.r.a) $(a r.a, r `n.a)))
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
    ?:  (gor b n.a)
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
  ++  dif                                               ::  difference
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
    ?:  (mor n.d n.e)
      d(r $(d r.d))
    e(l $(e l.e))
  ::
  ++  dig                                               ::  axis of a in b
    |=  b=*
    =+  c=1
    |-  ^-  (unit @)
    ?~  a  ~
    ?:  =(b n.a)  [~ u=(peg c 2)]
    ?:  (gor b n.a)
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
  ::  +has: does :b exist in :a?
  ::
  ++  has
    ~/  %has
    |*  b=*
    ^-  ?
    ::    wrap extracted item type in a unit because bunting fails
    ::
    ::  If we used the real item type of _?^(a n.a !!) as the sample type,
    ::  then hoon would bunt it to create the default sample for the gate.
    ::
    ::  However, bunting that expression fails if :a is ~. If we wrap it
    ::  in a unit, the bunted unit doesn't include the bunted item type.
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
    ?:  (gor b n.a)
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
    ?.  (mor n.a n.b)
      $(a b, b a)
    ?:  =(n.b n.a)
      a(l $(a l.a, b l.b), r $(a r.a, b r.b))
    ?:  (gor n.b n.a)
      %-  uni(a $(a l.a, r.b ~))  $(b r.b)
    %-  uni(a $(a r.a, l.b ~))  $(b l.b)
  ::
  ++  put                                               ::  puts b in a, sorted
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
    =+  c=`(set _?>(?=(^ a) (b n.a)))`~
    |-  ?~  a  c
    =.  c  (~(put in c) (b n.a))
    =.  c  $(a l.a, c c)
    $(a r.a, c c)
  ::
  ++  tap                                               ::  convert to list
    =<  $
    ~/  %tap
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
    ?:  (mor n.a n.b)
      ?:  (gor n.b n.a)
        $(l.a $(a l.a, r.b ~), b r.b)
      $(r.a $(a r.a, l.b ~), b l.b)
    ?:  (gor n.a n.b)
      $(l.b $(b l.b, r.a ~), a r.a)
    $(r.b $(b r.b, l.a ~), a l.a)
  ::
  ++  wyt                                               ::  size of set
    =<  $
    ~%  %wyt  +  ~
    |.  ^-  @
    ?~(a 0 +((add $(a l.a) $(a r.a))))
  --
::
::    2i: map logic
+|  %map-logic
::
++  by                                                  ::  map engine
  ~/  %by
  =|  a=(tree (pair))  ::  (map)
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
  ++  bif                                               ::  splits a by b
    ~/  %bif
    |*  b=*
    |-  ^+  [l=a r=a]
    ?~  a
      [~ ~]
    ?:  =(b p.n.a)
      +.a
    ?:  (gor b p.n.a)
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
    ?:  (mor p.n.d p.n.e)
      d(r $(d r.d))
    e(l $(e l.e))
  ::
  ++  dig                                               ::  axis of b key
    |=  b=*
    =+  c=1
    |-  ^-  (unit @)
    ?~  a  ~
    ?:  =(b p.n.a)  [~ u=(peg c 2)]
    ?:  (gor b p.n.a)
      $(a l.a, c (peg c 6))
    $(a r.a, c (peg c 7))
  ::
  ++  apt                                               ::  check correctness
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
  ++  gas                                               ::  concatenate
    ~/  %gas
    |*  b=(list [p=* q=*])
    =>  .(b `(list _?>(?=(^ a) n.a))`b)
    |-  ^+  a
    ?~  b
      a
    $(b t.b, a (put p.i.b q.i.b))
  ::
  ++  get                                               ::  grab value by key
    ~/  %get
    |*  b=*
    =>  .(b `_?>(?=(^ a) p.n.a)`b)
    |-  ^-  (unit _?>(?=(^ a) q.n.a))
    ?~  a
      ~
    ?:  =(b p.n.a)
      (some q.n.a)
    ?:  (gor b p.n.a)
      $(a l.a)
    $(a r.a)
  ::
  ++  got                                               ::  need value by key
    |*  b=*
    (need (get b))
  ::
  ++  gut                                               ::  fall value by key
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
    ?:  (mor p.n.a p.n.b)
      ?:  =(p.n.b p.n.a)
        b(l $(a l.a, b l.b), r $(a r.a, b r.b))
      ?:  (gor p.n.b p.n.a)
        %-  uni(a $(a l.a, r.b ~))  $(b r.b)
      %-  uni(a $(a r.a, l.b ~))  $(b l.b)
    ?:  =(p.n.a p.n.b)
      b(l $(b l.b, a l.a), r $(b r.b, a r.a))
    ?:  (gor p.n.a p.n.b)
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
    ?:  (gor key p.n.a)
      a(l $(a l.a))
    ::
    a(r $(a r.a))
  ::
  ++  mar                                               ::  add with validation
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
  ++  rep                                               ::  reduce to product
    ~/  %rep
    |*  b=_=>(~ |=([* *] +<+))
    |-
    ?~  a  +<+.b
    $(a r.a, +<+.b $(a l.a, +<+.b (b n.a +<+.b)))
  ::
  ++  rib                                               ::  transform + product
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
    ~/  %tap
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
    ?:  (mor p.n.a p.n.b)
      ?:  (gor p.n.b p.n.a)
        $(l.a $(a l.a, r.b ~), b r.b)
      $(r.a $(a r.a, l.b ~), b l.b)
    ?:  (gor p.n.a p.n.b)
      $(l.b $(b l.b, r.a ~), a r.a)
    $(r.b $(b r.b, l.a ~), a l.a)
  ::
  ++  uno                                               ::  general union
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
    ?:  (mor p.n.a p.n.b)
      ?:  (gor p.n.b p.n.a)
        $(l.a $(a l.a, r.b ~), b r.b)
      $(r.a $(a r.a, l.b ~), b l.b)
    ?:  (gor p.n.a p.n.b)
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
  ++  wyt                                               ::  depth of map
    =<  $
    ~%  %wyt  +  ~
    |.  ^-  @
    ?~(a 0 +((add $(a l.a) $(a r.a))))
  ::
  ++  key                                               ::  set of keys
    =<  $
    ~/  %key
    =+  b=`(set _?>(?=(^ a) p.n.a))`~
    |.  ^+  b
    ?~  a   b
    $(a r.a, b $(a l.a, b (~(put in b) p.n.a)))
  ::
  ++  val                                               ::  list of vals
    =+  b=`(list _?>(?=(^ a) q.n.a))`~
    |-  ^+  b
    ?~  a   b
    $(a r.a, b [q.n.a $(a l.a)])
  --
::
::    2j: jar and jug logic
+|  %jar-and-jug-logic
++  ja                                                  ::  jar engine
  =|  a=(tree (pair * (list)))  ::  (jar)
  |@
  ++  get                                               ::  gets list by key
    |*  b=*
    =+  c=(~(get by a) b)
    ?~(c ~ u.c)
  ::
  ++  add                                               ::  adds key-list pair
    |*  [b=* c=*]
    =+  d=(get b)
    (~(put by a) b [c d])
  ::
  ++  zip                                               ::  listify jar
    =<  $
    ~/  %zip
    =+  b=`(list _?>(?=([[* ^] *] a) [p=p q=i.q]:n.a))`~
    |.  ^+  b
    ?~  a   b
    %=  $
      a  r.a
      b  |-  ^+  b
         ?~  q.n.a  ^$(a l.a)
         [[p i.q]:n.a $(q.n.a t.q.n.a)]
    ==
  --
++  ju                                                  ::  jug engine
  =|  a=(tree (pair * (tree)))  ::  (jug)
  |@
  ++  del                                               ::  del key-set pair
    |*  [b=* c=*]
    ^+  a
    =+  d=(get b)
    =+  e=(~(del in d) c)
    ?~  e
      (~(del by a) b)
    (~(put by a) b e)
  ::
  ++  gas                                               ::  concatenate
    |*  b=(list [p=* q=*])
    =>  .(b `(list _?>(?=([[* ^] ^] a) [p=p q=n.q]:n.a))`b)
    |-  ^+  a
    ?~  b
      a
    $(b t.b, a (put p.i.b q.i.b))
  ::
  ++  get                                               ::  gets set by key
    |*  b=*
    =+  c=(~(get by a) b)
    ?~(c ~ u.c)
  ::
  ++  has                                               ::  existence check
    |*  [b=* c=*]
    ^-  ?
    (~(has in (get b)) c)
  ::
  ++  put                                               ::  add key-set pair
    |*  [b=* c=*]
    ^+  a
    =+  d=(get b)
    (~(put by a) b (~(put in d) c))
  --
::
::    2k: queue logic
+|  %queue-logic
::
++  to                                                  ::  queue engine
  =|  a=(tree)  ::  (qeu)
  |@
  ++  apt                                               ::  check correctness
    |-  ^-  ?
    ?~  a  &
    ?&  ?~(l.a & ?&((mor n.a n.l.a) $(a l.a)))
        ?~(r.a & ?&((mor n.a n.r.a) $(a r.a)))
    ==
  ::
  ++  bal
    |-  ^+  a
    ?~  a  ~
    ?.  |(?=(~ l.a) (mor n.a n.l.a))
      $(a l.a(r $(a a(l r.l.a))))
    ?.  |(?=(~ r.a) (mor n.a n.r.a))
      $(a r.a(l $(a a(r l.r.a))))
    a
  ::
  ++  dep                                               ::  max depth of queue
    |-  ^-  @
    ?~  a  0
    +((max $(a l.a) $(a r.a)))
  ::
  ++  gas                                               ::  insert list to que
    |=  b=(list _?>(?=(^ a) n.a))
    |-  ^+  a
    ?~(b a $(b t.b, a (put i.b)))
  ::
  ++  get                                               ::  head-rest pair
    |-  ^+  ?>(?=(^ a) [p=n.a q=*(tree _n.a)])
    ?~  a
      !!
    ?~  r.a
      [n.a l.a]
    =+  b=$(a r.a)
    :-  p.b
    ?:  |(?=(~ q.b) (mor n.a n.q.b))
      a(r q.b)
    a(n n.q.b, l a(r l.q.b), r r.q.b)
  ::
  ++  nip                                               ::  removes root
    |-  ^+  a
    ?~  a  ~
    ?~  l.a  r.a
    ?~  r.a  l.a
    ?:  (mor n.l.a n.r.a)
      l.a(r $(l.a r.l.a))
    r.a(l $(r.a l.r.a))
  ::
  ++  nap                                               ::  removes root
    ?>  ?=(^ a)
    ?:  =(~ l.a)  r.a
    =+  b=get(a l.a)
    bal(n.a p.b, l.a q.b)
  ::
  ++  put                                               ::  insert new tail
    |*  b=*
    |-  ^+  a
    ?~  a
      [b ~ ~]
    bal(l.a $(a l.a))
  ::
  ++  run                                               ::  apply gate to values
    |*  b=gate
    |-
    ?~  a  a
    [n=(b n.a) l=$(a l.a) r=$(a r.a)]
  ::
  ++  tap                                               ::  adds list to end
    =+  b=`(list _?>(?=(^ a) n.a))`~
    |-  ^+  b
    =+  0                                               ::  hack for jet match
    ?~  a
      b
    $(a r.a, b [n.a $(a l.a)])
  ::
  ++  top                                               ::  produces head
    |-  ^-  (unit _?>(?=(^ a) n.a))
    ?~  a  ~
    ?~(r.a [~ n.a] $(a r.a))
  --
::
::    2l: container from container
+|  %container-from-container
::
++  malt                                                ::  map from list
  |*  a=(list)
  (molt `(list [p=_-<.a q=_->.a])`a)
::
++  molt                                                ::  map from pair list
  |*  a=(list (pair))  ::  ^-  =,(i.-.a (map _p _q))
  (~(gas by `(tree [p=_p.i.-.a q=_q.i.-.a])`~) a)
::
++  silt                                                ::  set from list
  |*  a=(list)  ::  ^-  (set _i.-.a)
  =+  b=*(tree _?>(?=(^ a) i.a))
  (~(gas in b) a)
::
::    2m: container from noun
+|  %container-from-noun
::
++  ly                                                  ::  list from raw noun
  le:nl
::
++  my                                                  ::  map from raw noun
  my:nl
::
++  sy                                                  ::  set from raw noun
  si:nl
::
++  nl
  |%
  ::                                                    ::
  ++  le                                                ::  construct list
    |*  a=(list)
    ^+  =<  $
      |@  ++  $  ?:(*? ~ [i=(snag 0 a) t=$])
      --
    a
  ::                                                    ::
  ++  my                                                ::  construct map
    |*  a=(list (pair))
    =>  .(a ^+((le a) a))
    (~(gas by `(map _p.i.-.a _q.i.-.a)`~) a)
  ::                                                    ::
  ++  si                                                ::  construct set
    |*  a=(list)
    =>  .(a ^+((le a) a))
    (~(gas in `(set _i.-.a)`~) a)
  ::                                                    ::
  ++  snag                                              ::  index
    |*  [a=@ b=(list)]
    ?~  b
      ~_  leaf+"snag-fail"
      !!
    ?:  =(0 a)  i.b
    $(b t.b, a (dec a))
  ::                                                    ::
  ++  weld                                              ::  concatenate
    |*  [a=(list) b=(list)]
    =>  .(a ^+((le a) a), b ^+((le b) b))
    =+  42
    |-
    ?~  a  b
    [i=i.a t=$(a t.a)]
  --
::    2n: functional hacks
+|  %functional-hacks
::
++  aftr  |*(a=$-(* *) |*(b=$-(* *) (pair b a)))        ::  pair after
++  cork  |*([a=$-(* *) b=$-(* *)] (corl b a))          ::  compose forward
++  corl                                                ::  compose backwards
  |*  [a=$-(* *) b=$-(* *)]
  =<  +:|.((a (b)))      ::  type check
  |*  c=_,.+<.b
  (a (b c))
::
++  cury                                                ::  curry left
  |*  [a=$-(^ *) b=*]
  |*  c=_,.+<+.a
  (a b c)
::
++  curr                                                ::  curry right
  |*  [a=$-(^ *) c=*]
  |*  b=_,.+<-.a
  (a b c)
::
++  fore  |*(a=$-(* *) |*(b=$-(* *) (pair a b)))        ::  pair before
::
++  head  |*(^ ,:+<-)                                   ::  get head
++  same  |*(* +<)                                      ::  identity
::
++  succ  |=(@ +(+<))                                   ::  successor
::
++  tail  |*(^ ,:+<+)                                   ::  get tail
++  test  |=(^ =(+<- +<+))                              ::  equality
::
++  lead  |*(* |*(* [+>+< +<]))                         ::  put head
++  late  |*(* |*(* [+< +>+<]))                         ::  put tail
::
::    2o: containers
+|  %containers
++  jar  |$  [key value]  (map key (list value))        ::  map of lists
++  jug  |$  [key value]  (map key (set value))         ::  map of sets
::
++  map
  |$  [key value]                                       ::  table
  $|  (tree (pair key value))
  |=(a=(tree (pair)) ?:(=(~ a) & ~(apt by a)))
::
++  qeu
  |$  [item]                                            ::  queue
  $|  (tree item)
  |=(a=(tree) ?:(=(~ a) & ~(apt to a)))
::
++  set
  |$  [item]                                            ::  set
  $|  (tree item)
  |=(a=(tree) ?:(=(~ a) & ~(apt in a)))
::
::    2p: serialization
+|  %serialization
::
++  cue                                                 ::  unpack
  ~/  %cue
  |=  a=@
  ^-  *
  =+  b=0
  =+  m=`(map @ *)`~
  =<  q
  |-  ^-  [p=@ q=* r=(map @ *)]
  ?:  =(0 (cut 0 [b 1] a))
    =+  c=(rub +(b) a)
    [+(p.c) q.c (~(put by m) b q.c)]
  =+  c=(add 2 b)
  ?:  =(0 (cut 0 [+(b) 1] a))
    =+  u=$(b c)
    =+  v=$(b (add p.u c), m r.u)
    =+  w=[q.u q.v]
    [(add 2 (add p.u p.v)) w (~(put by r.v) b w)]
  =+  d=(rub c a)
  [(add 2 p.d) (need (~(get by m) q.d)) m]
::
++  jam                                                 ::  pack
  ~/  %jam
  |=  a=*
  ^-  @
  =+  b=0
  =+  m=`(map * @)`~
  =<  q
  |-  ^-  [p=@ q=@ r=(map * @)]
  =+  c=(~(get by m) a)
  ?~  c
    =>  .(m (~(put by m) a b))
    ?:  ?=(@ a)
      =+  d=(mat a)
      [(add 1 p.d) (lsh 0 q.d) m]
    =>  .(b (add 2 b))
    =+  d=$(a -.a)
    =+  e=$(a +.a, b (add b p.d), m r.d)
    [(add 2 (add p.d p.e)) (mix 1 (lsh [0 2] (cat 0 q.d q.e))) r.e]
  ?:  ?&(?=(@ a) (lte (met 0 a) (met 0 u.c)))
    =+  d=(mat a)
    [(add 1 p.d) (lsh 0 q.d) m]
  =+  d=(mat u.c)
  [(add 2 p.d) (mix 3 (lsh [0 2] q.d)) m]
::
++  mat                                                 ::  length-encode
  ~/  %mat
  |=  a=@
  ^-  [p=@ q=@]
  ?:  =(0 a)
    [1 1]
  =+  b=(met 0 a)
  =+  c=(met 0 b)
  :-  (add (add c c) b)
  (cat 0 (bex c) (mix (end [0 (dec c)] b) (lsh [0 (dec c)] a)))
::
++  rub                                                 ::  length-decode
  ~/  %rub
  |=  [a=@ b=@]
  ^-  [p=@ q=@]
  =+  ^=  c
      =+  [c=0 m=(met 0 b)]
      |-  ?<  (gth c m)
      ?.  =(0 (cut 0 [(add a c) 1] b))
        c
      $(c +(c))
  ?:  =(0 c)
    [1 0]
  =+  d=(add a +(c))
  =+  e=(add (bex (dec c)) (cut 0 [d (dec c)] b))
  [(add (add c c) e) (cut 0 [(add d (dec c)) e] b)]
::
++  fn  ::    float, infinity, or NaN
        ::
        ::  s=sign, e=exponent, a=arithmetic form
        ::  (-1)^s * a * 2^e
        $%  [%f s=? e=@s a=@u]
            [%i s=?]
            [%n ~]
        ==
::
++  dn  ::    decimal float, infinity, or NaN
        ::
        ::  (-1)^s * a * 10^e
        $%  [%d s=? e=@s a=@u]
            [%i s=?]
            [%n ~]
        ==
::
++  rn  ::    parsed decimal float
        ::
        $%  [%d a=? b=[c=@ [d=@ e=@] f=? i=@]]
            [%i a=?]
            [%n ~]
        ==
::
::    2q: molds and mold builders
+|  %molds-and-mold-builders
::
+$  axis  @                                             ::  tree address
+$  bean  ?                                             ::  0=&=yes, 1=|=no
+$  flag  ?
+$  char  @t                                            ::  UTF8 byte
+$  cord  @t                                            ::  UTF8, LSB first
+$  byts  [wid=@ud dat=@]                               ::  bytes, MSB first
+$  date  [[a=? y=@ud] m=@ud t=tarp]                    ::  parsed date
+$  knot  @ta                                           ::  ASCII text
+$  noun  *                                             ::  any noun
+$  path  (list knot)                                   ::  like unix path
+$  pith  (list iota)                                   ::  typed urbit path
+$  stud                                                ::  standard name
          $@  mark=@tas                                 ::  auth=urbit
          $:  auth=@tas                                 ::  standards authority
              type=path                                 ::  standard label
          ==                                            ::
+$  tang  (list tank)                                   ::  bottom-first error
::                                                      ::
+$  iota                                                ::  typed path segment
  $+  iota
  $~  [%n ~]
  $@  @tas
  $%  [%ub @ub]  [%uc @uc]  [%ud @ud]  [%ui @ui]
      [%ux @ux]  [%uv @uv]  [%uw @uw]
      [%sb @sb]  [%sc @sc]  [%sd @sd]  [%si @si]
      [%sx @sx]  [%sv @sv]  [%sw @sw]
      [%da @da]  [%dr @dr]
      [%f ?]     [%n ~]
      [%if @if]  [%is @is]
      [%t @t]    [%ta @ta]  ::  @tas
      [%p @p]    [%q @q]
      [%rs @rs]  [%rd @rd]  [%rh @rh]  [%rq @rq]
  ==
::
::  $tank: formatted print tree
::
::    just a cord, or
::    %leaf: just a tape
::    %palm: backstep list
::           flat-mid, open, flat-open, flat-close
::    %rose: flat list
::           flat-mid, open, close
::
+$  tank
  $+  tank
  $~  leaf/~
  $@  cord
  $%  [%leaf p=tape]
      [%palm p=(qual tape tape tape tape) q=(list tank)]
      [%rose p=(trel tape tape tape) q=(list tank)]
  ==
::
+$  tape  (list @tD)                                    ::  utf8 string as list
+$  tour  (list @c)                                     ::  utf32 clusters
+$  tarp  [d=@ud h=@ud m=@ud s=@ud f=(list @ux)]        ::  parsed time
+$  term  @tas                                          ::  ascii symbol
+$  wain  (list cord)                                   ::  text lines
+$  wall  (list tape)                                   ::  text lines
::
--
=>
|%
::    3a: signed and modular ints
+|  %signed-and-modular-ints
::
++  egcd                                                ::  schneier's egcd
  |=  [a=@ b=@]
  =+  si
  =+  [c=(sun a) d=(sun b)]
  =+  [u=[c=(sun 1) d=--0] v=[c=--0 d=(sun 1)]]
  |-  ^-  [d=@ u=@s v=@s]
  ?:  =(--0 c)
    [(abs d) d.u d.v]
  ::  ?>  ?&  =(c (sum (pro (sun a) c.u) (pro (sun b) c.v)))
  ::          =(d (sum (pro (sun a) d.u) (pro (sun b) d.v)))
  ::      ==
  =+  q=(fra d c)
  %=  $
    c  (dif d (pro q c))
    d  c
    u  [(dif d.u (pro q c.u)) c.u]
    v  [(dif d.v (pro q c.v)) c.v]
  ==
::
++  fo                                                  ::  modulo prime
  ^|
  |_  a=@
  ++  dif
    |=  [b=@ c=@]
    (sit (sub (add a b) (sit c)))
  ::
  ++  exp
    |=  [b=@ c=@]
    ?:  =(0 b)
      1
    =+  d=$(b (rsh 0 b))
    =+  e=(pro d d)
    ?:(=(0 (end 0 b)) e (pro c e))
  ::
  ++  fra
    |=  [b=@ c=@]
    (pro b (inv c))
  ::
  ++  inv
    |=  b=@
    =+  c=(dul:si u:(egcd b a) a)
    c
  ::
  ++  pro
    |=  [b=@ c=@]
    (sit (mul b c))
  ::
  ++  sit
    |=  b=@
    (mod b a)
  ::
  ++  sum
    |=  [b=@ c=@]
    (sit (add b c))
  --
::
++  si                                                  ::  signed integer
  ^?
  |%
  ++  abs  |=(a=@s (add (end 0 a) (rsh 0 a)))           ::  absolute value
  ++  dif  |=  [a=@s b=@s]                              ::  subtraction
           (sum a (new !(syn b) (abs b)))
  ++  dul  |=  [a=@s b=@]                               ::  modulus
           =+(c=(old a) ?:(-.c (mod +.c b) (sub b +.c)))
  ++  fra  |=  [a=@s b=@s]                              ::  divide
           (new =(0 (mix (syn a) (syn b))) (div (abs a) (abs b)))
  ++  new  |=  [a=? b=@]                                ::  [sign value] to @s
           `@s`?:(a (mul 2 b) ?:(=(0 b) 0 +((mul 2 (dec b)))))
  ++  old  |=(a=@s [(syn a) (abs a)])                   ::  [sign value]
  ++  pro  |=  [a=@s b=@s]                              ::  multiplication
           (new =(0 (mix (syn a) (syn b))) (mul (abs a) (abs b)))
  ++  rem  |=([a=@s b=@s] (dif a (pro b (fra a b))))    ::  remainder
  ++  sum  |=  [a=@s b=@s]                              ::  addition
           =+  [c=(old a) d=(old b)]
           ?:  -.c
             ?:  -.d
               (new & (add +.c +.d))
             ?:  (gte +.c +.d)
               (new & (sub +.c +.d))
             (new | (sub +.d +.c))
           ?:  -.d
             ?:  (gte +.c +.d)
               (new | (sub +.c +.d))
             (new & (sub +.d +.c))
           (new | (add +.c +.d))
  ++  sun  |=(a=@u (mul 2 a))                           ::  @u to @s
  ++  syn  |=(a=@s =(0 (end 0 a)))                      ::  sign test
  ++  cmp  |=  [a=@s b=@s]                              ::  compare
           ^-  @s
           ?:  =(a b)
             --0
           ?:  (syn a)
             ?:  (syn b)
               ?:  (gth a b)
                 --1
               -1
             --1
          ?:  (syn b)
            -1
          ?:  (gth a b)
            -1
          --1
  --
::
::    3b: floating point
+|  %floating-point
::
++  fl                                                  ::  arb. precision fp
  =/  [[p=@u v=@s w=@u] r=$?(%n %u %d %z %a) d=$?(%d %f %i)]
    [[113 -16.494 32.765] %n %d]
  ::  p=precision:     number of bits in arithmetic form; must be at least 2
  ::  v=min exponent:  minimum value of e
  ::  w=width:         max - min value of e, 0 is fixed point
  ::  r=rounding mode: nearest (ties to even), up, down, to zero, away from zero
  ::  d=behavior:      return denormals, flush denormals to zero,
  ::                   infinite exponent range
  =>
    ~%  %cofl  +>  ~
    ::    cofl
    ::
    ::  internal functions; mostly operating on [e=@s a=@u], in other words
    ::  positive numbers. many of these error out if a=0.
    |%
    ++  rou
      |=  [a=[e=@s a=@u]]  ^-  fn  (rau a &)
    ::
    ++  rau
      |=  [a=[e=@s a=@u] t=?]  ^-  fn
      ?-  r
        %z  (lug %fl a t)  %d  (lug %fl a t)
        %a  (lug %ce a t)  %u  (lug %ce a t)
        %n  (lug %ne a t)
      ==
    ::
    ++  add                                             ::  add; exact if e
      |=  [a=[e=@s a=@u] b=[e=@s a=@u] e=?]  ^-  fn
      =+  q=(dif:si e.a e.b)
      |-  ?.  (syn:si q)  $(b a, a b, q +(q))           ::  a has larger exp
      ?:  e
        [%f & e.b (^add (lsh [0 (abs:si q)] a.a) a.b)]
      =+  [ma=(met 0 a.a) mb=(met 0 a.b)]
      =+  ^=  w  %+  dif:si  e.a  %-  sun:si            ::  expanded exp of a
        ?:  (gth prc ma)  (^sub prc ma)  0
      =+  ^=  x  %+  sum:si  e.b  (sun:si mb)           ::  highest exp for b
      ?:  =((cmp:si w x) --1)                           ::  don't need to add
        ?-  r
          %z  (lug %fl a &)  %d  (lug %fl a &)
          %a  (lug %lg a &)  %u  (lug %lg a &)
          %n  (lug %na a &)
        ==
      (rou [e.b (^add (lsh [0 (abs:si q)] a.a) a.b)])
    ::
    ++  sub                                             ::  subtract; exact if e
      |=  [a=[e=@s a=@u] b=[e=@s a=@u] e=?]  ^-  fn
      =+  q=(dif:si e.a e.b)
      |-  ?.  (syn:si q)
        (fli $(b a, a b, q +(q), r swr))
      =+  [ma=(met 0 a.a) mb=(met 0 a.b)]
      =+  ^=  w  %+  dif:si  e.a  %-  sun:si
        ?:  (gth prc ma)  (^sub prc ma)  0
      =+  ^=  x  %+  sum:si  e.b  (sun:si +(mb))
      ?:  &(!e =((cmp:si w x) --1))
        ?-  r
          %z  (lug %sm a &)  %d  (lug %sm a &)
          %a  (lug %ce a &)  %u  (lug %ce a &)
          %n  (lug %nt a &)
        ==
      =+  j=(lsh [0 (abs:si q)] a.a)
      |-  ?.  (gte j a.b)
        (fli $(a.b j, j a.b, r swr))
      =+  i=(^sub j a.b)
      ?~  i  [%f & zer]
      ?:  e  [%f & e.b i]  (rou [e.b i])
    ::
    ++  mul                                             ::  multiply
      |=  [a=[e=@s a=@u] b=[e=@s a=@u]]  ^-  fn
      (rou (sum:si e.a e.b) (^mul a.a a.b))
    ::
    ++  div                                             ::  divide
      |=  [a=[e=@s a=@u] b=[e=@s a=@u]]  ^-  fn
      =+  [ma=(met 0 a.a) mb=(met 0 a.b)]
      =+  v=(dif:si (sun:si ma) (sun:si +((^add mb prc))))
      =.  a  ?:  (syn:si v)  a
      a(e (sum:si v e.a), a (lsh [0 (abs:si v)] a.a))
      =+  [j=(dif:si e.a e.b) q=(dvr a.a a.b)]
      (rau [j p.q] =(q.q 0))
    ::
    ++  sqt                                             ::  square root
      |=  [a=[e=@s a=@u]]  ^-  fn
      =.  a
        =+  [w=(met 0 a.a) x=(^mul +(prc) 2)]
        =+  ?:((^lth w x) (^sub x w) 0)
        =+  ?:  =((dis - 1) (dis (abs:si e.a) 1))  -
          (^add - 1)
        a(e (dif:si e.a (sun:si -)), a (lsh [0 -] a.a))
      =+  [y=(^sqt a.a) z=(fra:si e.a --2)]
      (rau [z p.y] =(q.y 0))
    ::
    ++  lth                                             ::  less-than
      |=  [a=[e=@s a=@u] b=[e=@s a=@u]]  ^-  ?
      ?:  =(e.a e.b)  (^lth a.a a.b)
      =+  c=(cmp:si (ibl a) (ibl b))
      ?:  =(c -1)  &  ?:  =(c --1)  |
      ?:  =((cmp:si e.a e.b) -1)
        (^lth (rsh [0 (abs:si (dif:si e.a e.b))] a.a) a.b)
      (^lth (lsh [0 (abs:si (dif:si e.a e.b))] a.a) a.b)
    ::
    ++  equ                                             ::  equals
      |=  [a=[e=@s a=@u] b=[e=@s a=@u]]  ^-  ?
      ?.  =((ibl a) (ibl b))  |
      ?:  =((cmp:si e.a e.b) -1)
        =((lsh [0 (abs:si (dif:si e.a e.b))] a.b) a.a)
      =((lsh [0 (abs:si (dif:si e.a e.b))] a.a) a.b)
    ::
    ::    integer binary logarithm: 2^ibl(a) <= |a| < 2^(ibl(a)+1)
    ++  ibl
      |=  [a=[e=@s a=@u]]  ^-  @s
      (sum:si (sun:si (dec (met 0 a.a))) e.a)
    ::
    ::  +uni
    ::
    ::    change to a representation where a.a is odd
    ::    every fn has a unique representation of this kind
    ++  uni
      |=  [a=[e=@s a=@u]]
      |-  ?:  =((end 0 a.a) 1)  a
      $(a.a (rsh 0 a.a), e.a (sum:si e.a --1))
    ::
    ::  +xpd: expands to either full precision or to denormalized
    ++  xpd
      |=  [a=[e=@s a=@u]]
      =+  ma=(met 0 a.a)
      ?:  (gte ma prc)  a
      =+  ?:  =(den %i)  (^sub prc ma)
          =+  ^=  q
            =+  w=(dif:si e.a emn)
            ?:  (syn:si w)  (abs:si w)  0
          (min q (^sub prc ma))
      a(e (dif:si e.a (sun:si -)), a (lsh [0 -] a.a))
    ::
    ::  +lug: central rounding mechanism
    ::
    ::    can perform: floor, ceiling, smaller, larger,
    ::                 nearest (round ties to: even, away from 0, toward 0)
    ::    s is sticky bit: represents a value less than ulp(a) = 2^(e.a)
    ::
    ++  lug
      ~/  %lug
      |=  [t=$?(%fl %ce %sm %lg %ne %na %nt) a=[e=@s a=@u] s=?]  ^-  fn
      ?<  =(a.a 0)
      =-
        ?.  =(den %f)  -                                ::  flush denormals
        ?.  ?=([%f *] -)  -
        ?:  =((met 0 ->+>) prc)  -  [%f & zer]
      ::
      =+  m=(met 0 a.a)
      ?>  |(s (gth m prc))                              ::  require precision
      =+  ^=  q  %+  max
          ?:  (gth m prc)  (^sub m prc)  0              ::  reduce precision
        %-  abs:si  ?:  =(den %i)  --0                  ::  enforce min. exp
        ?:  =((cmp:si e.a emn) -1)  (dif:si emn e.a)  --0
      =^  b  a  :-  (end [0 q] a.a)
        a(e (sum:si e.a (sun:si q)), a (rsh [0 q] a.a))
      ::
      ?~  a.a
        ?<  =(den %i)
        ?-  t
          %fl  [%f & zer]
          %sm  [%f & zer]
          %ce  [%f & spd]
          %lg  [%f & spd]
          %ne  ?:  s  [%f & ?:((lte b (bex (dec q))) zer spd)]
               [%f & ?:((^lth b (bex (dec q))) zer spd)]
          %nt  ?:  s  [%f & ?:((lte b (bex (dec q))) zer spd)]
               [%f & ?:((^lth b (bex (dec q))) zer spd)]
          %na  [%f & ?:((^lth b (bex (dec q))) zer spd)]
        ==
      ::
      =.  a  (xpd a)
      ::
      =.  a
        ?-  t
          %fl  a
          %lg  a(a +(a.a))
          %sm  ?.  &(=(b 0) s)  a
               ?:  &(=(e.a emn) !=(den %i))  a(a (dec a.a))
               =+  y=(dec (^mul a.a 2))
               ?.  (lte (met 0 y) prc)  a(a (dec a.a))
               [(dif:si e.a --1) y]
          %ce  ?:  &(=(b 0) s)  a  a(a +(a.a))
          %ne  ?~  b  a
               =+  y=(bex (dec q))
               ?:  &(=(b y) s)                          ::  round halfs to even
                 ?~  (dis a.a 1)  a  a(a +(a.a))
               ?:  (^lth b y)  a  a(a +(a.a))
          %na  ?~  b  a
               =+  y=(bex (dec q))
               ?:  (^lth b y)  a  a(a +(a.a))
          %nt  ?~  b  a
               =+  y=(bex (dec q))
               ?:  =(b y)  ?:  s  a  a(a +(a.a))
               ?:  (^lth b y)  a  a(a +(a.a))
        ==
      ::
      =.  a  ?.  =((met 0 a.a) +(prc))  a
        a(a (rsh 0 a.a), e (sum:si e.a --1))
      ?~  a.a  [%f & zer]
      ::
      ?:  =(den %i)  [%f & a]
      ?:  =((cmp:si emx e.a) -1)  [%i &]  [%f & a]      ::  enforce max. exp
    ::
    ++  drg                                             ::  dragon4; get
      ~/  %drg                                          ::  printable decimal;
      |=  [a=[e=@s a=@u]]  ^-  [@s @u]                  ::  guaranteed accurate
      ?<  =(a.a 0)                                      ::  for rounded floats
      =.  a  (xpd a)
      =+  r=(lsh [0 ?:((syn:si e.a) (abs:si e.a) 0)] a.a)
      =+  s=(lsh [0 ?.((syn:si e.a) (abs:si e.a) 0)] 1)
      =+  mn=(lsh [0 ?:((syn:si e.a) (abs:si e.a) 0)] 1)
      =+  mp=mn
      =>  ?.
            ?&  =(a.a (bex (dec prc)))                  ::  if next smallest
                |(!=(e.a emn) =(den %i))                ::  float is half ULP,
            ==                                          ::  tighten lower bound
          .
        %=  .
          mp  (lsh 0 mp)
          r  (lsh 0 r)
          s  (lsh 0 s)
        ==
      =+  [k=--0 q=(^div (^add s 9) 10)]
      |-  ?:  (^lth r q)
        %=  $
          k  (dif:si k --1)
          r  (^mul r 10)
          mn  (^mul mn 10)
          mp  (^mul mp 10)
        ==
      |-  ?:  (gte (^add (^mul r 2) mp) (^mul s 2))
        $(s (^mul s 10), k (sum:si k --1))
      =+  [u=0 o=0]
      |-                                                ::  r/s+o = a*10^-k
      =+  v=(dvr (^mul r 10) s)
      =>  %=  .
          k  (dif:si k --1)
          u  p.v
          r  q.v
          mn  (^mul mn 10)
          mp  (^mul mp 10)
        ==
      =+  l=(^lth (^mul r 2) mn)                        ::  in lower bound
      =+  ^=  h                                         ::  in upper bound
        ?|  (^lth (^mul s 2) mp)
            (gth (^mul r 2) (^sub (^mul s 2) mp))
        ==
      ?:  &(!l !h)
        $(o (^add (^mul o 10) u))
      =+  q=&(h |(!l (gth (^mul r 2) s)))
      =.  o  (^add (^mul o 10) ?:(q +(u) u))
      [k o]
    ::
    ++  toj                                             ::  round to integer
      |=  [a=[e=@s a=@u]]  ^-  fn
      ?.  =((cmp:si e.a --0) -1)  [%f & a]
      =+  x=(abs:si e.a)
      =+  y=(rsh [0 x] a.a)
      ?:  |(=(r %d) =(r %z))  [%f & --0 y]
      =+  z=(end [0 x] a.a)
      ?:  |(=(r %u) =(r %a))  [%f & --0 ?~(z y +(y))]
      =+  i=(bex (dec x))
      ?:  &(=(z i) =((dis y 1) 0))  [%f & --0 y]
      ?:  (^lth z i)  [%f & --0 y]  [%f & --0 +(y)]
    ::
    ++  ned                                             ::  require ?=([%f *] a)
      |=  [a=fn]  ^-  [%f s=? e=@s a=@u]
      ?:  ?=([%f *] a)  a
      ~_  leaf+"need-float"
      !!
    ::
    ++  shf                                             ::  a * 2^b; no rounding
      |=  [a=fn b=@s]
      ?:  |(?=([%n *] a) ?=([%i *] a))  a
      a(e (sum:si e.a b))
    ::
    ++  fli                                             ::  flip sign
      |=  [a=fn]  ^-  fn
      ?-(-.a %f a(s !s.a), %i a(s !s.a), %n a)
    ::
    ++  swr  ?+(r r %d %u, %u %d)                       ::  flipped rounding
    ++  prc  ?>((gth p 1) p)                            ::  force >= 2 precision
    ++  den  d                                          ::  denorm+flush+inf exp
    ++  emn  v                                          ::  minimum exponent
    ++  emx  (sum:si emn (sun:si w))                    ::  maximum exponent
    ++  spd  [e=emn a=1]                                ::  smallest denormal
    ++  spn  [e=emn a=(bex (dec prc))]                  ::  smallest normal
    ++  lfn  [e=emx a=(fil 0 prc 1)]                    ::  largest
    ++  lfe  (sum:si emx (sun:si prc))                  ::  2^lfe is > than all
    ++  zer  [e=--0 a=0]
    --
  |%
  ++  rou                                               ::  round
    |=  [a=fn]  ^-  fn
    ?.  ?=([%f *] a)  a
    ?~  a.a  [%f s.a zer]
    ?:  s.a  (^rou +>.a)
    =.(r swr (fli (^rou +>.a)))
  ::
  ++  syn                                               ::  get sign
    |=  [a=fn]  ^-  ?
    ?-(-.a %f s.a, %i s.a, %n &)
  ::
  ++  abs                                               ::  absolute value
    |=  [a=fn]  ^-  fn
    ?:  ?=([%f *] a)  [%f & e.a a.a]
    ?:  ?=([%i *] a)  [%i &]  [%n ~]
  ::
  ++  add                                               ::  add
    |=  [a=fn b=fn]  ^-  fn
    ?:  |(?=([%n *] a) ?=([%n *] b))  [%n ~]
    ?:  |(?=([%i *] a) ?=([%i *] b))
      ?:  &(?=([%i *] a) ?=([%i *] b))
        ?:  =(a b)  a  [%n ~]
      ?:  ?=([%i *] a)  a  b
    ?:  |(=(a.a 0) =(a.b 0))
      ?.  &(=(a.a 0) =(a.b 0))  %-  rou  ?~(a.a b a)
      [%f ?:(=(r %d) &(s.a s.b) |(s.a s.b)) zer]
    %-  |=  [a=fn]
        ?.  ?=([%f *] a)  a
        ?.  =(a.a 0)  a
        [%f !=(r %d) zer]
    ?:  =(s.a s.b)
      ?:  s.a  (^add +>.a +>.b |)
      =.(r swr (fli (^add +>.a +>.b |)))
    ?:  s.a  (^sub +>.a +>.b |)
    (^sub +>.b +>.a |)
  ::
  ++  ead                                               ::  exact add
    |=  [a=fn b=fn]  ^-  fn
    ?:  |(?=([%n *] a) ?=([%n *] b))  [%n ~]
    ?:  |(?=([%i *] a) ?=([%i *] b))
      ?:  &(?=([%i *] a) ?=([%i *] b))
        ?:  =(a b)  a  [%n ~]
      ?:  ?=([%i *] a)  a  b
    ?:  |(=(a.a 0) =(a.b 0))
      ?.  &(=(a.a 0) =(a.b 0))  ?~(a.a b a)
      [%f ?:(=(r %d) &(s.a s.b) |(s.a s.b)) zer]
    %-  |=  [a=fn]
        ?.  ?=([%f *] a)  a
        ?.  =(a.a 0)  a
        [%f !=(r %d) zer]
    ?:  =(s.a s.b)
      ?:  s.a  (^add +>.a +>.b &)
      (fli (^add +>.a +>.b &))
    ?:  s.a  (^sub +>.a +>.b &)
    (^sub +>.b +>.a &)
  ::
  ++  sub                                               ::  subtract
    |=  [a=fn b=fn]  ^-  fn  (add a (fli b))
  ::
  ++  mul                                               ::  multiply
    |=  [a=fn b=fn]  ^-  fn
    ?:  |(?=([%n *] a) ?=([%n *] b))  [%n ~]
    ?:  ?=([%i *] a)
      ?:  ?=([%i *] b)
        [%i =(s.a s.b)]
      ?:  =(a.b 0)  [%n ~]  [%i =(s.a s.b)]
    ?:  ?=([%i *] b)
      ?:  =(a.a 0)  [%n ~]  [%i =(s.a s.b)]
    ?:  |(=(a.a 0) =(a.b 0))  [%f =(s.a s.b) zer]
    ?:  =(s.a s.b)  (^mul +>.a +>.b)
    =.(r swr (fli (^mul +>.a +>.b)))
  ::
  ++  emu                                               ::  exact multiply
    |=  [a=fn b=fn]  ^-  fn
    ?:  |(?=([%n *] a) ?=([%n *] b))  [%n ~]
    ?:  ?=([%i *] a)
      ?:  ?=([%i *] b)
        [%i =(s.a s.b)]
      ?:  =(a.b 0)  [%n ~]  [%i =(s.a s.b)]
    ?:  ?=([%i *] b)
      ?:  =(a.a 0)  [%n ~]  [%i =(s.a s.b)]
    ?:  |(=(a.a 0) =(a.b 0))  [%f =(s.a s.b) zer]
    [%f =(s.a s.b) (sum:si e.a e.b) (^^mul a.a a.b)]
  ::
  ++  div                                               ::  divide
    |=  [a=fn b=fn]  ^-  fn
    ?:  |(?=([%n *] a) ?=([%n *] b))  [%n ~]
    ?:  ?=([%i *] a)
      ?:  ?=([%i *] b)  [%n ~]  [%i =(s.a s.b)]
    ?:  ?=([%i *] b)  [%f =(s.a s.b) zer]
    ?:  =(a.a 0)  ?:  =(a.b 0)  [%n ~]  [%f =(s.a s.b) zer]
    ?:  =(a.b 0)  [%i =(s.a s.b)]
    ?:  =(s.a s.b)  (^div +>.a +>.b)
    =.(r swr (fli (^div +>.a +>.b)))
  ::
  ++  fma                                               ::  fused multiply-add
    |=  [a=fn b=fn c=fn]  ^-  fn                        ::  (a * b) + c
    (add (emu a b) c)
  ::
  ++  sqt                                               ::  square root
    |=  [a=fn]  ^-  fn
    ?:  ?=([%n *] a)  [%n ~]
    ?:  ?=([%i *] a)  ?:(s.a a [%n ~])
    ?~  a.a  [%f s.a zer]
    ?:  s.a  (^sqt +>.a)  [%n ~]
  ::
  ++  inv                                               ::  inverse
    |=  [a=fn]  ^-  fn
    (div [%f & --0 1] a)
  ::
  ++  sun                                               ::  uns integer to float
    |=  [a=@u]  ^-  fn
    (rou [%f & --0 a])
  ::
  ++  san                                               ::  sgn integer to float
    |=  [a=@s]  ^-  fn
    =+  b=(old:si a)
    (rou [%f -.b --0 +.b])
  ::
  ++  lth                                               ::  less-than
    ::    comparisons return ~ in the event of a NaN
    |=  [a=fn b=fn]  ^-  (unit ?)
    ?:  |(?=([%n *] a) ?=([%n *] b))  ~  :-  ~
    ?:  =(a b)  |
    ?:  ?=([%i *] a)  !s.a  ?:  ?=([%i *] b)  s.b
    ?:  |(=(a.a 0) =(a.b 0))
      ?:  &(=(a.a 0) =(a.b 0))  |
      ?:  =(a.a 0)  s.b  !s.a
    ?:  !=(s.a s.b)  s.b
    ?:  s.a  (^lth +>.a +>.b)  (^lth +>.b +>.a)
  ::
  ++  lte                                               ::  less-equal
    |=  [a=fn b=fn]  ^-  (unit ?)
    %+  bind  (lth b a)  |=  a=?  !a
  ::
  ++  equ                                               ::  equal
    |=  [a=fn b=fn]  ^-  (unit ?)
    ?:  |(?=([%n *] a) ?=([%n *] b))  ~  :-  ~
    ?:  =(a b)  &
    ?:  |(?=([%i *] a) ?=([%i *] b))  |
    ?:  |(=(a.a 0) =(a.b 0))
      ?:  &(=(a.a 0) =(a.b 0))  &  |
    ?:  |(=(e.a e.b) !=(s.a s.b))  |
    (^equ +>.a +>.b)
  ::
  ++  gte                                               ::  greater-equal
    |=  [a=fn b=fn]  ^-  (unit ?)  (lte b a)
  ::
  ++  gth                                               ::  greater-than
    |=  [a=fn b=fn]  ^-  (unit ?)  (lth b a)
  ::
  ++  drg                                               ::  float to decimal
    |=  [a=fn]  ^-  dn
    ?:  ?=([%n *] a)  [%n ~]
    ?:  ?=([%i *] a)  [%i s.a]
    ?~  a.a  [%d s.a --0 0]
    [%d s.a (^drg +>.a)]
  ::
  ++  grd                                               ::  decimal to float
    |=  [a=dn]  ^-  fn
    ?:  ?=([%n *] a)  [%n ~]
    ?:  ?=([%i *] a)  [%i s.a]
    =>  .(r %n)
    =+  q=(abs:si e.a)
    ?:  (syn:si e.a)
      (mul [%f s.a --0 a.a] [%f & e.a (pow 5 q)])
    (div [%f s.a --0 a.a] [%f & (sun:si q) (pow 5 q)])
  ::
  ++  toi                                               ::  round to integer @s
    |=  [a=fn]  ^-  (unit @s)
    =+  b=(toj a)
    ?.  ?=([%f *] b)  ~  :-  ~
    =+  c=(^^mul (bex (abs:si e.b)) a.b)
    (new:si s.b c)
  ::
  ++  toj                                               ::  round to integer fn
    |=  [a=fn]  ^-  fn
    ?.  ?=([%f *] a)  a
    ?~  a.a  [%f s.a zer]
    ?:  s.a  (^toj +>.a)
    =.(r swr (fli (^toj +>.a)))
  --
::    +ff
::
::  this core has no use outside of the functionality
::  provided to ++rd, ++rs, ++rq, and ++rh
::
::  w=width:         bits in exponent field
::  p=precision:     bits in fraction field
::  b=bias:          added to exponent when storing
::  r=rounding mode: same as in ++fl
++  ff                                                  ::  ieee 754 format fp
  |_  [[w=@u p=@u b=@s] r=$?(%n %u %d %z %a)]
  ::
  ++  sb  (bex (^add w p))                              ::  sign bit
  ++  me  (dif:si (dif:si --1 b) (sun:si p))            ::  minimum exponent
  ::
  ++  pa
    %*(. fl p +(p), v me, w (^sub (bex w) 3), d %d, r r)
  ::
  ++  sea                                               ::  @r to fn
    |=  [a=@r]  ^-  fn
    =+  [f=(cut 0 [0 p] a) e=(cut 0 [p w] a)]
    =+  s=(sig a)
    ?:  =(e 0)
      ?:  =(f 0)  [%f s --0 0]  [%f s me f]
    ?:  =(e (fil 0 w 1))
      ?:  =(f 0)  [%i s]  [%n ~]
    =+  q=:(sum:si (sun:si e) me -1)
    =+  r=(^add f (bex p))
    [%f s q r]
  ::
  ++  bit  |=  [a=fn]  (bif (rou:pa a))                 ::  fn to @r w+ rounding
  ::
  ++  bif                                               ::  fn to @r no rounding
    |=  [a=fn]  ^-  @r
    ?:  ?=([%i *] a)
      =+  q=(lsh [0 p] (fil 0 w 1))
      ?:  s.a  q  (^add q sb)
    ?:  ?=([%n *] a)  (lsh [0 (dec p)] (fil 0 +(w) 1))
    ?~  a.a  ?:  s.a  `@r`0  sb
    =+  ma=(met 0 a.a)
    ?.  =(ma +(p))
      ?>  =(e.a me)
      ?>  (^lth ma +(p))
      ?:  s.a  `@r`a.a  (^add a.a sb)
    =+  q=(sum:si (dif:si e.a me) --1)
    =+  r=(^add (lsh [0 p] (abs:si q)) (end [0 p] a.a))
    ?:  s.a  r  (^add r sb)
  ::
  ++  sig                                               ::  get sign
    |=  [a=@r]  ^-  ?
    =(0 (cut 0 [(^add p w) 1] a))
  ::
  ++  exp                                               ::  get exponent
    |=  [a=@r]  ^-  @s
    (dif:si (sun:si (cut 0 [p w] a)) b)
  ::
  ++  add                                               ::  add
    |=  [a=@r b=@r]
    (bif (add:pa (sea a) (sea b)))
  ::
  ++  sub                                               ::  subtract
    |=  [a=@r b=@r]
    (bif (sub:pa (sea a) (sea b)))
  ::
  ++  mul                                               ::  multiply
    |=  [a=@r b=@r]
    (bif (mul:pa (sea a) (sea b)))
  ::
  ++  div                                               ::  divide
    |=  [a=@r b=@r]
    (bif (div:pa (sea a) (sea b)))
  ::
  ++  fma                                               ::  fused multiply-add
    |=  [a=@r b=@r c=@r]
    (bif (fma:pa (sea a) (sea b) (sea c)))
  ::
  ++  sqt                                               ::  square root
    |=  [a=@r]
    (bif (sqt:pa (sea a)))
  ::
  ++  lth                                               ::  less-than
    |=  [a=@r b=@r]  (fall (lth:pa (sea a) (sea b)) |)
  ++  lte                                               ::  less-equals
    |=  [a=@r b=@r]  (fall (lte:pa (sea a) (sea b)) |)
  ++  equ                                               ::  equals
    |=  [a=@r b=@r]  (fall (equ:pa (sea a) (sea b)) |)
  ++  gte                                               ::  greater-equals
    |=  [a=@r b=@r]  (fall (gte:pa (sea a) (sea b)) |)
  ++  gth                                               ::  greater-than
    |=  [a=@r b=@r]  (fall (gth:pa (sea a) (sea b)) |)
  ++  sun                                               ::  uns integer to @r
    |=  [a=@u]  (bit [%f & --0 a])
  ++  san                                               ::  signed integer to @r
    |=  [a=@s]  (bit [%f (syn:si a) --0 (abs:si a)])
  ++  toi                                               ::  round to integer
    |=  [a=@r]  (toi:pa (sea a))
  ++  drg                                               ::  @r to decimal float
    |=  [a=@r]  (drg:pa (sea a))
  ++  grd                                               ::  decimal float to @r
    |=  [a=dn]  (bif (grd:pa a))
  --
::
++  rlyd  |=  a=@rd  ^-  dn  (drg:rd a)                 ::  prep @rd for print
++  rlys  |=  a=@rs  ^-  dn  (drg:rs a)                 ::  prep @rs for print
++  rlyh  |=  a=@rh  ^-  dn  (drg:rh a)                 ::  prep @rh for print
++  rlyq  |=  a=@rq  ^-  dn  (drg:rq a)                 ::  prep @rq for print
++  ryld  |=  a=dn  ^-  @rd  (grd:rd a)                 ::  finish parsing @rd
++  ryls  |=  a=dn  ^-  @rs  (grd:rs a)                 ::  finish parsing @rs
++  rylh  |=  a=dn  ^-  @rh  (grd:rh a)                 ::  finish parsing @rh
++  rylq  |=  a=dn  ^-  @rq  (grd:rq a)                 ::  finish parsing @rq
::
++  rd                                                  ::  double precision fp
  ^|
  ~%  %rd  +>  ~
  |_  r=$?(%n %u %d %z)
  ::  round to nearest, round up, round down, round to zero
  ::
  ++  ma
    %*(. ff w 11, p 52, b --1.023, r r)
  ::
  ++  sea                                               ::  @rd to fn
    |=  [a=@rd]  (sea:ma a)
  ::
  ++  bit                                               ::  fn to @rd
    |=  [a=fn]  ^-  @rd  (bit:ma a)
  ::
  ++  add  ~/  %add                                     ::  add
    |=  [a=@rd b=@rd]  ^-  @rd
    ~_  leaf+"rd-fail"
    (add:ma a b)
  ::
  ++  sub  ~/  %sub                                     ::  subtract
    |=  [a=@rd b=@rd]  ^-  @rd
    ~_  leaf+"rd-fail"
    (sub:ma a b)
  ::
  ++  mul  ~/  %mul                                     ::  multiply
    |=  [a=@rd b=@rd]  ^-  @rd
    ~_  leaf+"rd-fail"
    (mul:ma a b)
  ::
  ++  div  ~/  %div                                     ::  divide
    |=  [a=@rd b=@rd]  ^-  @rd
    ~_  leaf+"rd-fail"
    (div:ma a b)
  ::
  ++  fma  ~/  %fma                                     ::  fused multiply-add
    |=  [a=@rd b=@rd c=@rd]  ^-  @rd
    ~_  leaf+"rd-fail"
    (fma:ma a b c)
  ::
  ++  sqt  ~/  %sqt                                     ::  square root
    |=  [a=@rd]  ^-  @rd  ~_  leaf+"rd-fail"
    (sqt:ma a)
  ::
  ++  lth  ~/  %lth                                     ::  less-than
    |=  [a=@rd b=@rd]
    ~_  leaf+"rd-fail"
    (lth:ma a b)
  ::
  ++  lte  ~/  %lte                                     ::  less-equals
    |=  [a=@rd b=@rd]
    ~_  leaf+"rd-fail"
    (lte:ma a b)
  ::
  ++  equ  ~/  %equ                                     ::  equals
    |=  [a=@rd b=@rd]
    ~_  leaf+"rd-fail"
    (equ:ma a b)
  ::
  ++  gte  ~/  %gte                                     ::  greater-equals
    |=  [a=@rd b=@rd]
    ~_  leaf+"rd-fail"
    (gte:ma a b)
  ::
  ++  gth  ~/  %gth                                     ::  greater-than
    |=  [a=@rd b=@rd]
    ~_  leaf+"rd-fail"
    (gth:ma a b)
  ::
  ++  sun  |=  [a=@u]  ^-  @rd  (sun:ma a)              ::  uns integer to @rd
  ++  san  |=  [a=@s]  ^-  @rd  (san:ma a)              ::  sgn integer to @rd
  ++  sig  |=  [a=@rd]  ^-  ?  (sig:ma a)               ::  get sign
  ++  exp  |=  [a=@rd]  ^-  @s  (exp:ma a)              ::  get exponent
  ++  toi  |=  [a=@rd]  ^-  (unit @s)  (toi:ma a)       ::  round to integer
  ++  drg  |=  [a=@rd]  ^-  dn  (drg:ma a)              ::  @rd to decimal float
  ++  grd  |=  [a=dn]  ^-  @rd  (grd:ma a)              ::  decimal float to @rd
  --
::
++  rs                                                  ::  single precision fp
  ~%  %rs  +>  ~
  ^|
  ::    round to nearest, round up, round down, round to zero
  |_  r=$?(%n %u %d %z)
  ::
  ++  ma
    %*(. ff w 8, p 23, b --127, r r)
  ::
  ++  sea                                               ::  @rs to fn
    |=  [a=@rs]  (sea:ma a)
  ::
  ++  bit                                               ::  fn to @rs
    |=  [a=fn]  ^-  @rs  (bit:ma a)
  ::
  ++  add  ~/  %add                                     ::  add
    |=  [a=@rs b=@rs]  ^-  @rs
    ~_  leaf+"rs-fail"
    (add:ma a b)
  ::
  ++  sub  ~/  %sub                                     ::  subtract
    |=  [a=@rs b=@rs]  ^-  @rs
    ~_  leaf+"rs-fail"
    (sub:ma a b)
  ::
  ++  mul  ~/  %mul                                     ::  multiply
    |=  [a=@rs b=@rs]  ^-  @rs
    ~_  leaf+"rs-fail"
    (mul:ma a b)
  ::
  ++  div  ~/  %div                                     ::  divide
    |=  [a=@rs b=@rs]  ^-  @rs
    ~_  leaf+"rs-fail"
    (div:ma a b)
  ::
  ++  fma  ~/  %fma                                     ::  fused multiply-add
    |=  [a=@rs b=@rs c=@rs]  ^-  @rs
    ~_  leaf+"rs-fail"
    (fma:ma a b c)
  ::
  ++  sqt  ~/  %sqt                                     ::  square root
    |=  [a=@rs]  ^-  @rs
    ~_  leaf+"rs-fail"
    (sqt:ma a)
  ::
  ++  lth  ~/  %lth                                     ::  less-than
    |=  [a=@rs b=@rs]
    ~_  leaf+"rs-fail"
    (lth:ma a b)
  ::
  ++  lte  ~/  %lte                                     ::  less-equals
    |=  [a=@rs b=@rs]
    ~_  leaf+"rs-fail"
    (lte:ma a b)
  ::
  ++  equ  ~/  %equ                                     ::  equals
    |=  [a=@rs b=@rs]
    ~_  leaf+"rs-fail"
    (equ:ma a b)
  ::
  ++  gte  ~/  %gte                                     ::  greater-equals
    |=  [a=@rs b=@rs]
    ~_  leaf+"rs-fail"
    (gte:ma a b)
  ::
  ++  gth  ~/  %gth                                     ::  greater-than
    |=  [a=@rs b=@rs]
    ~_  leaf+"rs-fail"
    (gth:ma a b)
  ::
  ++  sun  |=  [a=@u]  ^-  @rs  (sun:ma a)              ::  uns integer to @rs
  ++  san  |=  [a=@s]  ^-  @rs  (san:ma a)              ::  sgn integer to @rs
  ++  sig  |=  [a=@rs]  ^-  ?  (sig:ma a)               ::  get sign
  ++  exp  |=  [a=@rs]  ^-  @s  (exp:ma a)              ::  get exponent
  ++  toi  |=  [a=@rs]  ^-  (unit @s)  (toi:ma a)       ::  round to integer
  ++  drg  |=  [a=@rs]  ^-  dn  (drg:ma a)              ::  @rs to decimal float
  ++  grd  |=  [a=dn]  ^-  @rs  (grd:ma a)              ::  decimal float to @rs
  --
::
++  rq                                                  ::  quad precision fp
  ~%  %rq  +>  ~
  ^|
  ::    round to nearest, round up, round down, round to zero
  |_  r=$?(%n %u %d %z)
  ::
  ++  ma
    %*(. ff w 15, p 112, b --16.383, r r)
  ::
  ++  sea                                               ::  @rq to fn
    |=  [a=@rq]  (sea:ma a)
  ::
  ++  bit                                               ::  fn to @rq
    |=  [a=fn]  ^-  @rq  (bit:ma a)
  ::
  ++  add  ~/  %add                                     ::  add
    |=  [a=@rq b=@rq]  ^-  @rq
    ~_  leaf+"rq-fail"
    (add:ma a b)
  ::
  ++  sub  ~/  %sub                                     ::  subtract
    |=  [a=@rq b=@rq]  ^-  @rq
    ~_  leaf+"rq-fail"
    (sub:ma a b)
  ::
  ++  mul  ~/  %mul                                     ::  multiply
    |=  [a=@rq b=@rq]  ^-  @rq
    ~_  leaf+"rq-fail"
    (mul:ma a b)
  ::
  ++  div  ~/  %div                                     ::  divide
    |=  [a=@rq b=@rq]  ^-  @rq
    ~_  leaf+"rq-fail"
    (div:ma a b)
  ::
  ++  fma  ~/  %fma                                     ::  fused multiply-add
    |=  [a=@rq b=@rq c=@rq]  ^-  @rq
    ~_  leaf+"rq-fail"
    (fma:ma a b c)
  ::
  ++  sqt  ~/  %sqt                                     ::  square root
    |=  [a=@rq]  ^-  @rq
    ~_  leaf+"rq-fail"
    (sqt:ma a)
  ::
  ++  lth  ~/  %lth                                     ::  less-than
    |=  [a=@rq b=@rq]
    ~_  leaf+"rq-fail"
    (lth:ma a b)
  ::
  ++  lte  ~/  %lte                                     ::  less-equals
    |=  [a=@rq b=@rq]
    ~_  leaf+"rq-fail"
    (lte:ma a b)
  ::
  ++  equ  ~/  %equ                                     ::  equals
    |=  [a=@rq b=@rq]
    ~_  leaf+"rq-fail"
    (equ:ma a b)
  ::
  ++  gte  ~/  %gte                                     ::  greater-equals
    |=  [a=@rq b=@rq]
    ~_  leaf+"rq-fail"
    (gte:ma a b)
  ::
  ++  gth  ~/  %gth                                     ::  greater-than
    |=  [a=@rq b=@rq]
    ~_  leaf+"rq-fail"
    (gth:ma a b)
  ::
  ++  sun  |=  [a=@u]  ^-  @rq  (sun:ma a)              ::  uns integer to @rq
  ++  san  |=  [a=@s]  ^-  @rq  (san:ma a)              ::  sgn integer to @rq
  ++  sig  |=  [a=@rq]  ^-  ?  (sig:ma a)               ::  get sign
  ++  exp  |=  [a=@rq]  ^-  @s  (exp:ma a)              ::  get exponent
  ++  toi  |=  [a=@rq]  ^-  (unit @s)  (toi:ma a)       ::  round to integer
  ++  drg  |=  [a=@rq]  ^-  dn  (drg:ma a)              ::  @rq to decimal float
  ++  grd  |=  [a=dn]  ^-  @rq  (grd:ma a)              ::  decimal float to @rq
  --
::
++  rh                                                  ::  half precision fp
  ~%  %rh  +>  ~
  ^|
  ::    round to nearest, round up, round down, round to zero
  |_  r=$?(%n %u %d %z)
  ::
  ++  ma
    %*(. ff w 5, p 10, b --15, r r)
  ::
  ++  sea                                               ::  @rh to fn
    |=  [a=@rh]  (sea:ma a)
  ::
  ++  bit                                               ::  fn to @rh
    |=  [a=fn]  ^-  @rh  (bit:ma a)
  ::
  ++  add  ~/  %add                                     ::  add
    |=  [a=@rh b=@rh]  ^-  @rh
    ~_  leaf+"rh-fail"
    (add:ma a b)
  ::
  ++  sub  ~/  %sub                                     ::  subtract
    |=  [a=@rh b=@rh]  ^-  @rh
    ~_  leaf+"rh-fail"
    (sub:ma a b)
  ::
  ++  mul  ~/  %mul                                     ::  multiply
    |=  [a=@rh b=@rh]  ^-  @rh
    ~_  leaf+"rh-fail"
    (mul:ma a b)
  ::
  ++  div  ~/  %div                                     ::  divide
    |=  [a=@rh b=@rh]  ^-  @rh
    ~_  leaf+"rh-fail"
    (div:ma a b)
  ::
  ++  fma  ~/  %fma                                     ::  fused multiply-add
    |=  [a=@rh b=@rh c=@rh]  ^-  @rh
    ~_  leaf+"rh-fail"
    (fma:ma a b c)
  ::
  ++  sqt  ~/  %sqt                                     ::  square root
    |=  [a=@rh]  ^-  @rh
    ~_  leaf+"rh-fail"
    (sqt:ma a)
  ::
  ++  lth  ~/  %lth                                     ::  less-than
    |=  [a=@rh b=@rh]
    ~_  leaf+"rh-fail"
    (lth:ma a b)
  ::
  ++  lte  ~/  %lte                                     ::  less-equals
    |=  [a=@rh b=@rh]
    ~_  leaf+"rh-fail"
    (lte:ma a b)
  ::
  ++  equ  ~/  %equ                                     ::  equals
    |=  [a=@rh b=@rh]
    ~_  leaf+"rh-fail"
    (equ:ma a b)
  ::
  ++  gte  ~/  %gte                                     ::  greater-equals
    |=  [a=@rh b=@rh]
    ~_  leaf+"rh-fail"
    (gte:ma a b)
  ::
  ++  gth  ~/  %gth                                     ::  greater-than
    |=  [a=@rh b=@rh]
    ~_  leaf+"rh-fail"
    (gth:ma a b)
  ::
  ++  tos                                               ::  @rh to @rs
    |=  [a=@rh]  (bit:rs (sea a))
  ::
  ++  fos                                               ::  @rs to @rh
    |=  [a=@rs]  (bit (sea:rs a))
  ::
  ++  sun  |=  [a=@u]  ^-  @rh  (sun:ma a)              ::  uns integer to @rh
  ++  san  |=  [a=@s]  ^-  @rh  (san:ma a)              ::  sgn integer to @rh
  ++  sig  |=  [a=@rh]  ^-  ?  (sig:ma a)               ::  get sign
  ++  exp  |=  [a=@rh]  ^-  @s  (exp:ma a)              ::  get exponent
  ++  toi  |=  [a=@rh]  ^-  (unit @s)  (toi:ma a)       ::  round to integer
  ++  drg  |=  [a=@rh]  ^-  dn  (drg:ma a)              ::  @rh to decimal float
  ++  grd  |=  [a=dn]  ^-  @rh  (grd:ma a)              ::  decimal float to @rh
  --
::
::    3c: urbit time
+|  %urbit-time
::
++  year                                                ::  date to @d
  |=  det=date
  ^-  @da
  =+  ^=  yer
      ?:  a.det
        (add 292.277.024.400 y.det)
      (sub 292.277.024.400 (dec y.det))
  =+  day=(yawn yer m.det d.t.det)
  (yule day h.t.det m.t.det s.t.det f.t.det)
::
++  yore                                                ::  @d to date
  |=  now=@da
  ^-  date
  =+  rip=(yell now)
  =+  ger=(yall d.rip)
  :-  ?:  (gth y.ger 292.277.024.400)
        [a=& y=(sub y.ger 292.277.024.400)]
      [a=| y=+((sub 292.277.024.400 y.ger))]
  [m.ger d.ger h.rip m.rip s.rip f.rip]
::
++  yell                                                ::  tarp from @d
  |=  now=@d
  ^-  tarp
  =+  sec=(rsh 6 now)
  =+  ^=  fan
      =+  [muc=4 raw=(end 6 now)]
      |-  ^-  (list @ux)
      ?:  |(=(0 raw) =(0 muc))
        ~
      =>  .(muc (dec muc))
      [(cut 4 [muc 1] raw) $(raw (end [4 muc] raw))]
  =+  day=(div sec day:yo)
  =>  .(sec (mod sec day:yo))
  =+  hor=(div sec hor:yo)
  =>  .(sec (mod sec hor:yo))
  =+  mit=(div sec mit:yo)
  =>  .(sec (mod sec mit:yo))
  [day hor mit sec fan]
::
++  yule                                                ::  time atom
  |=  rip=tarp
  ^-  @d
  =+  ^=  sec  ;:  add
                 (mul d.rip day:yo)
                 (mul h.rip hor:yo)
                 (mul m.rip mit:yo)
                 s.rip
               ==
  =+  ^=  fac  =+  muc=4
               |-  ^-  @
               ?~  f.rip
                 0
               =>  .(muc (dec muc))
               (add (lsh [4 muc] i.f.rip) $(f.rip t.f.rip))
  (con (lsh 6 sec) fac)
::
++  yall                                                ::  day / to day of year
  |=  day=@ud
  ^-  [y=@ud m=@ud d=@ud]
  =+  [era=0 cet=0 lep=*?]
  =>  .(era (div day era:yo), day (mod day era:yo))
  =>  ^+  .
      ?:  (lth day +(cet:yo))
        .(lep &, cet 0)
      =>  .(lep |, cet 1, day (sub day +(cet:yo)))
      .(cet (add cet (div day cet:yo)), day (mod day cet:yo))
  =+  yer=(add (mul 400 era) (mul 100 cet))
  |-  ^-  [y=@ud m=@ud d=@ud]
  =+  dis=?:(lep 366 365)
  ?.  (lth day dis)
    =+  ner=+(yer)
    $(yer ner, day (sub day dis), lep =(0 (end [0 2] ner)))
  |-  ^-  [y=@ud m=@ud d=@ud]
  =+  [mot=0 cah=?:(lep moy:yo moh:yo)]
  |-  ^-  [y=@ud m=@ud d=@ud]
  =+  zis=(snag mot cah)
  ?:  (lth day zis)
    [yer +(mot) +(day)]
  $(mot +(mot), day (sub day zis))
::
++  yawn                                                ::  days since Jesus
  |=  [yer=@ud mot=@ud day=@ud]
  ^-  @ud
  =>  .(mot (dec mot), day (dec day))
  =>  ^+  .
      %=    .
          day
        =+  cah=?:((yelp yer) moy:yo moh:yo)
        |-  ^-  @ud
        ?:  =(0 mot)
          day
        $(mot (dec mot), cah (slag 1 cah), day (add day (snag 0 cah)))
      ==
  |-  ^-  @ud
  ?.  =(0 (mod yer 4))
    =+  ney=(dec yer)
    $(yer ney, day (add day ?:((yelp ney) 366 365)))
  ?.  =(0 (mod yer 100))
    =+  nef=(sub yer 4)
    $(yer nef, day (add day ?:((yelp nef) 1.461 1.460)))
  ?.  =(0 (mod yer 400))
    =+  nec=(sub yer 100)
    $(yer nec, day (add day ?:((yelp nec) 36.525 36.524)))
  (add day (mul (div yer 400) (add 1 (mul 4 36.524))))
::
++  yelp                                                ::  leap year
  |=  yer=@ud  ^-  ?
  &(=(0 (mod yer 4)) |(!=(0 (mod yer 100)) =(0 (mod yer 400))))
::
++  yo                                                  ::  time constants
  |%  ++  cet  36.524                 ::  (add 24 (mul 100 365))
      ++  day  86.400                 ::  (mul 24 hor)
      ++  era  146.097                ::  (add 1 (mul 4 cet))
      ++  hor  3.600                  ::  (mul 60 mit)
      ++  jes  106.751.991.084.417    ::  (mul 730.692.561 era)
      ++  mit  60
      ++  moh  `(list @ud)`[31 28 31 30 31 30 31 31 30 31 30 31 ~]
      ++  moy  `(list @ud)`[31 29 31 30 31 30 31 31 30 31 30 31 ~]
      ++  qad  126.144.001            ::  (add 1 (mul 4 yer))
      ++  yer  31.536.000             ::  (mul 365 day)
  --
::
::    3d: SHA hash family
+|  %sha-hash-family
::
++  shad  |=(ruz=@ (shax (shax ruz)))                   ::  double sha-256
++  shaf                                                ::  half sha-256
  |=  [sal=@ ruz=@]
  =+  haz=(shas sal ruz)
  (mix (end 7 haz) (rsh 7 haz))
::
++  sham                                                ::  128bit noun hash
  |=  yux=*  ^-  @uvH  ^-  @
  ?@  yux
    (shaf %mash yux)
  (shaf %sham (jam yux))
::
++  shas                                                ::  salted hash
  ~/  %shas
  |=  [sal=@ ruz=@]
  =/  len  (max 32 (met 3 sal))
  (shay len (mix sal (shax ruz)))
::
++  shax                                                ::  sha-256
  ~/  %shax
  |=  ruz=@  ^-  @
  (shay [(met 3 ruz) ruz])
::
++  shay                                                ::  sha-256 with length
  ~/  %shay
  |=  [len=@u ruz=@]  ^-  @
  =>  .(ruz (cut 3 [0 len] ruz))
  =+  [few==>(fe .(a 5)) wac=|=([a=@ b=@] (cut 5 [a 1] b))]
  =+  [sum=sum.few ror=ror.few net=net.few inv=inv.few]
  =+  ral=(lsh [0 3] len)
  =+  ^=  ful
      %+  can  0
      :~  [ral ruz]
          [8 128]
          [(mod (sub 960 (mod (add 8 ral) 512)) 512) 0]
          [64 (~(net fe 6) ral)]
      ==
  =+  lex=(met 9 ful)
  =+  ^=  kbx  0xc671.78f2.bef9.a3f7.a450.6ceb.90be.fffa.
                 8cc7.0208.84c8.7814.78a5.636f.748f.82ee.
                 682e.6ff3.5b9c.ca4f.4ed8.aa4a.391c.0cb3.
                 34b0.bcb5.2748.774c.1e37.6c08.19a4.c116.
                 106a.a070.f40e.3585.d699.0624.d192.e819.
                 c76c.51a3.c24b.8b70.a81a.664b.a2bf.e8a1.
                 9272.2c85.81c2.c92e.766a.0abb.650a.7354.
                 5338.0d13.4d2c.6dfc.2e1b.2138.27b7.0a85.
                 1429.2967.06ca.6351.d5a7.9147.c6e0.0bf3.
                 bf59.7fc7.b003.27c8.a831.c66d.983e.5152.
                 76f9.88da.5cb0.a9dc.4a74.84aa.2de9.2c6f.
                 240c.a1cc.0fc1.9dc6.efbe.4786.e49b.69c1.
                 c19b.f174.9bdc.06a7.80de.b1fe.72be.5d74.
                 550c.7dc3.2431.85be.1283.5b01.d807.aa98.
                 ab1c.5ed5.923f.82a4.59f1.11f1.3956.c25b.
                 e9b5.dba5.b5c0.fbcf.7137.4491.428a.2f98
  =+  ^=  hax  0x5be0.cd19.1f83.d9ab.9b05.688c.510e.527f.
                 a54f.f53a.3c6e.f372.bb67.ae85.6a09.e667
  =+  i=0
  |-  ^-  @
  ?:  =(i lex)
    (run 5 hax net)
  =+  ^=  wox
      =+  dux=(cut 9 [i 1] ful)
      =+  wox=(run 5 dux net)
      =+  j=16
      |-  ^-  @
      ?:  =(64 j)
        wox
      =+  :*  l=(wac (sub j 15) wox)
              m=(wac (sub j 2) wox)
              n=(wac (sub j 16) wox)
              o=(wac (sub j 7) wox)
          ==
      =+  x=:(mix (ror 0 7 l) (ror 0 18 l) (rsh [0 3] l))
      =+  y=:(mix (ror 0 17 m) (ror 0 19 m) (rsh [0 10] m))
      =+  z=:(sum n x o y)
      $(wox (con (lsh [5 j] z) wox), j +(j))
  =+  j=0
  =+  :*  a=(wac 0 hax)
          b=(wac 1 hax)
          c=(wac 2 hax)
          d=(wac 3 hax)
          e=(wac 4 hax)
          f=(wac 5 hax)
          g=(wac 6 hax)
          h=(wac 7 hax)
      ==
  |-  ^-  @
  ?:  =(64 j)
    %=  ^$
      i  +(i)
      hax  %+  rep  5
           :~  (sum a (wac 0 hax))
               (sum b (wac 1 hax))
               (sum c (wac 2 hax))
               (sum d (wac 3 hax))
               (sum e (wac 4 hax))
               (sum f (wac 5 hax))
               (sum g (wac 6 hax))
               (sum h (wac 7 hax))
           ==
    ==
  =+  l=:(mix (ror 0 2 a) (ror 0 13 a) (ror 0 22 a))    ::  s0
  =+  m=:(mix (dis a b) (dis a c) (dis b c))            ::  maj
  =+  n=(sum l m)                                       ::  t2
  =+  o=:(mix (ror 0 6 e) (ror 0 11 e) (ror 0 25 e))    ::  s1
  =+  p=(mix (dis e f) (dis (inv e) g))                 ::  ch
  =+  q=:(sum h o p (wac j kbx) (wac j wox))            ::  t1
  $(j +(j), a (sum q n), b a, c b, d c, e (sum d q), f e, g f, h g)
::
++  shaw                                                ::  hash to nbits
  |=  [sal=@ len=@ ruz=@]
  (~(raw og (shas sal (mix len ruz))) len)
::
++  shaz                                                ::  sha-512
  |=  ruz=@  ^-  @
  (shal [(met 3 ruz) ruz])
::
++  shal                                                ::  sha-512 with length
  ~/  %shal
  |=  [len=@ ruz=@]  ^-  @
  =>  .(ruz (cut 3 [0 len] ruz))
  =+  [few==>(fe .(a 6)) wac=|=([a=@ b=@] (cut 6 [a 1] b))]
  =+  [sum=sum.few ror=ror.few net=net.few inv=inv.few]
  =+  ral=(lsh [0 3] len)
  =+  ^=  ful
      %+  can  0
      :~  [ral ruz]
          [8 128]
          [(mod (sub 1.920 (mod (add 8 ral) 1.024)) 1.024) 0]
          [128 (~(net fe 7) ral)]
      ==
  =+  lex=(met 10 ful)
  =+  ^=  kbx  0x6c44.198c.4a47.5817.5fcb.6fab.3ad6.faec.
                 597f.299c.fc65.7e2a.4cc5.d4be.cb3e.42b6.
                 431d.67c4.9c10.0d4c.3c9e.be0a.15c9.bebc.
                 32ca.ab7b.40c7.2493.28db.77f5.2304.7d84.
                 1b71.0b35.131c.471b.113f.9804.bef9.0dae.
                 0a63.7dc5.a2c8.98a6.06f0.67aa.7217.6fba.
                 f57d.4f7f.ee6e.d178.eada.7dd6.cde0.eb1e.
                 d186.b8c7.21c0.c207.ca27.3ece.ea26.619c.
                 c671.78f2.e372.532b.bef9.a3f7.b2c6.7915.
                 a450.6ceb.de82.bde9.90be.fffa.2363.1e28.
                 8cc7.0208.1a64.39ec.84c8.7814.a1f0.ab72.
                 78a5.636f.4317.2f60.748f.82ee.5def.b2fc.
                 682e.6ff3.d6b2.b8a3.5b9c.ca4f.7763.e373.
                 4ed8.aa4a.e341.8acb.391c.0cb3.c5c9.5a63.
                 34b0.bcb5.e19b.48a8.2748.774c.df8e.eb99.
                 1e37.6c08.5141.ab53.19a4.c116.b8d2.d0c8.
                 106a.a070.32bb.d1b8.f40e.3585.5771.202a.
                 d699.0624.5565.a910.d192.e819.d6ef.5218.
                 c76c.51a3.0654.be30.c24b.8b70.d0f8.9791.
                 a81a.664b.bc42.3001.a2bf.e8a1.4cf1.0364.
                 9272.2c85.1482.353b.81c2.c92e.47ed.aee6.
                 766a.0abb.3c77.b2a8.650a.7354.8baf.63de.
                 5338.0d13.9d95.b3df.4d2c.6dfc.5ac4.2aed.
                 2e1b.2138.5c26.c926.27b7.0a85.46d2.2ffc.
                 1429.2967.0a0e.6e70.06ca.6351.e003.826f.
                 d5a7.9147.930a.a725.c6e0.0bf3.3da8.8fc2.
                 bf59.7fc7.beef.0ee4.b003.27c8.98fb.213f.
                 a831.c66d.2db4.3210.983e.5152.ee66.dfab.
                 76f9.88da.8311.53b5.5cb0.a9dc.bd41.fbd4.
                 4a74.84aa.6ea6.e483.2de9.2c6f.592b.0275.
                 240c.a1cc.77ac.9c65.0fc1.9dc6.8b8c.d5b5.
                 efbe.4786.384f.25e3.e49b.69c1.9ef1.4ad2.
                 c19b.f174.cf69.2694.9bdc.06a7.25c7.1235.
                 80de.b1fe.3b16.96b1.72be.5d74.f27b.896f.
                 550c.7dc3.d5ff.b4e2.2431.85be.4ee4.b28c.
                 1283.5b01.4570.6fbe.d807.aa98.a303.0242.
                 ab1c.5ed5.da6d.8118.923f.82a4.af19.4f9b.
                 59f1.11f1.b605.d019.3956.c25b.f348.b538.
                 e9b5.dba5.8189.dbbc.b5c0.fbcf.ec4d.3b2f.
                 7137.4491.23ef.65cd.428a.2f98.d728.ae22
  =+  ^=  hax  0x5be0.cd19.137e.2179.1f83.d9ab.fb41.bd6b.
                 9b05.688c.2b3e.6c1f.510e.527f.ade6.82d1.
                 a54f.f53a.5f1d.36f1.3c6e.f372.fe94.f82b.
                 bb67.ae85.84ca.a73b.6a09.e667.f3bc.c908
  =+  i=0
  |-  ^-  @
  ?:  =(i lex)
    (run 6 hax net)
  =+  ^=  wox
      =+  dux=(cut 10 [i 1] ful)
      =+  wox=(run 6 dux net)
      =+  j=16
      |-  ^-  @
      ?:  =(80 j)
        wox
      =+  :*  l=(wac (sub j 15) wox)
              m=(wac (sub j 2) wox)
              n=(wac (sub j 16) wox)
              o=(wac (sub j 7) wox)
          ==
      =+  x=:(mix (ror 0 1 l) (ror 0 8 l) (rsh [0 7] l))
      =+  y=:(mix (ror 0 19 m) (ror 0 61 m) (rsh [0 6] m))
      =+  z=:(sum n x o y)
      $(wox (con (lsh [6 j] z) wox), j +(j))
  =+  j=0
  =+  :*  a=(wac 0 hax)
          b=(wac 1 hax)
          c=(wac 2 hax)
          d=(wac 3 hax)
          e=(wac 4 hax)
          f=(wac 5 hax)
          g=(wac 6 hax)
          h=(wac 7 hax)
      ==
  |-  ^-  @
  ?:  =(80 j)
    %=  ^$
      i  +(i)
      hax  %+  rep  6
           :~  (sum a (wac 0 hax))
               (sum b (wac 1 hax))
               (sum c (wac 2 hax))
               (sum d (wac 3 hax))
               (sum e (wac 4 hax))
               (sum f (wac 5 hax))
               (sum g (wac 6 hax))
               (sum h (wac 7 hax))
           ==
    ==
  =+  l=:(mix (ror 0 28 a) (ror 0 34 a) (ror 0 39 a))   ::  S0
  =+  m=:(mix (dis a b) (dis a c) (dis b c))            ::  maj
  =+  n=(sum l m)                                       ::  t2
  =+  o=:(mix (ror 0 14 e) (ror 0 18 e) (ror 0 41 e))   ::  S1
  =+  p=(mix (dis e f) (dis (inv e) g))                 ::  ch
  =+  q=:(sum h o p (wac j kbx) (wac j wox))            ::  t1
  $(j +(j), a (sum q n), b a, c b, d c, e (sum d q), f e, g f, h g)
::
++  shan                                                ::  sha-1 (deprecated)
  |=  ruz=@
  =+  [few==>(fe .(a 5)) wac=|=([a=@ b=@] (cut 5 [a 1] b))]
  =+  [sum=sum.few ror=ror.few rol=rol.few net=net.few inv=inv.few]
  =+  ral=(lsh [0 3] (met 3 ruz))
  =+  ^=  ful
      %+  can  0
      :~  [ral ruz]
          [8 128]
          [(mod (sub 960 (mod (add 8 ral) 512)) 512) 0]
          [64 (~(net fe 6) ral)]
      ==
  =+  lex=(met 9 ful)
  =+  kbx=0xca62.c1d6.8f1b.bcdc.6ed9.eba1.5a82.7999
  =+  hax=0xc3d2.e1f0.1032.5476.98ba.dcfe.efcd.ab89.6745.2301
  =+  i=0
  |-
  ?:  =(i lex)
    (rep 5 (flop (rip 5 hax)))
  =+  ^=  wox
      =+  dux=(cut 9 [i 1] ful)
      =+  wox=(rep 5 (turn (rip 5 dux) net))
      =+  j=16
      |-  ^-  @
      ?:  =(80 j)
        wox
      =+  :*  l=(wac (sub j 3) wox)
              m=(wac (sub j 8) wox)
              n=(wac (sub j 14) wox)
              o=(wac (sub j 16) wox)
          ==
      =+  z=(rol 0 1 :(mix l m n o))
      $(wox (con (lsh [5 j] z) wox), j +(j))
  =+  j=0
  =+  :*  a=(wac 0 hax)
          b=(wac 1 hax)
          c=(wac 2 hax)
          d=(wac 3 hax)
          e=(wac 4 hax)
      ==
  |-  ^-  @
  ?:  =(80 j)
    %=  ^$
      i  +(i)
      hax  %+  rep  5
           :~
               (sum a (wac 0 hax))
               (sum b (wac 1 hax))
               (sum c (wac 2 hax))
               (sum d (wac 3 hax))
               (sum e (wac 4 hax))
           ==
    ==
  =+  fx=(con (dis b c) (dis (not 5 1 b) d))
  =+  fy=:(mix b c d)
  =+  fz=:(con (dis b c) (dis b d) (dis c d))
  =+  ^=  tem
      ?:  &((gte j 0) (lte j 19))
        :(sum (rol 0 5 a) fx e (wac 0 kbx) (wac j wox))
      ?:  &((gte j 20) (lte j 39))
        :(sum (rol 0 5 a) fy e (wac 1 kbx) (wac j wox))
      ?:  &((gte j 40) (lte j 59))
        :(sum (rol 0 5 a) fz e (wac 2 kbx) (wac j wox))
      :(sum (rol 0 5 a) fy e (wac 3 kbx) (wac j wox))
  $(j +(j), a tem, b a, c (rol 0 30 b), d c, e d)
::
::  NEVER USE: broken piece of trash
++  og                                                  ::  shax-powered rng
  |_  a=@
  ++  rad                                               ::  random in range
    |=  b=@  ^-  @
    !!
  ::
  ++  rads                                              ::  random continuation
    |=  b=@
    !!
  ::
  ++  raw                                               ::  random bits
    |=  b=@  ^-  @
    !!
  ::
  ++  raws                                              ::  random bits
    |=  b=@                                             ::  continuation
    !!
  --
::
++  sha                                                 ::  correct byte-order
  ~%  %sha  ..sha  ~
  =>  |%
      ++  flin  |=(a=@ (swp 3 a))                       ::  flip input
      ++  flim  |=(byts [wid (rev 3 wid dat)])          ::  flip input w= length
      ++  flip  |=(w=@u (cury (cury rev 3) w))          ::  flip output of size
      ++  meet  |=(a=@ [(met 3 a) a])                   ::  measure input size
      --
  |%
  ::
  ::  use with @
  ::
  ++  sha-1     (cork meet sha-1l)
  ++  sha-256   :(cork flin shax (flip 32))
  ++  sha-512   :(cork flin shaz (flip 64))
  ::
  ::  use with byts
  ::
  ++  sha-256l  :(cork flim shay (flip 32))
  ++  sha-512l  :(cork flim shal (flip 64))
  ::
  ++  sha-1l
    ~/  %sha1
    |=  byts
    ^-  @
    =+  [few==>(fe .(a 5)) wac=|=([a=@ b=@] (cut 5 [a 1] b))]
    =+  [sum=sum.few ror=ror.few rol=rol.few net=net.few inv=inv.few]
    =+  ral=(lsh [0 3] wid)
    =+  ^=  ful
        %+  can  0
        :~  [ral (rev 3 wid dat)]
            [8 128]
            [(mod (sub 960 (mod (add 8 ral) 512)) 512) 0]
            [64 (~(net fe 6) ral)]
        ==
    =+  lex=(met 9 ful)
    =+  kbx=0xca62.c1d6.8f1b.bcdc.6ed9.eba1.5a82.7999
    =+  hax=0xc3d2.e1f0.1032.5476.98ba.dcfe.efcd.ab89.6745.2301
    =+  i=0
    |-
    ?:  =(i lex)
      (rep 5 (flop (rip 5 hax)))
    =+  ^=  wox
        =+  dux=(cut 9 [i 1] ful)
        =+  wox=(rep 5 (turn (rip 5 dux) net))
        =+  j=16
        |-  ^-  @
        ?:  =(80 j)
          wox
        =+  :*  l=(wac (sub j 3) wox)
                m=(wac (sub j 8) wox)
                n=(wac (sub j 14) wox)
                o=(wac (sub j 16) wox)
            ==
        =+  z=(rol 0 1 :(mix l m n o))
        $(wox (con (lsh [5 j] z) wox), j +(j))
    =+  j=0
    =+  :*  a=(wac 0 hax)
            b=(wac 1 hax)
            c=(wac 2 hax)
            d=(wac 3 hax)
            e=(wac 4 hax)
        ==
    |-  ^-  @
    ?:  =(80 j)
      %=  ^$
        i  +(i)
        hax  %+  rep  5
             :~
                 (sum a (wac 0 hax))
                 (sum b (wac 1 hax))
                 (sum c (wac 2 hax))
                 (sum d (wac 3 hax))
                 (sum e (wac 4 hax))
             ==
      ==
    =+  fx=(con (dis b c) (dis (not 5 1 b) d))
    =+  fy=:(mix b c d)
    =+  fz=:(con (dis b c) (dis b d) (dis c d))
    =+  ^=  tem
        ?:  &((gte j 0) (lte j 19))
          :(sum (rol 0 5 a) fx e (wac 0 kbx) (wac j wox))
        ?:  &((gte j 20) (lte j 39))
          :(sum (rol 0 5 a) fy e (wac 1 kbx) (wac j wox))
        ?:  &((gte j 40) (lte j 59))
          :(sum (rol 0 5 a) fz e (wac 2 kbx) (wac j wox))
        :(sum (rol 0 5 a) fy e (wac 3 kbx) (wac j wox))
    $(j +(j), a tem, b a, c (rol 0 30 b), d c, e d)
  --
::    3f: scrambling
+|  %scrambling
::
++  un                                                  ::  =(x (wred (wren x)))
  |%
  ++  wren                                              ::  conceal structure
    |=  pyn=@  ^-  @
    =+  len=(met 3 pyn)
    ?:  =(0 len)
      0
    =>  .(len (dec len))
    =+  mig=(zaft (xafo len (cut 3 [len 1] pyn)))
    %+  can  3
    %-  flop  ^-  (list [@ @])
    :-  [1 mig]
    |-  ^-  (list [@ @])
    ?:  =(0 len)
      ~
    =>  .(len (dec len))
    =+  mog=(zyft :(mix mig (end 3 len) (cut 3 [len 1] pyn)))
    [[1 mog] $(mig mog)]
  ::
  ++  wred                                              ::  restore structure
    |=  cry=@  ^-  @
    =+  len=(met 3 cry)
    ?:  =(0 len)
      0
    =>  .(len (dec len))
    =+  mig=(cut 3 [len 1] cry)
    %+  can  3
    %-  flop  ^-  (list [@ @])
    :-  [1 (xaro len (zart mig))]
    |-  ^-  (list [@ @])
    ?:  =(0 len)
      ~
    =>  .(len (dec len))
    =+  mog=(cut 3 [len 1] cry)
    [[1 :(mix mig (end 3 len) (zyrt mog))] $(mig mog)]
  ::
  ++  xafo  |=([a=@ b=@] +((mod (add (dec b) a) 255)))
  ++  xaro  |=([a=@ b=@] +((mod (add (dec b) (sub 255 (mod a 255))) 255)))
  ::
  ++  zaft                                              ::  forward 255-sbox
    |=  a=@D
    =+  ^=  b
        0xcc.75bc.86c8.2fb1.9a42.f0b3.79a0.92ca.21f6.1e41.cde5.fcc0.
        7e85.51ae.1005.c72d.1246.07e8.7c64.a914.8d69.d9f4.59c2.8038.
        1f4a.dca2.6fdf.66f9.f561.a12e.5a16.f7b0.a39f.364e.cb70.7318.
        1de1.ad31.63d1.abd4.db68.6a33.134d.a760.edee.5434.493a.e323.
        930d.8f3d.3562.bb81.0b24.43cf.bea5.a6eb.52b4.0229.06b2.6704.
        78c9.45ec.d75e.58af.c577.b7b9.c40e.017d.90c3.87f8.96fa.1153.
        0372.7f30.1c32.ac83.ff17.c6e4.d36d.6b55.e2ce.8c71.8a5b.b6f3.
        9d4b.eab5.8b3c.e7f2.a8fe.9574.5de0.bf20.3f15.9784.9939.5f9c.
        e609.564f.d8a4.b825.9819.94aa.2c08.8e4c.9b22.477a.2840.3ed6.
        3750.6ef1.44dd.89ef.6576.d00a.fbda.9ed2.3b6c.7b0c.bde9.2ade.
        5c88.c182.481a.1b0f.2bfd.d591.2726.57ba
    (cut 3 [(dec a) 1] b)
  ::
  ++  zart                                              ::  reverse 255-sbox
    |=  a=@D
    =+  ^=  b
        0x68.4f07.ea1c.73c9.75c2.efc8.d559.5125.f621.a7a8.8591.5613.
        dd52.40eb.65a2.60b7.4bcb.1123.ceb0.1bd6.3c84.2906.b164.19b3.
        1e95.5fec.ffbc.f187.fbe2.6680.7c77.d30e.e94a.9414.fd9a.017d.
        3a7e.5a55.8ff5.8bf9.c181.e5b6.6ab2.35da.50aa.9293.3bc0.cdc6.
        f3bf.1a58.4130.f844.3846.744e.36a0.f205.789e.32d8.5e54.5c22.
        0f76.fce7.4569.0d99.d26e.e879.dc16.2df4.887f.1ffe.4dba.6f5d.
        bbcc.2663.1762.aed7.af8a.ca20.dbb4.9bc7.a942.834c.105b.c4d4.
        8202.3e61.a671.90e6.273d.bdab.3157.cfa4.0c2e.df86.2496.f7ed.
        2b48.2a9d.5318.a343.d128.be9c.a5ad.6bb5.6dfa.c5e1.3408.128d.
        2c04.0339.97a1.2ff0.49d0.eeb8.6c0a.0b37.b967.c347.d9ac.e072.
        e409.7b9f.1598.1d3f.33de.8ce3.8970.8e7a
    (cut 3 [(dec a) 1] b)
  ::
  ++  zyft                                              ::  forward 256-sbox
    |=  a=@D
    =+  ^=  b
        0xbb49.b71f.b881.b402.17e4.6b86.69b5.1647.115f.dddb.7ca5.
          8371.4bd5.19a9.b092.605d.0d9b.e030.a0cc.78ba.5706.4d2d.
          986a.768c.f8e8.c4c7.2f1c.effe.3cae.01c0.253e.65d3.3872.
          ce0e.7a74.8ac6.daac.7e5c.6479.44ec.4143.3d20.4af0.ee6c.
          c828.deca.0377.249f.ffcd.7b4f.eb7d.66f2.8951.042e.595a.
          8e13.f9c3.a79a.f788.6199.9391.7fab.6200.4ce5.0758.e2f1.
          7594.c945.d218.4248.afa1.e61a.54fb.1482.bea4.96a2.3473.
          63c2.e7cb.155b.120a.4ed7.bfd8.b31b.4008.f329.fca3.5380.
          9556.0cb2.8722.2bea.e96e.3ac5.d1bc.10e3.2c52.a62a.b1d6.
          35aa.d05e.f6a8.0f3b.31ed.559d.09ad.f585.6d21.fd1d.8d67.
          370b.26f4.70c1.b923.4684.6fbd.cf8b.5036.0539.9cdc.d93f.
          9068.1edf.8f33.b632.d427.97fa.9ee1
    (cut 3 [a 1] b)
  ::
  ++  zyrt                                              ::  reverse 256-sbox
    |=  a=@D
    =+  ^=  b
        0x9fc8.2753.6e02.8fcf.8b35.2b20.5598.7caa.c9a9.30b0.9b48.
          47ce.6371.80f6.407d.00dd.0aa5.ed10.ecb7.0f5a.5c3a.e605.
          c077.4337.17bd.9eda.62a4.79a7.ccb8.44cd.8e64.1ec4.5b6b.
          1842.ffd8.1dfb.fd07.f2f9.594c.3be3.73c6.2cb6.8438.e434.
          8d3d.ea6a.5268.72db.a001.2e11.de8c.88d3.0369.4f7a.87e2.
          860d.0991.25d0.16b9.978a.4bf4.2a1a.e96c.fa50.85b5.9aeb.
          9dbb.b2d9.a2d1.7bba.66be.e81f.1946.29a8.f5d2.f30c.2499.
          c1b3.6583.89e1.ee36.e0b4.6092.937e.d74e.2f6f.513e.9615.
          9c5d.d581.e7ab.fe74.f01b.78b1.ae75.af57.0ec2.adc7.3245.
          12bf.2314.3967.0806.31dc.cb94.d43f.493c.54a6.0421.c3a1.
          1c4a.28ac.fc0b.26ca.5870.e576.f7f1.616d.905f.ef41.33bc.
          df4d.225e.2d56.7fd6.1395.a3f8.c582
    (cut 3 [a 1] b)
  --
::
++  ob
  ~%  %ob  ..ob
    ==
      %fein  fein
      %fynd  fynd
    ==
  |%
  ::
  ::  +fein: conceal structure, v3.
  ::
  ::    +fein conceals planet-sized atoms.  The idea is that it should not be
  ::    trivial to tell which planet a star has spawned under.
  ::
  ++  fein
    ~/  %fein
    |=  pyn=@  ^-  @
    ?:  &((gte pyn 0x1.0000) (lte pyn 0xffff.ffff))
      (add 0x1.0000 (feis (sub pyn 0x1.0000)))
    ?:  &((gte pyn 0x1.0000.0000) (lte pyn 0xffff.ffff.ffff.ffff))
      =/  lo  (dis pyn 0xffff.ffff)
      =/  hi  (dis pyn 0xffff.ffff.0000.0000)
      %+  con  hi
      $(pyn lo)
    pyn
  ::
  ::  +fynd: restore structure, v3.
  ::
  ::    Restores obfuscated values that have been enciphered with +fein.
  ::
  ++  fynd
    ~/  %fynd
    |=  cry=@  ^-  @
    ?:  &((gte cry 0x1.0000) (lte cry 0xffff.ffff))
      (add 0x1.0000 (tail (sub cry 0x1.0000)))
    ?:  &((gte cry 0x1.0000.0000) (lte cry 0xffff.ffff.ffff.ffff))
      =/  lo  (dis cry 0xffff.ffff)
      =/  hi  (dis cry 0xffff.ffff.0000.0000)
      %+  con  hi
      $(cry lo)
    cry
  ::  +feis: a four-round generalised Feistel cipher over the domain
  ::         [0, 2^32 - 2^16 - 1].
  ::
  ::    See: Black & Rogaway (2002), Ciphers for arbitrary finite domains.
  ::
  ++  feis
    |=  m=@
    ^-  @
    (fee 4 0xffff 0x1.0000 (mul 0xffff 0x1.0000) eff m)
  ::
  ::  +tail: reverse +feis.
  ::
  ++  tail
    |=  m=@
    ^-  @
    (feen 4 0xffff 0x1.0000 (mul 0xffff 0x1.0000) eff m)
  ::
  ::  +fee: "Fe" in B&R (2002).
  ::
  ::    A Feistel cipher given the following parameters:
  ::
  ::    r:    number of Feistel rounds
  ::    a, b: parameters such that ab >= k
  ::    k:    value such that the domain of the cipher is [0, k - 1]
  ::    prf:  a gate denoting a family of pseudorandom functions indexed by
  ::          its first argument and taking its second argument as input
  ::    m:    an input value in the domain [0, k - 1]
  ::
  ++  fee
    |=  [r=@ a=@ b=@ k=@ prf=$-([j=@ r=@] @) m=@]
    ^-  @
    =/  c  (fe r a b prf m)
    ?:  (lth c k)
      c
    (fe r a b prf c)
  ::
  ::  +feen: "Fe^-1" in B&R (2002).
  ::
  ::    Reverses a Feistel cipher constructed with parameters as described in
  ::    +fee.
  ::
  ++  feen
    |=  [r=@ a=@ b=@ k=@ prf=$-([j=@ r=@] @) m=@]
    ^-  @
    =/  c  (fen r a b prf m)
    ?:  (lth c k)
      c
    (fen r a b prf c)
  ::
  ::  +fe:  "fe" in B&R (2002).
  ::
  ::    An internal function to +fee.
  ::
  ::    Note that this implementation differs slightly from the reference paper
  ::    to support some legacy behaviour.  See urbit/arvo#1105.
  ::
  ++  fe
    |=  [r=@ a=@ b=@ prf=$-([j=@ r=@] @) m=@]
    =/  j  1
    =/  ell  (mod m a)
    =/  arr  (div m a)
    |-  ^-  @
    ::
    ?:  (gth j r)
      ?.  =((mod r 2) 0)
        (add (mul arr a) ell)
      ::
      :: Note that +fe differs from B&R (2002)'s "fe" below, as a previous
      :: implementation of this cipher contained a bug such that certain inputs
      :: could encipher to the same output.
      ::
      :: To correct these problem cases while also preserving the cipher's
      :: legacy behaviour on most inputs, we check for a problem case (which
      :: occurs when 'arr' is equal to 'a') and, if detected, use an alternate
      :: permutation instead.
      ::
      ?:  =(arr a)
        (add (mul arr a) ell)
      (add (mul ell a) arr)
    ::
    =/  f  (prf (sub j 1) arr)
    ::
    =/  tmp
      ?.  =((mod j 2) 0)
        (mod (add f ell) a)
      (mod (add f ell) b)
    ::
    $(j +(j), ell arr, arr tmp)
  ::
  ::  +fen:  "fe^-1" in B&R (2002).
  ::
  ::    Note that this implementation differs slightly from the reference paper
  ::    to support some legacy behaviour.  See urbit/arvo#1105.
  ::
  ++  fen
    |=  [r=@ a=@ b=@ prf=$-([j=@ r=@] @) m=@]
    =/  j  r
    ::
    =/  ahh
      ?.  =((mod r 2) 0)
        (div m a)
      (mod m a)
    ::
    =/  ale
      ?.  =((mod r 2) 0)
        (mod m a)
      (div m a)
    ::
    :: Similar to the comment in +fe, +fen differs from B&R (2002)'s "fe^-1"
    :: here in order to preserve the legacy cipher's behaviour on most inputs.
    ::
    :: Here problem cases can be identified by 'ahh' equating with 'a'; we
    :: correct those cases by swapping the values of 'ahh' and 'ale'.
    ::
    =/  ell
      ?:  =(ale a)
        ahh
      ale
    ::
    =/  arr
      ?:  =(ale a)
        ale
      ahh
    ::
    |-  ^-  @
    ?:  (lth j 1)
      (add (mul arr a) ell)
    =/  f  (prf (sub j 1) ell)
    ::
    ::  Note that there is a slight deviation here to avoid dealing with
    ::  negative values.  We add 'a' or 'b' to arr as appropriate and reduce
    ::  'f' modulo the same number before performing subtraction.
    ::
    =/  tmp
      ?.  =((mod j 2) 0)
        (mod (sub (add arr a) (mod f a)) a)
      (mod (sub (add arr b) (mod f b)) b)
    ::
    $(j (sub j 1), ell tmp, arr ell)
  ::
  ::  +eff: a murmur3-based pseudorandom function.  'F' in B&R (2002).
  ::
  ++  eff
    |=  [j=@ r=@]
    ^-  @
    (muk (snag j raku) 2 r)
  ::
  ::  +raku: seeds for eff.
  ::
  ++  raku
    ^-  (list @ux)
    :~  0xb76d.5eed
        0xee28.1300
        0x85bc.ae01
        0x4b38.7af7
    ==
  ::
  --
::
::    3g: molds and mold builders
+|  %molds-and-mold-builders
::
+$  coin  $~  [%$ %ud 0]                                ::  print format
          $%  [%$ p=dime]                               ::
              [%blob p=*]                               ::
              [%many p=(list coin)]                     ::
          ==                                            ::
+$  dime  [p=@ta q=@]                                   ::
+$  edge  [p=hair q=(unit [p=* q=nail])]                ::  parsing output
+$  hair  [p=@ud q=@ud]                                 ::  parsing trace
++  like  |*  a=$-(* *)                                 ::  generic edge
          |:  b=`*`[(hair) ~]                           ::
          :-  p=(hair -.b)                              ::
          ^=  q                                         ::
          ?@  +.b  ~                                    ::
          :-  ~                                         ::
          u=[p=(a +>-.b) q=[p=(hair -.b) q=(tape +.b)]] ::
+$  nail  [p=hair q=tape]                               ::  parsing input
+$  pint  [p=[p=@ q=@] q=[p=@ q=@]]                     ::  line+column range
+$  rule  _|:($:nail $:edge)                            ::  parsing rule
+$  spot  [p=path q=pint]                               ::  range in file
+$  tone  $%  [%0 product=*]                            ::  success
              [%1 block=*]                              ::  single block
              [%2 trace=(list [@ta *])]                 ::  error report
          ==                                            ::
+$  toon  $%  [%0 p=*]                                  ::  success
              [%1 p=*]                                  ::  block
              [%2 p=(list tank)]                        ::  stack trace
          ==                                            ::
++  wonk  |*  veq=_$:edge                                ::  product from edge
          ?~(q.veq !! p.u.q.veq)             ::
--
|%
::
::    4a: exotic bases
+|  %exotic-bases
::
++  po                                                  ::  phonetic base
  ~/  %po
  =+  :-  ^=  sis                                       ::  prefix syllables
      'dozmarbinwansamlitsighidfidlissogdirwacsabwissib\
      /rigsoldopmodfoglidhopdardorlorhodfolrintogsilmir\
      /holpaslacrovlivdalsatlibtabhanticpidtorbolfosdot\
      /losdilforpilramtirwintadbicdifrocwidbisdasmidlop\
      /rilnardapmolsanlocnovsitnidtipsicropwitnatpanmin\
      /ritpodmottamtolsavposnapnopsomfinfonbanmorworsip\
      /ronnorbotwicsocwatdolmagpicdavbidbaltimtasmallig\
      /sivtagpadsaldivdactansidfabtarmonranniswolmispal\
      /lasdismaprabtobrollatlonnodnavfignomnibpagsopral\
      /bilhaddocridmocpacravripfaltodtiltinhapmicfanpat\
      /taclabmogsimsonpinlomrictapfirhasbosbatpochactid\
      /havsaplindibhosdabbitbarracparloddosbortochilmac\
      /tomdigfilfasmithobharmighinradmashalraglagfadtop\
      /mophabnilnosmilfopfamdatnoldinhatnacrisfotribhoc\
      /nimlarfitwalrapsarnalmoslandondanladdovrivbacpol\
      /laptalpitnambonrostonfodponsovnocsorlavmatmipfip'
      ^=  dex                                           ::  suffix syllables
      'zodnecbudwessevpersutletfulpensytdurwepserwylsun\
      /rypsyxdyrnuphebpeglupdepdysputlughecryttyvsydnex\
      /lunmeplutseppesdelsulpedtemledtulmetwenbynhexfeb\
      /pyldulhetmevruttylwydtepbesdexsefwycburderneppur\
      /rysrebdennutsubpetrulsynregtydsupsemwynrecmegnet\
      /secmulnymtevwebsummutnyxrextebfushepbenmuswyxsym\
      /selrucdecwexsyrwetdylmynmesdetbetbeltuxtugmyrpel\
      /syptermebsetdutdegtexsurfeltudnuxruxrenwytnubmed\
      /lytdusnebrumtynseglyxpunresredfunrevrefmectedrus\
      /bexlebduxrynnumpyxrygryxfeptyrtustyclegnemfermer\
      /tenlusnussyltecmexpubrymtucfyllepdebbermughuttun\
      /bylsudpemdevlurdefbusbeprunmelpexdytbyttyplevmyl\
      /wedducfurfexnulluclennerlexrupnedlecrydlydfenwel\
      /nydhusrelrudneshesfetdesretdunlernyrsebhulryllud\
      /remlysfynwerrycsugnysnyllyndyndemluxfedsedbecmun\
      /lyrtesmudnytbyrsenwegfyrmurtelreptegpecnelnevfes'
  |%
  ++  ins  ~/  %ins                                     ::  parse prefix
           |=  a=@tas
           =+  b=0
           |-  ^-  (unit @)
           ?:(=(256 b) ~ ?:(=(a (tos b)) [~ b] $(b +(b))))
  ++  ind  ~/  %ind                                     ::  parse suffix
           |=  a=@tas
           =+  b=0
           |-  ^-  (unit @)
           ?:(=(256 b) ~ ?:(=(a (tod b)) [~ b] $(b +(b))))
  ++  tos  ~/  %tos                                     ::  fetch prefix
           |=(a=@ ?>((lth a 256) (cut 3 [(mul 3 a) 3] sis)))
  ++  tod  ~/  %tod                                     ::  fetch suffix
           |=(a=@ ?>((lth a 256) (cut 3 [(mul 3 a) 3] dex)))
  --
::
++  fa                                                  ::  base58check
  =+  key='123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz'
  =/  yek=@ux  ~+
      =-  yek:(roll (rip 3 key) -)
      =+  [a=*char b=*@ yek=`@ux`(fil 3 256 0xff)]
      |.
      [+(b) (mix yek (lsh [3 `@u`a] (~(inv fe 3) b)))]
  |%
  ++  cha  |=(a=char `(unit @uF)`=+(b=(cut 3 [`@`a 1] yek) ?:(=(b 0xff) ~ `b)))
  ++  tok
    |=  a=@ux  ^-  @ux
    =+  b=(pad a)
    =-  (~(net fe 5) (end [3 4] (shay 32 -)))
    (shay (add b (met 3 a)) (lsh [3 b] (swp 3 a)))
  ::
  ++  pad  |=(a=@ =+(b=(met 3 a) ?:((gte b 21) 0 (sub 21 b))))
  ++  enc  |=(a=@ux `@ux`(mix (lsh [3 4] a) (tok a)))
  ++  den
    |=  a=@ux  ^-  (unit @ux)
    =+  b=(rsh [3 4] a)
    ?.  =((tok b) (end [3 4] a))
      ~
    `b
  --
::    4b: text processing
+|  %text-processing
::
++  at                                                  ::  basic printing
  |_  a=@
  ++  r
    ?:  ?&  (gte (met 3 a) 2)
            |-
            ?:  =(0 a)
              &
            =+  vis=(end 3 a)
            ?&  ?|(=('-' vis) ?&((gte vis 'a') (lte vis 'z')))
                $(a (rsh 3 a))
            ==
        ==
      rtam
    ?:  (lte (met 3 a) 2)
      rud
    rux
  ::
  ++  rf    `tape`[?-(a %& '&', %| '|', * !!) ~]
  ++  rn    `tape`[?>(=(0 a) '~') ~]
  ++  rt    `tape`['\'' (weld (mesc (trip a)) `tape`['\'' ~])]
  ++  rta   rt
  ++  rtam  `tape`['%' (trip a)]
  ++  rub   `tape`['0' 'b' (rum 2 ~ |=(b=@ (add '0' b)))]
  ++  rud   (rum 10 ~ |=(b=@ (add '0' b)))
  ++  rum
    |=  [b=@ c=tape d=$-(@ @)]
    ^-  tape
    ?:  =(0 a)
      [(d 0) c]
    =+  e=0
    |-  ^-  tape
    ?:  =(0 a)
      c
    =+  f=&(!=(0 e) =(0 (mod e ?:(=(10 b) 3 4))))
    %=  $
      a  (div a b)
      c  [(d (mod a b)) ?:(f [?:(=(10 b) ',' '-') c] c)]
      e  +(e)
    ==
  ::
  ++  rup
    =+  b=(met 3 a)
    ^-  tape
    :-  '-'
    |-  ^-  tape
    ?:  (gth (met 5 a) 1)
      %+  weld
        $(a (rsh 5 a), b (sub b 4))
      `tape`['-' '-' $(a (end 5 a), b 4)]
    ?:  =(0 b)
      ['~' ~]
    ?:  (lte b 1)
      (trip (tos:po a))
    |-  ^-  tape
    ?:  =(2 b)
      =+  c=(rsh 3 a)
      =+  d=(end 3 a)
      (weld (trip (tod:po c)) (trip (tos:po (mix c d))))
    =+  c=(rsh [3 2] a)
    =+  d=(end [3 2] a)
    (weld ^$(a c, b (met 3 c)) `tape`['-' $(a (mix c d), b 2)])
  ::
  ++  ruv
    ^-  tape
    :+  '0'
      'v'
    %^    rum
        64
      ~
    |=  b=@
    ?:  =(63 b)
      '+'
    ?:  =(62 b)
      '-'
    ?:((lth b 26) (add 65 b) ?:((lth b 52) (add 71 b) (sub b 4)))
  ::
  ++  rux  `tape`['0' 'x' (rum 16 ~ |=(b=@ (add b ?:((lth b 10) 48 87))))]
  --
++  cass                                                ::  lowercase
  |=  vib=tape
  ^-  tape
  (turn vib |=(a=@ ?.(&((gte a 'A') (lte a 'Z')) a (add 32 a))))
::
++  cuss                                                ::  uppercase
  |=  vib=tape
  ^-  tape
  (turn vib |=(a=@ ?.(&((gte a 'a') (lte a 'z')) a (sub a 32))))
::
++  crip  |=(a=tape `@t`(rap 3 a))                      ::  tape to cord
::
++  mesc                                                ::  ctrl code escape
  |=  vib=tape
  ^-  tape
  ?~  vib
    ~
  ?:  =('\\' i.vib)
    ['\\' '\\' $(vib t.vib)]
  ?:  ?|((gth i.vib 126) (lth i.vib 32) =(`@`39 i.vib))
    ['\\' (welp ~(rux at i.vib) '/' $(vib t.vib))]
  [i.vib $(vib t.vib)]
::
++  runt                                                ::  prepend repeatedly
  |=  [[a=@ b=@] c=tape]
  ^-  tape
  ?:  =(0 a)
    c
  [b $(a (dec a))]
::
++  sand                                                ::  atom sanity
  |=  a=@ta
  (flit (sane a))
::
++  sane                                                ::  atom sanity
  |=  a=@ta
  |=  b=@  ^-  ?
  ?.  =(%t (end 3 a))
    ::  XX more and better sanity
    ::
    &
  =+  [inx=0 len=(met 3 b)]
  ?:  =(%tas a)
    |-  ^-  ?
    ?:  =(inx len)  &
    =+  cur=(cut 3 [inx 1] b)
    ?&  ?|  &((gte cur 'a') (lte cur 'z'))
            &(=('-' cur) !=(0 inx) !=(len inx))
            &(&((gte cur '0') (lte cur '9')) !=(0 inx))
        ==
        $(inx +(inx))
    ==
  ?:  =(%ta a)
    |-  ^-  ?
    ?:  =(inx len)  &
    =+  cur=(cut 3 [inx 1] b)
    ?&  ?|  &((gte cur 'a') (lte cur 'z'))
            &((gte cur '0') (lte cur '9'))
            |(=('-' cur) =('~' cur) =('_' cur) =('.' cur))
        ==
        $(inx +(inx))
    ==
  |-  ^-  ?
  ?:  =(inx len)  &
  =+  cur=(cut 3 [inx 1] b)
  ?:  &((lth cur 32) !=(10 cur))  |
  =+  tef=(teff cur)
  ?&  ?|  =(1 tef)
          =+  i=1
          |-  ^-  ?
          ?|  =(i tef)
              ?&  (gte (cut 3 [(add i inx) 1] b) 128)
                  $(i +(i))
      ==  ==  ==
      $(inx (add inx tef))
  ==
::
++  ruth                                                ::  biblical sanity
  |=  [a=@ta b=*]
  ^-  @
  ?^  b  !!
  ::  ?.  ((sane a) b)  !!
  b
::
++  trim                                                ::  tape split
  |=  [a=@ b=tape]
  ^-  [p=tape q=tape]
  ?~  b
    [~ ~]
  ?:  =(0 a)
    [~ b]
  =+  c=$(a (dec a), b t.b)
  [[i.b p.c] q.c]
::
++  trip                                                ::  cord to tape
  ~/  %trip
  |=  a=@  ^-  tape
  ?:  =(0 (met 3 a))
    ~
  [^-(@ta (end 3 a)) $(a (rsh 3 a))]
::
++  teff                                                ::  length utf8
  |=  a=@t  ^-  @
  =+  b=(end 3 a)
  ?:  =(0 b)
    ?>(=(`@`0 a) 0)
  ?>  |((gte b 32) =(10 b))
  ?:((lte b 127) 1 ?:((lte b 223) 2 ?:((lte b 239) 3 4)))
::
++  taft                                                ::  utf8 to utf32
  |=  a=@t
  ^-  @c
  %+  rap  5
  |-  ^-  (list @c)
  =+  b=(teff a)
  ?:  =(0 b)  ~
  =+  ^=  c
      %+  can  0
      %+  turn
        ^-  (list [p=@ q=@])
        ?+  b  !!
          %1  [[0 7] ~]
          %2  [[8 6] [0 5] ~]
          %3  [[16 6] [8 6] [0 4] ~]
          %4  [[24 6] [16 6] [8 6] [0 3] ~]
        ==
      |=([p=@ q=@] [q (cut 0 [p q] a)])
  ?>  =((tuft c) (end [3 b] a))
  [c $(a (rsh [3 b] a))]
::
++  tuba                                                ::  utf8 to utf32 tape
  |=  a=tape
  ^-  (list @c)
  (rip 5 (taft (rap 3 a)))                              ::  XX horrible
::
++  tufa                                                ::  utf32 to utf8 tape
  |=  a=(list @c)
  ^-  tape
  ?~  a  ""
  (weld (rip 3 (tuft i.a)) $(a t.a))
::
++  tuft                                                ::  utf32 to utf8 text
  |=  a=@c
  ^-  @t
  %+  rap  3
  |-  ^-  (list @)
  ?:  =(`@`0 a)
    ~
  =+  b=(end 5 a)
  =+  c=$(a (rsh 5 a))
  ?:  (lte b 0x7f)
    [b c]
  ?:  (lte b 0x7ff)
    :*  (mix 0b1100.0000 (cut 0 [6 5] b))
        (mix 0b1000.0000 (end [0 6] b))
        c
    ==
  ?:  (lte b 0xffff)
    :*  (mix 0b1110.0000 (cut 0 [12 4] b))
        (mix 0b1000.0000 (cut 0 [6 6] b))
        (mix 0b1000.0000 (end [0 6] b))
        c
    ==
  :*  (mix 0b1111.0000 (cut 0 [18 3] b))
      (mix 0b1000.0000 (cut 0 [12 6] b))
      (mix 0b1000.0000 (cut 0 [6 6] b))
      (mix 0b1000.0000 (end [0 6] b))
      c
  ==
::
++  wack                                                ::  knot escape
  |=  a=@ta
  ^-  @ta
  =+  b=(rip 3 a)
  %+  rap  3
  |-  ^-  tape
  ?~  b
    ~
  ?:  =('~' i.b)  ['~' '~' $(b t.b)]
  ?:  =('_' i.b)  ['~' '-' $(b t.b)]
  [i.b $(b t.b)]
::
++  wick                                                ::  knot unescape
  |=  a=@
  ^-  (unit @ta)
  =+  b=(rip 3 a)
  =-  ?^(b ~ (some (rap 3 (flop c))))
  =|  c=tape
  |-  ^-  [b=tape c=tape]
  ?~  b  [~ c]
  ?.  =('~' i.b)
    $(b t.b, c [i.b c])
  ?~  t.b  [b ~]
  ?-  i.t.b
    %'~'  $(b t.t.b, c ['~' c])
    %'-'  $(b t.t.b, c ['_' c])
    @     [b ~]
  ==
::
++  woad                                                ::  cord unescape
  |=  a=@ta
  ^-  @t
  %+  rap  3
  |-  ^-  (list @)
  ?:  =(`@`0 a)
    ~
  =+  b=(end 3 a)
  =+  c=(rsh 3 a)
  ?:  =('.' b)
    [' ' $(a c)]
  ?.  =('~' b)
    [b $(a c)]
  =>  .(b (end 3 c), c (rsh 3 c))
  ?+  b  =-  (weld (rip 3 (tuft p.d)) $(a q.d))
         ^=  d
         =+  d=0
         |-  ^-  [p=@ q=@]
         ?:  =('.' b)
           [d c]
         ?<  =(0 c)
         %=    $
            b  (end 3 c)
            c  (rsh 3 c)
            d  %+  add  (mul 16 d)
               %+  sub  b
               ?:  &((gte b '0') (lte b '9'))  48
               ?>(&((gte b 'a') (lte b 'z')) 87)
         ==
    %'.'  ['.' $(a c)]
    %'~'  ['~' $(a c)]
  ==
::
++  wood                                                ::  cord escape
  |=  a=@t
  ^-  @ta
  %+  rap  3
  |-  ^-  (list @)
  ?:  =(`@`0 a)
    ~
  =+  b=(teff a)
  =+  c=(taft (end [3 b] a))
  =+  d=$(a (rsh [3 b] a))
  ?:  ?|  &((gte c 'a') (lte c 'z'))
          &((gte c '0') (lte c '9'))
          =(`@`'-' c)
      ==
    [c d]
  ?+  c
    :-  '~'
    =+  e=(met 2 c)
    |-  ^-  tape
    ?:  =(0 e)
      ['.' d]
    =.  e  (dec e)
    =+  f=(rsh [2 e] c)
    [(add ?:((lte f 9) 48 87) f) $(c (end [2 e] c))]
  ::
    %' '  ['.' d]
    %'.'  ['~' '.' d]
    %'~'  ['~' '~' d]
  ==
::
::  4c: tank printer
+|  %tank-printer
::
++  wash                                                ::  render tank at width
  |=  [[tab=@ edg=@] tac=tank]  ^-  wall
  (~(win re tac) tab edg)
::
::  +re: tank renderer
::
++  re
  |_  tac=tank
  ::  +ram: render a tank to one line (flat)
  ::
  ++  ram
    ^-  tape
    ?@  tac
      (trip tac)
    ?-    -.tac
        %leaf  p.tac
    ::
    ::  flat %palm rendered as %rose with welded openers
    ::
        %palm
      =*  mid  p.p.tac
      =*  for  (weld q.p.tac r.p.tac)
      =*  end  s.p.tac
      ram(tac [%rose [mid for end] q.tac])
    ::
    ::  flat %rose rendered with open/mid/close
    ::
        %rose
      =*  mid  p.p.tac
      =*  for  q.p.tac
      =*  end  r.p.tac
      =*  lit  q.tac
      %+  weld
        for
      |-  ^-  tape
      ?~  lit
        end
      %+  weld
        ram(tac i.lit)
      =*  voz  $(lit t.lit)
      ?~(t.lit voz (weld mid voz))
    ==
  ::  +win: render a tank to multiple lines (tall)
  ::
  ::    indented by .tab, soft-wrapped at .edg
  ::
  ++  win
    |=  [tab=@ud edg=@ud]
    ::  output stack
    ::
    =|  lug=wall
    |^  ^-  wall
        ?@  tac
          (rig (trip tac))
        ?-    -.tac
            %leaf  (rig p.tac)
        ::
            %palm
          =/  hom  ram
          ?:  (lte (lent hom) (sub edg tab))
            (rig hom)
          ::
          =*  for  q.p.tac
          =*  lit  q.tac
          ?~  lit
            (rig for)
          ?~  t.lit
            =:  tab  (add 2 tab)
                lug  $(tac i.lit)
              ==
            (rig for)
          ::
          =>  .(lit `(list tank)`lit)
          =/  lyn  (mul 2 (lent lit))
          =.  lug
            |-  ^-  wall
            ?~  lit
              lug
            =/  nyl  (sub lyn 2)
            %=  ^$
              tac  i.lit
              tab  (add tab nyl)
              lug  $(lit t.lit, lyn nyl)
            ==
          (wig for)
        ::
            %rose
          =/  hom  ram
          ?:  (lte (lent hom) (sub edg tab))
            (rig hom)
          ::
          =*  for  q.p.tac
          =*  end  r.p.tac
          =*  lit  q.tac
          =.  lug
            |-  ^-  wall
            ?~  lit
              ?~(end lug (rig end))
            %=  ^$
              tac  i.lit
              tab  (mod (add 2 tab) (mul 2 (div edg 3)))
              lug  $(lit t.lit)
            ==
          ?~(for lug (wig for))
        ==
    ::  +rig: indent tape and cons with output stack
    ::
    ++  rig
      |=  hom=tape
      ^-  wall
      [(runt [tab ' '] hom) lug]
    ::  +wig: indent tape and cons with output stack
    ::
    ::    joined with the top line if whitespace/indentation allow
    ::
    ++  wig
      |=  hom=tape
      ^-  wall
      ?~  lug
        (rig hom)
      =/  wug  :(add 1 tab (lent hom))
      ?.  =+  mir=i.lug
          |-  ^-  ?
          ?~  mir  |
          ?|  =(0 wug)
              ?&(=(' ' i.mir) $(mir t.mir, wug (dec wug)))
          ==
        (rig hom)       :: ^ XX regular form?
      :_  t.lug
      %+  runt  [tab ' ']
      (weld hom `tape`[' ' (slag wug i.lug)])
    --
  --
++  show                                                ::  XX deprecated!
  |=  vem=*
  |^  ^-  tank
      ?:  ?=(@ vem)
        [%leaf (mesc (trip vem))]
      ?-    vem
          [s=~ c=*]
        [%leaf '\'' (weld (mesc (tape +.vem)) `tape`['\'' ~])]
      ::
          [s=%a c=@]        [%leaf (mesc (trip c.vem))]
          [s=%b c=*]        (shop c.vem |=(a=@ ~(rub at a)))
          [s=[%c p=@] c=*]
        :+  %palm
          [['.' ~] ['-' ~] ~ ~]
        [[%leaf (mesc (trip p.s.vem))] $(vem c.vem) ~]
      ::
          [s=%d c=*]        (shop c.vem |=(a=@ ~(rud at a)))
          [s=%k c=*]        (tank c.vem)
          [s=%h c=*]
        :+  %rose
          [['/' ~] ['/' ~] ~]
        =+  yol=((list @ta) c.vem)
        (turn yol |=(a=@ta [%leaf (trip a)]))
      ::
          [s=%l c=*]        (shol c.vem)
          [s=%o c=*]
        %=    $
            vem
          :-  [%m '%h::[%d %d].[%d %d]>']
          [-.c.vem +<-.c.vem +<+.c.vem +>-.c.vem +>+.c.vem ~]
        ==
      ::
          [s=%p c=*]        (shop c.vem |=(a=@ ~(rup at a)))
          [s=%q c=*]        (shop c.vem |=(a=@ ~(r at a)))
          [s=%r c=*]        $(vem [[%r ' ' '{' '}'] c.vem])
          [s=%t c=*]        (shop c.vem |=(a=@ ~(rt at a)))
          [s=%v c=*]        (shop c.vem |=(a=@ ~(ruv at a)))
          [s=%x c=*]        (shop c.vem |=(a=@ ~(rux at a)))
          [s=[%m p=@] c=*]  (shep p.s.vem c.vem)
          [s=[%r p=@] c=*]
        $(vem [[%r ' ' (cut 3 [0 1] p.s.vem) (cut 3 [1 1] p.s.vem)] c.vem])
      ::
          [s=[%r p=@ q=@ r=@] c=*]
        :+  %rose
          :*  p=(mesc (trip p.s.vem))
              q=(mesc (trip q.s.vem))
              r=(mesc (trip r.s.vem))
          ==
        |-  ^-  (list tank)
        ?@  c.vem
          ~
        [^$(vem -.c.vem) $(c.vem +.c.vem)]
      ::
          [s=%z c=*]        $(vem [[%r %$ %$ %$] c.vem])
          *                 !!
      ==
  ++  shep
    |=  [fom=@ gar=*]
    ^-  tank
    =+  l=(met 3 fom)
    =+  i=0
    :-  %leaf
    |-  ^-  tape
    ?:  (gte i l)
      ~
    =+  c=(cut 3 [i 1] fom)
    ?.  =(37 c)
      (weld (mesc [c ~]) $(i +(i)))
    =+  d=(cut 3 [+(i) 1] fom)
    ?.  .?(gar)
      ['\\' '#' $(i (add 2 i))]
    (weld ~(ram re (show d -.gar)) $(i (add 2 i), gar +.gar))
  ::
  ++  shop
    |=  [aug=* vel=$-(a=@ tape)]
    ^-  tank
    ?:  ?=(@ aug)
      [%leaf (vel aug)]
    :+  %rose
      [[' ' ~] ['[' ~] [']' ~]]
    =>  .(aug `*`aug)
    |-  ^-  (list tank)
    ?:  ?=(@ aug)
      [^$ ~]
    [^$(aug -.aug) $(aug +.aug)]
  ::
  ++  shol
    |=  lim=*
    :+  %rose
      [['.' ~] ~ ~]
    |-    ^-  (list tank)
    ?:  ?=(@ lim)  ~
    :_  $(lim +.lim)
    ?+  -.lim  (show '#')
        ~   (show '$')
        c=@  (show c.lim)
        [%& %1]  (show '.')
        [%& c=@]
      [%leaf '+' ~(rud at c.lim)]
    ::
        [%| @ ~]  (show ',')
        [%| n=@ ~ c=@]
      [%leaf (weld (reap n.lim '^') ?~(c.lim "$" (trip c.lim)))]
    ==
  --
::
::    4d: parsing (tracing)
+|  %parsing-tracing
::
++  last  |=  [zyc=hair naz=hair]                       ::  farther trace
          ^-  hair
          ?:  =(p.zyc p.naz)
            ?:((gth q.zyc q.naz) zyc naz)
          ?:((gth p.zyc p.naz) zyc naz)
::
++  lust  |=  [weq=char naz=hair]                       ::  detect newline
          ^-  hair
          ?:(=(`@`10 weq) [+(p.naz) 1] [p.naz +(q.naz)])
::
::    4e: parsing (combinators)
+|  %parsing-combinators
::
++  bend                                                ::  conditional comp
  ~/  %bend
  |*  raq=_|*([a=* b=*] [~ u=[a b]])
  ~/  %fun
  |*  [vex=edge sab=rule]
  ?~  q.vex
    vex
  =+  yit=(sab q.u.q.vex)
  =+  yur=(last p.vex p.yit)
  ?~  q.yit
    [p=yur q=q.vex]
  =+  vux=(raq p.u.q.vex p.u.q.yit)
  ?~  vux
    [p=yur q=q.vex]
  [p=yur q=[~ u=[p=u.vux q=q.u.q.yit]]]
::
++  comp
  ~/  %comp
  |*  raq=_|*([a=* b=*] [a b])                           ::  arbitrary compose
  ~/  %fun
  |*  [vex=edge sab=rule]
  ~!  +<
  ?~  q.vex
    vex
  =+  yit=(sab q.u.q.vex)
  =+  yur=(last p.vex p.yit)
  ?~  q.yit
    [p=yur q=q.yit]
  [p=yur q=[~ u=[p=(raq p.u.q.vex p.u.q.yit) q=q.u.q.yit]]]
::
++  fail  |=(tub=nail [p=p.tub q=~])                    ::  never parse
++  glue                                                ::  add rule
  ~/  %glue
  |*  bus=rule
  ~/  %fun
  |*  [vex=edge sab=rule]
  (plug vex ;~(pfix bus sab))
::
++  less                                                ::  no first and second
  |*  [vex=edge sab=rule]
  ?~  q.vex
    =+  roq=(sab)
    [p=(last p.vex p.roq) q=q.roq]
  (fail +<.sab)
::
++  pfix                                                ::  discard first rule
  ~/  %pfix
  |*  sam=[vex=edge sab=rule]
  %.  sam
  (comp |*([a=* b=*] b))
::
++  plug                                                ::  first then second
  ~/  %plug
  |*  [vex=edge sab=rule]
  ?~  q.vex
    vex
  =+  yit=(sab q.u.q.vex)
  =+  yur=(last p.vex p.yit)
  ?~  q.yit
    [p=yur q=q.yit]
  [p=yur q=[~ u=[p=[p.u.q.vex p.u.q.yit] q=q.u.q.yit]]]
::
++  pose                                                ::  first or second
  ~/  %pose
  |*  [vex=edge sab=rule]
  ?~  q.vex
    =+  roq=(sab)
    [p=(last p.vex p.roq) q=q.roq]
  vex
::
++  simu                                                ::  first and second
  |*  [vex=edge sab=rule]
  ?~  q.vex
    vex
  =+  roq=(sab)
  roq
::
++  sfix                                                ::  discard second rule
  ~/  %sfix
  |*  sam=[vex=edge sab=rule]
  %.  sam
  (comp |*([a=* b=*] a))
::
::    4f: parsing (rule builders)
+|  %parsing-rule-builders
::
++  bass                                                ::  leftmost base
  |*  [wuc=@ tyd=rule]
  %+  cook
    |=  waq=(list @)
    %+  roll
      waq
    =|([p=@ q=@] |.((add p (mul wuc q))))
  tyd
::
++  boss                                                ::  rightmost base
  |*  [wuc=@ tyd=rule]
  %+  cook
    |=  waq=(list @)
    %+  reel
      waq
    =|([p=@ q=@] |.((add p (mul wuc q))))
  tyd
::
++  cold                                                ::  replace w+ constant
  ~/  %cold
  |*  [cus=* sef=rule]
  ~/  %fun
  |=  tub=nail
  =+  vex=(sef tub)
  ?~  q.vex
    vex
  [p=p.vex q=[~ u=[p=cus q=q.u.q.vex]]]
::
++  cook                                                ::  apply gate
  ~/  %cook
  |*  [poq=gate sef=rule]
  ~/  %fun
  |=  tub=nail
  =+  vex=(sef tub)
  ?~  q.vex
    vex
  [p=p.vex q=[~ u=[p=(poq p.u.q.vex) q=q.u.q.vex]]]
::
++  easy                                                ::  always parse
  ~/  %easy
  |*  huf=*
  ~/  %fun
  |=  tub=nail
  ^-  (like _huf)
  [p=p.tub q=[~ u=[p=huf q=tub]]]
::
++  fuss
  |=  [sic=@t non=@t]
  ;~(pose (cold %& (jest sic)) (cold %| (jest non)))
::
++  full                                                ::  has to fully parse
  |*  sef=rule
  |=  tub=nail
  =+  vex=(sef tub)
  ?~(q.vex vex ?:(=(~ q.q.u.q.vex) vex [p=p.vex q=~]))
::
++  funk                                                ::  add to tape first
  |*  [pre=tape sef=rule]
  |=  tub=nail
  (sef p.tub (weld pre q.tub))
::
++  here                                                ::  place-based apply
  ~/  %here
  |*  [hez=_|=([a=pint b=*] [a b]) sef=rule]
  ~/  %fun
  |=  tub=nail
  =+  vex=(sef tub)
  ?~  q.vex
    vex
  [p=p.vex q=[~ u=[p=(hez [p.tub p.q.u.q.vex] p.u.q.vex) q=q.u.q.vex]]]
::
++  inde  |*  sef=rule                                  :: indentation block
  |=  nail  ^+  (sef)
  =+  [har tap]=[p q]:+<
  =+  lev=(fil 3 (dec q.har) ' ')
  =+  eol=(just `@t`10)
  =+  =-  roq=((star ;~(pose prn ;~(sfix eol (jest lev)) -)) har tap)
      ;~(simu ;~(plug eol eol) eol)
  ?~  q.roq  roq
  =+  vex=(sef har(q 1) p.u.q.roq)
  =+  fur=p.vex(q (add (dec q.har) q.p.vex))
  ?~  q.vex  vex(p fur)
  =-  vex(p fur, u.q -)
  :+  &3.vex
    &4.vex(q.p (add (dec q.har) q.p.&4.vex))
  =+  res=|4.vex
  |-  ?~  res  |4.roq
  ?.  =(10 -.res)  [-.res $(res +.res)]
  (welp [`@t`10 (trip lev)] $(res +.res))
::
++  ifix
  |*  [fel=[rule rule] hof=rule]
  ~!  +<
  ~!  +<:-.fel
  ~!  +<:+.fel
  ;~(pfix -.fel ;~(sfix hof +.fel))
::
++  jest                                                ::  match a cord
  |=  daf=@t
  |=  tub=nail
  =+  fad=daf
  |-  ^-  (like @t)
  ?:  =(`@`0 daf)
    [p=p.tub q=[~ u=[p=fad q=tub]]]
  ?:  |(?=(~ q.tub) !=((end 3 daf) i.q.tub))
    (fail tub)
  $(p.tub (lust i.q.tub p.tub), q.tub t.q.tub, daf (rsh 3 daf))
::
++  just                                                ::  XX redundant, jest
  ~/  %just                                             ::  match a char
  |=  daf=char
  ~/  %fun
  |=  tub=nail
  ^-  (like char)
  ?~  q.tub
    (fail tub)
  ?.  =(daf i.q.tub)
    (fail tub)
  (next tub)
::
++  knee                                                ::  callbacks
  |*  [gar=* sef=_|.(*rule)]
  |=  tub=nail
  ^-  (like _gar)
  ((sef) tub)
::
++  mask                                                ::  match char in set
  ~/  %mask
  |=  bud=(list char)
  ~/  %fun
  |=  tub=nail
  ^-  (like char)
  ?~  q.tub
    (fail tub)
  ?.  (lien bud |=(a=char =(i.q.tub a)))
    (fail tub)
  (next tub)
::
++  more                                                ::  separated, *
  |*  [bus=rule fel=rule]
  ;~(pose (most bus fel) (easy ~))
::
++  most                                                ::  separated, +
  |*  [bus=rule fel=rule]
  ;~(plug fel (star ;~(pfix bus fel)))
::
++  next                                                ::  consume a char
  |=  tub=nail
  ^-  (like char)
  ?~  q.tub
    (fail tub)
  =+  zac=(lust i.q.tub p.tub)
  [zac [~ i.q.tub [zac t.q.tub]]]
::
++  perk                                                ::  parse cube fork
  |*  a=(pole @tas)
  ?~  a  fail
  ;~  pose
    (cold -.a (jest -.a))
    $(a +.a)
  ==
::
++  pick                                                ::  rule for ++each
  |*  [a=rule b=rule]
  ;~  pose
    (stag %& a)
    (stag %| b)
  ==
++  plus  |*(fel=rule ;~(plug fel (star fel)))          ::
++  punt  |*([a=rule] ;~(pose (stag ~ a) (easy ~)))     ::
++  sear                                                ::  conditional cook
  |*  [pyq=$-(* (unit)) sef=rule]
  |=  tub=nail
  =+  vex=(sef tub)
  ?~  q.vex
    vex
  =+  gey=(pyq p.u.q.vex)
  ?~  gey
    [p=p.vex q=~]
  [p=p.vex q=[~ u=[p=u.gey q=q.u.q.vex]]]
::
++  shim                                                ::  match char in range
  ~/  %shim
  |=  [les=@ mos=@]
  ~/  %fun
  |=  tub=nail
  ^-  (like char)
  ?~  q.tub
    (fail tub)
  ?.  ?&((gte i.q.tub les) (lte i.q.tub mos))
    (fail tub)
  (next tub)
::
++  stag                                                ::  add a label
  ~/  %stag
  |*  [gob=* sef=rule]
  ~/  %fun
  |=  tub=nail
  =+  vex=(sef tub)
  ?~  q.vex
    vex
  [p=p.vex q=[~ u=[p=[gob p.u.q.vex] q=q.u.q.vex]]]
::
++  stet                                                ::
  |*  leh=(list [?(@ [@ @]) rule])
  |-
  ?~  leh
    ~
  [i=[p=-.i.leh q=+.i.leh] t=$(leh t.leh)]
::
++  stew                                                ::  switch by first char
  ~/  %stew
  |*  leh=(list [p=?(@ [@ @]) q=rule])                  ::  char+range keys
  =+  ^=  wor                                           ::  range complete lth
      |=  [ort=?(@ [@ @]) wan=?(@ [@ @])]
      ?@  ort
        ?@(wan (lth ort wan) (lth ort -.wan))
      ?@(wan (lth +.ort wan) (lth +.ort -.wan))
  =+  ^=  hel                                           ::  build parser map
      =+  hel=`(tree _?>(?=(^ leh) i.leh))`~
      |-  ^+  hel
      ?~  leh
        ~
      =+  yal=$(leh t.leh)
      |-  ^+  hel
      ?~  yal
        [i.leh ~ ~]
      ?:  (wor p.i.leh p.n.yal)
        =+  nuc=$(yal l.yal)
        ?>  ?=(^ nuc)
        ?:  (mor p.n.yal p.n.nuc)
          [n.yal nuc r.yal]
        [n.nuc l.nuc [n.yal r.nuc r.yal]]
      =+  nuc=$(yal r.yal)
      ?>  ?=(^ nuc)
      ?:  (mor p.n.yal p.n.nuc)
        [n.yal l.yal nuc]
      [n.nuc [n.yal l.yal l.nuc] r.nuc]
  ~%  %fun  ..^$  ~
  |=  tub=nail
  ?~  q.tub
    (fail tub)
  |-
  ?~  hel
    (fail tub)
  ?:  ?@  p.n.hel
        =(p.n.hel i.q.tub)
      ?&((gte i.q.tub -.p.n.hel) (lte i.q.tub +.p.n.hel))
    ::  (q.n.hel [(lust i.q.tub p.tub) t.q.tub])
    (q.n.hel tub)
  ?:  (wor i.q.tub p.n.hel)
    $(hel l.hel)
  $(hel r.hel)
::
++  slug                                                ::
  |*  raq=_=>(~ |*([a=* b=*] [a b]))
  |*  [bus=rule fel=rule]
  ;~((comp raq) fel (stir +<+.raq raq ;~(pfix bus fel)))
::
++  star                                                ::  0 or more times
  |*  fel=rule
  (stir `(list _(wonk *fel))`~ |*([a=* b=*] [a b]) fel)
::
++  stir
  ~/  %stir
  |*  [rud=* raq=_=>(~ |*([a=* b=*] [a b])) fel=rule]
  ~/  %fun
  |=  tub=nail
  ^-  (like _rud)
  ::
  ::  lef: successful interim parse results (per .fel)
  ::  wag: initial accumulator (.rud in .tub at farthest success)
  ::
  =+  ^=  [lef wag]
    =|  lef=(list _(fel tub))
    |-  ^-  [_lef (pair hair [~ u=(pair _rud nail)])]
    =+  vex=(fel tub)
    ?~  q.vex
      :-  lef
      [p.vex [~ rud tub]]
    $(lef [vex lef], tub q.u.q.vex)
  ::
  ::  fold .lef into .wag, combining results with .raq
  ::
  %+  roll  lef
  |=  _[vex=(fel tub) wag=wag]  :: q.vex is always (some)
  ^+  wag
  :-  (last p.vex p.wag)
  [~ (raq p.u.+.q.vex p.u.q.wag) q.u.q.wag]
::
++  stun                                                ::  parse several times
  ~/  %stun
  |*  [lig=[@ @] fel=rule]
  |=  tub=nail
  ^-  (like (list _(wonk (fel))))
  ?:  =(0 +.lig)
    [p.tub [~ ~ tub]]
  =+  vex=(fel tub)
  ?~  q.vex
    ?:  =(0 -.lig)
      [p.vex [~ ~ tub]]
    vex
  =+  ^=  wag  %=  $
                 -.lig  ?:(=(0 -.lig) 0 (dec -.lig))
                 +.lig  ?:(=(0 +.lig) 0 (dec +.lig))
                 tub  q.u.q.vex
               ==
  ?~  q.wag
    wag
  [p.wag [~ [p.u.q.vex p.u.q.wag] q.u.q.wag]]
::
::    4g: parsing (outside caller)
+|  %parsing-outside-caller
::
++  rash  |*([naf=@ sab=rule] (scan (trip naf) sab))
++  rose  |*  [los=tape sab=rule]
          =+  vex=(sab [[1 1] los])
          =+  len=(lent los)
          ?.  =(+(len) q.p.vex)  [%| p=(dec q.p.vex)]
          ?~  q.vex
            [%& p=~]
          [%& p=[~ u=p.u.q.vex]]
++  rush  |*([naf=@ sab=rule] (rust (trip naf) sab))
++  rust  |*  [los=tape sab=rule]
          =+  vex=((full sab) [[1 1] los])
          ?~(q.vex ~ [~ u=p.u.q.vex])
++  scan  |*  [los=tape sab=rule]
          =+  vex=((full sab) [[1 1] los])
          ?~  q.vex
            ~_  (show [%m '{%d %d}'] p.p.vex q.p.vex ~)
            ~_(leaf+"syntax error" !!)
          p.u.q.vex
::
::    4h: parsing (ascii glyphs)
+|  %parsing-ascii-glyphs
::
++  ace  (just ' ')                                     ::  spACE
++  bar  (just '|')                                     ::  vertical BAR
++  bas  (just '\\')                                    ::  Back Slash (escaped)
++  buc  (just '$')                                     ::  dollars BUCks
++  cab  (just '_')                                     ::  CABoose
++  cen  (just '%')                                     ::  perCENt
++  col  (just ':')                                     ::  COLon
++  com  (just ',')                                     ::  COMma
++  doq  (just '"')                                     ::  Double Quote
++  dot  (just '.')                                     ::  dot dot dot ...
++  fas  (just '/')                                     ::  Forward Slash
++  gal  (just '<')                                     ::  Greater Left
++  gar  (just '>')                                     ::  Greater Right
++  hax  (just '#')                                     ::  Hash
++  hep  (just '-')                                     ::  HyPhen
++  kel  (just '{')                                     ::  Curly Left
++  ker  (just '}')                                     ::  Curly Right
++  ket  (just '^')                                     ::  CareT
++  lus  (just '+')                                     ::  pLUS
++  mic  (just ';')                                     ::  seMIColon
++  pal  (just '(')                                     ::  Paren Left
++  pam  (just '&')                                     ::  AMPersand pampersand
++  par  (just ')')                                     ::  Paren Right
++  pat  (just '@')                                     ::  AT pat
++  sel  (just '[')                                     ::  Square Left
++  ser  (just ']')                                     ::  Square Right
++  sig  (just '~')                                     ::  SIGnature squiggle
++  soq  (just '\'')                                    ::  Single Quote
++  tar  (just '*')                                     ::  sTAR
++  tic  (just '`')                                     ::  backTiCk
++  tis  (just '=')                                     ::  'tis tis, it is
++  wut  (just '?')                                     ::  wut, what?
++  zap  (just '!')                                     ::  zap! bang! crash!!
::
::    4i: parsing (useful idioms)
+|  %parsing-useful-idioms
::
++  alf  ;~(pose low hig)                               ::  alphabetic
++  aln  ;~(pose low hig nud)                           ::  alphanumeric
++  alp  ;~(pose low hig nud hep)                       ::  alphanumeric and -
++  bet  ;~(pose (cold 2 hep) (cold 3 lus))             ::  axis syntax - +
++  bin  (bass 2 (most gon but))                        ::  binary to atom
++  but  (cook |=(a=@ (sub a '0')) (shim '0' '1'))      ::  binary digit
++  cit  (cook |=(a=@ (sub a '0')) (shim '0' '7'))      ::  octal digit
++  dem  (bass 10 (most gon dit))                       ::  decimal to atom
++  dit  (cook |=(a=@ (sub a '0')) (shim '0' '9'))      ::  decimal digit
++  dog  ;~(plug dot gay)                               ::  .  number separator
++  dof  ;~(plug hep gay)                               ::  - @q separator
++  doh  ;~(plug ;~(plug hep hep) gay)                  ::  --  phon separator
++  dun  (cold ~ ;~(plug hep hep))                      ::  -- (stop) to ~
++  duz  (cold ~ ;~(plug tis tis))                      ::  == (stet) to ~
++  gah  (mask [`@`10 ' ' ~])                           ::  newline or ace
++  gap  (cold ~ ;~(plug gaq (star ;~(pose vul gah))))  ::  plural space
++  gaq  ;~  pose                                       ::  end of line
             (just `@`10)
             ;~(plug gah ;~(pose gah vul))
             vul
         ==
++  gaw  (cold ~ (star ;~(pose vul gah)))               ::  classic white
++  gay  ;~(pose gap (easy ~))                          ::
++  gon  ;~(pose ;~(plug bas gay fas) (easy ~))         ::  long numbers \ /
++  gul  ;~(pose (cold 2 gal) (cold 3 gar))             ::  axis syntax < >
++  hex  (bass 16 (most gon hit))                       ::  hex to atom
++  hig  (shim 'A' 'Z')                                 ::  uppercase
++  hit  ;~  pose                                       ::  hex digits
           dit
           (cook |=(a=char (sub a 87)) (shim 'a' 'f'))
           (cook |=(a=char (sub a 55)) (shim 'A' 'F'))
         ==
++  iny                                                 :: indentation block
  |*  sef=rule
  |=  nail  ^+  (sef)
  =+  [har tap]=[p q]:+<
  =+  lev=(fil 3 (dec q.har) ' ')
  =+  eol=(just `@t`10)
  =+  =-  roq=((star ;~(pose prn ;~(sfix eol (jest lev)) -)) har tap)
      ;~(simu ;~(plug eol eol) eol)
  ?~  q.roq  roq
  =+  vex=(sef har(q 1) p.u.q.roq)
  =+  fur=p.vex(q (add (dec q.har) q.p.vex))
  ?~  q.vex  vex(p fur)
  =-  vex(p fur, u.q -)
  :+  &3.vex
    &4.vex(q.p (add (dec q.har) q.p.&4.vex))
  =+  res=|4.vex
  |-  ?~  res  |4.roq
  ?.  =(10 -.res)  [-.res $(res +.res)]
  (welp [`@t`10 (trip lev)] $(res +.res))
::
++  low  (shim 'a' 'z')                                 ::  lowercase
++  mes  %+  cook                                       ::  hexbyte
           |=([a=@ b=@] (add (mul 16 a) b))
         ;~(plug hit hit)
++  nix  (boss 256 (star ;~(pose aln cab)))             ::
++  nud  (shim '0' '9')                                 ::  numeric
++  prn  ;~(less (just `@`127) (shim 32 256))           ::  non-control
++  qat  ;~  pose                                       ::  chars in blockcord
             prn
             ;~(less ;~(plug (just `@`10) soz) (just `@`10))
         ==
++  qit  ;~  pose                                       ::  chars in a cord
             ;~(less bas soq prn)
             ;~(pfix bas ;~(pose bas soq mes))          ::  escape chars
         ==
++  qut  ;~  simu  soq                                  ::  cord
           ;~  pose
             ;~  less  soz
               (ifix [soq soq] (boss 256 (more gon qit)))
             ==
             =+  hed=;~(pose ;~(plug (plus ace) vul) (just '\0a'))
             %-  iny  %+  ifix
               :-  ;~(plug soz hed)
               ;~(plug (just '\0a') soz)
             (boss 256 (star qat))
           ==
         ==
++  soz  ;~(plug soq soq soq)                           ::  delimiting '''
++  sym                                                 ::  symbol
  %+  cook
    |=(a=tape (rap 3 ^-((list @) a)))
  ;~(plug low (star ;~(pose nud low hep)))
::
++  mixed-case-symbol
  %+  cook
    |=(a=tape (rap 3 ^-((list @) a)))
  ;~(plug alf (star alp))
::
++  ven  ;~  (comp |=([a=@ b=@] (peg a b)))             ::  +>- axis syntax
           bet
           =+  hom=`?`|
           |=  tub=nail
           ^-  (like @)
           =+  vex=?:(hom (bet tub) (gul tub))
           ?~  q.vex
             [p.tub [~ 1 tub]]
           =+  wag=$(p.tub p.vex, hom !hom, tub q.u.q.vex)
           ?>  ?=(^ q.wag)
           [p.wag [~ (peg p.u.q.vex p.u.q.wag) q.u.q.wag]]
         ==
++  vit                                                 ::  base64 digit
  ;~  pose
    (cook |=(a=@ (sub a 65)) (shim 'A' 'Z'))
    (cook |=(a=@ (sub a 71)) (shim 'a' 'z'))
    (cook |=(a=@ (add a 4)) (shim '0' '9'))
    (cold 62 (just '-'))
    (cold 63 (just '+'))
  ==
++  vul  %+  cold   ~                                   ::  comments
         ;~  plug  col  col
           (star prn)
           (just `@`10)
         ==
::
::    4j: parsing (bases and base digits)
+|  %parsing-bases-and-base-digits
::
++  ab
  |%
  ++  bix  (bass 16 (stun [2 2] six))
  ++  fem  (sear |=(a=@ (cha:fa a)) aln)
  ++  haf  (bass 256 ;~(plug tep tiq (easy ~)))
  ++  hef  %+  sear  |=(a=@ ?:(=(a 0) ~ (some a)))
           %+  bass  256
           ;~(plug tip tiq (easy ~))
  ++  hif  (bass 256 ;~(plug tip tiq (easy ~)))
  ++  hof  (bass 0x1.0000 ;~(plug hef (stun [1 3] ;~(pfix hep hif))))
  ++  huf  (bass 0x1.0000 ;~(plug hef (stun [0 3] ;~(pfix hep hif))))
  ++  hyf  (bass 0x1.0000 ;~(plug hif (stun [3 3] ;~(pfix hep hif))))
  ++  pev  (bass 32 ;~(plug sev (stun [0 4] siv)))
  ++  pew  (bass 64 ;~(plug sew (stun [0 4] siw)))
  ++  piv  (bass 32 (stun [5 5] siv))
  ++  piw  (bass 64 (stun [5 5] siw))
  ++  qeb  (bass 2 ;~(plug seb (stun [0 3] sib)))
  ++  qex  (bass 16 ;~(plug sex (stun [0 3] hit)))
  ++  qib  (bass 2 (stun [4 4] sib))
  ++  qix  (bass 16 (stun [4 4] six))
  ++  seb  (cold 1 (just '1'))
  ++  sed  (cook |=(a=@ (sub a '0')) (shim '1' '9'))
  ++  sev  ;~(pose sed sov)
  ++  sew  ;~(pose sed sow)
  ++  sex  ;~(pose sed sox)
  ++  sib  (cook |=(a=@ (sub a '0')) (shim '0' '1'))
  ++  sid  (cook |=(a=@ (sub a '0')) (shim '0' '9'))
  ++  siv  ;~(pose sid sov)
  ++  siw  ;~(pose sid sow)
  ++  six  ;~(pose sid sox)
  ++  sov  (cook |=(a=@ (sub a 87)) (shim 'a' 'v'))
  ++  sow  ;~  pose
             (cook |=(a=@ (sub a 87)) (shim 'a' 'z'))
             (cook |=(a=@ (sub a 29)) (shim 'A' 'Z'))
             (cold 62 (just '-'))
             (cold 63 (just '~'))
           ==
  ++  sox  (cook |=(a=@ (sub a 87)) (shim 'a' 'f'))
  ++  ted  (bass 10 ;~(plug sed (stun [0 2] sid)))
  ++  tep  (sear |=(a=@ ?:(=(a 'doz') ~ (ins:po a))) til)
  ++  tip  (sear |=(a=@ (ins:po a)) til)
  ++  tiq  (sear |=(a=@ (ind:po a)) til)
  ++  tid  (bass 10 (stun [3 3] sid))
  ++  til  (boss 256 (stun [3 3] low))
  ++  urs  %+  cook
             |=(a=tape (rap 3 ^-((list @) a)))
           (star ;~(pose nud low hep dot sig cab))
  ++  urt  %+  cook
             |=(a=tape (rap 3 ^-((list @) a)))
           (star ;~(pose nud low hep dot sig))
  ++  urx  %+  cook
             |=(a=tape (rap 3 ^-((list @) a)))
           %-  star
           ;~  pose
             nud
             low
             hep
             cab
             (cold ' ' dot)
             (cook tuft (ifix [sig dot] hex))
             ;~(pfix sig ;~(pose sig dot))
           ==
  ++  voy  ;~(pfix bas ;~(pose bas soq bix))
  --
++  ag
  |%
  ++  ape  |*(fel=rule ;~(pose (cold `@`0 (just '0')) fel))
  ++  bay  (ape (bass 16 ;~(plug qeb:ab (star ;~(pfix dog qib:ab)))))
  ++  bip  =+  tod=(ape qex:ab)
           (bass 0x1.0000 ;~(plug tod (stun [7 7] ;~(pfix dog tod))))
  ++  dem  (ape (bass 1.000 ;~(plug ted:ab (star ;~(pfix dog tid:ab)))))
  ++  dim  (ape dip)
  ++  dip  (bass 10 ;~(plug sed:ab (star sid:ab)))
  ++  dum  (bass 10 (plus sid:ab))
  ++  fed  %+  cook  fynd:ob
           ;~  pose
             %+  bass  0x1.0000.0000.0000.0000          ::  oversized
               ;~  plug
                 huf:ab
                 (plus ;~(pfix doh hyf:ab))
               ==
             hof:ab                                     ::  planet or moon
             haf:ab                                     ::  star
             tiq:ab                                     ::  galaxy
           ==
  ++  feq  %+  cook  |=(a=(list @) (rep 4 (flop a)))
           ;~  plug
             ;~(pose hif:ab tiq:ab)
             (star ;~(pfix dof hif:ab))
           ==
  ++  fim  (sear den:fa (bass 58 (plus fem:ab)))
  ++  hex  (ape (bass 0x1.0000 ;~(plug qex:ab (star ;~(pfix dog qix:ab)))))
  ++  lip  =+  tod=(ape ted:ab)
           (bass 256 ;~(plug tod (stun [3 3] ;~(pfix dog tod))))
  ++  mot  ;~  pose
             ;~  pfix
               (just '1')
               (cook |=(a=@ (add 10 (sub a '0'))) (shim '0' '2'))
             ==
             sed:ab
           ==
  ++  viz  (ape (bass 0x200.0000 ;~(plug pev:ab (star ;~(pfix dog piv:ab)))))
  ++  vum  (bass 32 (plus siv:ab))
  ++  wiz  (ape (bass 0x4000.0000 ;~(plug pew:ab (star ;~(pfix dog piw:ab)))))
  --
++  mu
  |_  [top=@ bot=@]
  ++  zag  [p=(end 4 (add top bot)) q=bot]
  ++  zig  [p=(end 4 (add top (sub 0x1.0000 bot))) q=bot]
  ++  zug  (mix (lsh 4 top) bot)
  --
++  ne
  |_  tig=@
  ++  c  (cut 3 [tig 1] key:fa)
  ++  d  (add tig '0')
  ++  x  ?:((gte tig 10) (add tig 87) d)
  ++  v  ?:((gte tig 10) (add tig 87) d)
  ++  w  ?:(=(tig 63) '~' ?:(=(tig 62) '-' ?:((gte tig 36) (add tig 29) x)))
  --
::
::    4k: atom printing
+|  %atom-printing
::
++  co
  !:
  ~%  %co  ..co  ~
  =<  |_  lot=coin
      ++  rear  |=(rom=tape rend(rep rom))
      ++  rent  ~+  `@ta`(rap 3 rend)
      ++  rend
        ^-  tape
        ~+
        ?:  ?=(%blob -.lot)
          ['~' '0' ((v-co 1) (jam p.lot))]
        ?:  ?=(%many -.lot)
          :-  '.'
          |-  ^-  tape
          ?~   p.lot
            ['_' '_' rep]
          ['_' (weld (trip (wack rent(lot i.p.lot))) $(p.lot t.p.lot))]
        =+  [yed=(end 3 p.p.lot) hay=(cut 3 [1 1] p.p.lot)]
        |-  ^-  tape
        ?+    yed  (z-co q.p.lot)
            %c   ['~' '-' (weld (rip 3 (wood (tuft q.p.lot))) rep)]
            %d
          ?+    hay  (z-co q.p.lot)
              %a
            =+  yod=(yore q.p.lot)
            =?  rep  ?=(^ f.t.yod)  ['.' (s-co f.t.yod)]
            =?  rep  !&(?=(~ f) =(0 h) =(0 m) =(0 s)):t.yod
              =.  rep  ['.' (y-co s.t.yod)]
              =.  rep  ['.' (y-co m.t.yod)]
              ['.' '.' (y-co h.t.yod)]
            =.  rep  ['.' (a-co d.t.yod)]
            =.  rep  ['.' (a-co m.yod)]
            =?  rep  !a.yod  ['-' rep]
            ['~' (a-co y.yod)]
          ::
              %r
            =+  yug=(yell q.p.lot)
            =?  rep  ?=(^ f.yug)  ['.' (s-co f.yug)]
            :-  '~'
            ?:  &(=(0 d.yug) =(0 m.yug) =(0 h.yug) =(0 s.yug))
              ['s' '0' rep]
            =?  rep  !=(0 s.yug)  ['.' 's' (a-co s.yug)]
            =?  rep  !=(0 m.yug)  ['.' 'm' (a-co m.yug)]
            =?  rep  !=(0 h.yug)  ['.' 'h' (a-co h.yug)]
            =?  rep  !=(0 d.yug)  ['.' 'd' (a-co d.yug)]
            +.rep
          ==
        ::
            %f
          ?:  =(& q.p.lot)
            ['.' 'y' rep]
          ?:(=(| q.p.lot) ['.' 'n' rep] (z-co q.p.lot))
        ::
            %n   ['~' rep]
            %i
          ?+  hay  (z-co q.p.lot)
            %f  ((ro-co [3 10 4] |=(a=@ ~(d ne a))) q.p.lot)
            %s  ((ro-co [4 16 8] |=(a=@ ~(x ne a))) q.p.lot)
          ==
        ::
            %p
          =+  sxz=(fein:ob q.p.lot)
          =+  dyx=(met 3 sxz)
          :-  '~'
          ?:  (lte dyx 1)
            (weld (trip (tod:po sxz)) rep)
          =+  dyy=(met 4 sxz)
          =|  imp=@ud
          |-  ^-  tape
          ?:  =(imp dyy)
            rep
          %=  $
            imp  +(imp)
            rep  =/  log  (cut 4 [imp 1] sxz)
                 ;:  weld
                   (trip (tos:po (rsh 3 log)))
                   (trip (tod:po (end 3 log)))
                   ?:(=((mod imp 4) 0) ?:(=(imp 0) "" "--") "-")
                   rep
          ==     ==
        ::
            %q
          :+  '.'  '~'
          =;  res=(pair ? tape)
            (weld q.res rep)
          %+  roll
            =*  val  q.p.lot
            ?:(=(0 val) ~[0] (rip 3 val))
          |=  [q=@ s=? r=tape]
          :-  !s
          %+  weld
           (trip (?:(s tod:po tos:po) q))
          ?.(&(s !=(r "")) r ['-' r])
        ::
            %r
          ?+  hay  (z-co q.p.lot)
            %d  ['.' '~' (r-co (rlyd q.p.lot))]
            %h  ['.' '~' '~' (r-co (rlyh q.p.lot))]
            %q  ['.' '~' '~' '~' (r-co (rlyq q.p.lot))]
            %s  ['.' (r-co (rlys q.p.lot))]
          ==
        ::
            %u
          ?:  ?=(%c hay)
            %+  welp  ['0' 'c' (reap (pad:fa q.p.lot) '1')]
            (c-co (enc:fa q.p.lot))
          ::
          =;  gam=(pair tape tape)
            (weld p.gam ?:(=(0 q.p.lot) `tape`['0' ~] q.gam))
          ?+  hay  [~ ((ox-co [10 3] |=(a=@ ~(d ne a))) q.p.lot)]
            %b  [['0' 'b' ~] ((ox-co [2 4] |=(a=@ ~(d ne a))) q.p.lot)]
            %i  [['0' 'i' ~] ((d-co 1) q.p.lot)]
            %x  [['0' 'x' ~] ((ox-co [16 4] |=(a=@ ~(x ne a))) q.p.lot)]
            %v  [['0' 'v' ~] ((ox-co [32 5] |=(a=@ ~(x ne a))) q.p.lot)]
            %w  [['0' 'w' ~] ((ox-co [64 5] |=(a=@ ~(w ne a))) q.p.lot)]
          ==
        ::
            %s
          %+  weld
            ?:((syn:si q.p.lot) "--" "-")
          $(yed 'u', q.p.lot (abs:si q.p.lot))
        ::
            %t
          ?:  =('a' hay)
            ?:  =('s' (cut 3 [2 1] p.p.lot))
              (weld (rip 3 q.p.lot) rep)
            ['~' '.' (weld (rip 3 q.p.lot) rep)]
          ['~' '~' (weld (rip 3 (wood q.p.lot)) rep)]
        ==
      --
  =|  rep=tape
  =<  |%
      ::  rendering idioms, output zero-padded to minimum lengths
      ::
      ::  +a-co: decimal
      ::  +c-co: base58check
      ::  +d-co: decimal, takes minimum output digits
      ::  +r-co: floating point
      ::  +s-co: list of '.'-prefixed base16, 4 digit minimum
      ::  +v-co: base32, takes minimum output digits
      ::  +w-co: base64, takes minimum output digits
      ::  +x-co: base16, takes minimum output digits
      ::  +y-co: decimal, 2 digit minimum
      ::  +z-co: '0x'-prefixed base16
      ::
      ++  a-co  |=(dat=@ ((d-co 1) dat))
      ++  c-co  (em-co [58 1] |=([? b=@ c=tape] [~(c ne b) c]))
      ++  d-co  |=(min=@ (em-co [10 min] |=([? b=@ c=tape] [~(d ne b) c])))
      ::
      ++  r-co
        |=  a=dn
        ?:  ?=([%i *] a)  (weld ?:(s.a "inf" "-inf") rep)
        ?:  ?=([%n *] a)  (weld "nan" rep)
        =;  rep  ?:(s.a rep ['-' rep])
        =/  f  ((d-co 1) a.a)
        =^  e  e.a
          =/  e=@s  (sun:si (lent f))
          =/  sci  :(sum:si e.a e -1)
          ?:  (syn:si (dif:si e.a --3))  [--1 sci]  :: 12000 -> 12e3 e>+2
          ?:  !(syn:si (dif:si sci -2))  [--1 sci]  :: 0.001 -> 1e-3 e<-2
          [(sum:si sci --1) --0] :: 1.234e2 -> '.'@3 -> 123 .4
        =?  rep  !=(--0 e.a)
          :(weld ?:((syn:si e.a) "e" "e-") ((d-co 1) (abs:si e.a)))
        (weld (ed-co e f) rep)
      ::
      ++  s-co
        |=  esc=(list @)  ^-  tape
        ?~  esc  rep
        ['.' =>(.(rep $(esc t.esc)) ((x-co 4) i.esc))]
      ::
      ++  v-co  |=(min=@ (em-co [32 min] |=([? b=@ c=tape] [~(v ne b) c])))
      ++  w-co  |=(min=@ (em-co [64 min] |=([? b=@ c=tape] [~(w ne b) c])))
      ++  x-co  |=(min=@ (em-co [16 min] |=([? b=@ c=tape] [~(x ne b) c])))
      ++  y-co  |=(dat=@ ((d-co 2) dat))
      ++  z-co  |=(dat=@ `tape`['0' 'x' ((x-co 1) dat)])
      --
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
  ::  +ed-co: format in numeric base, with output length
  ::
  ::    - like +em-co, but .par's sample will be [| digit output]
  ::      on the first call, regardless of the available digits in .hol
  ::    - used only for @r* floats
  ::
  ++  ed-co
    |=  [exp=@s int=tape]  ^-  tape
    =/  [pos=? dig=@u]  [=(--1 (cmp:si exp --0)) (abs:si exp)]
    ?.  pos
      (into (weld (reap +(dig) '0') int) 1 '.')
    =/  len  (lent int)
    ?:  (lth dig len)  (into int dig '.')
    (weld int (reap (sub dig len) '0'))
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
  ::
  ::  +ro-co: format '.'-prefixed bloqs in numeric base
  ::
  ::    in .bas, for .buz bloqs 0 to .dop, format at least one
  ::    digit of .hol, prefixed with '.'
  ::
  ::    - used only for @i* addresses
  ::
  ++  ro-co
    |=  [[buz=@ bas=@ dop=@] dug=$-(@ @)]
    |=  hol=@
    ^-  tape
    ?:  =(0 dop)
      rep
    :-  '.'
    =/  pod  (dec dop)
    %.  (cut buz [pod 1] hol)
    %+  em-co(rep $(dop pod))
      [bas 1]
    |=([? b=@ c=tape] [(dug b) c])
  --
::
::    4l: atom parsing
+|  %atom-parsing
::
++  so
  ~%  %so  +  ~
  |%
  ++  bisk
    ~+
    ;~  pose
      ;~  pfix  (just '0')
        ;~  pose
          (stag %ub ;~(pfix (just 'b') bay:ag))
          (stag %uc ;~(pfix (just 'c') fim:ag))
          (stag %ui ;~(pfix (just 'i') dim:ag))
          (stag %ux ;~(pfix (just 'x') hex:ag))
          (stag %uv ;~(pfix (just 'v') viz:ag))
          (stag %uw ;~(pfix (just 'w') wiz:ag))
        ==
      ==
      (stag %ud dem:ag)
    ==
  ++  crub
    ~+
    ;~  pose
      (cook |=(det=date `dime`[%da (year det)]) when)
    ::
      %+  cook
        |=  [a=(list [p=?(%d %h %m %s) q=@]) b=(list @)]
        =+  rop=`tarp`[0 0 0 0 b]
        |-  ^-  dime
        ?~  a
          [%dr (yule rop)]
        ?-  p.i.a
          %d  $(a t.a, d.rop (add q.i.a d.rop))
          %h  $(a t.a, h.rop (add q.i.a h.rop))
          %m  $(a t.a, m.rop (add q.i.a m.rop))
          %s  $(a t.a, s.rop (add q.i.a s.rop))
        ==
      ;~  plug
        %+  most
          dot
        ;~  pose
          ;~(pfix (just 'd') (stag %d dim:ag))
          ;~(pfix (just 'h') (stag %h dim:ag))
          ;~(pfix (just 'm') (stag %m dim:ag))
          ;~(pfix (just 's') (stag %s dim:ag))
        ==
        ;~(pose ;~(pfix ;~(plug dot dot) (most dot qix:ab)) (easy ~))
      ==
    ::
      (stag %p fed:ag)
      ;~(pfix dot (stag %ta urs:ab))
      ;~(pfix sig (stag %t urx:ab))
      ;~(pfix hep (stag %c (cook taft urx:ab)))
    ==
  ++  nuck
    ~/  %nuck  |=  a=nail  %.  a
    %+  knee  *coin  |.  ~+
    %-  stew
    ^.  stet  ^.  limo
    :~  :-  ['a' 'z']  (cook |=(a=@ta [%$ %tas a]) sym)
        :-  ['0' '9']  (stag %$ bisk)
        :-  '-'        (stag %$ tash)
        :-  '.'        ;~(pfix dot perd)
        :-  '~'        ;~(pfix sig ;~(pose twid (easy [%$ %n 0])))
    ==
  ++  nusk
    ~+
    :(sear |=(a=@ta (rush a nuck)) wick urt:ab)
  ++  perd
    ~+
    ;~  pose
      (stag %$ zust)
      (stag %many (ifix [cab ;~(plug cab cab)] (more cab nusk)))
    ==
  ++  royl
    ~+
    ;~  pose
      (stag %rh royl-rh)
      (stag %rq royl-rq)
      (stag %rd royl-rd)
      (stag %rs royl-rs)
    ==
  ::
  ++  royl-rh  (cook rylh ;~(pfix ;~(plug sig sig) (cook royl-cell royl-rn)))
  ++  royl-rq  (cook rylq ;~(pfix ;~(plug sig sig sig) (cook royl-cell royl-rn)))
  ++  royl-rd  (cook ryld ;~(pfix sig (cook royl-cell royl-rn)))
  ++  royl-rs  (cook ryls (cook royl-cell royl-rn))
  ::
  ++  royl-rn
    =/  moo
      |=  a=tape
      :-  (lent a)
      (scan a (bass 10 (plus sid:ab)))
    ;~  pose
      ;~  plug
        (easy %d)
        ;~(pose (cold | hep) (easy &))
        ;~  plug  dim:ag
          ;~  pose
            ;~(pfix dot (cook moo (plus (shim '0' '9'))))
            (easy [0 0])
          ==
          ;~  pose
            ;~  pfix
              (just 'e')
              ;~(plug ;~(pose (cold | hep) (easy &)) dim:ag)
            ==
            (easy [& 0])
          ==
        ==
      ==
      ::
      ;~  plug
        (easy %i)
        ;~  sfix
          ;~(pose (cold | hep) (easy &))
          (jest 'inf')
        ==
      ==
      ::
      ;~  plug
        (easy %n)
        (cold ~ (jest 'nan'))
      ==
    ==
  ::
  ++  royl-cell
    |=  rn
    ^-  dn
    ?.  ?=([%d *] +<)  +<
    =+  ^=  h
      (dif:si (new:si f.b i.b) (sun:si d.b))
    [%d a h (add (mul c.b (pow 10 d.b)) e.b)]
  ::
  ++  tash
    ~+
    =+  ^=  neg
        |=  [syn=? mol=dime]  ^-  dime
        ?>  =('u' (end 3 p.mol))
        [(cat 3 's' (rsh 3 p.mol)) (new:si syn q.mol)]
    ;~  pfix  hep
      ;~  pose
        (cook |=(a=dime (neg | a)) bisk)
        ;~(pfix hep (cook |=(a=dime (neg & a)) bisk))
      ==
    ==
  ::
  ++  twid
    ~+
    ;~  pose
      %+  stag  %blob
      %+  sear  |=(a=@ (mole |.((cue a))))
      ;~(pfix (just '0') vum:ag)
    ::
      (stag %$ crub)
    ==
  ::
  ++  when
    ~+
    ;~  plug
      %+  cook
        |=([a=@ b=?] [b a])
      ;~(plug dim:ag ;~(pose (cold | hep) (easy &)))
      ;~(pfix dot mot:ag)   ::  month
      ;~(pfix dot dip:ag)   ::  day
      ;~  pose
        ;~  pfix
          ;~(plug dot dot)
          ;~  plug
            dum:ag
            ;~(pfix dot dum:ag)
            ;~(pfix dot dum:ag)
            ;~(pose ;~(pfix ;~(plug dot dot) (most dot qix:ab)) (easy ~))
          ==
        ==
        (easy [0 0 0 ~])
      ==
    ==
  ::
  ++  zust
    ~+
    ;~  pose
      (stag %is bip:ag)
      (stag %if lip:ag)
      royl
      (stag %f ;~(pose (cold & (just 'y')) (cold | (just 'n'))))
      (stag %q ;~(pfix sig feq:ag))
    ==
  --
::
::    4m: formatting functions
+|  %formatting-functions
++  scot
  ~/  %scot
  |=(mol=dime ~(rent co %$ mol))
++  scow
  ~/  %scow
  |=(mol=dime ~(rend co %$ mol))
++  slat  |=(mod=@tas |=(txt=@ta (slaw mod txt)))
++  slav  |=([mod=@tas txt=@ta] (need (slaw mod txt)))
++  slaw
  ~/  %slaw
  |=  [mod=@tas txt=@ta]
  ^-  (unit @)
  ?+    mod
      ::  slow fallback case to the full slay
      ::
      =+  con=(slay txt)
      ?.(&(?=([~ %$ @ @] con) =(p.p.u.con mod)) ~ [~ q.p.u.con])
  ::
      %da
    (rush txt ;~(pfix sig (cook year when:so)))
  ::
      %p
    (rush txt ;~(pfix sig fed:ag))
  ::
      %ud
    (rush txt dem:ag)
  ::
      %ux
    (rush txt ;~(pfix (jest '0x') hex:ag))
  ::
      %uv
    (rush txt ;~(pfix (jest '0v') viz:ag))
  ::
      %ta
    (rush txt ;~(pfix ;~(plug sig dot) urs:ab))
  ::
      %tas
    (rush txt sym)
  ==
::
++  slay
  |=  txt=@ta  ^-  (unit coin)
  =+  ^=  vex
      ?:  (gth 0x7fff.ffff txt)                         ::  XX  petty cache
        ~+  ((full nuck:so) [[1 1] (trip txt)])
      ((full nuck:so) [[1 1] (trip txt)])
  ?~  q.vex
    ~
  [~ p.u.q.vex]
::
++  smyt                                                ::  pretty print path
  |=  bon=path  ^-  tank
  :+  %rose  [['/' ~] ['/' ~] ~]
  (turn bon |=(a=@ [%leaf (trip a)]))
::
++  spat  |=(pax=path (crip (spud pax)))                ::  render path to cord
++  spud  |=(pax=path ~(ram re (smyt pax)))             ::  render path to tape
++  stab  |=(zep=@t `path`(rash zep stap))              ::  parse cord to path
++  stap                                                ::  path parser
  %+  sear
    |=  p=path
    ^-  (unit path)
    ?:  ?=([~ ~] p)  `~
    ?.  =(~ (rear p))  `p
    ~
  ;~(pfix fas (most fas urs:ab))
::
++  stip                                                ::  typed path parser
  =<  swot
  |%
  ++  swot  |=(n=nail (;~(pfix fas (more fas spot)) n))
  ::
  ++  spot
    %+  sear  (soft iota)
    %-  stew
    ^.  stet  ^.  limo
    :~  :-  'a'^'z'  (stag %tas sym)
        :-  '$'      (cold [%tas %$] buc)
        :-  '0'^'9'  bisk:so
        :-  '-'      tash:so
        :-  '.'      zust:so
        :-  '~'      ;~(pfix sig ;~(pose crub:so (easy [%n ~])))
        :-  '\''     (stag %t qut)
    ==
  --
--
