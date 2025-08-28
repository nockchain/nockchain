/=  ztd-seven  /common/ztd/seven
=>  ztd-seven
~%  %stark-core  ..tlib  ~
::    stark-core
|%
+|  %types
::  $zerofier-cache: cache from table height -> zerofier
+$  constraint-degrees  [boundary=@ row=@ transition=@ terminal=@ extra=@]
::  $table-to-constraint-degree: a map from table number to maximum constraint degree for that table
+$  table-to-constraint-degree  (map @ constraint-degrees)
::  mp-ultra constraint along with corresponding degrees of the constraints inside
+$  constraint-data  [cs=mp-ultra degs=(list @)]
::  all constraints for one table
+$  constraints
  $:  boundary=(list constraint-data)
      row=(list constraint-data)
      transition=(list constraint-data)
      terminal=(list constraint-data)
      extra=(list constraint-data)
  ==
::  constraint types
+$  constraint-type    ?(%boundary %row %transition %terminal)
::
+$  constraint-counts
  $:  boundary=@
      row=@
      transition=@
      terminal=@
      extra=@
  ==
::
:: preprocessed constraint data
+$  preprocess-data
  $:  cd=table-to-constraint-degree       :: maximum degree of constraints for each table
      constraint-map=(map @ constraints)  :: map from table number -> constraints
      count-map=(map @ constraint-counts) :: map from table number -> constraint-counts
  ==
::
::  versioned constraints
::  version %0 and %1 use the same constraints
+$  preprocess-0-1  [%0 p=preprocess-data]
+$  preprocess-2    [%2 p=preprocess-data]
+$  preprocess
  $:  pre-0-1=preprocess-0-1    :: version %0 and %1 constraints
      pre-2=preprocess-2        :: version %2 constraints
  ==
::
::  $stark-config: prover+verifier parameters unrelated to a particular computation
+$  stark-config
  $:  conf=[log-expand-factor=_6 security-level=_50]
      prep=preprocess
  ==
::
+$  stark-input  =stark-config
::
+|  %cores
++  quot
  =/  util  constraint-util
  =|  tm=table-mary
  ~%  %quot  ..quot  ~
  |%
  ++  height  (height-mary:tlib p.tm)
  ++  omicron
    ::  TODO:  specialized for 2^64 - 2^32 + 1
    ~+((ordered-root height))
  ::
  ++  omicron-domain
    ^-  (list felt)
    ~+
    %+  turn  (range height)
    |=  i=@
    (lift (bpow omicron i))
  --  ::quot
::
+|  %misc
::  +t-order: order terms (table names) by <=, except %jute table is always last
++  t-order
  |=  [a=term b=term]
  ~+  ^-  ?
  ?:  =(b %jute)  %.y
  ?:  =(a %jute)  %.n
  (lth `@`a `@`b)
::
::  +td-order: order table-dats using +t-order
++  td-order
  |=  [a=table-dat b=table-dat]
  ~+  ^-  ?
  (t-order name.p.a name.p.b)
::
::  +tg-order: general ordering arm for lists with head given by table name
++  tg-order
  |=  [a=[name=term *] b=[name=term *]]
  ~+  ^-  ?
  (t-order name.a name.b)
::
::
::    jetted functions used by the stark prover
+|  %jetted-funcs
+$  codeword-commitments
   $:  polys=(list mary)
       codewords=mary
       merk-heap=(pair @ merk-heap:merkle)
   ==
::
++  compute-codeword-commitments
  ~/  %compute-codeword-commitments
  |=  $:  table-marys=(list mary)
          fri-domain-len=@
          total-cols=@
      ==
  ^-  codeword-commitments
  ::
  ::  convert the ext columns to marys
  ::
  ::  think of each mary as a list of the table's columns, interpolated to polynomials
  =/  table-polys=(list mary)
    (compute-table-polys table-marys)
  ::
  ::  this mary is a list of all tables' columns, extended to codewords
  =/  codewords=mary
    (compute-lde table-polys fri-domain-len total-cols)
  ::
  ::  this mary is a list of rows, each row the values of above codewords at a fixed domain elt
  =/  codeword-array=mary
    (transpose-bpolys codewords)
  =/  merk-heap=(pair @ merk-heap:merkle)
    (bp-build-merk-heap:merkle codeword-array)
  [table-polys codeword-array merk-heap]
::
:: interpolate polynomials through table columns
++  compute-table-polys
  |=  tables=(list mary)
  ^-  (list mary)
  %+  turn  tables
  |=  p=mary
  =/  height  (height-mary:tlib p)
  ?:  =(height 0)
    ~|("compute-table-polys: height 0 table detected" !!)
  (interpolate-table p height)
::
++  compute-lde
  ~/  %compute-lde
  |=  $:  table-polys=(list mary)
          fri-domain-len=@
          num-cols=@
      ==
  ^-  mary
  =/  fps=(list mary)
    %+  turn  table-polys
    |=  t=mary
    (turn-coseword t g fri-domain-len)
  =/  res=mary
    :+  step=fri-domain-len
      len=num-cols
    dat=(lsh [6 (mul fri-domain-len num-cols)] 1)
  =;  [@ ret=mary]
    ret
  %+  roll
    fps
  |=  [curr=mary [idx=@ res=_res]]
  ?>  =(step.curr fri-domain-len)
  =/  chunk  (mul step.curr len.array.curr)
  :-  (add idx chunk)
  res(dat.array (sew 6 [idx chunk dat.array.curr] dat.array.res))
::
::
::  the @ is a degree upper bound D_j of the associated composition
::  codeword, and thereby dependent on trace length, i.e.
::  deg(mp constraint)*(trace-len - 1) - deg(zerofier)
+$  constraints-w-deg
  $:  boundary=(list [(list @) mp-ultra])
      row=(list [(list @) mp-ultra])
      transition=(list [(list @) mp-ultra])
      terminal=(list [(list @) mp-ultra])
      extra=(list [(list @) mp-ultra])
  ==
::
::  fri-deg-bound is D-1, where D is the next power of 2 greater than
::  the degree bounds of all composition codewords
++  degree-processing
  |=  [heights=(list @) constraint-map=(map @ constraints) is-extra=?]
  ^-  [fri-deg-bound=@ constraint-w-deg-map=(map @ constraints-w-deg)]
  =-  [(dec (bex (xeb (dec d)))) m]
  %+  roll  (range (lent heights))
  |=  [i=@ d=@ m=(map @ constraints-w-deg)]
  =/  height=@  (snag i heights)
  =/  constraints  (~(got by constraint-map) i)
  =-  :-  :(max d d.bnd d.row d.trn d.trm d.xta)
      (~(put by m) i [c.bnd c.row c.trn c.trm c.xta])
  ::  attach composition degree to each mp & keep a running max of degrees
  ::  divided by boundary, row, transition, terminal
  :*
    ^=  bnd=[c d]
      %^  spin  boundary.constraints  0
      |=  [cd=constraint-data d=@]
      =;  degrees=(list @)
        :-  [degrees cs.cd]
        (roll `(list @)`[d degrees] max)
      %+  turn  degs.cd
      |=  deg=@
      ?:  =(height 1)  0
      (dec (mul deg (dec height)))
  ::
    ^=  row=[c d]
      %^  spin  row.constraints  0
      |=  [cd=constraint-data d=@]
      =;  degrees=(list @)
        :-  [degrees cs.cd]
        (roll `(list @)`[d degrees] max)
      %+  turn  degs.cd
      |=  deg=@
      ?:  ?|(=(height 1) =(deg 1))  0
      (sub (mul deg (dec height)) height)
  ::
    ^=  trn=[c d]
      %^  spin  transition.constraints  0
      |=  [cd=constraint-data d=@]
      =;  degrees=(list @)
        :-  [degrees cs.cd]
        (roll `(list @)`[d degrees] max)
      %+  turn  degs.cd
      |=(@ (mul (dec +<) (dec height)))
  ::
    ^=  trm=[c d]
      %^  spin  terminal.constraints  0
      |=  [cd=constraint-data d=@]
      =;  degrees=(list @)
        :-  [degrees cs.cd]
        (roll `(list @)`[d degrees] max)
      %+  turn  degs.cd
      |=  deg=@
      ?:  =(height 1)  0
      (dec (mul deg (dec height)))
  ::
    ^=  xta=[c d]
      ?.  is-extra  [~ 0]
      %^  spin  extra.constraints  0
      |=  [cd=constraint-data d=@]
      =;  degrees=(list @)
        :-  [degrees cs.cd]
        (roll `(list @)`[d degrees] max)
      %+  turn  degs.cd
      |=  deg=@
      ?:  ?|(=(height 1) =(deg 1))  0
      (sub (mul deg (dec height)) height)
  ==
::
++  compute-composition-poly
  ~/  %compute-composition-poly-hoon
  |=  $:  omicrons=bpoly
          heights=(list @)
          tworow-trace-polys=(list bpoly)
          constraint-map=(map @ constraints)
          constraint-counts=(map @ constraint-counts)
          weights-map=(map @ bpoly)
          challenges=bpoly
          dyn-list=(list bpoly)
          is-extra=?
      ==
  ^-  bpoly
  (do-compute-composition-poly +<)
::
++  do-compute-composition-poly
  ~/  %compute-composition-poly
  |=  $:  omicrons=bpoly
          heights=(list @)
          tworow-trace-polys=(list bpoly)
          constraint-map=(map @ constraints)
          constraint-counts=(map @ constraint-counts)
          weights-map=(map @ bpoly)
          challenges=bpoly
          dyn-list=(list bpoly)
          is-extra=?
      ==
  ^-  bpoly
  =/  max-height=@
    %-  bex  %-  xeb  %-  dec
    (roll heights max)
  =/  dp  (degree-processing heights constraint-map is-extra)
  |^
  =/  boundary-zerofier  (init-bpoly ~[(bneg 1) 1])          ::  f(X)=X-1
  ::
  %+  roll  (range len.omicrons)
  |=  [i=@ acc=_zero-bpoly]
  =/  height=@  (snag i heights)
  =/  omicron  (~(snag bop omicrons) i)
  =/  last-row  (init-bpoly ~[(bneg (binv omicron)) 1])      ::  f(X)=X-g^{-1}
  =/  weights  (~(got by weights-map) i)
  =/  trace  (snag i tworow-trace-polys)
  =/  constraints  (~(got by constraint-w-deg-map.dp) i)
  =/  counts  (~(got by constraint-counts) i)
  =/  dyns  (snag i dyn-list)
  ::
  =/  row-zerofier                                           ::  f(X) = (X^N-1)
    (bpsub (bppow id-bpoly height) one-bpoly)
  ::
  ;:  bpadd
    acc
  ::
    %-  bpdiv
    :_  boundary-zerofier
    %-  process-composition-constraints
    :*  boundary.constraints
        trace
        (~(scag bop weights) (mul 2 boundary.counts))
        dyns
    ==
  ::
    %-  bpdiv
    :_  row-zerofier
    %-  process-composition-constraints
    :*  row.constraints
        trace
      ::
        %+  ~(swag bop weights)
          (mul 2 boundary.counts)
        (mul 2 row.counts)
      ::
        dyns
    ==
  ::
    %-  bpdiv
    ::  note: the transition zerofier = row-zerofier/last-row
    ::  here, we are computing composition-constraints/transition-zerofier
    :_  row-zerofier
    %+  bpmul  last-row
    %-  process-composition-constraints
    :*  transition.constraints
        trace
      ::
        %+  ~(swag bop weights)
          (mul 2 (add boundary.counts row.counts))
        (mul 2 transition.counts)
      ::
        dyns
    ==
  ::
    %-  bpdiv
    :_  last-row
    %-  process-composition-constraints
    :*  terminal.constraints
        trace
      ::
        %+  ~(swag bop weights)
          (mul 2 :(add boundary.counts row.counts transition.counts))
        (mul 2 terminal.counts)
      ::
        dyns
    ==
  ::
    ?.  is-extra  zero-bpoly
    %-  bpdiv
    :_  row-zerofier
    %-  process-composition-constraints
    :*  extra.constraints
        trace
      ::
        %-  ~(slag bop weights)
        %+  mul  2
        ;:  add
          boundary.counts
          row.counts
          transition.counts
          terminal.counts
        ==
      ::
        dyns
    ==
  ::
  ==
  ::
  ++  process-composition-constraints
    |=  $:  constraints=(list [(list @) mp-ultra])
            trace=bpoly
            weights=bpoly
            dyns=bpoly
        ==
    =-  (bpcan acc)
    %+  roll  constraints
    |=  [[degs=(list @) mp=mp-ultra] [idx=@ acc=_zero-bpoly]]
    ::
    ::  mp-substitute-ultra returns a list because the %comp
    ::  constraint type can contain multiple mp-mega constraints.
    ::
    =/  comps=(list bpoly)
      (mp-substitute-ultra mp trace max-height challenges dyns)
    %+  roll
      (zip-up degs comps)
    |=  [[deg=@ comp=bpoly] [idx=_idx acc=_acc]]
    :-  +(idx)
    ::
    ::  Each constraint corresponds to two weights: alpha and beta. The verifier
    ::  samples 2*num_constraints random values and we assume that the alpha
    ::  and beta weights for a given constraint are situated next to each other
    ::  in the array.
    ::
    =/  alpha  (~(snag bop weights) (mul 2 idx))
    =/  beta   (~(snag bop weights) (add 1 (mul 2 idx)))
    ::~&  alpha+alpha
    ::~&  beta+beta
    ::
    ::  adjust degree up to fri-deg-bound.
    ::  if fri-deg-bound is D-1 then we construct:
    ::  p(x)*(α*X^{D-1-D_j} + β)
    ::  which will make the polynomial exactly degree D-1 which is what we want.
    =/  comp-coeff  (bp-ifft comp)
    %+  bpadd  acc
    %+  bpadd
      (bpscal beta comp-coeff)
    %-  %~  weld  bop
        (init-bpoly (reap (sub fri-deg-bound.dp deg) 0))
    (bpscal alpha comp-coeff)
  --
::
:: compute the DEEP Composition Polynomial
++  compute-deep
  ~/  %compute-deep
  |=  $:  trace-polys=(list mary)
          trace-openings=fpoly
          composition-pieces=(list fpoly)
          composition-piece-openings=fpoly
          weights=fpoly
          omicrons=fpoly
          deep-challenge=felt
          comp-eval-point=felt
      ==
  |^  ^-  fpoly
  =/  [acc=fpoly num=@]
    %^  zip-roll  (range (lent trace-polys))  trace-polys
    |=  [[i=@ p=mary] acc=_zero-fpoly num=@]
    =/  lis=(list fpoly)
      %+  turn  (range len.array.p)
      |=  i=@
      (bpoly-to-fpoly (~(snag-as-bpoly ave p) i))
    =/  omicron  (~(snag fop omicrons) i)
    =/  [first-row=fpoly num=@]    :: first row:  f(x)-f(Z)/x-Z
      %-  weighted-linear-combo
      :*  lis
          trace-openings
          num
          (fp-c deep-challenge)
          weights
      ==
    =/  [second-row=fpoly num=@]   :: second row:  f(x)-f(gZ)/x-gZ
      %-  weighted-linear-combo
      :*  lis
          trace-openings
          num
          (fp-c (fmul omicron deep-challenge))
          weights
      ==
    :_  num
    :(fpadd acc first-row second-row)
  ::
  ::  do the same thing for the second composition poly evals
  =/  [acc=fpoly num=@]
    %^  zip-roll  (range (lent trace-polys))  trace-polys
    |=  [[i=@ p=mary] acc=_acc num=_num]
    =/  lis=(list fpoly)
      %+  turn  (range len.array.p)
      |=  i=@
      (bpoly-to-fpoly (~(snag-as-bpoly ave p) i))
    =/  omicron  (~(snag fop omicrons) i)
    ::  add new composition poly
    =/  [new-first-row=fpoly num=@]    :: first row:  f(x)-f(Z)/x-Z
      %-  weighted-linear-combo
      :*  lis
          trace-openings
          num
          (fp-c comp-eval-point)
          weights
      ==
    ::  second row
    =/  [new-second-row=fpoly num=@]   :: second row:  f(x)-f(gZ)/x-gZ
      %-  weighted-linear-combo
      :*  lis
          trace-openings
          num
          (fp-c (fmul omicron comp-eval-point))
          weights
      ==
    :_  num
    :(fpadd acc new-first-row new-second-row)
  ::
  =/  [pieces=fpoly @]
    %-  weighted-linear-combo
    :*  composition-pieces
        composition-piece-openings
        0
        (fp-c (fpow deep-challenge (lent composition-pieces))) :: f(X)=X^D
        (~(slag fop weights) num)
    ==
  (fpadd acc pieces)
  ::
  ++  weighted-linear-combo
    |=  [polys=(list fpoly) openings=fpoly idx=@ x-poly=fpoly weights=fpoly]
    ^-  [fpoly @]
    =-  [acc num]
    %+  roll  polys
    |=  [poly=fpoly acc=_zero-fpoly num=_idx]
    :_  +(num)
    %+  fpadd  acc
    %+  fpscal  (~(snag fop weights) num)
    %+  fpdiv
      (fpsub poly (fp-c (~(snag fop openings) num)))
    (fpsub id-fpoly x-poly)
  --
::
::  +precompute-ntts
::
::
++  precompute-ntts
  ~/  %precompute-ntts
  |=  [polys=mary height=@ ntt-len=@]
  ^-  bpoly
  %-  need
  =/  new-len  (mul height ntt-len)
  %+  roll  (range len.array.polys)
  |=  [i=@ acc=(unit bpoly)]
  =/  p=bpoly  (~(snag-as-bpoly ave polys) i)
  =/  fft=bpoly
    (bp-fft (~(zero-extend bop p) (sub new-len len.p)))
  ?~  acc  (some fft)
  (some (~(weld bop u.acc) fft))
::
--  ::stark-core
::
=>
::    fock-core
~%  %fock-core  ..constraint-degrees  ~
|%
+|  %cores
++  fock  ::  /lib/fock
  =/  util  constraint-util
  =,  chal
  =>
  ~%  %fock  ..fock  ~
  |%
  ++  indirect-to-bits
    ~/  %indirect-to-bits
    |=  axi=@
    ^-  (list ?)
    =|  lis=(list ?)
    |-
    ?:  =(1 axi)  lis
    =/  hav  (dvr axi 2)
    $(axi p.hav, lis [=(q.hav 0) lis])
  ::
  ++  build-compute-queue
    ~/  %build-compute-queue
    |=  [lis=(list *) alf=pelt]
    ^-  compute-queue
    %+  turn  lis
    |=  t=*
    (build-tree-data:constraint-util t alf)
  ::
  ++  build-jute-list
    ~/  %build-jute-list
    |=  [lis=(list [@tas * *]) a=pelt b=pelt c=pelt alf=pelt]
    ^-  (list jute-data)
    %+  turn  lis
    |=  [name=@tas sam=* prod=*]
    :+  name
      (build-tree-data:constraint-util sam alf)
    (build-tree-data:constraint-util prod alf)
  ::
  ::  +noun-get-zero-mults: computes how many times a noun shows up in the zero table
  ::
  ::    utilized by the noun table to get %ext-mul and the exponent table to get the
  ::    correct exponent multiset multiplicities. See the leading comment in
  ::    table/noun.hoon for more information about what this is computing.
  ++  noun-get-zero-mults
    ~/  %noun-get-zero-mults
    |=  ret=fock-return
    ^-  (map tree=* m=@)
    =|  mult=(map tree=* m=@)
    %+  roll  ~(tap by zeroes.ret)
    |=  [[subject=* data=(map [axis=* new-subj=*] count=@)] mult=(map tree=* m=@)]
    %+  roll  ~(tap by data)
    |=  [[[axis=* new-subj=*] count=@] mult=_mult]
    ?>  ?=(belt axis)
    =-  mult
    ::
    %+  roll  (path-to-axis:shape axis)
    |=  [dir=belt tree=_subject new-tree=_new-subj mult=_mult]
    |^
    =/  child-tree      ?:(=(dir 0) -:tree +:tree)
    =/  new-child-tree  ?:(=(dir 0) -:new-tree +:new-tree)
    ::
    =.  mult  (put-tree mult -:tree)
    =.  mult  (put-tree mult +:tree)
    =.  mult  (put-tree mult -:new-tree)
    =.  mult  (put-tree mult +:new-tree)
    ::
    [child-tree new-child-tree mult]
    ::
    ++  put-tree
      |=  [mult=(map tree=* m=@) tree=*]
      (~(put by mult) tree +((~(gut by mult) tree 0)))
    --  ::+noun-get-zero-mults
  --
  ::
  =,  util
  ~%  %inner-fock  ..indirect-to-bits  ~
  |%
  ::
  ::  The natural way to interpret nock is in depth-first order, but the compute table needs
  ::  the data in breadth-first order. The solution is that axes are a breadth-first ordering
  ::  of a tree. So we do a normal DFS interpreter but tag each node in the "tree of formulae"
  ::  with its axis. Then sort by the axes to get them in BFS and we're done.
  ::
  ::  One detail is that nock 2 has 3 subformulaes (you have to count the outer eval itself)
  ::  and so the tree must be ternary instead of binary. So the axes are 3n, 3n+1, and 3n+2
  ::  for the children.
  ::
  ++  fink
    |=  sam=^
    ^-  (pair * fock-return)
    ::
    |^  ^-  (pair * fock-return)
    =/  data  (interpret 1 -.sam +.sam *interpret-data)
    =/  sorted-data=(list formula-data)
      %+  sort  formulae.q.data
      |=  [a=formula-data b=formula-data]
      (lth axis.a axis.b)
    =/  queue=(list *)
      %-  zing
      %-  flop
      %+  roll  sorted-data
      |=  [item=formula-data acc=(list (list *))]
      [extra-data.item ~[s.item f.item p.item] acc]
    =|  ret=fock-return
    :-  p.data
    %_  ret
      s  -.sam
      f  +.sam
      queue  queue
      zeroes  zeroes.q.data
      decodes  decodes.q.data
    ==
    ::
    ::  extra-data is extra nouns that the compute table needs for a
    ::  particular nock opcode. Usually the products of subformulae,
    ::  but it varies by opcode.
    +$  formula-data  [axis=@ s=* f=* p=* extra-data=(list *)]
    +$  interpret-data
      $:  zeroes=zero-map
          decodes=decode-map
          formulae=(list formula-data)
      ==
    ::
    ++  interpret
      |=  [axis=@ s=* f=* acc=interpret-data]
      ^-  (pair * interpret-data)
      ?@  f  !!
      ?+  f  ~|(bad-formula+f !!)
          [^ *]
        =/  left  (interpret (mul 3 axis) s -.f acc)
        =/  right  (interpret +((mul 3 axis)) s +.f q.left)
        =/  prod  [p.left p.right]
        =/  acc  q.right
        =.  decodes.acc
          %+  record-cons  decodes.acc
          [f -.f +.f]
        =.  formulae.acc
          [[axis s f prod ~[p.left p.right]] formulae.acc]
        :-  prod
        acc
      ::
          [%0 axis=*]
        =/  prod  (need (frag axis.f s))
        =.  decodes.acc
          %+  record-cons  decodes.acc
          [f %0 axis.f]
        =.  zeroes.acc
          %+  record  zeroes.acc
          [s axis.f s]
        =.  formulae.acc
          [[axis s f prod ~] formulae.acc]
        :-  prod
        acc
      ::
          [%1 constant=*]
        =.  decodes.acc
          %+  record-cons  decodes.acc
          [f %1 constant.f]
        =.  formulae.acc
          [[axis s f constant.f ~] formulae.acc]
        :-  constant.f
        acc
      ::
          [%2 subject=* formula=*]
        =/  sf1  (interpret (mul 3 axis) s subject.f acc)
        =/  sf2  (interpret +((mul 3 axis)) s formula.f q.sf1)
        =/  sf3  (interpret (add 2 (mul 3 axis)) p.sf1 p.sf2 q.sf2)
        =/  acc  q.sf3
        =.  decodes.acc
          %+  record-cons  decodes.acc
          [f %2 [subject.f formula.f]]
        =.  decodes.acc
          %+  record-cons  decodes.acc
          [[subject.f formula.f] subject.f formula.f]
        =.  formulae.acc
          [[axis s f p.sf3 ~[p.sf1 p.sf2]] formulae.acc]
        :-  p.sf3
        acc
      ::
          [%3 argument=*]
        =/  arg  (interpret (mul 3 axis) s argument.f acc)
        =/  prod  .?(p.arg)
        =/  acc  q.arg
        =.  decodes.acc
          %+  record-cons  decodes.acc
          [f %3 argument.f]
        =.  formulae.acc
          [[axis s f prod ~[p.arg]] formulae.acc]
        :-  prod
        acc
      ::
          [%4 argument=*]
        =/  arg  (interpret (mul 3 axis) s argument.f acc)
        ?^  p.arg  !!
        =/  prod  .+(p.arg)
        =/  acc  q.arg
        =.  decodes.acc
          %+  record-cons  decodes.acc
          [f %4 argument.f]
        =.  formulae.acc
          [[axis s f prod ~[p.arg]] formulae.acc]
        :-  prod
        acc
      ::
          [%5 a=* b=*]
        =/  a  (interpret (mul 3 axis) s a.f acc)
        =/  b  (interpret +((mul 3 axis)) s b.f q.a)
        =/  prod  .=(p.a p.b)
        =/  acc  q.b
        =.  decodes.acc
          %+  record-cons  decodes.acc
          [f %5 [a.f b.f]]
        =.  decodes.acc
          %+  record-cons  decodes.acc
          [[a.f b.f] a.f b.f]
        =.  formulae.acc
          [[axis s f prod ~[p.a p.b]] formulae.acc]
        :-  prod
        acc
      ::
          [%6 test=* yes=* no=*]
        =/  test  (interpret +((mul 3 axis)) s test.f acc)
        ?>  ?=(? p.test)
        =/  sf
          ?-  p.test
              %.y  yes.f
              %.n  no.f
          ==
        =/  prod  (interpret (mul 3 axis) s sf q.test)
        =/  acc  q.prod
        =.  decodes.acc
          %+  record-cons  decodes.acc
          [f %6 [test.f yes.f no.f]]
        =.  decodes.acc
          %+  record-cons  decodes.acc
          :+  [test.f yes.f no.f]
            test.f
          [yes.f no.f]
        =.  decodes.acc
          %+  record-cons  decodes.acc
          :+  [yes.f no.f]
            yes.f
          no.f
        =.  formulae.acc
          [[axis s f p.prod ~[sf p.prod p.test]] formulae.acc]
        :-  p.prod
        acc
      ::
          [%7 subject=* next=*]
        =/  sub  (interpret +((mul 3 axis)) s subject.f acc)
        =/  prod  (interpret (mul 3 axis) p.sub next.f q.sub)
        =/  acc  q.prod
        =.  decodes.acc
          %+  record-cons  decodes.acc
          [f %7 [subject.f next.f]]
        =.  decodes.acc
          %+  record-cons  decodes.acc
          [[subject.f next.f] subject.f next.f]
        =.  formulae.acc
          [[axis s f p.prod ~[p.sub]] formulae.acc]
        :-  p.prod
        acc
      ::
          [%8 head=* next=*]
        =/  head  (interpret +((mul 3 axis)) s head.f acc)
        =/  prod  (interpret (mul 3 axis) [p.head s] next.f q.head)
        =/  acc  q.prod
        =.  decodes.acc
          %+  record-cons  decodes.acc
          [f %8 [head.f next.f]]
        =.  decodes.acc
          %+  record-cons  decodes.acc
          [[head.f next.f] head.f next.f]
        =.  formulae.acc
          [[axis s f p.prod ~[[p.head s] p.head]] formulae.acc]
        :-  p.prod
        acc
      ::
          [%9 axis=* core=*]
        !!
      ::
          [%10 [axis=@ value=*] target=*]
        !!
      ::
          [%11 tag=@ next=*]
        !!
      ::
          [%11 [tag=@ clue=*] next=*]
        !!
      ==
    ++  edit
      |=  [axis=@ target=* value=*]
      ^-  (unit)
      ?:  =(1 axis)  `value
      ?@  target  ~
      =/  pick  (cap axis)
      =/  mutant
        %=  $
          axis    (mas axis)
          target  ?-(pick %2 -.target, %3 +.target)
        ==
      ?~  mutant  ~
      ?-  pick
        %2  `[u.mutant +.target]
        %3  `[-.target u.mutant]
      ==
    ::
    ++  record-all
      |=  [zeroes=zero-map zs=(list [subject=* axis=* new-subject=*])]
      ^-  zero-map
      %+  roll  zs
      |=  [z=[subject=* axis=* new-subject=*] new-zeroes=_zeroes]
      (record new-zeroes z)
    ::
    ++  record
      |=  [zeroes=zero-map subject=* axis=* new-subject=*]
      ^-  zero-map
      ?~  rec=(~(get by zeroes) subject)
        %+  ~(put by zeroes)
          subject
        (~(put by *(map [* *] @)) [axis new-subject] 1)
      ?~  mem=(~(get by u.rec) [axis new-subject])
        %+  ~(put by zeroes)
          subject
        (~(put by u.rec) [axis new-subject] 1)
      %+  ~(put by zeroes)
        subject
      (~(put by u.rec) [axis new-subject] +(u.mem))
    ::
    ++  record-cons
      |=  [decodes=decode-map formula=* head=* tail=*]
      ^-  decode-map
      %-  ~(put by decodes)
      :-  [formula head tail]
      .+  (~(gut by decodes) [formula head tail] 0)
    ::
    ++  frag
      |=  [axis=* noun=*]
      ^-  (unit)
      |^
      ?@  axis  (frag-atom axis noun)
      ~|(%axis-is-too-big !!)
      ::  TODO actually support the cell case
      ::?>  ?=(@ -.axis)
      ::$(axis +.axis, noun (need (frag-atom -.axis noun)))
      ::
      ++  frag-atom
        |=  [axis=@ noun=*]
        ^-  (unit)
        ?:  =(0 axis)  ~
        |-  ^-  (unit)
        ?:  =(1 axis)  `noun
        ?@  noun  ~
        =/  pick  (cap axis)
        %=  $
          axis  (mas axis)
          noun  ?-(pick %2 -.noun, %3 +.noun)
        ==
      --
    --
  --  ::fock
--
~%  %pow  ..fock  ~
|%
++  pow-len  `@`64
::
::  +puzzle-nock: powork puzzle
::
++  puzzle-nock
  ~/  %puzzle-nock
  |=  [block-commitment=noun-digest:tip5 nonce=noun-digest:tip5 length=@]
  ^-  [* *]
  =+  [a b c d e]=block-commitment
  =+  [f g h i j]=nonce
  =/  sponge  (new:sponge:tip5)
  =.  sponge  (absorb:sponge `(list belt)`[a b c d e f g h i j ~])
  =/  rng
    (new:tog:tip5 sponge:sponge)
  =^  belts-list  rng  (belts:rng length)
  =/  subj  (gen-tree belts-list)
  =/  form  (powork length)
  [subj form]
::
++  powork
  ~/  %powork
  |=  n=@
  ^-  nock
  =/  start  n
  =/  form=nock  [%1 0]
  %+  roll  (gulf 0 (dec n))
  |=  [i=@ nok=_form]
  =/  hed  (add start i)
  :-  [%6 [%3 %0 hed] [%0 0] [%0 hed]]
  nok
::
++  gen-tree
  ~/  %gen-tree
  |=  leaves=(list @)
  ^-  *
  ?:  ?=([@ ~] leaves)
    i.leaves
  =/  split-leaves  (split:shape (div (lent leaves) 2) leaves)
  :-  $(leaves -:split-leaves)
  $(leaves +:split-leaves)
::
--  ::  %pow
::
~%  %stark-engine  ..puzzle-nock  ~
::    stark-engine
|%
::
::  This is a dummy arm which is only here so lib/stark/prover.hoon can use it as its parent core.
::  Without it, jets won't work in that file.
++  stark-engine-jet-hook
  ~/  %stark-engine-jet-hook
  |=  n=@
  !!
::
++  stark-engine
  =/  util  constraint-util
  =,  mp-to-graph
  ~%  %stark-door  ..stark-engine  ~
  |_  stark-input
  ++  num-challenges     num-chals:chal
  ::  inverse of the rate of the RS code
  ++  expand-factor      (bex log-expand-factor.conf.stark-config)
  ::  Number of spot checks the verifier performs per round to check the folding
  ++  num-spot-checks    (div security-level.conf.stark-config log-expand-factor.conf.stark-config)
  ::  offset for the coset used for FRI
  ++  generator          7
  ::  size of each FRI fold step. codeword C_i is 1/fri-folding-deg the size of C_{i-i}
  ::  must be a power of 2
  ++  fri-folding-deg    8
  ::
  ++  calc
    ~/  %calc
    |_  [heights=(list @) cd=table-to-constraint-degree]
    ::
    ++  omega           ^~((ordered-root fri-domain-len))
    ::
    :: fri domain length should be the fri expand factor times the max table size
    :: (rounded to a power of 2)
    ++  fri-domain-len
      ~+
      =/  max-padded-height
        %-  bex  %-  xeb  %-  dec
        (roll heights max)
      (mul max-padded-height expand-factor)
    ::
    ++  fri
      ~+
      ~(. fri-door generator omega fri-domain-len expand-factor num-spot-checks fri-folding-deg)
    ::
    ++  max-constraint-degree
      ~/  %max-constraint-degree
      |=  cd=constraint-degrees
      ^-  @
      (max (max (max [boundary transition]:cd) row.cd) terminal.cd)
    ::
    ++  max-degree  $:do-max-degree
    ::
    ++  do-max-degree
      ~%  %max-degree  +  ~
      |.
      ~+
      %-  dec  %-  bex  %-  xeb  %-  dec
      %^  zip-roll  (range (lent heights))  heights
      |=  [[i=@ h=@] d=_5]
      =/  deg  (max-constraint-degree (~(got by cd) i))
      %+  max  d
      =/  trace-degree  h
      (sub (mul deg trace-degree) (dec h))
    --
  ::
  ++  compute-constraint-degree
    ~/  %compute-constraint-degree
    |=  [funcs=verifier-funcs maybe-jutes=(unit jute-map)]
    ^-  constraint-degrees
    =/  jutes=jute-map  ?^(maybe-jutes (need maybe-jutes) *jute-map)
    =-  [(snag 0 -) (snag 1 -) (snag 2 -) (snag 3 -) (snag 4 -)]
    %+  turn
      :~  (unlabel-constraints:util boundary-constraints:funcs)
          (unlabel-constraints:util row-constraints:funcs)
          (unlabel-constraints:util transition-constraints:funcs)
          (unlabel-constraints:util terminal-constraints:funcs)
          (unlabel-constraints:util extra-constraints:funcs)
      ==
    |=  l=(list mp-ultra)
    %+  roll
      l
    |=  [constraint=mp-ultra d=@]
    %+  roll
      (mp-degree-ultra constraint)
    |=  [a=@ d=_d]
    (max d a)
  ::
  ++  get-max-constraint-degree
    ~/  %get-max-constraint-degree
    |=  tab=table-to-constraint-degree
    ~+
    ^-  @
    %+  roll  ~(val by tab)
    |=  [cd=constraint-degrees acc=@]
    (max acc (max boundary.cd (max row.cd (max [transition terminal]:cd))))
  --  ::stark-engine
--
