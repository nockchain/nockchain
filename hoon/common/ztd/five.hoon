/=  ztd-four  /common/ztd/four
=>  ztd-four
~%  %utils  ..proof-path  ~
::    utils
|%
++  proof-stream  ::  /lib/proof-stream
  ~%  %proof-stream  +>  ~
  |_  proof
  ++  push
    ~/  %push
    |=  dat=proof-data
    ^-  proof
    ::  Proof-stream bookkeeping must not erase the protocol version.  The v3
    ::  FRI verifier uses this tag after earlier objects have been consumed.
    =/  new-objects  (snoc objects dat)
    =/  new-hashes
      (snoc hashes (hash-hashable:tip5 (hashable-proof-data dat)))
    ?-  version
      %0  [%0 new-objects new-hashes read-index]
      %1  [%1 new-objects new-hashes read-index]
      %2  [%2 new-objects new-hashes read-index]
      %3  [%3 new-objects new-hashes read-index]
    ==
  ::
  ++  pull
    ^-  [proof-data proof]
    ?>  (lth read-index (lent objects))
    =/  dat  (snag read-index objects)
    =/  new-hashes
      (snoc hashes (hash-hashable:tip5 (hashable-proof-data dat)))
    =/  new-read-index  +(read-index)
    :-  dat
    ?-  version
      %0  [%0 objects new-hashes new-read-index]
      %1  [%1 objects new-hashes new-read-index]
      %2  [%2 objects new-hashes new-read-index]
      %3  [%3 objects new-hashes new-read-index]
    ==
  ::
  ++  prover-fiat-shamir
    ^+  tog:tip5
    (absorb-proof-objects objects hashes)
  ::
  ++  verifier-fiat-shamir
    ^+  tog:tip5
    =/  objects=(list proof-data)       (scag read-index objects)
    =/  hashes=(list noun-digest:tip5)  (scag read-index hashes)
    (absorb-proof-objects objects hashes)
  --  ::proof-stream
::
+$  mp-pelt       [a=mp-mega b=mp-mega c=mp-mega]
+$  mp-pelt-comp  [dep=mp-pelt com=mp-pelt]
::
::  triple belt
++  rack
  |=  b=belt
  ^-  [belt belt belt]
  [b b b]
::
::  raise belt
++  reck
  |=  b=belt
  ^-  [belt belt belt]
  [b 0 0]
::
++  constraint-util  ::  /lib/constraint-util
  =,  mp-to-mega
  ~%  %constraint-util  ..constraint-util  ~
  |%
  ++  unlabel-constraints
    ~/  %unlabel-constraints
    |=  mp=(map term mp-ultra)
    ^-  (list mp-ultra)
    ~+
    (turn ~(tap by mp) tail)
  ::
  ++  lift-to-mp-pelt
    |=  m=mp-mega
    ^-  mp-pelt
    [m *mp-mega *mp-mega]
  ::
  ++  mpadd-pelt
    |=  [p=mp-pelt q=mp-pelt]
    ^-  mp-pelt
    :+  (mpadd a.p a.q)
      (mpadd b.p b.q)
    (mpadd c.p c.q)
  ::
  ++  mpsub-pelt
    |=  [p=mp-pelt q=mp-pelt]
    ^-  mp-pelt
    :+  (mpsub a.p a.q)
      (mpsub b.p b.q)
    (mpsub c.p c.q)
  ::
  ++  mpcomp-pelt
  |=  [p=mp-pelt q=mp-pelt]
  ^-  mp-pelt-comp
  [dep=p com=q]
  ::
  ++  mpmul-pelt
    |=  [p=mp-pelt q=mp-pelt]
    ^-  mp-pelt
    =/  m0   (mpmul a.p a.q)
    =/  m1   (mpmul b.p b.q)
    =/  m2   (mpmul c.p c.q)
    =/  n01
      %+  mpsub
        (mpmul (mpadd a.p b.p) (mpadd a.q b.q))
      (mpadd m0 m1)
    =/  n02
      %+  mpsub
        (mpmul (mpadd a.p c.p) (mpadd a.q c.q))
      (mpadd m0 m2)
    =/  n12
      %+  mpsub
        (mpmul (mpadd b.p c.p) (mpadd b.q c.q))
      (mpadd m1 m2)
    :+  (mpsub m0 n12)
      (mpsub (mpadd n01 n12) m2)
    :(mpadd n02 m1 m2)
  ::
  ::  pass in m=(mp-c c) to scale by belt constant c
  ++  mpscal-pelt
    |=  [m=mp-mega p=mp-pelt]
    ^-  mp-pelt
    :+  (mpmul m a.p)
      (mpmul m b.p)
    (mpmul m c.p)
  ::
  ++  lift-to-mega
    |=  =mp-mega
    ^-  mp-ultra
    [%mega mp-mega]
  ::
  ++  tag-mp-comp
    |=  [name=term mp=mp-pelt-comp tail=(list [term mp-ultra])]
    ^-  (list [term mp-ultra])
    :-  :-  (crip (weld (trip name) "-comp"))
        :+  %comp
          ~[a.dep.mp b.dep.mp c.dep.mp]
        ~[a.com.mp b.com.mp c.com.mp]
    tail
  ::
  ++  tag-mp-pelt
    |=  [name=term =mp-pelt tail=(list [term mp-ultra])]
    ^-  (list [term mp-ultra])
    :^    [(crip (weld (trip name) "-a")) [%mega a.mp-pelt]]
        [(crip (weld (trip name) "-b")) [%mega b.mp-pelt]]
      [(crip (weld (trip name) "-c")) [%mega c.mp-pelt]]
    tail
  ::
  ++  untagged-mp-pelt
    |=  [=mp-pelt tail=(list mp-ultra)]
    ^-  (list mp-ultra)
    :*  (lift-to-mega a.mp-pelt)
        (lift-to-mega b.mp-pelt)
        (lift-to-mega c.mp-pelt)
        tail
    ==
  ::
  ++  pelt-col
    |=  [name=term tail=(list term)]
    ^-  (list term)
    :^    (crip (weld (trip name) "-a"))
        (crip (weld (trip name) "-b"))
      (crip (weld (trip name) "-c"))
    tail
  ::
  +$  pelt-chal  @ux
  ++  make-pelt-chal
    |=  r=$-(term belt)
    |=  name=term
    ^-  pelt-chal
    =<  dat
    %-  init-bpoly
    :~  (r (crip (weld (trip name) "-a")))
        (r (crip (weld (trip name) "-b")))
        (r (crip (weld (trip name) "-c")))
    ==
  ::
  +$  felt-stack
    $:  alf=felt
        alf-inv=felt
        len=@
        dat=felt
    ==
  ::
  ++  init-fstack
    ~/  %init-fstack
    |=  alf=felt
    ^-  felt-stack
    =/  alf-inv=felt  (finv alf)
    [alf alf-inv 0 (lift 0)]
  ::
  ::  +fstack: door for working with a $felt-stack
  ::
  ::    bottom [a b c] top
  ::    empty stack [] <-> dat 0
  ++  fstack
    ~/  %fstack
    |_  fs=felt-stack
    ++  push
      ::    [a b c] =>  [a b c x]
      ~/  %push
      |=  x=felt
      ^-  felt-stack
      fs(len +(len.fs), dat (fadd (fmul dat.fs alf.fs) x))
    ++  pop
      ::    [a b c x] => [a b c]
      ~/  %pop
      |=  x=felt
      ^-  felt-stack
      ?>  (gth len.fs 0)
      fs(len (dec len.fs), dat (fmul (fsub dat.fs x) alf-inv.fs))
    ++  push-all
      ::    [a b c] => [a b c x1 ... xn]
      ~/  %push-all
      |=  xs=(list felt)
      ^-  felt-stack
      %+  roll  xs
      |=  [x=felt fs-new=_fs]
      (~(push fstack fs-new) x)
    ++  push-bottom
      ::    [a b c] => [x a b c]
      ~/  %push-bottom
      |=  x=felt
      ^-  felt-stack
      ::    alf^len * x + dat.fs
      fs(len +(len.fs), dat (fadd (fmul (fpow alf.fs len.fs) x) dat.fs))
    ++  push-bottom-all
      ::    [a b c] => [x0 ... xn a b c]
      ~/  %push-bottom-all
      |=  xs=(list felt)
      ^-  felt-stack
      %+  roll  (flop xs)
      ::  let sx = (flop xs)
      ::    [a b c] => [sx2 sx1 sx0 a b c]
      ::  = [a b c] => [xs0 sx1 sx2 a b c]
      |=  [x=felt fs-new=_fs]
      (~(push-bottom fstack fs-new) x)
    ++  cons
      ::    stack cons: [a b], [c d] => [a b c d]
      ~/  %cons
      |=  other=felt-stack
      ^-  felt-stack
      ?>  =(alf.fs alf.other)
      ::    alf^len(other) * dat.fs + dat.other
      %_  fs
        len  (add len.fs len.other)
        dat  (fadd (fmul (fpow alf.fs len.other) dat.fs) dat.other)
      ==
    ++  pop-all
      ~/  %pop-all
      |=  xs=(list felt)
      ^-  felt-stack
      %+  roll  xs
      |=  [x=felt fs-new=_fs]
      ?>  (gth len.fs 0)
      (~(pop fstack fs-new) x)
    ::
    ++  is-empty  =(len.fs 0)
    --
  ::
  +$  belt-stack
    $:  alf=belt
        alf-inv=belt
        len=@
        dat=belt
    ==
  ::
  ++  init-bstack
    ~/  %init-bstack
    |=  alf=belt
    ^-  belt-stack
    =/  alf-inv=belt  (binv alf)
    [alf alf-inv 0 0]
  ::
  ::  +bstack: door for working with a $belt-stack
  ::
  ::    bottom [a b c] top
  ::    empty stack [] <-> dat 0
  ++  bstack
    ~/  %bstack
    |_  bs=belt-stack
    ++  push
      ::    [a b c] =>  [a b c x]
      ~/  %push
      |=  x=belt
      ^-  belt-stack
      bs(len +(len.bs), dat (badd (bmul dat.bs alf.bs) x))
    ++  pop
      ::    [a b c x] => [a b c]
      ~/  %pop
      |=  x=belt
      ^-  belt-stack
      ?>  (gth len.bs 0)
      bs(len (dec len.bs), dat (bmul (bsub dat.bs x) alf-inv.bs))
    ++  push-all
      ::    [a b c] => [a b c x1 ... xn]
      ~/  %push-all
      |=  xs=(list belt)
      ^-  belt-stack
      %+  roll  xs
      |=  [x=belt bs-new=_bs]
      (~(push bstack bs-new) x)
    ++  push-bottom
      ::    [a b c] => [x a b c]
      ~/  %push-bottom
      |=  x=belt
      ^-  belt-stack
      ::    alf^len * x + dat.fs
      bs(len +(len.bs), dat (badd (bmul (bpow alf.bs len.bs) x) dat.bs))
    ++  push-bottom-all
      ::    [a b c] => [x0 ... xn a b c]
      ~/  %push-bottom-all
      |=  xs=(list belt)
      ^-  belt-stack
      %+  roll  (flop xs)
      ::  let sx = (flop xs)
      ::    [a b c] => [sx2 sx1 sx0 a b c]
      ::  = [a b c] => [xs0 sx1 sx2 a b c]
      |=  [x=belt bs-new=_bs]
      (~(push-bottom bstack bs-new) x)
    ++  cons
      ::    stack cons: [a b], [c d] => [a b c d]
      ~/  %cons
      |=  other=belt-stack
      ^-  belt-stack
      ?>  =(alf.bs alf.other)
      ::    alf^len(other) * dat.bs + dat.other
      %_  bs
        len  (add len.bs len.other)
        dat  (badd (bmul (bpow alf.bs len.other) dat.bs) dat.other)
      ==
    ++  pop-all
      ~/  %pop-all
      |=  xs=(list belt)
      ^-  belt-stack
      %+  roll  xs
      |=  [x=belt bs-new=_bs]
      ?>  (gth len.bs 0)
      (~(pop bstack bs-new) x)
    ::
    ++  is-empty  =(len.bs 0)
    --
  ::
  +$  pelt-stack
    $:  alf=pelt
        alf-inv=pelt
        len=@
        dat=pelt
    ==
  ::
  ++  init-pstack
    ~/  %init-pstack
    |=  alf=pelt
    ^-  pelt-stack
    =/  alf-inv=pelt  (pinv alf)
    [alf alf-inv 0 (pelt-lift 0)]
  ::
  ::  +pstack: door for working with a $pelt-stack
  ::
  ::    bottom [a b c] top
  ::    empty stack [] <-> dat 0
  ++  pstack
    ~/  %pstack
    |_  ps=pelt-stack
    ++  push
      ::    [a b c] =>  [a b c x]
      ~/  %push
      |=  x=pelt
      ^-  pelt-stack
      ps(len +(len.ps), dat (padd (pmul dat.ps alf.ps) x))
    ++  pop
      ::    [a b c x] => [a b c]
      ~/  %pop
      |=  x=pelt
      ^-  pelt-stack
      ?>  (gth len.ps 0)
      ps(len (dec len.ps), dat (pmul (psub dat.ps x) alf-inv.ps))
    ++  push-all
      ::    [a b c] => [a b c x1 ... xn]
      ~/  %push-all
      |=  xs=(list pelt)
      ^-  pelt-stack
      %+  roll  xs
      |=  [x=pelt ps-new=_ps]
      (~(push pstack ps-new) x)
    ++  push-bottom
      ::    [a b c] => [x a b c]
      ~/  %push-bottom
      |=  x=pelt
      ^-  pelt-stack
      ::    alf^len * x + dat.fs
      ps(len +(len.ps), dat (padd (pmul (ppow alf.ps len.ps) x) dat.ps))
    ++  push-bottom-all
      ::    [a b c] => [x0 ... xn a b c]
      ~/  %push-bottom-all
      |=  xs=(list pelt)
      ^-  pelt-stack
      %+  roll  (flop xs)
      ::  let sx = (flop xs)
      ::    [a b c] => [sx2 sx1 sx0 a b c]
      ::  = [a b c] => [xs0 sx1 sx2 a b c]
      |=  [x=pelt ps-new=_ps]
      (~(push-bottom pstack ps-new) x)
    ++  cons
      ::    stack cons: [a b], [c d] => [a b c d]
      ~/  %cons
      |=  other=pelt-stack
      ^-  pelt-stack
      ?>  =(alf.ps alf.other)
      ::    alf^len(other) * dat.bs + dat.other
      %_  ps
        len  (add len.ps len.other)
        dat  (padd (pmul (ppow alf.ps len.other) dat.ps) dat.other)
      ==
    ++  pop-all
      ~/  %pop-all
      |=  xs=(list pelt)
      ^-  pelt-stack
      %+  roll  xs
      |=  [x=pelt ps-new=_ps]
      ?>  (gth len.ps 0)
      (~(pop pstack ps-new) x)
    ::
    ++  is-empty  =(len.ps 0)
    --
  ::    utilities for working with log derivative multisets
  ::
  ::  $ld-mset: multiset based on the logarithmic derivative
  +$  ld-mset
    $~  [(lift 0) (lift 0)]
    $:  bet=felt    :: beta - random challenge for polynomial
        dat=felt    :: data - actual value of multiset to write into trace
    ==
  ::
  ++  init-ld-mset
    |=  bet=felt
    ^-  ld-mset
    [bet (lift 0)]
  ::
  ++  ld-union
  |=  msets=(list ld-mset)
  ^-  ld-mset
  ?~  msets
    *ld-mset
  =/  bet  bet.i.msets
  ?:  ?!((levy `(list ld-mset)`msets |=(=ld-mset =(bet.ld-mset bet))))
    !!
  [bet (roll `(list ld-mset)`msets |=([=ld-mset f=felt] (fadd f dat.ld-mset)))]
  ::
  ::  +ld: door for working with ld-msets
  ++  ld
    ~/  %ld
    |_  ms=ld-mset
    ::
    ::  +add: add f to the multiset n times
    ::
    ::    dat' = dat + n/(bet - f)
    ++  add
      ~/  %add
      |=  [f=felt n=@]
      ^-  ld-mset
      :-  bet.ms
      (fadd dat.ms (fmul (lift n) (finv (fsub bet.ms f))))
    ::  +add-all: add a list of [felt, multiplicity] pairs to the multiset
    ::
    ::    adds them one at a time starting with ms and returns a list of
    ::    each intermediate memset in order.
    ++  add-all
      ~/  %add-all
      |=  l=(list [f=felt n=@])
      ^-  (list ld-mset)
      %+  spun  l
      |=  [[f=felt n=@] acc=_ms]
      =/  ret  (~(add ld acc) f n)
      [ret ret]
    ::
    ::    +remove: remove a felt n times
    ++  remove
      |=  [f=felt n=@]
      ^-  ld-mset
      :-  bet.ms
      (fsub dat.ms (fmul (lift n) (finv (fsub bet.ms f))))
    ::
    ::  +union: union multiset ms with multiset ms1
    ++  union
      |=  ms1=ld-mset
      ^-  ld-mset
      ::  randomness has to be the same to perform union
      ?>  =(bet.ms bet.ms1)
      :-  bet.ms
      (fadd dat.ms dat.ms1)
    --
 ::
 ::  $ld-mset-bf: multiset based on the logarithmic derivative
  +$  ld-mset-bf
    $:  bet=belt    :: beta - random challenge for polynomial
        dat=belt    :: data - actual value of multiset to write into trace
    ==
  ::
  ++  init-ld-mset-bf
    |=  bet=belt
    ^-  ld-mset-bf
    [bet 0]
  ::
  ++  ld-union-bf
    |=  msets=(list ld-mset-bf)
    ^-  ld-mset-bf
    ?~  msets
      *ld-mset-bf
    =/  bet  bet.i.msets
    ?:  ?!((levy `(list ld-mset-bf)`msets |=(=ld-mset =(bet.ld-mset bet))))
      !!
    [bet (roll `(list ld-mset-bf)`msets |=([=ld-mset f=belt] (badd f dat.ld-mset)))]
  ::
  ::  +ld: door for working with ld-msets
  ++  ld-bf
    ~/  %ld-bf
    |_  ms=ld-mset-bf
    ::
    ::    +add: add b to the multiset n times
    ::
    ::  dat' = dat + n/(bet - b)
    ++  add
      ~/  %add
      |=  [b=belt n=@]
      ^-  ld-mset-bf
      :-  bet.ms
      (badd dat.ms (bmul n (binv (bsub bet.ms b))))
    ::    +add-all: add a list of [belt, multiplicity] pairs to the multiset
    ::
    ::  adds them one at a time starting with ms and returns a list of
    ::  each intermediate memset in order.
    ++  add-all
      ~/  %add-all
      |=  l=(list [b=belt n=@])
      ^-  (list ld-mset-bf)
      %+  spun  l
      |=  [[b=belt n=@] acc=_ms]
      =/  ret  (~(add ld acc) b n)
      [ret ret]
    ::
    ::    +remove: remove a belt n times
    ++  remove
      |=  [b=belt n=@]
      ^-  ld-mset-bf
      :-  bet.ms
      (bsub dat.ms (bmul n (binv (bsub bet.ms b))))
    ::
    ::    +union: union multiset ms with multiset ms1
    ++  union
      |=  ms1=ld-mset-bf
      ::  randomness has to be the same to perform union
      ?>  =(bet.ms bet.ms1)
      :-  bet.ms
      (badd dat.ms dat.ms1)
    --
  ::
  ::  $ld-mset-pelt: multiset based on the logarithmic derivative
  +$  ld-mset-pelt
    $~  [pzero pzero]
    $:  bet=pelt    :: beta - random challenge for polynomial
        dat=pelt    :: data - actual value of multiset to write into trace
    ==
  ::
  ++  init-ld-mset-pelt
    |=  bet=pelt
    ^-  ld-mset-pelt
    [bet pzero]
  ::
  ++  ld-pelt-union
  |=  msets=(list ld-mset-pelt)
  ^-  ld-mset-pelt
  ?~  msets
    *ld-mset-pelt
  =/  bet  bet.i.msets
  ?:  ?!((levy `(list ld-mset-pelt)`msets |=(=ld-mset-pelt =(bet.ld-mset-pelt bet))))
    !!
  [bet (roll `(list ld-mset-pelt)`msets |=([=ld-mset-pelt p=pelt] (padd p dat.ld-mset-pelt)))]
  ::
  ::  +ld: door for working with ld-msets
  ++  ld-pelt
    ~/  %ld-pelt
    |_  ms=ld-mset-pelt
    ::
    ::  +add: add f to the multiset n times
    ::
    ::    dat' = dat + n/(bet - f)
    ++  add
      ~/  %add
      |=  [p=pelt n=@]
      ^-  ld-mset-pelt
      :-  bet.ms
      (padd dat.ms (pmul (pelt-lift n) (pinv (psub bet.ms p))))
    ::  +add-all: add a list of [felt, multiplicity] pairs to the multiset
    ::
    ::    adds them one at a time starting with ms and returns a list of
    ::    each intermediate memset in order.
    ++  add-all
      ~/  %add-all
      |=  l=(list [p=pelt n=@])
      ^-  (list ld-mset-pelt)
      %+  spun  l
      |=  [[p=pelt n=@] acc=_ms]
      =/  ret  (~(add ld-pelt acc) p n)
      [ret ret]
    ::
    ::    +remove: remove a felt n times
    ++  remove
      |=  [p=pelt n=@]
      ^-  ld-mset-pelt
      :-  bet.ms
      (psub dat.ms (pmul (pelt-lift n) (pinv (psub bet.ms p))))
    ::
    ::  +union: union multiset ms with multiset ms1
    ++  union
      |=  ms1=ld-mset-pelt
      ^-  ld-mset-pelt
      ::  randomness has to be the same to perform union
      ?>  =(bet.ms bet.ms1)
      :-  bet.ms
      (padd dat.ms dat.ms1)
    --
  ::
  ::  stack in triplicate
  +$  tri-stack  [a=belt-stack b=belt-stack c=belt-stack]
  ::  mset in triplicate
  +$  tri-mset  [a=ld-mset-bf b=ld-mset-bf c=ld-mset-bf]
  ++  print-tri-mset
    |=  [m=tri-mset t=(list belt)]
    ^-  (list belt)
    [dat.a.m dat.b.m dat.c.m t]
  ::
  ::  door to manipulate tri-stack
  ++  tstack
    |_  s=tri-stack
    ++  push
      |=  [a=belt b=belt c=belt]
      ^-  tri-stack
      :+  (~(push bstack a.s) a)
        (~(push bstack b.s) b)
      (~(push bstack c.s) c)
    ::
    ++  pop
      |=  [a=belt b=belt c=belt]
      ^-  tri-stack
      :+  (~(pop bstack a.s) a)
        (~(pop bstack b.s) b)
      (~(pop bstack c.s) c)
    ::
    ++  push-all
      |=  xs=(list [belt belt belt])
      ^-  tri-stack
      %+  roll  xs
      |=  [x=[belt belt belt] acc=_s]
      (~(push tstack acc) x)
    ::
    ++  pop-all
      |=  xs=(list [belt belt belt])
      ^-  tri-stack
      %+  roll  xs
      |=  [x=[belt belt belt] acc=_s]
      (~(pop tstack acc) x)
    ++  is-empty
      ^-  ?
      ~(is-empty bstack a.s)
    --
  ::
  ::  door to manipulate mset-state
  ++  tmset
    |_  ms=tri-mset
    ++  add
      |=  [b=triple-belt n=@]
      ^-  tri-mset
      :+  (~(add ld-bf a.ms) a.b n)
        (~(add ld-bf b.ms) b.b n)
      (~(add ld-bf c.ms) c.b n)
    ::
    ++  add-all
      |=  l=(list [b=triple-belt n=@])
      ^-  (list tri-mset)
      %+  spun  l
      |=  [[b=triple-belt n=@] st=_ms]
      =/  ret=tri-mset  (~(add tmset st) b n)
      [ret ret]
    ::
    ++  remove
      |=  [b=belt n=@]
      ^-  tri-mset
      :+  (~(remove ld-bf a.ms) b n)
        (~(remove ld-bf b.ms) b n)
      (~(remove ld-bf c.ms) b n)
    ::
    ++  union
      |=  ms1=tri-mset
      :+  (~(union ld-bf a.ms) a.ms1)
        (~(union ld-bf b.ms) b.ms1)
      (~(union ld-bf c.ms) c.ms1)
    --
  ::
  ++  init-tri-stack
    |=  [alf-a=belt alf-b=belt alf-c=belt]
    ^-  tri-stack
    :+  (init-bstack alf-a)
      (init-bstack alf-b)
    (init-bstack alf-c)
  ::
  ++  init-tri-mset
    |=  [bet-a=belt bet-b=belt bet-c=belt]
    ^-  tri-mset
    :+  (init-ld-mset-bf bet-a)
      (init-ld-mset-bf bet-b)
    (init-ld-mset-bf bet-c)
  ::
  ::  +poly-ld: door handling polynomial constraints for log derivative multiset
  ++  poly-ld
    ~/  %poly-ld
    |_  bet=mp-mega
    ::
    ::    +add: add element v with n multiplicity to old muliset and store in new
    ::
    ::  add element v with n multiplicity to the old multiset in mold and store in
    ::  the new multiset mnew.
    ::
    ::  ldc' = ldc + n / (bet - value)
    ::  => (bet-value)*ldc' = (bet-value)*ldc + n
    ::  => (bet-value)*ldc' - [(bet-value)*ldc) + n] = 0
    ++  add
      ~/  %add
      |=  [mold=mp-mega mnew=mp-mega v=mp-mega n=mp-mega]
      ^-  mp-mega
      %+  mpsub  (mpmul (mpsub bet v) mnew)
      (mpadd n (mpmul (mpsub bet v) mold))
    ::
    ++  add-two
    |=  [mold=mp-mega mnew=mp-mega p1=mp-mega p2=mp-mega]
    ^-  mp-mega
    %+  mpsub
      :(mpmul mnew (mpsub bet p1) (mpsub bet p2))
    ;:  mpadd
      (mpsub bet p1)
      (mpsub bet p2)
      :(mpmul mold (mpsub bet p1) (mpsub bet p2))
    ==
    ::
    ++  remove
      |=  [mold=mp-mega mnew=mp-mega v=mp-mega n=mp-mega]
      ^-  mp-mega
      %+  mpsub  (mpmul (mpsub bet v) mnew)
      (mpsub (mpmul (mpsub bet v) mold) n)
    --
  ::
  ::  +poly-ld: door handling polynomial constraints for log derivative multiset
  ++  poly-ld-pelt
    |_  bet=mp-pelt
    ::
    ::    +add: add element v with n multiplicity to old muliset and store in new
    ::
    ::  add element v with n multiplicity to the old multiset in mold and store in
    ::  the new multiset mnew.
    ::
    ::  ldc' = ldc + n / (bet - value)
    ::  => (bet-value)*ldc' = (bet-value)*ldc + n
    ::  => (bet-value)*ldc' - [(bet-value)*ldc) + n] = 0
    ++  add
      ~/  %add
      |=  [mold=mp-pelt mnew=mp-pelt v=mp-pelt n=mp-mega]
      ^-  mp-pelt
      %+  mpsub-pelt  (mpmul-pelt (mpsub-pelt bet v) mnew)
      (mpadd-pelt (lift-to-mp-pelt n) (mpmul-pelt (mpsub-pelt bet v) mold))
    ::
    ++  add-two
    |=  [mold=mp-pelt mnew=mp-pelt p1=mp-pelt p2=mp-pelt]
    ^-  mp-pelt
    %+  mpsub-pelt
      :(mpmul-pelt mnew (mpsub-pelt bet p1) (mpsub-pelt bet p2))
    ;:  mpadd-pelt
      (mpsub-pelt bet p1)
      (mpsub-pelt bet p2)
      :(mpmul-pelt mold (mpsub-pelt bet p1) (mpsub-pelt bet p2))
    ==
    ::
    ++  remove
      |=  [mold=mp-pelt mnew=mp-pelt v=mp-pelt n=mp-mega]
      ^-  mp-pelt
      %+  mpsub-pelt  (mpmul-pelt (mpsub-pelt bet v) mnew)
      (mpsub-pelt (mpmul-pelt (mpsub-pelt bet v) mold) (lift-to-mp-pelt n))
    --
  ::  +subtree-ld-utils: door for ld subtrees
  ::
  ::    utilities for creating zeroes and tens for the log derivative memory multiset
  ++  subtree-ld-utils
    ~/  %subtree-ld-utils
    |_  cs=(list mp-mega)
    ::
    ::  +make-zero: Create a compressed felt of a zero access which can be added to a multiset
    ++  make-zero
      ~/  %make-zero
      |=  [noun=mp-mega axis=mp-mega child=mp-mega]
      ^-  mp-mega
      (make-ten noun axis child noun child)
    ++  make-ten
      ~/  %make-ten
      |=  $:  noun=mp-mega
              axis=mp-mega
              child=mp-mega
              new-noun=mp-mega
              new-child=mp-mega
          ==
      ^-  mp-mega
      (~(compress poly-tupler cs) ~[noun axis child new-noun new-child])
    --
  ::
  ::
  ++  tuple
    ~/  %tuple
    |_  cs=(list felt)
    ++  compress
      ~/  %compress
      |=  fs=(list felt)
      ^-  felt
      %^  zip-roll  cs  fs
      |=  [[c=felt f=felt] acc=_(lift 0)]
      (fadd acc (fmul c f))
    --
  ::
  ++  tuple-bf
    ~/  %tuple-bf
    |_  cs=(list belt)
    ++  compress
      ~/  %compress
      |=  bs=(list belt)
      ^-  belt
      %^  zip-roll  cs  bs
      |=  [[c=belt b=belt] acc=belt]
      (badd acc (bmul c b))
    --
  ::
  +$  triple-belt  [a=belt b=belt c=belt]
  ::
  ++  tuple-trip
    ~/  %tuple-trip
    |_  cs=(list triple-belt)
    ++  compress
      ~/  %compress
      |=  bs=(list triple-belt)
      ^-  triple-belt
      %^  zip-roll  cs  bs
      |=  [[c=triple-belt b=triple-belt] acc=triple-belt]
      :+  (badd a.acc (bmul a.c a.b))
        (badd b.acc (bmul b.c b.b))
      (badd c.acc (bmul c.c c.b))
    --
  ::
  ::    utilities for working with polynomial stacks
  ::
  ::  +poly-stack: door for working with polynomial stacks
  ++  poly-stack
    ~/  %poly-stack
    |_  [alf=mp-mega alf-inv=mp-mega vars=(map term mp-mega)]
    ++  v
      ~/  %v
      |=  nam=term
      ^-  mp-mega
      ~+
      ~|  var-not-found+nam
      (~(got by vars) nam)
    ++  push
      ~/  %push
      |=  [s=mp-mega nam=mp-mega]
      ^-  mp-mega
      (mpadd (mpmul alf s) nam)
    ++  push-all
      ~/  %push-all
      |=  [s=mp-mega nams=(list mp-mega)]
      ^-  mp-mega
      %+  roll  nams
      |:  [nam=`mp-mega`(mp-c 0) mp=`mp-mega`s]
      (push mp nam)
    ++  pop
      ~/  %pop
      |=  [s=mp-mega nam=mp-mega]
      ^-  mp-mega
      (mpmul alf-inv (mpsub s nam))
    ++  pop-all
      ~/  %pop-all
      |=  [s=mp-mega nams=(list mp-mega)]
      ^-  mp-mega
      %+  roll  nams
      |:  [nam=`mp-mega`(mp-c 0) mp=`mp-mega`s]
      (pop mp nam)
    --
  ::
  ++  poly-tupler
    ~/  %poly-tupler
    |_  cs=(list mp-mega)
    ++  compress
      ~/  %compress
      |=  nams=(list mp-mega)
      ^-  mp-mega
      %^  zip-roll  cs  nams
      |=  [[c=mp-mega n=mp-mega] acc=_(mp-c 0)]
      (mpadd acc (mpmul c n))
    --
  ::
  ++  poly-tupler-pelt
    ~/  %poly-tupler-pelt
    |_  cs=(list mp-pelt)
    ++  compress
      ~/  %compress
      |=  nams=(list mp-pelt)
      ^-  mp-pelt
      =/  acc=mp-pelt  [(mp-c 0) (mp-c 0) (mp-c 0)]
      %^  zip-roll  cs  nams
      |=  [[c=mp-pelt n=mp-pelt] acc=_acc]
      (mpadd-pelt acc (mpmul-pelt c n))
    --
  ::    column name utilities
  ::
  ::  +grab: alias for snagging from a row using the var name instead of col index
  ++  grab
    ~/  %grab
    |=  [label=term row=fpoly var-indices=(map term @)]
    %-  ~(snag fop row)
    (~(got by var-indices) label)
  ::
  ++  grab-bf
    ~/  %grab-bf
    |=  [label=term row=bpoly var-indices=(map term @)]
    %-  ~(snag bop row)
    (~(got by var-indices) label)
  ::
  ::    noun utilities
  ::
  ++  noun-utils
    ~/  %noun-utils
    |_  $:  noun-chals=[a=felt b=felt c=felt alf=felt]
            tuple-chals=[p=felt q=felt r=felt s=felt t=felt]
        ==
    ++  build-atom
      ~/  %build-atom
      |=  atom=@
      ^-  felt
      ::  general format: (a * len) + (b * dyck) + (c * leaf)
      ::  for atoms: (a * 1) + (b * 0) + (c * <atom>)
      (fadd a.noun-chals (fmul c.noun-chals (lift atom)))
    ::
    ++  make-zero-ld
      ~/  %make-zero-ld
      |=  [memset=ld-mset noun=felt axis=@ child=felt num=@]
      ^-  ld-mset
      %-  ~(add ld memset)
      :_  num
      %-  ~(compress tuple [p q r s t ~]:tuple-chals)
      ~[noun (build-atom axis) child noun child]
    ::
    ++  make-zeroes-ld
      ~/  %make-zeroed-ld
      |=  [memset=ld-mset zs=(list [noun=felt axis=@ child=felt num=@])]
      ^-  (list ld-mset)
      %-  ~(add-all ld memset)
      %+  turn  zs
      |=  [noun=felt axis=@ child=felt num=@]
      :_  num
      %-  ~(compress tuple [p q r s t ~]:tuple-chals)
      ~[noun (build-atom axis) child noun child]
    --
  ::
  ::
  ++  noun-poly-utils
    ~/  %noun-poly-utils
    |_  $:  noun-chals=[a=mp-mega b=mp-mega c=mp-mega alf=mp-mega]
            vars=(map term mp-mega)
        ==
    ++  v
      ~/  %v
      |=  nam=term
      ^-  mp-mega
      ~+
      ~|  var-not-found+nam
      (~(got by vars) nam)
    ++  build-atom-literal
      ~/  %build-atom-literal
      |=  atom=@
      ^-  mp-mega
      (mpadd a.noun-chals (mpscal atom c.noun-chals))
    ++  build-atom-reg
      ~/  %build-atom-reg
      |=  atom=@tas
      ^-  mp-mega
      (mpadd a.noun-chals (mpmul c.noun-chals (v atom)))
    ++  build-atom-poly
      ~/  %build-atom-poly
      |=  atom=mp-mega
      ^-  mp-mega
      (mpadd a.noun-chals (mpmul c.noun-chals atom))
    --
  ::
  ::
  ::TODO the %tree-utils chapter (and arms formerly in it, such as
  ::+get-subtree-multiset) consists of some of the worst code in
  ::the system. they're used for generating data for the tables, and there's a
  ::lot of duplicated code between the arms that just different enough to make
  ::it a pain to refactor. I suspect that some of this should actually be put
  ::into $fock-return to avoid recalculating the same things between tables (or
  ::even between +build and +extend for the same table). this deserves special
  ::attention and a partial solution shouldn't be thrown in as a "btw" for
  ::another PR. see also the work on $bushes in the PR section of the old zkvm
  ::repo.
  ::
  ::  TODO make these arms a door over the randomness
  ::
  ::  a, b, c, and alf are random weights from the challenges
  ::
  ::
  ++  build-tree-data
    ~/  %build-tree-data
    |=  [t=* alf=pelt]
    ^-  tree-data
    ~+
    =/  leaf=(list pelt)  (turn (leaf-sequence:shape t) pelt-lift)
    =/  dyck=(list pelt)  (turn (dyck:shape t) pelt-lift)
    =/  leaf-pelt=pelt
      dat:(~(push-all pstack (init-pstack alf)) leaf)
    =/  dyck-pelt=pelt
      dat:(~(push-all pstack (init-pstack alf)) dyck)
    =/  len  (lent leaf)
    =/  size  (ppow alf len)
    :*  size
        dyck-pelt
        leaf-pelt
        t
    ==
  --  ::constraint
--
