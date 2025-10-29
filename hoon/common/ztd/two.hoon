/=  ztd-one  /common/ztd/one
=>  ztd-one
~%  %ext-field  ..belt  ~
::    math-ext: arithmetic for elements and polynomials over the extension field.
|_  deg=_`@`3  ::  field extension degree
::    finite field arithmetic
+|  %felt-math
::
::  +deg-to-irp:
++  deg-to-irp
  ^-  (map @ bpoly)
  %-  ~(gas by *(map @ bpoly))
  :~  [1 (init-bpoly ~[0 1])]
      [2 (init-bpoly ~[2 18.446.744.069.414.584.320 1])]
      [3 (init-bpoly ~[1 18.446.744.069.414.584.320 0 1])]
  ==
::
++  f0  0x1.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000
++  f1  0x1.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0001
::
::  +lift: the unique lift of a base field element into an extension field
++  lift
  |=  =belt
  ~+
  ^-  felt
  %-  frep
  :-  belt
  (reap (dec deg) 0)
::
++  drop
  |=  =felt
  ^-  belt
  ::  inverse of lift
  ::  (lift 7) => <felt ~[7 0 0 0]>
  ::  (snag 0 <felt ~[7 0 0 0]>)  => 7
  (snag 0 (felt-to-list felt))
::
++  felt-to-list
  |=  fel=felt
  ^-  (list belt)
  (bpoly-to-list [deg fel])
::
::
::  +fat: is the atom a felt?
++  fat
  ~/  %fat
  |=  a=@
  ^-  ?
  ~+
  =/  v  (rip 6 a)
  ?&  =((lent v) +(deg))
      (levy v based)
  ==
::
::  +frip: field rip - rip a felt into a list of belts
++  frip
  ~/  %frip
  |=  a=felt
  ^-  (list belt)
  ~+
  ?>  (fat a)
  (bpoly-to-list [deg a])
::
::  +frep: inverse of frip; list of belts are rep'd to a felt
++  frep
  ~/  %frep
  |=  x=(list belt)
  ^-  felt
  ~+
  ?>  =((lent x) deg)
  ?>  (levy x based)
  dat:(init-bpoly x)
::
::  +fadd: field addition
++  fadd
  ~/  %fadd
  |:  [a=`felt`(lift 0) b=`felt`(lift 0)]
  ~+
  ~|  a
  ?>  (fat a)
  ~|  b
  ?>  (fat b)
  %-  frep
  (zip (frip a) (frip b) badd)
::
::  +fneg: field negation
++  fneg
  ~/  %fneg
  |:  a=`felt`(lift 0)
  ~+
  ?>  (fat a)
  %-  frep
  (turn (frip a) bneg)
::
::  +fsub: field subtraction
++  fsub
  ~/  %fsub
  |:  [a=`felt`(lift 0) b=`felt`(lift 0)]
  ^-  felt
  ~+
  (fadd a (fneg b))
::
++  fmul-naive
  ~/  %fmul-naive
  |:  [a=`felt`(lift 1) b=`felt`(lift 1)]
  ^-  felt
  ~+
  ~|  `@ux`a
  ?>  (fat a)
  ~|  `@ux`b
  ?>  (fat b)
  =/  result-poly
    %-  bpoly-to-list
    %+  bpmod
      (bpmul deg^a deg^b)
    (~(got by deg-to-irp) deg)
  %-  frep
  %+  bzero-extend  result-poly
  (sub deg (lent result-poly))
::
::
::  +fmul: field multiplication
::
::  Multiply field extension elements and reduce by using the algebraic identities of the basis
::  vectors.
::
::  We are reducing mod x^3-x+1. So,
::  x^3-x+1=0
::  => x^3 = x-1
::  => x^4 = x^2-x
::
::  So,
::  (a0+a1x+a2x^2)*(b0+b1x+b2x^2)
::  = a0b0 + [a0b1+a1b0]x + [a0b2+a1b1+a2b0]x^2 + [a1b2+a2b1]x^3 + a2b2x^4
::
::  Substituting the reductions above we get
::
::  (a0+a1x+a2x^2)*(b0+b1x+b2x^2)
::  = a0b0 + [a0b1+a1b0]x + [a0b2+a1b1+a2b0]x^2 + [a1b2+a2b1](x-1) + a2b2(x^2-x)
::
::  = [a0b0-a1b2-a2b1] + [a0b1+a1b0+a1b2+a2b1-a2b2]x + [a0b2+a1b1+a2b0+a2b2]x^2.
::
::  And finally we use the karatsuba trick to reduce multiplications.
::
++  fmul
  ~/  %fmul
  |:  [a=`felt`(lift 1) b=`felt`(lift 1)]
  ^-  felt
  ~+
  ~|  `@ux`a
  ?>  (fat a)
  ~|  `@ux`b
  ?>  (fat b)
  =/  a  [3 a]
  =/  b  [3 b]
  =/  a0=belt  (~(snag bop a) 0)
  =/  a1=belt  (~(snag bop a) 1)
  =/  a2=belt  (~(snag bop a) 2)
  =/  b0=belt  (~(snag bop b) 0)
  =/  b1=belt  (~(snag bop b) 1)
  =/  b2=belt  (~(snag bop b) 2)
  ::
  =/  a0b0  (bmul a0 b0)
  =/  a1b1  (bmul a1 b1)
  =/  a2b2  (bmul a2 b2)
  =/  a0b1-a1b0  (bsub (bsub (bmul (badd a0 a1) (badd b0 b1)) a0b0) a1b1)
  =/  a1b2-a2b1  (bsub (bsub (bmul (badd a1 a2) (badd b1 b2)) a1b1) a2b2)
  =/  a0b2-a2b0  (bsub (bsub (bmul (badd a0 a2) (badd b0 b2)) a0b0) a2b2)
  %-  frep
  :~  (bsub a0b0 a1b2-a2b1)
      (bsub (badd a0b1-a1b0 a1b2-a2b1) a2b2)
      :(badd a0b2-a2b0 a1b1 a2b2)
  ==
::
::  +finv: field inversion
++  finv
  ~/  %finv
  |:  a=`felt`(lift 1)
  ^-  felt
  ~+
  ?>  (fat a)
  ?<  =(a (lift 0))
  =/  egcd=[d=bpoly u=bpoly v=bpoly]
    %+  bpegcd
      (~(got by deg-to-irp) deg)
    deg^a
  =/  d  (bpoly-to-list d.egcd)
  =/  u  (bpoly-to-list u.egcd)
  =/  v  (bpoly-to-list v.egcd)
  ?>  =((bdegree d) 0)
  ?<  ?=(~ d)
  =/  result-poly
    %-  bpoly-to-list
    (bpscal (binv i.d) v.egcd)
  %-  frep
  %+  bzero-extend  result-poly
  (sub deg (lent result-poly))
::
::  +mass-inversion: inverts list of elements by cleverly performing only a single inversion
++  mass-inversion
  ~/  %mass-inversion
  |=  lis=(list felt)
  ^-  (list felt)
  |^
  =/  all-prods  (accumulate-products lis)
  ?<  ?=(~ all-prods)
  =.  lis    (flop lis)
  =/  acc    (finv i.all-prods)
  =/  prods  t.all-prods
  =|  invs=(list felt)
  |-
  ?~  prods
    [acc invs]
  ?<  ?=(~ lis)
  %=  $
    lis    t.lis
    prods  t.prods
    acc    (fmul acc i.lis)
    invs    [(fmul acc i.prods) invs]
  ==
  ++  accumulate-products
    |=  lis=(list felt)
    ^-  (list felt)
    =|  res=(list felt)
    =/  acc  (lift 1)
    |-
    ?~  lis
      res
    ?:  =(i.lis (lift 0))
      ~|  "Cannot invert 0!"
      !!
    =/  new  (fmul acc i.lis)
    $(acc new, res [new res], lis t.lis)
  --
::
::  +fdiv: division of field elements
++  fdiv
  ~/  %fdiv
  |:  [a=`felt`(lift 1) b=`felt`(lift 1)]
  ^-  felt
  ~+
  (fmul a (finv b))
::
::  +fpow: field power; computes x^n
++  fpow
  ~/  %fpow
  |:  [x=`felt`(lift 1) n=`@`0]
  ^-  felt
  ~+
  ?>  (fat x)
  ~?  (gte (met 3 n) (met 3 (lift 0)))  "fpow: n is wayy too high and is likely a felt by accident."
  %.  [(lift 1) x n]
  |=  [y=felt x=felt n=@]
  ^-  felt
  ?>  (fat y)
  ?:  =(n 0)
    y
  ::  parity flag
  =/  f=@  (end 0 n)
  ?:  =(0 f)
    $(x (fmul x x), n (rsh 0 n))
  $(y (fmul y x), x (fmul x x), n (rsh 0 n))
::
::  +bpeval-lift: evaluate a bpoly at a felt
++  bpeval-lift
  ~/  %bpeval-lift
  |:  [bp=`bpoly`one-bpoly x=`felt`(lift 1)]
  ^-  felt
  ~+
  ?:  (bp-is-zero bp)  (lift 0)
  ?:  =(len.bp 1)  (lift (~(snag bop bp) 0))
  =/  p  ~(to-poly bop bp)
  =.  p  (flop p)
  =/  res=@  (lift 0)
  |-
  ?~  p    !!
  ?~  t.p
    (fadd (fmul res x) (lift i.p))
  ::  based on p(x) = (...((a_n)x + a_{n-1})x + a_{n-2})x + ... )
  $(res (fadd (fmul res x) (lift i.p)), p t.p)
::
::    general field polynomial methods and math
+|  %fpoly-math
::
::
++  fop  ~(ops array-op 3)
::
::  +fpadd: field polynomial addition
++  fpadd
  ~/  %fpadd
  |:  [fp=`fpoly`zero-fpoly fq=`fpoly`zero-fpoly]
  ^-  fpoly
  ?>  &(!=(len.fp 0) !=(len.fq 0))
  =/  p  ~(to-poly fop fp)
  =/  q  ~(to-poly fop fq)
  =/  lp  (lent p)
  =/  lq  (lent q)
  =/  m  (max lp lq)
  =:  p  (weld p (reap (sub m lp) (lift 0)))
      q  (weld q (reap (sub m lq) (lift 0)))
    ==
  %-  init-fpoly
  (zip p q fadd)
::
::  +fpneg: additive inverse of a field polynomial
++  fpneg
  ~/  %fpneg
  |:  fp=`fpoly`zero-fpoly
  ^-  fpoly
  ?>  !=(len.fp 0)
  ~+
  =/  p  ~(to-poly fop fp)
  %-  init-fpoly
  (turn p fneg)
::
::  fpscal: scale a polynomial by a field element
++  fpscal
  ~/  %fpscal
  |:  [c=`felt`(lift 1) fp=`fpoly`one-fpoly]
  ^-  fpoly
  ~+
  =/  p  ~(to-poly fop fp)
  %-  init-fpoly
  %+  turn
    p
  (cury fmul c)
::
::  +fpsub:  field polynomial subtraction
++  fpsub
  ~/  %fpsub
  |:  [p=`fpoly`zero-fpoly q=`fpoly`zero-fpoly]
  ^-  fpoly
  ~+
  ?>  &(!=(len.p 0) !=(len.q 0))
  (fpadd p (fpneg q))
::
++  fp-is-zero
  ~/  %fp-is-zero
  |=  p=fpoly
  ^-  ?
  ~+
  =.  p  (fpcan p)
  |(=(len.p 0) =(p zero-fpoly))
::
++  fp-is-one
  ~/  %fp-is-one
  |=  p=fpoly
  ^-  ?
  ~+
  =.  p  (fpcan p)
  &(=(len.p 1) =((~(snag fop p) 0) (lift 1)))
::
::  f(x)=0
++  zero-fpoly
  ~+
  ^-  fpoly
  (init-fpoly ~[(lift 0)])
::
::  f(x)=1
++  one-fpoly
  ~+
  ^-  fpoly
  (init-fpoly ~[(lift 1)])
::
::  f(x)=x
++  id-fpoly
  ~+
  ^-  fpoly
  (init-fpoly ~[(lift 0) (lift 1)])
::
::  +init-fpoly: transforms a list of felts into its fpoly equivalent
++  init-fpoly
~/  %init-fpoly
  |=  poly=(list felt)
  ^-  fpoly
  ?~  poly  [0 (lift 0)]
  array:(init-mary poly)
::
::  +fcan: gives the canonical leading-zero-stripped representation of p(x)
++  fcan
  |=  p=poly
  ^-  poly
  =.  p  (flop p)
  |-
  ?~  p
    ~
  ?:  =(i.p (lift 0))
    $(p t.p)
  (flop p)
::
++  fpcan
  |=  fp=fpoly
  ^-  fpoly
  =/  p  ~(to-poly fop fp)
  (init-fpoly (fcan p))
::
::  +fdegree: computes the degree of a polynomial
++  fdegree
  |=  p=poly
  ^-  @
  =/  cp=poly  (fcan p)
  ?~  cp  0
  (dec (lent cp))
::
::  +fzero-extend: make the zero coefficients for powers of x higher than deg(p) explicit
++  fzero-extend
  |=  [p=poly much=@]
  ^-  poly
  (weld p (reap much (lift 0)))
::
++  lift-to-fpoly
  ~/  %lift-to-fpoly
  |=  poly=(list belt)
  ^-  fpoly
  ?>  (levy poly based)
  (init-fpoly (turn poly lift))
::
++  fpoly-to-list
  ~/  %fpoly-to-list
  |=  fp=fpoly
  ^-  (list felt)
  ~|  "len.fp must not be 0"
  ?>  !=(len.fp 0)
  (mary-to-list [3 fp])
::
::  +bpoly-to-fpoly: lift a bpoly to an fpoly
++  bpoly-to-fpoly
  ~/  %bpoly-to-fpoly
  |=  bp=bpoly
  ^-  fpoly
  (lift-to-fpoly ~(to-poly bop bp))
::
::  +fpoly-from-dat
::
::  given a dat=@ux, compute the length using met and return an fpoly
++  fpoly-from-dat
  |=  dat=@ux
  ^-  fpoly
  [(div (dec (met 6 dat)) 3) dat]
::
::  +fp-ntt: number theoretic transform for fpolys based on anatomy of a stark
++  fp-ntt
  ~/  %fp-ntt
  |=  [fp=fpoly root=felt]
  ^-  fpoly
  ~+
  ?:  =(len.fp 1)
    fp
  =/  half  (div len.fp 2)
  ?>  =((fpow root len.fp) (lift 1))
  ?<  =((fpow root half) (lift 1))
  =/  odds
    %+  fp-ntt
      %-  init-fpoly
      %+  murn  (range len.fp)
      |=  i=@
      ?:  =(0 (mod i 2))
        ~
      `(~(snag fop fp) i)
    (fmul root root)
  =/  evens
    %+  fp-ntt
      %-  init-fpoly
      %+  murn  (range len.fp)
      |=  i=@
      ?:  =(1 (mod i 2))
        ~
      `(~(snag fop fp) i)
    (fmul root root)
  %-  init-fpoly
  %+  turn  (range len.fp)
  |=  i=@
  %+  fadd  (~(snag fop evens) (mod i half))
  %+  fmul  (fpow root i)
  (~(snag fop odds) (mod i half))
::
::  +bp-ntt: ntt over base field
++  bp-ntt
  ~/  %bp-ntt
  |=  [bp=bpoly root=belt]
  ^-  bpoly
  ~+
  ?:  =(len.bp 1)
    bp
  =/  half  (div len.bp 2)
  ?>  =((bpow root len.bp) 1)
  ?<  =((bpow root half) 1)
  =/  odds
    %+  bp-ntt
      %-  init-bpoly
      %+  murn  (range len.bp)
      |=  i=@
      ?:  =(0 (mod i 2))
        ~
      `(~(snag bop bp) i)
    (bmul root root)
  =/  evens
    %+  bp-ntt
      %-  init-bpoly
      %+  murn  (range len.bp)
      |=  i=@
      ?:  =(1 (mod i 2))
        ~
      `(~(snag bop bp) i)
    (bmul root root)
  %-  init-bpoly
  %+  turn  (range len.bp)
  |=  i=@
  %+  badd  (~(snag bop evens) (mod i half))
  %+  bmul  (bpow root i)
  (~(snag bop odds) (mod i half))
::
::  +fp-fft: Discrete Fourier Transform (DFT) with Fast Fourier Transform (FFT) algorithm
++  fp-fft
  ~/  %fp-fft
  |=  p=fpoly
  ^-  fpoly
  ~+
  ~|  "fft: must have power-of-2-many coefficients."
  ?>  =(0 (dis len.p (dec len.p)))
  (fp-ntt p (lift (ordered-root len.p)))
::
::  +fp-ifft: Inverse DFT with FFT algorithm
++  fp-ifft
  ~/  %ifft
  |=  p=fpoly
  ^-  fpoly
  ~+
  ~|  "ifft: must have power-of-2-many coefficients."
  ?>  =((dis len.p (dec len.p)) 0)
  %+  fpscal  (lift (binv len.p))
  (fp-ntt p (lift (binv (ordered-root len.p))))
::
::  +bp-fft: fft over base field
++  bp-fft
  ~/  %bp-fft
  |=  p=bpoly
  ^-  bpoly
  ~+
  ~|  "bp-fft: must have power-of-2-many coefficients."
  ?>  =(0 (dis len.p (dec len.p)))
  (bp-ntt p (ordered-root len.p))
::
::  +bp-ifft: ifft over base field
++  bp-ifft
  ~/  %bp-ifft
  |=  p=bpoly
  ^-  bpoly
  ~+
  ~|  "bp-ifft: must have power-of-2-many coefficients."
  ?>  =((dis len.p (dec len.p)) 0)
  %+  bpscal  (binv len.p)
  (bp-ntt p (binv (ordered-root len.p)))
::
::  +fpmul-naive: high school polynomial multiplication
++  fpmul-naive
  ~/  %fpmul-naive
  |=  [fp=fpoly fq=fpoly]
  ^-  fpoly
  ~+
  =/  p  ~(to-poly fop fp)
  =/  q  ~(to-poly fop fq)
  %-  init-fpoly
  ?:  ?|(=(~ p) =(~ q))
    ~
  =/  v=(list felt)
    %-  weld
    :_  p
    (reap (dec (lent q)) (lift 0))
  =/  w=(list felt)  (flop q)
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
    |=  [v=(list felt) w=(list felt)]
    ^-  felt
    =/  dot=felt  (lift 0)
    |-
    ?:  ?|(?=(~ v) ?=(~ w))
      dot
    $(v t.v, w t.w, dot (fadd dot (fmul i.v i.w)))
  ==
::
::  +fpmul-fast: polynomial multiplication with fft
++  fpmul-fast
  ~/  %fpmul-fast
  |=  [fp=fpoly fq=fpoly]
  ^-  fpoly
  ~+
  =:  fp  (fpcan fp)
      fq  (fpcan fq)
    ==
  ?:  ?|(=(fp zero-fpoly) =(fq zero-fpoly))
    zero-fpoly
  =*  deg-p  len.fp
  =*  deg-q  len.fq
  =/  deg-prod  (bex (xeb (dec (add deg-p deg-q))))
  %-  fpcan
  %-  fp-ifft
  %+  %~  zip  fop
      (fp-fft (~(zero-extend fop fp) (sub deg-prod deg-p)))
    (fp-fft (~(zero-extend fop fq) (sub deg-prod deg-q)))
  fmul
::
::  +fpmul: polynomial multiplication
++  fpmul
  ~/  %fpmul
  |:  [fp=`fpoly`one-fpoly fq=`fpoly`one-fpoly]
  ^-  fpoly
  ~+
  ?:  |(=(len.fp 0) =(len.fq 0))
    (init-fpoly ~[(lift 0)])
  =/  p  ~(to-poly fop fp)
  =/  q  ~(to-poly fop fq)
  ?:  (lth (add (fdegree p) (fdegree q)) 8)
    (fpmul-naive fp fq)
  (fpmul-fast fp fq)
::
::  fppow: compute (p(x))^k
++  fppow
  ~/  %fppow
  |=  [fp=fpoly k=@]
  ^-  fpoly
  ~+
  ?>  !=(len.fp 0)
  %.  [(init-fpoly ~[(lift 1)]) fp k]
  |=  [q=fpoly p=fpoly k=@]
  ?:  =(k 0)
    q
  =/  f=@  (end 0 k)
  ?:  =(f 0)
    %_  $
      p  (fpmul p p)
      k  (rsh 0 k)
    ==
  %_  $
    q  (fpmul q p)
    p  (fpmul p p)
    k  (rsh 0 k)
  ==
::
::  +fp-hadamard
::
::  Hadamard product of two fpolys. This is just a fancy name for pointwise multiplication.
++  fp-hadamard
  ~/  %fp-hadamard
  |:  [fp=`fpoly`one-fpoly fq=`fpoly`one-fpoly]
  ^-  fpoly
  =:  fp  (fpcan fp)
      fq  (fpcan fq)
    ==
  ?:  |((fp-is-zero fp) (fp-is-zero fq))  zero-fpoly
  ?:  (fp-is-one fp)  fq
  ?:  (fp-is-one fq)  fp
  ?:  |(=(len.fp 0) =(len.fq 0))  zero-fpoly
  (~(zip fop fp) fq fmul)
::
::  +fp-hadamard-pow
::
::  Hadamard product with itself n times
++  fp-hadamard-pow
  ~/  %fp-hadamard-pow
  |:  [fa=`fpoly`one-fpoly n=`@`0]
  ^-  fpoly
  ?:  =(n 0)  one-fpoly
  ?:  =(n 1)  fa
  %+  roll  (range n)
  |=  [i=@ acc=_one-fpoly]
  ?:  =(i 0)  fa
  (fp-hadamard acc fa)
::
::  fpdvr
++  fpdvr
  ~/  %fpdvr
  |:  [fa=`fpoly`one-fpoly fb=`fpoly`one-fpoly]
  ^-  [q=fpoly r=fpoly]
  ~+
  ?>  &(!=(len.fa 0) !=(len.fb 0))
  =/  a  ~(to-poly fop fa)
  =/  b  ~(to-poly fop fb)
  =^  rem  b
    :-  (flop (fcan a))
    (flop (fcan b))
  ~|  "Cannot divide by the zero polynomial."
  ?<  ?=(~ b)
  =/  db  (dec (lent b))
  ?:  =(db 0)
    :_  (init-fpoly ~)
    (fpscal (finv i.b) fa)
  =|  quot=poly
  |-
  ?:  (lth (fdegree (flop rem)) db)
    :-  (init-fpoly quot)
    %-  init-fpoly
    (fcan (flop rem))
  ?<  ?=(~ rem)
  =/  new-coeff  (fdiv i.rem i.b)
  =/  new-rem
    %~  to-poly  fop
    %+  fpsub
      (init-fpoly rem)
    (fpscal new-coeff (init-fpoly b))
  ?<  ?=(~ new-rem)
  %=  $
    quot  [new-coeff quot]
    rem   t.new-rem
  ==
::
::  +fpdiv: polynomial division
::
::    Quasilinear algo, faster than naive. Based on the formula
::    rev(p/q) = rev(q)^{-1} rev(p) mod x^{deg(p) - deg(q) + 1}.
::    Why?: we can compute rev(f)^{-1} mod x^l quickly.
++  fpdiv
  ~/  %fpdiv
  |:  [p=`fpoly`one-fpoly q=`fpoly`one-fpoly]
  ^-  fpoly
  ~+
  ?>  &(!=(len.p 0) !=(len.q 0))
  |^
  =:  p  (fpcan p)
      q  (fpcan q)
    ==
  ?:  (fp-is-zero q)
    ~|  "Cannot divide by the zero polynomial!"
    !!
  ?:  (fp-is-zero p)
    zero-fpoly
  =/  [c=felt f=fpoly]  (con-mon p)
  =/  [d=felt g=fpoly]  (con-mon q)
  =/  lead=felt  (fdiv c d)
  =/  rf=fpoly  ~(flop fop f)
  =/  rg=fpoly  ~(flop fop g)
  =/  df=@  (fdegree ~(to-poly fop f))
  =/  dg=@  (fdegree ~(to-poly fop g))
  ?:  (lth df dg)
    zero-fpoly
  =/  dq=@  (sub df dg)
  %+  fpscal
    lead
  %~  flop  fop
  %.  +(dq)
  %~  scag  fop
  (fpmul (pinv-mod-x-to +(dq) rg) rf)
  ::
  ::  +pinv-mod-x-to: computes p^{-1} mod x^l
  ++  pinv-mod-x-to
    |=  [l=@ p=fpoly]
    ^-  fpoly
    (~(scag fop (hensel-lift-inverse p (xeb l))) l)
  ::
  ::  +hensel-lift-inverse: if p_0 = 1, compute p^{-1} mod x^{2^l} (l = level parameter below)
  ::
  ::    Given a(x) such that p(x)a(x) = 1 mod x^{2^i}, then a*p = 1 + x^{2^i}s(x) (see s below).
  ::    Letting t(x) = -a(x)*s(x) mod x^{2^i}, then p's inverse modulo x^{2^{i+1}} is
  ::    a(x) + x^{2^i}t(x)
  ++  hensel-lift-inverse
    |=  [p=fpoly level=@]
    ^-  fpoly
    ~|  "Polynomial must have constant term equal to 1."
    ?>  =(~(head fop p) (lift 1))
    ::  since p_0 = 1, 1 is p's inverse mod x (x = x^{2^0})
    =/  inv=fpoly  one-fpoly
    ::  have solution for level i, i.e. mod x^{2^i}, bootstrapping to next level
    =/  i=@  0
    |-
    ?:  =(i level)
      inv
    =/  bex-i=@  (bex i)
    =/  s  (~(slag fop (fpmul p inv)) bex-i)
    =/  t  (~(scag fop (fpmul (fpscal (lift (bneg 1)) inv) s)) bex-i)
    $(i +(i), inv (fpadd inv (pmul-by-x-to bex-i t)))
  ::
  ::  pmul-by-x-to: multiply by x to the power l
  ++  pmul-by-x-to
    |=  [l=@ p=fpoly]
    ^-  fpoly
    %.  p
    ~(weld fop (init-fpoly (reap l (lift 0))))
  ::
  ::  con-mon: split p(x)!=0 uniquely into c*f(x) where c is constant f monic
  ++  con-mon
    |=  fp=fpoly
    ^-  [felt fpoly]
    ~+
    =.  fp  ~(flop fop (fpcan fp))
    ~|  "Cannot accept the zero polynomial!"
    ?<  =(zero-fpoly fp)
    :-  ~(head fop fp)
    %~  flop  fop
    (fpscal (finv ~(head fop fp)) fp)
  --
::
::  fpmod: f(x) mod g(x), gives remainder r of f/g
++  fpmod
  ~/  %fpmod
  |:  [f=`fpoly`one-fpoly g=`fpoly`one-fpoly]
  ^-  fpoly
  ~+
  ::  f - g*q, stripped of leading zeroes
  %-  fpcan
  (fpsub f (fpmul g (fpdiv f g)))
::
::  fpeval: evaluate a polynomial with Horner's method.
++  fpeval
  ~/  %fpeval
  |:  [fp=`fpoly`one-fpoly x=`felt`(lift 1)]
  ^-  felt
  ~+
  ?:  (fp-is-zero fp)  (lift 0)
  ?:  =(len.fp 1)  (~(snag fop fp) 0)
  =/  p  ~(to-poly fop fp)
  =.  p  (flop p)
  =/  res=@  (lift 0)
  |-
  ?~  p    !!
  ?~  t.p
    (fadd (fmul res x) i.p)
  ::  based on p(x) = (...((a_n)x + a_{n-1})x + a_{n-2})x + ... )
  $(res (fadd (fmul res x) i.p), p t.p)
::
::  +fpcompose: given fpolys P(X) and Q(X), compute P(Q(X))
++  fpcompose
  ~/  %fpcompose
  |:  [p=`fpoly`zero-fpoly q=`fpoly`zero-fpoly]
  ^-  fpoly
  ~+
  =.  p  (fpcan p)
  =.  q  (fpcan q)
  ?:  |((fp-is-zero p) (fp-is-zero q))  zero-fpoly
  =-  -<
  %+  roll  (range len.p)
  |=  [n=@ acc=_zero-fpoly q-pow=_one-fpoly]
  :_  (fpmul q-pow q)
  %+  fpadd  acc
  (fpscal (~(snag fop p) n) q-pow)
::
::  construct the constant fpoly f(X)=c
++  fp-c
  |=  c=felt
  ^-  fpoly
  ~+
  (init-fpoly ~[c])
::
::  +fp-decompose
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
++  fp-decompose
  ~/  %fp-decompose
  |=  [p=_one-fpoly d=@]
  ^-  (list fpoly)
  =/  total-deg=@  (fdegree (fpoly-to-list p))
  =/  deg=@
    =/  dvr  (dvr total-deg d)
    ?:(=(q.dvr 0) p.dvr (add p.dvr 1))
  =/  acc=(list (list felt))  (reap d ~)
  =-
    %+  turn  -
    |=  poly=(list felt)
    ?~  poly  zero-fpoly
    (init-fpoly (flop poly))
  %+  roll  (range (add 1 deg))
  |=  [i=@ acc=_acc]
  %+  iturn  acc
  |=  [n=@ l=(list felt)]
  =/  idx  (add (mul i d) n)
  ?:  (gth idx total-deg)  l
  [(~(snag fop p) idx) l]
::
::  +fp-decomposition-eval
::
::  given a decomposition created by +fp-decompose, evaluate it.
::
::  input:
::    n=number of pieces,
::    {h_i(X): 0 <= i < n }
::    c=felt (evaluation point)
::
::  output:
::  h_0(c^n) + X*h_1(c^n) + X^2*h_2(c^n) + ... + x^{n-1)}*h_{n-1}(x^n)
::
++  fp-decomposition-eval
  |=  [n=@ polys=(list fpoly) eval-point=felt]
  ^-  felt
  =/  c  (fpow eval-point n)
  %^  zip-roll  (range n)  polys
  |=  [[i=@ poly=fpoly] acc=_(lift 0)]
  (fadd acc (fmul (fpow eval-point i) (fpeval poly c)))
::
::    specialized fpoly manipulations mostly used by the prover
+|  %prover-math
::  codeword: compute a Reed-Solomon codeword, i.e. evaluate a poly on a domain
++  codeword
  ~/  %codeword
  |=  [fp=fpoly fdomain=fpoly]
  ^-  fpoly
  ?:  =(fdomain zero-fpoly)
    fdomain
  ?:  =(1 len.fdomain)
    (init-fpoly (fpeval fp ~(head fop fdomain))^~)
  =/  half  (div len.fdomain 2)
  =/  lef-zerofier  (zerofier (~(scag fop fdomain) half))
  =/  rig-zerofier  (zerofier (~(slag fop fdomain) half))
  =/  lef
    $(fp (fpmod fp lef-zerofier), fdomain (~(scag fop fdomain) half))
  =/  rig
    $(fp (fpmod fp rig-zerofier), fdomain (~(slag fop fdomain) half))
  (~(weld fop lef) rig)
::
++  zerofier
  ~/  %zerofier
  |=  fdomain=fpoly
  ^-  fpoly
  ~+
  ?:  =(fdomain zero-fpoly)
    fdomain
  ?:  =(1 len.fdomain)
    %-  init-fpoly
    [(fneg ~(head fop fdomain)) (lift 1) ~]
  =/  half  (div len.fdomain 2)
  =/  lef   $(fdomain (~(scag fop fdomain) half))
  =/  rig   $(fdomain (~(slag fop fdomain) half))
  (fpmul lef rig)
::
::  interpolate: compute the poly of minimal degree which evaluates to values on domain
++  interpolate
  ~/  %interpolate
  |=  [fdomain=fpoly fvalues=fpoly]
  ^-  fpoly
  ~+
  ?>  =(len.fdomain len.fvalues)
  ?:  =(fdomain zero-fpoly)
    fdomain
  ?:  =(1 len.fdomain)  fvalues
  =/  half  (div len.fdomain 2)
  =/  half-1  (~(scag fop fdomain) half)
  =/  half-2  (~(slag fop fdomain) half)
  =/  lef-zerofier  (zerofier half-1)
  =/  rig-zerofier  (zerofier half-2)
  =/  lef-offset  (codeword rig-zerofier half-1)
  =/  rig-offset  (codeword lef-zerofier half-2)
  =/  lef-target
    %+  ~(zip fop (~(scag fop fvalues) half))
      lef-offset
    fdiv
  =/  rig-target
    %+  ~(zip fop (~(slag fop fvalues) half))
      rig-offset
    fdiv
  =/  lef-interpolant
    $(fdomain half-1, fvalues lef-target)
  =/  rig-interpolant
    $(fdomain half-2, fvalues rig-target)
  %+  fpadd
    (fpmul lef-interpolant rig-zerofier)
  (fpmul rig-interpolant lef-zerofier)
::
++  test-colinearity
  |=  points=(list (pair felt felt))
  ^-  ?
  ?<  ?|(?=(~ points) ?=(~ t.points))
  =*  x0  p.i.points
  =*  y0  q.i.points
  =*  x1  p.i.t.points
  =*  y1  q.i.t.points
  ~|  "x-coordinates must be distinct"
  ?<  =(x0 x1)
  =/  line=fpoly
    (interpolate (init-fpoly ~[x0 x1]) (init-fpoly ~[y0 y1]))
  =/  bool=?  %.y
  =/  iter  t.t.points
  |-
  ?~  iter
    bool
  %=  $
    iter  t.iter
    bool  ?&  bool
              =((fpeval line p.i.iter) q.i.iter)
          ==
  ==
::  +shift: produces the polynomial q(x) such that p(c*x) = q(x), i.e. q_i = (p_i)*(c^i)
::
::    Usecase:
::    If p is a polynomial you want to evaluate on coset cH of subgroup H, then you can
::    instead evaluate q on H. The value of q on h is that of p on ch: q(h) = p(ch).
++  shift
  ~/  %shift
  |:  [fp=`fpoly`one-fpoly c=`felt`(lift 1)]
  ^-  fpoly
  =/  p  ~(to-poly fop fp)
  =/  power=felt  (lift 1)
  =|  q=poly
  |-
  ?~  p
    (init-fpoly (flop q))
  $(q [(fmul i.p power) q], power (fmul power c), p t.p)
::
++  bp-shift
  ~/  %bp-shift
  |:  [bp=`bpoly`one-bpoly c=`belt`1]
  ^-  bpoly
  =/  p  ~(to-poly bop bp)
  =/  power=belt  1
  =|  q=poly
  |-
  ?~  p
    (init-bpoly (flop q))
  $(q [(bmul i.p power) q], power (bmul power c), p t.p)
::
::
::  +shift-by-unity
::
::  compose a polynomial in eval form over a root of unity with a power of that root of unity. It just
::  has to shift the vector to the left by pow steps and wrap back to the right.
++  shift-by-unity
  ~/  %shift-by-unity
  |=  [fp=fpoly n=@]
  ^-  fpoly
  ?:  |(=(len.fp 0) =(len.fp 1))  fp
  (~(weld fop (~(slag fop fp) n)) (~(scag fop fp) n))
::
++  bp-shift-by-unity
  ~/  %bp-shift-by-unity
  |=  [bp=bpoly n=@]
  ^-  bpoly
  ?:  |(=(len.bp 0) =(len.bp 1))  bp
  (~(weld bop (~(slag bop bp) n)) (~(scag bop bp) n))
::
++  turn-coseword
  ~/  %turn-coseword
  |=  [polys=mary offset=belt order=@]
  ^-  mary
  %-  zing-bpolys
  %+  turn  (range len.array.polys)
  |=  i=@
  =/  bp=bpoly  (~(snag-as-bpoly ave polys) i)
  (bp-coseword bp offset order)
::
::  +coseword: fast evaluation on a coset of a binary subgroup
::
::    Portmanteau of coset and codeword. If we want to evaluate a polynomial p on a coset of
::    a subgroup H, this is the same as evaluating the shifted polynomial q on H. If H is
::    generated by a binary root of unity, this evaluation is the same as an FFT.
::    NOTE: the polynomial being evaluated should have length less than the size of H.
::    This is because an FFT of a polynomial uses a root of unity of order the power of 2
::    which is larger than the length of the polynomial.
::    NOTE: 'order' is the size of H. It suffices for this single number to be our proxy for
::    H because there is a unique subgroup of this size. (Follows from the fact that F* is cyclic.)
++  coseword
  ~/  %coseword
  |=  [p=fpoly offset=felt order=@]
  ^-  fpoly
  ~|  "Order must be a power of 2."
  ?>  =((dis order (dec order)) 0)
  %-  fp-fft
  %-  ~(zero-extend fop (shift p offset))
  (sub order len.p)
::
::  +bp-coseword: coseword over base field
++  bp-coseword
  ~/  %bp-coseword
  |=  [p=bpoly offset=belt order=@]
  ^-  bpoly
  ~|  "Order must be a power of 2."
  ?>  =((dis order (dec order)) 0)
  %-  bp-fft
  %-  ~(zero-extend bop (bp-shift p offset))
  (sub order len.p)
::
::  +intercosate: interpolate a polynomial taking particular values over a binary subgroup coset
::
::    Returns a polynomial p satisfying p(c*w^i) = v_i where w generates a cyclic subgroup of
::    binary order. This is accomplished by first obtaining q = (ifft values), which satisfies
::    q(w^i) = v_i. This is equivalent to q(c^{-1}*(c*w^i)) = v_i so comparing to our desired
::    equation we want p(x) = q(c^{-1}*x); i.e. we need to shift q by c^{-1}.
++  intercosate
  ~/  %intercosate
  |=  [offset=felt order=@ values=fpoly]
  ^-  fpoly
  ~+
  ::  order = |H| is a power of 2
  ?>  =((dis order (dec order)) 0)
  ::  number of values should match the number of points in the coset
  ?>  =(len.values order)
  (shift (fp-ifft values) (finv offset))
::
++  bp-intercosate
  ~/  %bp-intercosate
  |=  [offset=belt order=@ values=bpoly]
  ^-  bpoly
  ~+
  ::  order = |H| is a power of 2
  ?>  =((dis order (dec order)) 0)
  ::  number of values should match the number of points in the coset
  ?>  =(len.values order)
  =/  ifft  (bp-ifft values)
  (bp-shift (bp-ifft values) (binv offset))
::
::
++  interpolate-table
  ~/  %interpolate-table
  |=  [table=mary domain-len=@]
  ^-  mary
  =/  trace=mary  (transpose-bpolys table)
  %-  zing-bpolys
  %+  turn  (range len.array.trace)
  |=  col=@
  ^-  bpoly
  =/  values-old=bpoly  (~(snag-as-bpoly ave trace) col)
  =/  values  (~(zero-extend bop values-old) (sub domain-len len.values-old))
  ?>  =(len.values domain-len)
  ?>  =((dis domain-len (dec domain-len)) 0)
  (bp-intercosate 1 domain-len values)
::
::
++  mp-to-mega
  ~%  %mp-to-mega  +  ~
  |%
  ::  type of a term
  +$  mega-typ  ?(%var %rnd %dyn %con %com)
  ::
  ::  bit flags for type of term
  ::  constant: just a belt constant term
  +$  con-id  %0
  ::  variable: index is index of variable
  +$  var-id  %1
  ::  challenge: index is index in challenge list
  +$  rnd-id  %2
  ::  dynamic terminal: index is index in dynamic list
  +$  dyn-id  %3
  ::  composition: index of result of previous evaluation
  +$  com-id  %4
  ::
  ::  bit length of type
  ++  typ-len  3
  ::  bit length of index
  ++  idx-len  10
  ::  bit length of exponent
  ++  exp-len  30
  ::
  ::  one term inside a monomial. fits in a direct atom.
  +$  mega-term  @ux
  ::
  ++  mega
    |_  term=mega-term
    ::  retrieve type of terminal
    ++  typ
      ^-  mega-typ
      ?+  (cut 0 [0 typ-len] term)  !!
        con-id  %con
        var-id  %var
        rnd-id  %rnd
        dyn-id  %dyn
        com-id  %com
      ==
    ::
    :: retrieve index of terminal
    ++  idx
      ^-  @ud
      (cut 0 [typ-len idx-len] term)
    ::
    :: retrieve exponent of terminal
    ++  exp
      ^-  @ud
      (cut 0 [(add typ-len idx-len) exp-len] term)
    ::
    --
  ::
  ::  assemble a mega term out of a (type, index, exponent) triple
  ++  form-mega
    |=  [typ=mega-typ idx=@ exp=@ud]
    ^-  mega-term
    ?>  (lte (met 0 idx) idx-len)
    ?>  (lte (met 0 exp) exp-len)
    =/  t=@
      ?-  typ
        %con  *con-id
        %var  *var-id
        %rnd  *rnd-id
        %dyn  *dyn-id
        %com  *com-id
      ==
    (can 0 ~[[typ-len t] [idx-len idx] [exp-len exp]])
  ::
  :: dissable a term into its (type, index, exponent) triple
  ++  brek
    |=  ter=mega-term
    ^-  [mega-typ @ @ud]
    :+  ~(typ mega ter)
      ~(idx mega ter)
    ~(exp mega ter)
  ::
  ::  +mp-is-constant: is mp a constant polynomial
  ++  mp-is-constant
    |=  mp=mp-mega
    ^-  ?
    |^
    %+  levy  ~(tap by mp)
    |=  [k=bpoly v=belt]
    (is-monomial-constant k)
    ::
    ++  is-monomial-constant
      |=  monomial=bpoly
      ^-  ?
      %-  levy
      :_  same
      %+  turn  (range len.monomial)
      |=  i=@
      ^-  ?
      =/  ter  (~(snag bop monomial) i)
      !?=(%var ~(typ mega ter))
    --
  ::
  ::  print out an mp-mega
  ++  print-mega
    |=  mp=mp-mega
    ^-  tape
    %-  zing
    =-  (flop acc)
    %+  roll  ~(tap by mp)
    |=  [[k=bpoly v=belt] first=? acc=(list tape)]
    =/  plus  ?:(first "" " + ")
    =/  coeff  ?:(=(1 v) "" "{(scow %ud v)}*")
    =;  str=tape
      :+  %.n
        :(weld plus coeff str)
      acc
    %-  zing
    =-  (flop acc)
    %+  roll  (range len.k)
    |=  [i=@ first=? acc=(list tape)]
    =/  times=tape  ?:(first "" "*")
    =/  ter  (~(snag bop k) i)
    =/  exp
      ?:  =(1 ~(exp mega ter))
        ""
      "^{(scow %ud ~(exp mega ter))}"
    :-  %.n
    :_  acc
    ;:  weld
      times
      (trip ~(typ mega ter))
      (scow %ud ~(idx mega ter))
      exp
    ==
  ::
  ::
  ::  f(x0, x1, ..., xn) = r[i] where r is a result from a previous computation
  ++  mp-com
    |=  i=@
    ^-  mp-mega
    ~+
    (my [[(init-bpoly ~[(form-mega %com i 1)]) 1] ~])
  ::
  ::
  ::  f(x0, x1, ..., xn) = c where c is a belt constant
  ++  mp-c
    |=  c=belt
    ^-  mp-mega
    ~+
    ?>  (based c)
    ?:  =(c 0)
      ~
    (my [[zero-bpoly c] ~])
  ::
  ::  f(x0, x1, ..., x0)=xi where i is the argument
  ++  mp-var
    |=  which-variable=@
    ^-  mp-mega
    ~+
    (my [[(init-bpoly ~[(form-mega %var which-variable 1)]) 1] ~])
  ::
  ::  f(x0, x1, ..., x0) = d where d is the value of the dynamic terminal
  ::  with index dyn
  ++  mp-dyn
    |=  dyn=@
    ^-  mp-mega
    ~+
    (my [[(init-bpoly ~[(form-mega %dyn dyn 1)]) 1] ~])
  ::
  ::  f(x0, x1, ..., x0) = r where r is a random challenge with index rnd
  ++  mp-chal
    |=  rnd=@
    ^-  mp-mega
    ~+
    (my [[(init-bpoly ~[(form-mega %rnd rnd 1)]) 1] ~])
  ::
  ++  mpadd
    ~/  %mpadd
    |=  [f=mp-mega g=mp-mega]
    ^-  mp-mega
    ?:  =(~ f)
      g
    ?:  =(~ g)
      f
    %+  roll  ~(tap by g)
    |=  [[gk=bpoly gv=belt] res=_f]
    =/  fv  (~(get by f) gk)
    ?~  fv
      (~(put by res) gk gv)
    (~(put by res) gk (badd u.fv gv))
  ::
  ++  mpsub
    ~/  %mpsub
    |=  [f=mp-mega g=mp-mega]
    ^-  mp-mega
    ?:  =(~ f)
      (~(run by g) bneg)
    ?:  =(~ g)
      f
    %+  roll  ~(tap by g)
    |=  [[gk=bpoly gv=belt] res=_f]
    =/  fv  (~(get by f) gk)
    ?~  fv
      (~(put by res) gk (bneg gv))
    (~(put by res) gk (bsub u.fv gv))
  ::
  ++  mpscal
    ~/  %mpsub
    |=  [c=belt f=mp-mega]
    ^-  mp-mega
    ?:  =(0 c)
      ~
    ?:  =(~ f)
      f
    (~(run by f) (curr bmul c))
  ::
  ++  mpmul
    ~/  %mpmul
    |=  [f=mp-mega g=mp-mega]
    ^-  mp-mega
    |^
    ?:  |(=(~ f) =(~ g))
      ~
    %+  roll  ~(tap by f)
    |=  [[fk=bpoly fv=belt] acc=(map bpoly belt)]
    %+  roll  ~(tap by g)
    |=  [[gk=bpoly gv=belt] acc=_acc]
    =/  key  (mul-key fk gk)
    =/  val  (bmul fv gv)
    =/  lookup  (~(get by acc) key)
    ?~  lookup
      (~(put by acc) key val)
    (~(put by acc) key (badd u.lookup val))
    ::
    ++  mul-key
      |=  [f=bpoly g=bpoly]
      ^-  bpoly
      ?:  (bp-is-zero f)
        g
      ?:  (bp-is-zero g)
        f
      =/  f-map
        %-  ~(gas by *(map [mega-typ @] @ud))
        %+  turn  (range len.f)
        |=  i=@
        =/  ter  (~(snag bop f) i)
        :-  [~(typ mega ter) ~(idx mega ter)]
        ~(exp mega ter)
      =;  acc-map=(map [mega-typ @] @ud)
        %-  init-bpoly
        %+  turn  ~(tap by acc-map)
        |=  [[typ=mega-typ idx=@] exp=@ud]
        (form-mega typ idx exp)
      %+  roll  (range len.g)
      |=  [j=@ acc=_f-map]
      =/  ter  (~(snag bop g) j)
      =/  exp  (~(get by acc) [~(typ mega ter) ~(idx mega ter)])
      ?~  exp
        (~(put by acc) [~(typ mega ter) ~(idx mega ter)] ~(exp mega ter))
      (~(put by acc) [~(typ mega ter) ~(idx mega ter)] (add ~(exp mega ter) u.exp))
    ::
    --
  ::
  ++  mp-degree
    |=  [f=mp-mega com-map=(map @ @)]
    ^-  @ud
    %-  roll
    :_  max
    %+  turn  ~(tap by f)
    |=  [k=bpoly v=belt]
    ^-  @ud
    ?:  =(v 0)
      0
    %+  roll  (range len.k)
    |=  [i=@ deg=@ud]
    =/  ter  (~(snag bop k) i)
    =/  [typ=mega-typ idx=@ exp=@ud]  (brek ter)
    ?+    typ  deg
        %var
      (add deg exp)
    ::
        %com
      =/  dep-deg  (~(got by com-map) idx)
      %+  add
        deg
      (mul dep-deg exp)
    ==
  ::
  ++  mpeval
    ~/  %mpeval
    |=  $:  field=?(%ext %base)
            mp=mp-mega
            args=bpoly  :: can be bpoly or fpoly
            chals=bpoly
            dyns=bpoly
            com-map=(map @ elt)
        ==
    ^-  elt
    =/  add-op   ?:(=(field %base) badd fadd)
    =/  mul-op   ?:(=(field %base) bmul fmul)
    =/  pow-op   ?:(=(field %base) bpow fpow)
    =/  aop-door   ?:(=(field %base) bop fop)
    =/  lift-op  ?:(=(field %base) |=(v=@ `@ux`v) lift)
    =/  init-zero=@ux  (lift-op 0)
    =/  init-one=@ux  (lift-op 1)
    ?:  =(~ mp)
      init-zero
    %+  roll  ~(tap by mp)
    |=  [[k=bpoly v=belt] acc=_init-zero]
    =/  coeff=@ux  (lift-op v)
    ?:  =(init-zero coeff)
      acc
    %+  add-op  acc
    %+  mul-op  coeff
    %+  roll  (range len.k)
    |=  [i=@ res=_init-one]
    ?:  =(init-zero res)
      init-zero
    %+  mul-op  res
    =/  ter  (~(snag bop k) i)
    =/  [typ=mega-typ idx=@ exp=@ud]  (brek ter)
    ?-  typ
        %var
      %+  pow-op
        (~(snag aop-door args) idx)
      exp
    ::
        %rnd
      %+  pow-op
        (lift-op (~(snag bop chals) idx))
      exp
    ::
        %dyn
      %+  pow-op
        (lift-op (~(snag bop dyns) idx))
      exp
    ::
        %con
      init-one
    ::
        %com
      %+  pow-op
        (~(got by com-map) idx)
      exp
    ==
  --
::
++  mp-degree-mega
  |=  [f=mp-mega com-map=(map @ @)]
  ^-  @
  (mp-degree:mp-to-mega f com-map)
::
++  mp-degree-ultra
  |=  f=mp-ultra
  ^-  (list @)
  ?-    -.f
      %mega
    :~  (mp-degree-mega +.f ~)
    ==
  ::
      %comp
    =;  com-degs=(map @ @)
      %+  turn
        com.f
      |=  mp=mp-mega
      (mp-degree-mega mp com-degs)
    %+  roll
      (range (lent dep.f))
    |=  [i=@ acc=(map @ @)]
    =/  mp  (snag i dep.f)
    %-  ~(put by acc)
    :-  i
    (mp-degree-mega mp ~)
  ==
::
::  +mp-to-graph: multipoly arithmetic in the mp-graph representation
::
::    you can usually convert mpoly arithmetic into mp-graph arithmetic by
::    writing =, mp-to-graph and leaving everything else the same.
++  mp-to-graph
  ~%  %mp-to-graph  +  ~
  |%
  ++  make-variable
    |=  which-variable=@
    ^-  mp-graph
    [%var +<]
  ::
  ++  inv
    |=  a=mp-graph
    ^-  mp-graph
    [%inv a]
  ::
  ++  neg
    |=  a=mp-graph
    ^-  mp-graph
    [%neg a]
  ::
  ++  mpadd
    |=  [a=mp-graph b=mp-graph]
    ^-  mp-graph
    [%addl ~[a b]]
  ::
  ++  mpsub
    |=  [a=mp-graph b=mp-graph]
    ^-  mp-graph
    [%addl ~[a [%scal (bneg 1) b]]]
  ::
  ++  mpmul
    |=  [a=mp-graph b=mp-graph]
    ^-  mp-graph
    [%mul +<]
  ::
  ++  mppow
    |=  [a=mp-graph n=@ud]
    ^-  mp-graph
    [%pow +<]
  ::
  ++  mpscal
    |=  [c=belt f=mp-graph]
    ^-  mp-graph
    [%scal +<]
  ::
  ++  mp-c
    |=  a=belt
    ^-  mp-graph
    [%con a]
  ::
  ++  mp-cb
    |=  a=belt
    ^-  mp-graph
    [%con a]
  ::
  ++  mp-dyn
    |=  d=term
    ^-  mp-graph
    [%dyn d]
  ::
  --
::
++  mpeval-mega
  ~/  %mpeval-mega
  |=  $:  field=?(%ext %base)
          p=mp-mega
          args=bpoly  :: can be bpoly or fpoly
          chals=bpoly
          dyns=bpoly
          com-map=(map @ elt)
      ==
  ^-  elt
  (mpeval:mp-to-mega +<)
::
++  mpeval-ultra
  ~/  %mpeval-ultra
  |=  $:  field=?(%ext %base)
          p=mp-ultra
          args=bpoly  :: can be bpoly or fpoly
          chals=bpoly
          dyns=bpoly
      ==
  ^-  (list elt)
  ?-    -.p
      %mega
    :~  (mpeval-mega field +.p args chals dyns ~)
    ==
  ::
      %comp
    =;  com-map=(map @ elt)
      %+  turn
        com.p
      |=  mp=mp-mega
      (mpeval-mega field mp args chals dyns com-map)
    %+  roll
      (range (lent dep.p))
    |=  [i=@ acc=(map @ elt)]
    =/  mp  (snag i dep.p)
    %-  ~(put by acc)
    :-  i
    (mpeval-mega field mp args chals dyns ~)
  ==
::
::
::  +mp-substitute-ultra
::
::  Handles substitution for %mega and %comp mp-ultra cases. If the multi-poly is a
::  single mp-mega constraint, we just call mp-substitute-mega on it. On the other hand
::  if it is a composition, we must first evaluate its dependencies, collating the
::  indexed results in a map. We then pass the map in as input when we substitute
::  the actual computation.
::
++  mp-substitute-ultra
  ~/  %mp-substitute-ultra
  |=  [p=mp-ultra trace-evals=bpoly height=@ chals=bpoly dyns=bpoly]
  ^-  (list bpoly)
  ?-    -.p
      %mega
    :~  (mp-substitute-mega +.p trace-evals height chals dyns ~)
    ==
  ::
      %comp
    =;  com-map=(map @ bpoly)
      %+  turn
        com.p
      |=  mp=mp-mega
      (mp-substitute-mega mp trace-evals height chals dyns com-map)
    ::
    :: Materialize the dependencies and label them based on order
    %+  roll
      (range (lent dep.p))
    |=  [i=@ acc=(map @ bpoly)]
    =/  mp=mp-mega  (snag i dep.p)
    %-  ~(put by acc)
    :-  i
    (mp-substitute-mega mp trace-evals height chals dyns ~)
  ==
::
::  +mp-substitute-mega: Given a multipoly: sub in the chals, dyns, vars, and composition dependencies:
::
::  For vars, the trace polys: ~[p0(t) p1(t) ... ] are in eval form and we substitute pi(t) for xi.
::
::  The key insight is that multiplication is much faster on polynomials in eval form instead of
::  coefficient form. Calling bpmul will do ntt's on the arguments and an ifft on the result
::  over and over again. Instead we precompute the ntts for all the polynomials and those
::  are the arguments to substitute. Since they're already in the correct form we just compute
::  hadamard products on them, sum up all the terms, and do an ifft to get the result.
::
::  Another optimization is that the polynomials in eval form must be the length of the degree
::  of the final product. Since the max degree of the constraints is 4 (this method has this
::  constraint degree hardcoded for optimization purposes and must be changed by hand
::  if the constraint degree changes), the vectors must be 4*n where n is the height.
::
++  mp-substitute-mega
  ~/  %mp-substitute-mega
  |=  [p=mp-mega trace-evals=bpoly height=@ chals=bpoly dyns=bpoly com-map=(map @ bpoly)]
  ^-  bpoly
  %+  roll  ~(tap by p)
  |=  [[k=bpoly v=belt] acc=_zero-bpoly]
  =/  [poly=bpoly len=@]  [trace-evals (mul 4 height)]
  =/  ones=bpoly  (init-bpoly (reap len 1))
  ?:  =(v 0)  acc
  %+  bpadd  acc
  %+  bpscal  v
  %+  roll  (range len.k)
  |=  [i=@ acc=_ones]
  ^-  bpoly
  =/  ter  (~(snag bop k) i)
  =/  [typ=mega-typ:mp-to-mega idx=@ exp=@ud]
    (brek:mp-to-mega ter)
  ?-  typ
      %var
    =/  var=bpoly  (~(swag bop poly) (mul idx len) len)
    %+  roll  (range exp)
    |=  [i=@ power=_acc]
    (bp-hadamard power var)
  ::
      %rnd
    =/  rnd  (~(snag bop chals) idx)
    (bpscal (bpow rnd exp) acc)
  ::
      %dyn
    =/  dyn  (~(snag bop dyns) idx)
    (bpscal (bpow dyn exp) acc)
  ::
      %con
    acc
  ::
      %com
    =/  com=bpoly  (~(got by com-map) idx)
    %+  roll  (range exp)
    |=  [i=@ power=_acc]
    (bp-hadamard power com)
  ==
::
:: ++  fpdiv-test
::   |=  [p=poly q=poly]
::   ^-  ?
::   =(-:(fpdvr p q) (fpdiv p q))
:: ::
:: ++  fpdvr-test
::   |=  [a=poly b=poly]
::   ^-  ?
::   =/  [q=poly r=poly]  (fpdvr a b)
::   ?&  =(a (fpadd (fpmul-naive b q) r))
::       (lth (degree r) (degree b))
::   ==
::
--  ::ext-field
