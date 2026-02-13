~%  %zeke  ..ut  ~
::    math-base: base field definitions and arithmetic
|%
+|  %sur-field
::  $belt: base field element
::
::    An integer in the interval [0, p).
::    Due to a well chosen p, almost all numbers representable with 64 bits
::    are present in the interval.
::
::    In other words, a belt under our choice of p will always fit in 64 bits.
::
+$  belt  @
::
::  $felt: extension field element
::
::    A list of base field elements encoded as a byte array in a single atom.
::    Note that a high bit is set to force allocation of the whole memory region.
::
::    The length is assumed by field math door defined later on,
::    based on degree provided to it.
::
::    If G is a degree 3 extension field over field F, then G's elements
::    are 4-tuples of the form F^4 = (F, F, F, F). E.g., if F = {0, 1},
::    an element of F would be 1, while an example element of G is (0, 1, 1, 0).
::
::    The felt type represents exactly such elements of our extension field G.
::
::    Since the extension field over the base field "happens to be" a polynomial ring,
::    the felt (1, 2, 3) can be thought of as a polynomial (1 + 2x + 3x^2).
::    However, it is recommended by the elders to avoid thinking of felts
::    as polynomials, and to maintain a more abstract conception of them as tuples.
::
+$  felt  @ux
::
::  $melt: Montgomery space element
::
::    `Montgomery space` is obtained from the base field (Z_p, +, •) by replacing ordinary
::    modular multiplication • with Montgomery multiplication *: a*b = abr^{-1} mod p, where
::    r = 2^64. The map a --> r•a is a field isomorphism, so in particular
::    (r•a)*(r•b) = r•(a*b). Note that (r mod p) is the mult. identity in Montgomery space.
::
+$  melt  @
::
::  $bpoly: a polynomial of explicit length with base field coefficients.
::
::    A pair of a length (must fit within 32 bits) and dat, which is
::    a list of base field coefficients, encoded as a byte array.
::    Note that a high bit is set to force allocation of the whole memory region.
::
::    Critically, a bpoly is isomorphic to a felt (at least when its lte is lower than degree).
::
::    In other words, a polynomial defined as a list of base element coefficients
::    is equivalent to a single element of the extension field.
::
::    N.B: Sometimes, bpoly is used to represent a list of belt values
::         of length greater than degree that are not meant to be interpreted
::         as extension field elements (!).
::
::    TODO: would be nice to have a separate typedef for the arb. len. list case
::
+$  bpoly  [len=@ dat=@ux]
::
::  $fpoly: a polynomial of explicit length with *extension field* coefficients.
::
::    A pair of a length (must fit inside 32 bits) and dat, a big atom
::    made up of (D * len) base field coefficients, where D is the extension degree.
::    Note that a high bit is set to force allocation of the whole memory region.
::
::    Put another way, an fpoly is a polynomial whose coefficients are felts
::    (i.e. tuple of belts) instead of numbers (belts).
::
::    N.B: Sometimes, fpoly is used to represent a list of felt values
::         that aren't meant to be interpreted interpreted as polynomials
::         with felt coefficients (!).
::
+$  fpoly  [len=@ dat=@ux]
::
::
::  $poly: list of coefficients [a_0 a_1 a_2 ... ] representing a_0 + a_1*x + a_2*x^2 + ...
::
::    Note any polynomial has an infinite set of list representatives by adding 0's
::    arbitrarily to the end of the list.
::    Note also that ~ represents the zero polynomial.
::
::    Somewhat surprisingly, this type can reprsent both polynomials
::    whose coefficients are belts (aka felts), and polynomials whose
::    coefficients are themselves felts, aka fpolys. This works because felts
::    are encoded as a single atom, the same form factor as belts.
::
+$  poly  (list @)
::
:::  $array
::
::    An array of u64 words stored as an atom. This is exactly the same thing as bpoly and fpoly
::    but is more general and used for any data you want to store in a contiguous array and not
::    only polynomials.
::
+$  array  [len=@ dat=@ux]
::
::  $mary
::
::    An array where each element is step size (in u64 words). This can be used to build
::    multi-dimensional arrays or to store any data you want in one contiguous array.
::
+$  mary  [step=@ =array]
::
:: $mp-mega: multivariate polynomials in their final form
::
::    The multivariate polynomial is stored in a sparse map like in the multi-poly data type.
::    For each monomial term, there is a key and a value. The value is just the belt coefficient.
::    The key is a bpoly which packs in each element of the monomial. It looks like this:
::
::    [term term term ... term]=bpoly
::
::    where each term is one 64-bit direct atom. The format of a term is this:
::
::    3 bits - type of term
::    10 bits - index of term into list of variables / challenges / dynamics
::    30 bits - exponent as @ud
::
::    [TTIIIIIIIIIIEEEEEEEEEEEEEEEEEEEEEEEEEEEEEE]
::
::    This only uses 43 bits which is plenty since the exponent can only be max 4 anyway.
::    So it safely fits inside a direct atom.
::
::    The type of term can be:
::      con - constant (so it's just the zero bpoly and the coefficient is the value)
::      var - variable. the index is the index of the variable.
::      rnd - random challenge from the verifier. the index is the index into the challenge list.
::      dyn - dynamic element so terminal. the index is the index into the dynamic list.
::
::    The reason for this is that the constraints are static and so we would like to build
::    them into an efficient data structure during a preprocess step and not every time we
::    generate a proof. The problem is that we don't know the challenges or the dynamics until
::    we are in the middle of generating a proof. So we store the index of the challenges and
::    dynamics in the data structure and read them out when we evaluate or substitute the polys.
::
+$  mp-mega  (map bpoly belt)
+$  mp-comp  [dep=(list mp-mega) com=(list mp-mega)]
+$  mp-ultra
  $%  [%mega mp-mega]
      [%comp mp-comp]
  ==
::
::  $mp-graph: A multi-poly stored as an expression graph to preserve semantic information.
::
+$  mp-graph
  $~  [%con a=0]
  $%  [%con a=belt]   ::  belt constant
      [%rnd t=term]   ::  random challenge, labeled by name
      [%var col=@]    ::  variable. col is 0-based column index
      [%dyn d=term]   ::  dynamic values for dynamically generated constraint
      [%add a=mp-graph b=mp-graph]
      [%sub a=mp-graph b=mp-graph]
      [%mul a=mp-graph b=mp-graph]
      ::
      ::  note: inv used for field inversion of constant polynomial
      [%inv a=mp-graph]
      [%neg a=mp-graph]
      [%pow a=mp-graph n=@]
      [%scal c=belt a=mp-graph]
      [%mon-belt c=belt v=bpoly]
      [%mon-exp c=mp-graph v=bpoly]
      [%addl l=(list mp-graph)]
      [%rnd-e t=term]
      [%dyn-e d=term]
      [%nil ~]     :: annihilate
  ==
::
+|  %lib-list
+$  elt  @ux
::
++  rip-correct
  ~/  %rip-correct
  |=  [a=bite b=@]
  ^-  (list @)
  ?:  =(0 b)  ~[0]
  (rip a b)
::
++  mary-to-list
  ~/  %mary-to-list
  |=  ma=mary
  ^-  (list elt)
  ?:  =(len.array.ma 0)  ~
  %+  turn  (snip (rip [6 step.ma] dat.array.ma))
  |=  elem=@
  ^-  elt
  %+  add  elem
  ?:(=(step.ma 1) 0 (lsh [6 step.ma] 1))
::
++  zing-bpolys
  ~/  %zing-bpolys
  |=  l=(list bpoly)
  ^-  mary
  ?~  l  !!  :: can't return zero-mary because we can't figure out the step from ~
  %+  do-init-mary  len:(head l)
  (turn l |=(=bpoly dat.bpoly))
::
++  zing-fpolys
  ~/  %zing-fpolys
  |=  l=(list fpoly)
  ^-  mary
  ?~  l  !!  :: can't return zero-mary because we can't figure out the step from ~
  %+  do-init-mary  (mul 3 len:(head l))
  (turn l |=(=fpoly dat.fpoly))
::
++  zing-marys
  ~/  %zing-marys
  |=  l=(list mary)
  ^-  mary
  ?~  l  !!  :: can't return zero-mary because we can't figure out the step from ~
  %+  do-init-mary  len.array:(head l)
  (turn l |=(=mary dat.array.mary))
::
::  +met-elt measure the size of the elements in a mary
::
::  This is used to compute what the step should be when turning a list into a mary.
::  If elt is larger than a belt, then (met 6 elt) will include the word for the high bit.
::  We don't want this and so subtract it off. But if elt is a belt then it has no high bit.
::  (met 6 elt) will return 1 and (dec (met 6 elt)) would erroneously return 0. So this is special
::  cased by setting the max to be 2. This way belts return the correct size of 1.
++  met-elt
  |=  =elt
  ^-  @
  (dec (max 2 (met 6 elt)))
::
++  init-mary
  ~/  %init-mary
  |=  poly=(list elt)
  ^-  mary
  ?~  poly  !!  :: can't return zero-mary because we can't figure out the step from ~
  (do-init-mary (met-elt (head poly)) poly)
::
++  do-init-mary
  ~/  %do-init-mary
  |=  [step=@ poly=(list elt)]
  ^-  mary
  ?:  =(~ poly)
    ~(zero-mary mary-utils step)
  ?>  (lth (lent poly) (bex 32))
  ?>  (levy poly |=(=elt &((~(fet mary-utils step) elt) =(step (met-elt elt)))))
  :-  step
  :-  (lent poly)
  =/  high-bit  (lsh [0 (mul (bex 6) (mul step (lent poly)))] 1)
  (add (rep [6 step] poly) high-bit)
::
++  mary-utils
  ~/  %mary-utils
  |_  step=@
  ++  fet
    ~/  %fet
    |=  a=@
    ^-  ?
    ~+
    =/  v  (rip-correct 6 a)
    ?&  |(&(=((lent v) 1) =(step 1)) =((lent v) +(step)))
        (levy v based)
    ==
  ::
  ++  lift-elt
    ~/  %lift-elt
    |=  a=@
    ^-  elt
    ?:(=(step 1) `@ux`a dat:(init-bpoly [a (reap (dec step) 0)]))
  ::
  ++  zero-mary
    ~+
    ^-  mary
    ?:  =(step 1)  [1 1 `@ux`0]
    (init-mary ~[(lift-elt 0)])
  --
::
++  ave
  ~/  %ave
  =|  ma=mary
  |%
    ++  flop
    ^-  mary
    =/  p  to-poly
    (do-init-mary step.ma (^flop p))
  ::
  ++  zip
    ~/  %zip
    |=  [na=mary a=$-([elt elt] elt)]
    ^-  mary
    ?>  =(len.array.ma len.array.na)
    :-  step.ma
    :-  len.array.ma
    =/  i  0
    |-
    ?:  =(i len.array.ma)
      dat.array.ma
    =/  mi  (snag i)
    =/  ni  (~(snag ave na) i)
    $(i +(i), ma (stow i (a mi ni)))
  ::
  ++  zero-extend
    ~/  %zero-extend
    |=  n=@
    ^-  mary
    :-  step.ma
    :-  (add len.array.ma n)
    =/  i  0
    =/  dat  dat.array.ma
    |-
    ?:  =(i n)
      dat
    %_  $
      dat  dat.array:(~(snoc ave [step.ma (add len.array.ma i) dat]) (~(lift-elt mary-utils step.ma) 0))
      i    +(i)
    ==
  ::
  ++  weld
    ~/  %weld
    |=  ma2=mary
    ^-  mary
    :-  step.ma
    :-  (add len.array.ma len.array.ma2)
    =/  i  0
    =/  dat  dat.array.ma
    |-
    ?:  =(i len.array.ma2)
      dat
    %_  $
      dat  dat.array:(~(snoc ave [step.ma (add len.array.ma i) dat]) (~(snag ave ma2) i))
      i    +(i)
    ==
  ::
  ++  head
    ^-  elt
    (snag 0)
  ::
  ++  rear
    ^-  elt
    (snag (dec len.array.ma))
  ::
  ++  snag-as-mary
    ~/  %snag-as-mary
    |=  i=@
    ^-  mary
    [step.ma 1 (snag i)]
  ::
  ++  snag-as-fpoly
    ~/  %snag-as-fpoly
    |=  i=@
    ^-  fpoly
    [(div step.ma 3) (snag i)]
  ::
  ++  snag-as-bpoly
    ~/  %snag-as-bpoly
    |=  i=@
    ^-  bpoly
    :-  step.ma
    =/  dat  (snag i)
    ?:  =(step.ma 1)
      =/  high-bit  (lsh [0 (mul (bex 6) step.ma)] 1)
      (add high-bit dat)
    dat
  ::
  ++  snag
    ~/  %snag
    |=  i=@
    ^-  elt
    ?>  (lth i len.array.ma)
    =/  res  (cut 6 [(mul i step.ma) step.ma] dat.array.ma)
    ?:  =(step.ma 1)  res
    =/  high-bit  (lsh [0 (mul (bex 6) step.ma)] 1)
    (add high-bit res)
  ::
  ++  scag
    ~/  %scag
    |=  i=@
    ^-  mary
    ?:  =(i 0)
      ~(zero-mary mary-utils step.ma)
    ?:  (gte i len.array.ma)  ma
    :-  step.ma
    :-  i
    =/  high-bit  (lsh [0 (mul i (mul (bex 6) step.ma))] 1)
    %+  add  high-bit
    (cut 6 [0 (mul i step.ma)] dat.array.ma)
  ::
  ++  slag
    ~/  %slag
    |=  i=@
    ^-  mary
    ?:  (gte i len.array.ma)
      ~(zero-mary mary-utils step.ma)
    [step.ma (sub len.array.ma i) (rsh [6 (mul i step.ma)] dat.array.ma)]
  ::
  ++  swag
    ~/  %swag
    |=  [i=@ j=@]
    ^-  mary
    (~(scag ave (slag i)) j)
  ::
  ++  snoc
    ~/  %snoc
    |=  j=elt
    ^-  mary
    ::  fix bunt
    ::
    ::  *bpoly should be [1 0x1] not [1 0x0] so it has a high bit with no data. We can't
    ::  change the bunt right now, so this is a workaround just for snoc. Without it,
    ::  snoccing onto *bpoly fails.
    =.  ma
      ?.  =(ma [1 *bpoly])  ma
      [1 0 0x1]
    ?>  (~(fet mary-utils step.ma) j)
    :-  step.ma
    :-  +(len.array.ma)
    =/  new-high-bit  (lsh [0 (mul (bex 6) (mul step.ma +(len.array.ma)))] 1)
    =/  old-high-bit  (lsh [0 (mul (bex 6) (mul step.ma len.array.ma))] 1)
    %^  sew  6
      [(mul len.array.ma step.ma) step.ma j]
    (sub (add new-high-bit dat.array.ma) old-high-bit)
  ::
  ++  weld-step
    ~/  %weld-step
    |=  na=mary
    ^-  mary
    ?>  =(len.array.ma len.array.na)
    %+  roll  (range len.array.na)
    =/  mu=mary
      :+  (add step.ma step.na)
        len.array.ma
      (lsh [6 (mul (add step.ma step.na) len.array.ma)] 1)
    |=  [i=@ mu=_mu]
    =;  weld-dat
      (~(stow ave mu) i weld-dat)
    =/  r1  (snag i)
    =?  r1  !=(step.ma 1)
      (sub r1 (lsh [6 step.ma] 1))
    =/  r2  (~(snag ave na) i)
    =?  r2  !=(step.na 1)
      (add (lsh [6 step.na] 1) r2)
    (add (lsh [6 step.ma] r2) r1)
  ::
  ++  stow
    ~/  %stow
    |=  [i=@ j=elt]
    ^-  mary
    ?>  (~(fet mary-utils step.ma) j)
    ?>  (lth i len.array.ma)
    =/  item
      ?:  =(step.ma 1)
        j
      (rep 6 (snip (rip 6 j)))
    [step.ma len.array.ma (sew 6 [(mul i step.ma) step.ma item] dat.array.ma)]
  ::
  ++  change-step
    ~/  %change-step
    |=  [new-step=@]
    ^-  mary
    ?:  =(step.ma new-step)  ma
    ?>  =((mod (mul step.ma len.array.ma) new-step) 0)
    :+  new-step
      (div (mul step.ma len.array.ma) new-step)
    dat.array.ma
  ::
  ++  to-poly
    ^-  poly
    (mary-to-list ma)
  ::
  ++  transpose
    ~/  %transpose
    |=  offset=@
    ^-  mary
    =/  res-step  (mul len.array.ma offset)
    =/  res-len  (div step.ma offset)
    =/  res=mary  [res-step [res-len (lsh [6 (mul res-step res-len)] 1)]]
    =/  num-cols  res-len
    =/  num-rows  len.array.ma
    %+  roll  (range num-cols)
    |=  [i=@ m=_res]
    %+  roll  (range num-rows)
    |=  [j=@ m=_m]
    =/  target-index  (add (mul i num-rows) j)
    ::
    =/  source  (cut 6 [(mul offset (add (mul j num-cols) i)) offset] dat.array.ma)
    m(dat.array (sew 6 [(mul offset target-index) offset source] dat.array.m))
  ::
  ::  check if len matches the actual size of dat
  ++  chck
    ^-  ?
    =((mul step.ma len.array.ma) (dec (met 6 dat.array.ma)))
  --  :: ave
::
++  transpose-fpolys
  ~/  %transpose-fpolys
  |=  fpolys=mary
  ^-  mary
  (~(transpose ave fpolys) 3)
::
++  transpose-bpolys
  ~/  %transpose-bpolys
  |=  bpolys=mary
  ^-  mary
  (~(transpose ave bpolys) 1)
::
++  array-op
  ~/  %array-op
  |_  step=@
  ::
  ++  ops
    =|  fp=array
    ~/  %ops
    |%
    ++  op  ~(. ave [step fp])
    ++  flop
      ^-  fpoly
      array:flop:op
    ++  zip
      ~/  %zip
      |=  [gp=fpoly a=$-([felt felt] felt)]
      ^-  fpoly
      array:(zip:op [step gp] a)
    ::
    ++  zero-extend
      ~/  %zero-extend
      |=  n=@
      array:(zero-extend:op n)
    ::
    ++  weld
      ~/  %weld
      |=  fp2=fpoly
      ^-  fpoly
      array:(weld:op [step fp2])
    ::
    ++  head
      ^-  felt
      (snag 0)
    ::
    ++  rear
      ^-  felt
      (snag (dec len.fp))
    ::
    ++  snag
      ~/  %snag
      |=  i=@
      ^-  felt
      (snag:op i)
    ::
    ++  scag
      ~/  %scag
      |=  i=@
      ^-  fpoly
      array:(scag:op i)
    ::
    ++  slag
      ~/  %slag
      |=  i=@
      ^-  fpoly
      array:(slag:op i)
    ::
    ++  swag
      ~/  %swag
      |=  [i=@ j=@]
      ^-  fpoly
      array:(swag:op i j)
    ::
    ++  snoc
      ~/  %snoc
      |=  j=felt
      ^-  fpoly
      array:(snoc:op j)
    ::
    ++  stow
      ~/  %stow
      |=  [i=@ j=felt]
      ^-  fpoly
      array:(stow:op i j)
    ::
    ++  to-poly
      ^-  poly
      (mary-to-list [step fp])
    ::
    ++  chck
      ^-  ?
      chck:op
    --
  --
::
::  turn a 1-dimensional array into a 1-element 2-dimensional array
++  lift-mop
  |=  fp=fpoly
  ^-  mary
  [(mul 3 len.fp) 1 dat.fp]
::
::  +fpoly-to-mary
::
::  View an fpoly as a 3-step mary.
::
::  Note that +fpoly-to-marry just views the fpoly as an mary. It has the same length
::  and each element is a felt. +lift-mop on the other hand turns the fpoly into an array
::  of fpolys which has step=len.fpoly and contains exactly one element- the fpoly passed in.
::
++  fpoly-to-mary
  |=  fp=fpoly
  ^-  mary
  [3 fp]
::
::  +bpoly-to-mary: view a bpoly as 1-step mary
++  bpoly-to-mary
  |=  bp=bpoly
  ^-  mary
  [1 bp]
::
::  +i: reverse index lookup:
::
::    given an item and a list,
::    produce the index of the item in the list
++  i
  ~+
  |=  [item=@tas lis=(list @tas)]
  ::TODO make this wet
  ^-  @
  (need (find ~[item] lis))
::
++  zip-up
  ~/  %zip-up
  |*  [p=(list) q=(list)]
  ^-  (list _?>(?&(?=(^ p) ?=(^ q)) [i.p i.q]))
  (zip p q same)
::
++  zip
  ~/  %zip
  |*  [p=(list) q=(list) r=gate]
  ^-  (list _?>(?&(?=(^ p) ?=(^ q)) (r i.p i.q)))
  |-
  ?:  ?&(?=(~ p) ?=(~ q))
    ~
  ?.  ?&(?=(^ p) ?=(^ q))  ~|(%zip-fail-unequal-lengths !!)
  [i=(r i.p i.q) t=$(p t.p, q t.q)]
::
++  sum
  ~/  %sum
  |=  lis=(list @)
  ^-  @
  %+  roll  lis
  |=  [a=@ r=@]
  (add a r)
::
++  mul-all
  ~/  %mul-all
  |=  [lis=(list @) x=@]
  ^-  (list @)
  %+  turn  lis
  |=(a=@ (mul a x))
::
++  add-all
  ~/  %add-all
  |=  [lis=(list @) x=@]
  ^-  (list @)
  %+  turn  lis
  |=(a=@ (add a x))
::
++  mod-all
  ~/  %mod-all
  |=  [lis=(list @) x=@]
  ^-  (list @)
  %+  turn  lis
  |=(a=@ (mod a x))
::
++  zip-roll
  ~/  %zip-roll
  |*  [p=(list) q=(list) r=_=>(~ |=([[* *] *] +<+))]
  |-  ^+  ,.+<+.r
  ?~  p
    ?~  q
      +<+.r
    !!
  ?.  ?=(^ q)  ~|(%zip-roll-fail-unequal-lengths !!)
  $(p t.p, q t.q, r r(+<+ (r [i.p i.q] +<+.r)))
::
++  range
  ~/  %range
  |=  $@(@ ?(@ (pair @ @)))
  ^-  (list @)
  ?@  +<  ?~(+< ~ (gulf 0 (dec +<)))
  (gulf p (dec q))
::
::  +mevy: maybe error levy
::
++  mevy
  ::    ~ if no failures or (some err) of the first error encountered
  ~/  %mevy
  |*  [a=(list) b=$-(* (unit))]
  =*  b-product  _?>(?=(^ a) (b i.a))
  |-  ^-  b-product
  ?~  a  ~
  ?^  err=(b i.a)  err
  $(a t.a)
::
:: +iturn: indexed turn. Gate gets a 0-based index of which list element it is on.
++  iturn
  ~/  %iturn
  |*  [a=(list) b=_|=([@ *] +<+)]
  ^-  (list _?>(?=(^ a) (b i.a)))
  =/  n  0
  |-
  ?~  a  ~
  [i=(b [n i.a]) t=$(a t.a, n +(n))]
::
::  +median: computes the median value of a (list @)
::
::    if the length of the list is odd, its the middle value. if its
::    even, we average the two middle values (rounding down)
++  median
  ~/  %median
  |=  xs=(list @)
  ^-  @
  =/  len=@            (lent xs)
  =/  parity=@         (mod (lent xs) 2)
  =/  sorted=(list @)  (sort xs lth)
  |-
  ?~  sorted  ~|("median of empty list" !!)
  ::TODO why do i need to cast to (list @) everywhere?
  ?:  ?&  =(parity 0)         :: even case
          =(len 2)            :: 2 elements remain
      ==
    %-  div
    :_  2
    %+  add  (snag 0 `(list @)`sorted)
    (snag 1 `(list @)`sorted)
  ?:  ?&  =(parity 1)         :: odd case
          =(len 1)            :: 1 element remains
      ==
    (snag 0 `(list @)`sorted)
  ::  otherwise, remove first and last element and repeat
  %_  $
    sorted  (snip (slag 1 `(list @)`sorted))
    len     (dec (dec len))
  ==
::
::  clev: cleave a list into a list of lists w specified sizes (unless list is exhausted)
::  TODO: could be made wet wrt a
++  clev
  |=  [a=(list @) sizes=(list @)]
  ^-  (list (list @))
  =-  (flop acc)
  %+  roll  sizes
  |=  [s=@ a=_a acc=(list (list @))]
  :-  (slag s a)
  [(scag s a) acc]
::
::
+|  %base58
++  en-base58
  ~/  %en-base58
  |=  dat=@
  =/  cha
    '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz'
  %-  flop
  |-  ^-  tape
  ?:  =(0 dat)  ~
  :-  (cut 3 [(mod dat 58) 1] cha)
  $(dat (div dat 58))
::
++  de-base58
  ~/  %de-base58
  |=  t=tape
  =-  (scan t (bass 58 (plus -)))
  ;~  pose
    (cook |=(a=@ (sub a 56)) (shim 'A' 'H'))
    (cook |=(a=@ (sub a 57)) (shim 'J' 'N'))
    (cook |=(a=@ (sub a 58)) (shim 'P' 'Z'))
    (cook |=(a=@ (sub a 64)) (shim 'a' 'k'))
    (cook |=(a=@ (sub a 65)) (shim 'm' 'z'))
    (cook |=(a=@ (sub a 49)) (shim '1' '9'))
  ==
::
+|  %constants
::  +p: field characteristic p = 2^64 - 2^32 + 1 = (2^32)*3*5*17*257*65537 + 1
::  +r: radix r = 2^64
::  +r-mod-p: r-mod-p = r - p
::  +r2: r^2 mod p = (2^32 - 1)^2 = 2^64 - 2*2^32 + 1 = p - 2^32
::  +rp: r*p
::  +g: ord(g) = p - 1, i.e. g generates the (multiplicative group of the) field
::  +h: ord(h) = 2^32, i.e. h = (2^32)th root of unity
++  p  0xffff.ffff.0000.0001
++  r  0x1.0000.0000.0000.0000
++  r-mod-p  4.294.967.295
++  r2  0xffff.fffe.0000.0001
++  rp  0xffff.ffff.0000.0001.0000.0000.0000.0000
++  g  7
++  h  20.033.703.337
+|  %base-field
::  +based: in base field?
++  based
  ~/  %based
  |=  a=@
  ^-  ?
  (lth a p)
::
::  is this noun based
++  based-noun
  ~/  %based-noun
  |=  n=*
  ^-  ?
  ?@  n
    (based n)
  &($(n -.n) $(n +.n))
::
::  +badd: base field addition
++  badd
  ~/  %badd
  |=  [a=belt b=belt]
  ^-  belt
  ?>  ?&((based a) (based b))
  (mod (add a b) p)
::
::  +bneg: base field negation
++  bneg
  ~/  %bneg
  |=  a=belt
  ^-  belt
  ?>  (based a)
  ?:  =(a 0)
    0
  (sub p a)
::
::  +bsub: base field subtraction
++  bsub
  ~/  %bsub
  |=  [a=belt b=belt]
  ^-  belt
  (badd a (bneg b))
::
::  +bmul: base field multiplication
++  bmul
  ~/  %bmul
  |=  [a=belt b=belt]
  ^-  belt
  ?>  ?&((based a) (based b))
  (mod (mul a b) p)
::
::  +bmix: binary XOR mod p
++  bmix
  ~/  %bmix
  |=  [a=@ b=@]
  ^-  @
  (mod (mix a b) p)
::
::  +mont-reduction: special algorithm; computes x•r^{-1} = (xr^{-1} mod p).
::
::    Note this is the inverse of x --> r•x, so conceptually this is a map from
::    Montgomery space to the base field.
::
::    It's `special` bc we gain efficiency by examining the general algo by hand
::    and deducing the form of the final answer, which we can code directly.
::    If you compute the algo by hand you will find it convenient to write
::    x = x_2*2^64 + x_1*2^32 + x_0 where x_2 is a 64-bit number less than
::    p (so x < pr) and x_1, x_0 are 32-bit numbers.
::    The formula comes, basically, to (x_2 - (2^32(x_0 + x_1) - x_1 - f*p));
::    f is a flag bit for the overflow of 2^32(x_0 + x_1) past 64 bits.
::    "Basically" means we have to add p if the formula is negative.
++  mont-reduction
  ~/  %mont-reduction
  |=  x=melt
  ^-  belt
  ?>  (lth x rp)
  =/  x1  (cut 5 [1 1] x)
  =/  x2  (rsh 6 x)
  =/  c
    =/  x0  (end 5 x)
    (lsh 5 (add x0 x1))
  =/  f   (rsh 6 c)
  =/  d   (sub c (add x1 (mul f p)))
  ?:  (gte x2 d)
    (sub x2 d)
  (sub (add x2 p) d)
::
::  +montiply: computes a*b = (abr^{-1} mod p); note mul, not fmul: avoids mod p reduction!
++  montiply
  ~/  %montiply
  |:  [a=`melt`r-mod-p b=`melt`r-mod-p]
  ^-  belt
  ~+
  ?>  ?&((based a) (based b))
  (mont-reduction (mul a b))
::
::  +montify: transform to Montgomery space, i.e. compute x•r = xr mod p
++  montify
  ~/  %montify
  |=  x=belt
  ^-  melt
  ~+
  (montiply x r2)
::
::  +bpow: fast modular exponentiation using x^n mod p = 1*(xr)*...*(xr)
++  bpow
  ~/  %bpow
  |=  [x=belt n=@]
  ^-  belt
  ?>  (based x)
  ~+
  %.  [1 (montify x) n]
  |=  [y=melt x=melt n=@]
  ^-  melt
  ?:  =(n 0)
    y
  ::  parity flag
  =/  f=@  (end 0 n)
  ?:  =(0 f)
    $(x (montiply x x), n (rsh 0 n))
  $(y (montiply y x), x (montiply x x), n (rsh 0 n))
::
::  +binv: base field multiplicative inversion
++  binv
  ~/  %binv
  |=  x=belt
  ^-  belt
  ~+
  |^
  ~|  "x not in field"
  ?>  (based x)
  ~|  "cannot divide by 0"
  ?<  =(x 0)
  =/  y=melt  (montify x)
  =/  y2    (montiply y (montiply y y))
  =/  y3    (montiply y (montiply y2 y2))
  =/  y5    (montiply y2 (montwop y3 2))
  =/  y10   (montiply y5 (montwop y5 5))
  =/  y20   (montiply y10 (montwop y10 10))
  =/  y30   (montiply y10 (montwop y20 10))
  =/  y31   (montiply y (montiply y30 y30))
  %-  mont-reduction
  %+  montiply  y
  =+  (montiply (montwop y31 32) y31)
  (montiply - -)
  ++  montwop
    |=  [a=melt n=@]
    ^-  melt
    ~+
    ?>  (based a)
    ?:  =(n 0)
      a
    $(a (montiply a a), n (sub n 1))
  --
::
::  +bdiv: base field division
++  bdiv
  ~/  %bdiv
  |=  [a=belt b=belt]
  ^-  belt
  (bmul a (binv b))
::
++  ordered-root
  ~/  %ordered-root
  |=  n=@
  ^-  @
  ~+
  |^
  ?>  (lte n (bex 32))
  ?>  =((dis n (dec n)) 0)  :: assert it's a power of two
  =/  log-of-n  (dec (xeb n))
  ?>  (lth log-of-n (lent roots))
  (snag log-of-n roots)
  ::
  ::  precomputed roots matching the ROOTS array in belt.rs
  ++  roots
    ^-  (list @)
    :~  0x1  0xffff.ffff.0000.0000  0x1.0000.0000.0000  0xffff.fffe.ff00.0001
        0xefff.ffff.0000.0001  0x3fff.ffff.c000  0x80.0000.0000  0xf800.07ff.0800.0001
        0xbf79.143c.e60c.a966  0x1905.d02a.5c41.1f4e  0x9d8f.2ad7.8bfe.d972  0x653.b480.1da1.c8cf
        0xf2c3.5199.959d.fcb6  0x1544.ef23.35d1.7997  0xe0ee.0993.10bb.a1e2  0xf6b2.cffe.2306.baac
        0x54df.9630.bf79.450e  0xabd0.a6e8.aa3d.8a0e  0x8128.1a7b.05f9.beac  0xfbd4.1c6b.8caa.3302
        0x30ba.2ecd.5e93.e76d  0xf502.aef5.3232.2654  0x4b2a.18ad.e672.46b5  0xea9d.5a13.36fb.c98b
        0x86cd.cc31.c307.e171  0x4bba.f597.6ecf.efd8  0xed41.d05b.78d6.e286  0x10d7.8dd8.915a.171d
        0x5904.9500.004a.4485  0xdfa8.c93b.a46d.2666  0x7e9b.d009.b86a.0845  0x400a.7f75.5588.e659
        0x1856.29dc.da58.878c
    ==
  --
::
::  +compute-size: computes the size in bits of a jammed noun
::
++  compute-size-jam
  |=  n=*
  ^-  @
  (met 0 (jam n))
::
::  +compute-size-noun: computes the size in bits of a noun in unrolled form in the memory arena
::
++  compute-size-noun
  |=  n=*
  ^-  @
  |-
  ?@  n
    ?>  (based n)  :: check that atom is a belt
    64
  (add 64 (add $(n -.n) $(n +.n)))
::
+|  %bpoly
::  +bcan: gives the canonical leading-zero-stripped representation of p(x)
++  bcan
  |=  p=poly
  ^-  poly
  =.  p  (flop p)
  |-
  ?~  p
    ::  TODO: fix this
    ~[0]
  ?:  =(i.p 0)
    $(p t.p)
  (flop p)
::
::
::  +bdegree: computes the degree of a polynomial
::
::    Not just (dec (lent p)) because we need to discard possible extraneous "leading zeroes"!
::    Be very careful in using lent vs. degree!
::    NOTE: degree(~) = 0 when it should really be -∞ to preserve degree(fg) = degree(f) +
::    degree(g). So if we use the RHS of this equation to compute the LHS the cases where
::    either are the zero polynomial must be handled separately.
++  bdegree
  |=  p=poly
  ^-  @
  =/  cp=poly  (bcan p)
  ?~  cp  0
  (dec (lent cp))
::
::  +bzero-extend: make the zero coefficients for powers of x higher than deg(p) explicit
++  bzero-extend
  |=  [p=poly much=@]
  ^-  poly
  (weld p (reap much 0))
::
::  +binary-zero-extend: extend with zeroes until the length is the next power of 2
++  bbinary-zero-extend
  |=  [p=poly]
  ^-  poly
  ?~  p  ~
  =/  l=@  (lent p)
  ?:  =((dis l (dec l)) 0)
    p
  (bzero-extend p (sub (bex (xeb l)) l))
::
::  +poly-to-map: takes list (a_i) and makes map i --> a_i
++  poly-to-map
  |=  p=poly
  ^-  (map @ felt)
  =|  mp=(map @ felt)
  =/  i=@  0
  |-
  ?~  p
    mp
  $(mp (~(put by mp) i i.p), p t.p, i +(i))
::
::  +map-to-poly: inverse of poly-to-map
++  map-to-poly
  ::  keys need to be 0, 1, 2, ... which is enforced by "got" below
  |=  mp=(map @ felt)
  ^-  poly
  =|  p=poly
  =/  size=@  ~(wyt by mp)
  =/  i=@  size
  |-
  ?:  =(i 0)
    p
  $(p [(~(got by mp) (dec i)) p], i (dec i))
::
++  bop  ~(ops array-op 1)
::
::  +init-bpoly: given a list of belts, create a bpoly representing it
++  init-bpoly
  ~/  %init-bpoly
  |=  poly=(list belt)
  ^-  bpoly
  ?:  =(~ poly)
    zero-bpoly
  ?>  (lth (lent poly) (bex 32))
  :-  (lent poly)
  =/  high-bit  (lsh [0 (mul (bex 6) (lent poly))] 1)
  (add (rep 6 poly) high-bit)
::
++  bpoly-to-list
  ~/  %bpoly-to-list
  |=  bp=bpoly
  ^-  poly
  ?>  !=(len.bp 0)
  (snip (rip 6 dat.bp))
::
::
++  zero-bpoly  ~+((init-bpoly ~[0]))
++  one-bpoly   ~+((init-bpoly ~[1]))
++  id-bpoly
  ~+
  ^-  bpoly
  (init-bpoly ~[0 1])
::
++  bpcan
  |=  bp=bpoly
  ^-  bpoly
  =/  p  ~(to-poly bop bp)
  (init-bpoly (bcan p))
::
::
++  bp-is-zero
  ~/  %bp-is-zero
  |=  p=bpoly
  ^-  ?
  ~+
  =.  p  (bpcan p)
  |(=(len.p 0) =(p zero-bpoly))
::
::
++  bp-is-one
  ~/  %bp-is-one
  |=  p=bpoly
  ^-  ?
  ~+
  =.  p  (bpcan p)
  &(=(len.p 1) =((~(snag bop p) 0) 1))
::
::  +bpadd: base field polynomial addition
++  bpadd
  ~/  %bpadd
  |:  [bp=`bpoly`zero-bpoly bq=`bpoly`zero-bpoly]
  ^-  bpoly
  ?>  &(!=(len.bp 0) !=(len.bq 0))
  =/  p  (bpoly-to-list bp)
  =/  q  (bpoly-to-list bq)
  =/  lp  (lent p)
  =/  lq  (lent q)
  =/  m  (max lp lq)
  =:  p  (weld p (reap (sub m lp) 0))
      q  (weld q (reap (sub m lq) 0))
    ==
  %-  init-bpoly
  (zip p q badd)
::
::  +bpneg: additive inverse of a base field polynomial
++  bpneg
  ~/  %bpneg
  |=  bp=bpoly
  ^-  bpoly
  ?>  !=(len.bp 0)
  =/  p  (bpoly-to-list bp)
  %-  init-bpoly
  (turn p bneg)
::
::  +bpsub:  field polynomial subtraction
++  bpsub
  ~/  %bpsub
  |=  [p=bpoly q=bpoly]
  ^-  bpoly
  (bpadd p (bpneg q))
::
::  bpscal:  multiply base field scalar c by base field polynomial p
++  bpscal
  ~/  %bpscal
  |=  [c=belt bp=bpoly]
  ^-  bpoly
  =/  p  (bpoly-to-list bp)
  %-  init-bpoly
  %+  turn  p
  (cury bmul c)
::
::  +bpmul: base field polynomial multiplication; naive algorithm; necessary for fmul!
++  bpmul
  ~/  %bpmul
  |:  [bp=`bpoly`one-bpoly bq=`bpoly`one-bpoly]
  ^-  bpoly
  ?>  &(!=(len.bp 0) !=(len.bq 0))
  %-  init-bpoly
  ?:  ?|(=(bp zero-bpoly) =(bq zero-bpoly))
    ~[0]
  =/  p  (bpoly-to-list bp)
  =/  q  (bpoly-to-list bq)
  =/  v=(list melt)
    %-  weld
    :_  (turn p montify)
    (reap (dec (lent q)) 0)
  =/  w=(list melt)  (flop (turn q montify))
  =|  prod=poly
  |-
  ?~  v
    (flop prod)
  %=  $
    v  t.v
    ::
      prod
    :_  prod
    %.  [v w]
    ::  computes a "dot product" (actually a bilinear form that just looks like
    ::  one) of v and w by implicitly zero-extending if lengths unequal we
    ::  don't actually zero-extend to save a constant time factor
    |=  [v=(list melt) w=(list melt)]
    ^-  melt
    =|  dot=belt
    |-
    ?:  ?|(?=(~ v) ?=(~ w))
      (mont-reduction dot)
    $(v t.v, w t.w, dot (badd dot (montiply i.v i.w)))
  ==
::
::  +bp-hadamard
::
::  Hadamard product of two bpolys. This is just a fancy name for pointwise multiplication.
++  bp-hadamard
  ~/  %bp-hadamard
  |:  [bp=`bpoly`one-bpoly bq=`bpoly`one-bpoly]
  ^-  bpoly
  ?>  =(len.bp len.bq)
  ?:  |(=(len.bp 0) =(len.bq 0))  zero-bpoly
  (~(zip bop bp) bq bmul)
::
::
::  +bpdvr: base field polynomial division with remainder
::
::    Analogous to integer division: (bpdvr a b) = [q r] where a = bq + r and degree(r)
::    < degree(b). (Using the mathematical degree where degree(~) = -∞.)
::    This implies q and r are unique.
::
::    Algorithm is the usual one taught in high school.
++  bpdvr
  ~/  %bpdvr
  |:  [ba=`bpoly`one-bpoly bb=`bpoly`one-bpoly]
  ^-  [q=bpoly r=bpoly]
  ?>  &(!=(len.ba 0) !=(len.bb 0))
  =/  a  (bpoly-to-list ba)
  =/  b  (bpoly-to-list bb)
  ::  rem = remainder; a is effectively first candidate since (degree a) < (degree b) => done
  ::  rem, b are written high powers to low, as in high school algorithm
  =^  rem  b
    :-  (flop (bcan a))
    (flop (bcan b))
  ~|  "Cannot divide by the zero polynomial."
  ?<  ?=(~ b)
  =/  db  (dec (lent b))
  ::  db = 0, rem = ~ => condition below this one is false and we fail the subsequent assertion;
  ::  Problem is (degree ~) = 0 is wrong mathematically; so we simply handle db = 0 separately.
  ?:  =(db 0)
    :_  zero-bpoly
    (bpscal (binv i.b) ba)
  ::  coeff = next coefficient added to the quotient, starting with highest power
  =|  coeff=belt
  =|  quot=poly
  |-
  ?:  (lth (bdegree (flop rem)) db)
    :-  (init-bpoly quot)
    (init-bpoly (bcan (flop rem)))
  ?<  ?=(~ rem)
  =/  new-coeff  (bdiv i.rem i.b)
  =/  new-rem
    %-  bpoly-to-list
    (bpsub (init-bpoly rem) (bpscal new-coeff (init-bpoly b)))
  ?<  ?=(~ new-rem)
  %=  $
    coeff  new-coeff
    quot  [new-coeff quot]
    rem   t.new-rem
  ==
::
::  +bpdiv: a/b for base field polynomials; q component of bpdvr
++  bpdiv
  ~/  %bpdiv
  |=  [a=bpoly b=bpoly]
  ^-  bpoly
  q:(bpdvr a b)
::
::  +bppow::  bppow: compute (p(x))^k
++  bppow
  ~/  %bppow
  |=  [bp=bpoly k=@]
  ^-  bpoly
  ~+
  ?>  !=(len.bp 0)
  %.  [(init-bpoly ~[1]) bp k]
  |=  [q=bpoly p=bpoly k=@]
  ?:  =(k 0)
    q
  =/  f=@  (end 0 k)
  ?:  =(f 0)
    %_  $
      p  (bpmul p p)
      k  (rsh 0 k)
    ==
  %_  $
    q  (bpmul q p)
    p  (bpmul p p)
    k  (rsh 0 k)
  ==
::
::
::  +bpmod: a mod b for base field polynomials; r component of bpdvr
++  bpmod
  ~/  %bpmod
  |=  [a=bpoly b=bpoly]
  ^-  bpoly
  r:(bpdvr a b)
::
::  +bpegcd: base field polynomial extended Euclidean algorithm
::
::    Gives gcd = d and u, v such that d = ua + vb from the Euclidean algorithm.
::    The algorithm is based on repeatedly dividing-with-remainder: a = bq + r,
::    b = rq_1 + r_1, etc. since gcd(a, b) = gcd(b, r) = ... (exercise) etc. The
::    pairs being divided in sequence are (a, b), (b, r), (r, r_1), etc. with update
::    rule new_first = old_second, new_second = remainder upon division of old_first
::    and old_second. One stops when a division by 0 would be necessary to generate
::    new_second, and then d = gcd is the second of the last full pair generated.
::    To see that u and v exist, repeatedly write d in terms of earlier and earlier
::    dividing pairs. To progressively generate the correct u, v, reexamine the original
::    calculation and write the remainders in terms of a, b at each step. Since each
::    remainder depends on the previous two, the same is true of u and v. This is the
::    reason for e.g. m1.u, which semantically is `u at time minus 1`; one can verify
::    the given initialization of these quantities.
::    NOTE: mathematically, gcd is not unique (only up to a scalar).
++  bpegcd
  ~/  %bpegcd
  |=  [a=bpoly b=bpoly]
  ^-  [d=bpoly u=bpoly v=bpoly]
  =/  [u=[m1=bpoly m2=bpoly] v=[m1=bpoly m2=bpoly]]
    :-  zero-bpoly^one-bpoly
    one-bpoly^zero-bpoly
  |-
  ?:  =((bcan (bpoly-to-list b)) ~[0])
    :+  (init-bpoly (bcan (bpoly-to-list a)))
      (init-bpoly (bcan (bpoly-to-list m2.u)))
    (init-bpoly (bcan (bpoly-to-list m2.v)))
  =/  q-r  (bpdvr a b)
  %=  $
    a  b
    b  r:q-r
    u  [(bpsub m2.u (bpmul q:q-r m1.u)) m1.u]
    v  [(bpsub m2.v (bpmul q:q-r m1.v)) m1.v]
  ==
::
::  +bpeval: evaluate a bpoly at a belt
++  bpeval
  ~/  %bpeval
  |:  [bp=`bpoly`one-bpoly x=`belt`1]
  ^-  belt
  ~+
  ?:  (bp-is-zero bp)  0
  ?:  =(len.bp 1)  (~(snag bop bp) 0)
  =/  p  ~(to-poly bop bp)
  =.  p  (flop p)
  =/  res=@  0
  |-
  ?~  p    !!
  ?~  t.p
    (badd (bmul res x) i.p)
  ::  based on p(x) = (...((a_n)x + a_{n-1})x + a_{n-2})x + ... )
  $(res (badd (bmul res x) i.p), p t.p)
::
::  construct the constant bpoly f(X)=c
++  bp-c
  |=  c=belt
  ^-  bpoly
  ~+
  (init-bpoly ~[c])
::
::  +bp-decompose
::
::  given a polynomial f(X) of degree at most D*N, decompose into D polynomials
::  {h_i(X) : 0 <= i < D} each of degree at most N such that
::
::  f(X) = h_0(X^D) + X*h_1(X^D) + X^2*h_2(X^D) + ... + X^{D-1}*h_{D-1}(X^D)
::
::  This is just a generalization of splitting a polynomial into even and odd terms
::  as the FFT does.
::  h_i(X) is the terms whose degree is congruent to i modulo D.
::
::  Passing in d=2 will split into even and odd terms.
::
++  bp-decompose
  ~/  %bp-decompose
  |=  [p=bpoly d=@]
  ^-  (list bpoly)
  =/  total-deg=@  (bdegree (bpoly-to-list p))
  =/  deg=@
    =/  dvr  (dvr total-deg d)
    ?:(=(q.dvr 0) p.dvr (add p.dvr 1))
  =/  acc=(list (list belt))  (reap d ~)
  =-
    %+  turn  -
    |=  poly=(list belt)
    ?~  poly  zero-bpoly
    (init-bpoly (flop poly))
  %+  roll  (range (add 1 deg))
  |=  [n=@ acc=_acc]
  %+  iturn  acc
  |=  [i=@ l=(list belt)]
  =/  idx  (add (mul n d) i)
  ?:  (gth idx total-deg)  l
  [(~(snag bop p) idx) l]
--
