/=  compute-table-v0-v1  /common/v0-v1/table/prover/compute
/=  compute-table-v2  /common/v2/table/prover/compute
/=  memory-table-v0-v1   /common/v0-v1/table/prover/memory
/=  memory-table-v2   /common/v2/table/prover/memory
/=  *  /common/zeke
/=  nock-common-v0-v1  /common/v0-v1/nock-common
/=  nock-common-v2     /common/v2/nock-common
::
=>  :*  stark-engine
        nock-common-v0-v1=nock-common-v0-v1
        nock-common-v2=nock-common-v2
        compute-table-v0-v1=compute-table-v0-v1
        compute-table-v2=compute-table-v2
        memory-table-v0-v1=memory-table-v0-v1
        memory-table-v2=memory-table-v2
    ==
~%  %stark-prover  ..stark-engine-jet-hook  ~
|%
+$  prover-input
  $%  $:  version=%0
          header=noun-digest:tip5
          nonce=noun-digest:tip5
          pow-len=@
      ==
  ::
      $:  version=%1
          header=noun-digest:tip5
          nonce=noun-digest:tip5
          pow-len=@
      ==
  ::
      $:  version=%2
          header=noun-digest:tip5
          nonce=noun-digest:tip5
          pow-len=@
      ==
  ::
      $:  version=%3
          header=noun-digest:tip5
          nonce=noun-digest:tip5
          pow-len=@
      ==
  ==
::
+$  prove-result  (each =proof err=prove-err)
+$  prove-err     $%([%too-big heights=(list @)] [%invalid-stream ~])
+$  prover-output    [=proof deep-codeword=fpoly]
+$  proof-snapshot
  $:  format=@
      ext=codeword-commitments
      =proof
      num-tables=@
      base=codeword-commitments
      tables=(list table-mary)
      chals-rd1=(list belt)
      s=*
      f=*
      return=fock-return
      heights=(list @)
      num-extra-constraints=@
      version=proof-version
      constraint-map=(map @ constraints)
      count-map=(map @ constraint-counts)
  ==
:::
+$  proof-stream-range
  $:  start=@
      end=(unit @)
  ==
:::
+$  proof-stream-context
  $:  total=@
      digest=noun-digest:tip5
  ==
:::
+$  proof-stream-window
  $:  format=@
      version=proof-version
      range=proof-stream-range
      objects=proof-objects
      context=proof-stream-context
  ==
:::
+$  proof-stream-window-result
  (each =proof-stream-window err=prove-err)
:::
::
+$  proof-work
  $:  ext=codeword-commitments
      =proof
      num-tables=@
      base=codeword-commitments
      tables=(list table-dat)
      chals-rd1=(list belt)
      heights=(list @)
      fri-domain-len=@
      num-extra-constraints=@
  ==
::
::
::  +prove: prove the Nock computation [s f]
++  prove
  ~/  %prove
  |=  prover-input
  ^-  prove-result
  =/  [s=* f=*]  (puzzle-nock header nonce pow-len)
  =/  [prod=* return=fock-return]  (fink:fock [s f])
  =/  nock-common=_nock-common-v0-v1
    ?-  version
      %0  nock-common-v0-v1
      %1  nock-common-v0-v1
      %2  nock-common-v2
      %3  nock-common-v2
    ==
  =/  compute-funcs=table-funcs
    ?-  version
      %0  funcs:compute-table-v0-v1
      %1  funcs:compute-table-v0-v1
      %2  funcs:compute-table-v2
      %3  funcs:compute-table-v2
    ==
  =/  compute-common=static-table-common
    ?-  version
      %0  static:common:compute-table-v0-v1
      %1  static:common:compute-table-v0-v1
      %2  static:common:compute-table-v2
      %3  static:common:compute-table-v2
    ==
  =/  memory-funcs=table-funcs
    ?-  version
      %0  funcs:memory-table-v0-v1
      %1  funcs:memory-table-v0-v1
      %2  funcs:memory-table-v2
      %3  funcs:memory-table-v2
    ==
  =/  memory-common=static-table-common
    ?-  version
      %0  static:common:memory-table-v0-v1
      %1  static:common:memory-table-v0-v1
      %2  static:common:memory-table-v2
      %3  static:common:memory-table-v2
    ==
  =/  pre=preprocess-data
    ?-  version
      %0  p.pre-0-1.prep.stark-config
      %1  p.pre-0-1.prep.stark-config
      %2  p.pre-2.prep.stark-config
      %3  p.pre-2.prep.stark-config
    ==
  %-  %~  generate-proof
        prove-door
      :*  nock-common
          compute-funcs
          compute-common
          memory-funcs
          memory-common
          pre
        ==
  [version header nonce pow-len s f prod return]
::
++  snapshot
  ~/  %snapshot
  |=  prover-input
  ^-  proof-snapshot
  =/  [s=* f=*]  (puzzle-nock header nonce pow-len)
  =/  [prod=* return=fock-return]  (fink:fock [s f])
  =/  nock-common=_nock-common-v0-v1
    ?-  version
      %0  nock-common-v0-v1
      %1  nock-common-v0-v1
      %2  nock-common-v2
      %3  nock-common-v2
    ==
  =/  compute-funcs=table-funcs
    ?-  version
      %0  funcs:compute-table-v0-v1
      %1  funcs:compute-table-v0-v1
      %2  funcs:compute-table-v2
      %3  funcs:compute-table-v2
    ==
  =/  compute-common=static-table-common
    ?-  version
      %0  static:common:compute-table-v0-v1
      %1  static:common:compute-table-v0-v1
      %2  static:common:compute-table-v2
      %3  static:common:compute-table-v2
    ==
  =/  memory-funcs=table-funcs
    ?-  version
      %0  funcs:memory-table-v0-v1
      %1  funcs:memory-table-v0-v1
      %2  funcs:memory-table-v2
      %3  funcs:memory-table-v2
    ==
  =/  memory-common=static-table-common
    ?-  version
      %0  static:common:memory-table-v0-v1
      %1  static:common:memory-table-v0-v1
      %2  static:common:memory-table-v2
      %3  static:common:memory-table-v2
    ==
  =/  pre=preprocess-data
    ?-  version
      %0  p.pre-0-1.prep.stark-config
      %1  p.pre-0-1.prep.stark-config
      %2  p.pre-2.prep.stark-config
      %3  p.pre-2.prep.stark-config
    ==
  %-  %~  make-proof-snapshot
        prove-door
      :*  nock-common
          compute-funcs
          compute-common
          memory-funcs
          memory-common
          pre
        ==
  [version header nonce pow-len s f prod return]
::
::
++  make-proof-stream-window
  |=  [input=prover-input range=proof-stream-range]
  ^-  proof-stream-window-result
  =/  res=prove-result  (prove input)
  ?-  -.res
    %|  [%| p.res]
    %&  [%& (stream-window p.res range)]
  ==
::
:::
++  assemble-proof-stream
  |=  windows=(list proof-stream-window)
  ^-  prove-result
  ?~  windows  [%| [%invalid-stream ~]]
  =/  first=proof-stream-window  i.windows
  ?.  =(format.first 0)  [%| [%invalid-stream ~]]
  =/  ctx=proof-stream-context  context.first
  =/  version=proof-version  version.first
  =/  expected=@  0
  =/  objects=proof-objects  ~
  =/  remaining=(list proof-stream-window)  windows
  |-  ^-  prove-result
  ?~  remaining
    ?.  =(expected total.ctx)  [%| [%invalid-stream ~]]
    =/  proof=proof
      ?-  version
        %0  [%0 objects ~ 0]
        %1  [%1 objects ~ 0]
        %2  [%2 objects ~ 0]
        %3  [%3 objects ~ 0]
      ==
    ?.  =(digest.ctx (hash-proof proof))  [%| [%invalid-stream ~]]
    [%& proof]
  =/  win=proof-stream-window  i.remaining
  ?.  =(format.win 0)  [%| [%invalid-stream ~]]
  ?.  =(version.win version)  [%| [%invalid-stream ~]]
  ?.  =(context.win ctx)  [%| [%invalid-stream ~]]
  ?.  =(start.range.win expected)  [%| [%invalid-stream ~]]
  =/  end=@  ?~(end.range.win total.ctx u.end.range.win)
  ?.  =(end (add start.range.win (lent objects.win)))  [%| [%invalid-stream ~]]
  $(remaining t.remaining, expected end, objects (weld objects objects.win))
:::
++  assemble-proof-continuation
  |=  [snapshot=proof-snapshot context=proof-stream-context windows=(list proof-stream-window)]
  ^-  prove-result
  ?.  =(format.snapshot 0)  [%| [%invalid-stream ~]]
  =/  p=proof  proof.snapshot
  =/  prefix-len=@  (lent objects.p)
  =/  prefix=proof-stream-window
    :*  0
        version.snapshot
        [0 (some prefix-len)]
        objects.p
        context
    ==
  (assemble-proof-stream [prefix windows])
::
++  stream-window
  |=  [p=proof range=proof-stream-range]
  ^-  proof-stream-window
  =/  total=@  (lent objects.p)
  =/  end=@  ?~(end.range total u.end.range)
  ?>  ?&  (lte start.range end)
          (lte end total)
      ==
  =/  count=@  (sub end start.range)
  :*  0
      version.p
      range
      (scag count (slag start.range objects.p))
      [total (hash-proof p)]
  ==
++  prove-door
  ~/  %prove-door
  |_  $:  nock-common=_nock-common-v0-v1
          compute-funcs=table-funcs
          compute-common=static-table-common
          memory-funcs=table-funcs
          memory-common=static-table-common
          pre=preprocess-data
      ==
  ::
  ++  make-proof-snapshot
    |=  $:  version=proof-version
            header=noun-digest:tip5
            nonce=noun-digest:tip5
            pow-len=@
            s=*
            f=*
            prod=*
            return=fock-return
        ==
    ^-  proof-snapshot
    =/  work=proof-work  (make-proof-work +<)
    :*  0
        ext.work
        proof.work
        num-tables.work
        base.work
        (turn tables.work |=(t=table-dat p.t))
        chals-rd1.work
        s
        f
        return
        heights.work
        num-extra-constraints.work
        version
        constraint-map.pre
        count-map.pre
    ==
  ::
  ++  make-proof-work
    |=  $:  version=proof-version
            header=noun-digest:tip5
            nonce=noun-digest:tip5
            pow-len=@
            s=*
            f=*
            prod=*
            return=fock-return
        ==
    ^-  proof-work
    =|  =proof
    =.  proof  (~(push proof-stream proof) [%puzzle header nonce pow-len prod])
    =/  tables=(list table-dat)
      (build-table-dats return)
    =/  num-tables  (lent tables)
    =/  heights=(list @)
      %+  turn  tables
      |=  t=table-dat
      =/  len  len.array.p.p.t
      ?:(=(len 0) 0 (bex (xeb (dec len))))
    =.  proof  (~(push proof-stream proof) [%heights heights])
    =/  clc  ~(. calc heights cd.pre)
    =*  fri-domain-len=@  init-domain-len:fri:clc
    =/  [base-marys=(list mary) width=@]
      %^  spin  tables
        0
      |=([t=table-dat width=@] [p.p.t (add width base-width.p.t)])
    =/  base=codeword-commitments
      (compute-codeword-commitments base-marys fri-domain-len width)
    =.  proof  (~(push proof-stream proof) [%m-root h.q.merk-heap.base])
    =/  rng  ~(prover-fiat-shamir proof-stream proof)
    =^  chals-rd1=(list belt)  rng  (belts:rng num-chals-rd1:chal)
    =/  table-exts=(list table-mary)
      %+  turn  tables
      |=  t=table-dat
      ^-  table-mary
      (extend:q.t p.t chals-rd1 return)
    =.  tables
      %+  turn
      (zip-up tables table-exts)
      |=  [t=table-dat ext=table-mary]
      ^-  table-dat
      :_  [q.t r.t]
      (weld-exts:tlib p.t ext)
    =/  [ext-marys=(list mary) width=@]
      %^  spin  table-exts
        0
      |=([t=table-mary width=@] [p.t (add width ext-width.t)])
    =/  ext=codeword-commitments
      (compute-codeword-commitments ext-marys fri-domain-len width)
    =/  num-extra-constraints=@
      %+  roll  (range num-tables)
      |=  [i=@ acc=@]
      =/  cs  (~(got by count-map.pre) i)
      ;:  add
        acc
        boundary.cs
        row.cs
        transition.cs
        terminal.cs
        extra.cs
      ==
    :*  ext
        proof
        num-tables
        base
        tables
        chals-rd1
        heights
        fri-domain-len
        num-extra-constraints
    ==
  ::
  :: generate-proof is the main body of the prover.
  ++  generate-proof
    :: Disabled jet hint for now, under development.
    :: ~/  %generate-proof
    |=  $:  version=proof-version
            header=noun-digest:tip5
            nonce=noun-digest:tip5
            pow-len=@
            s=*
            f=*
            prod=*
            return=fock-return
        ==
    ^-  prove-result
    =/  work=proof-work  (make-proof-work +<)
    =/  proof  proof.work
    =/  tables  tables.work
    =/  num-tables  num-tables.work
    =/  heights  heights.work
    =/  fri-domain-len  fri-domain-len.work
    =/  clc  ~(. calc heights cd.pre)
    =/  base  base.work
    =/  ext  ext.work
    =/  chals-rd1  chals-rd1.work
    =.  proof  (~(push proof-stream proof) [%m-root h.q.merk-heap.ext])
    ::
    ::  reseed the rng
    =/  rng  ~(prover-fiat-shamir proof-stream proof)
    ::
    ::  get coefficients for table extensions, extend tables
    ::  round two challenges: β, z
    =^  chals-rd2=(list belt)  rng  (belts:rng num-chals-rd2:chal)
    =/  challenges  (weld chals-rd1 chals-rd2)
    ::
    ::  build mega-extension columns
    =/  table-mega-exts=(list table-mary)
      (build-mega-extend tables challenges return)
    ::~&  %tables-built
    ::
    ::  augment challenges with derived challenges
    =/  augmented-chals=bpoly
      (augment-challenges:chal challenges s f)
    ::
    =.  tables
      %+  turn  (zip-up tables table-mega-exts)
      |=  [t=table-dat mega-ext=table-mary]
      ^-  table-dat
      :_  [q.t r.t]
      (weld-exts:tlib p.t mega-ext)
    ::
    ::  check that the tables have correct num of ext cols. Comment this out for production.
    ::~&  >>  %check-mega-ext-cols
    ::?:  %+  levy  (zip-up tables table-mega-exts)
        ::|=  [table=table-dat mext=table-mary]
        ::!=(step.p.mext mega-ext-width.p.table)
      ::~&  %widths-mismatch
      ::~|("prove: mismatch between table ext widths and actual ext widths" !!)
    ::
    ::  convert the mega-ext columns to marys
    ::
    ::  think of each mary as a list of the table's rows
    =/  [mega-ext-marys=(list mary) width=@]
      %^  spin  table-mega-exts
        0
      |=  [t=table-mary width=@]
      [p.t (add width mega-ext-width.t)]
    =/  mega-ext=codeword-commitments
      (compute-codeword-commitments mega-ext-marys fri-domain-len width)
    ::
    ::  get terminal values for use in permutation/evaluation arguments
    =/  dyn-list=(list bpoly)
      %+  turn  tables
      |=  t=table-dat
      (terminal:q.t p.t)
    ::
    ::  weld terminals from each table together
    =/  terminals=bpoly
      %+  roll  (range (lent tables))
      |=  [i=@ acc=bpoly]
      (~(weld bop acc) (snag i dyn-list))
    ::  send terminals to verifier
    =.  proof  (~(push proof-stream proof) terms+terminals)
    ::  reseed the rng
    =.  rng  ~(prover-fiat-shamir proof-stream proof)
    ::
    ::
    ::
    ::  This chunk of code plugs all the rows into the constraints to check if they really do
    ::  evaluate to 0. Verifying the proof also checks this and is much faster, but this code
    ::  is useful if you are debugging constraints. Keep it commented out unless you need it.
    ::  It should never be run in production.
    ::
    ::?>  %+  levy  (zip-up (range (lent tables)) tables)
    ::    |=  [i=@ t=table-dat]
    ::    %-  (test:zkvm-debug p.t s f)
    ::    [challenges (~(got by dyn-map) i) r.t]
    ::~&  %passed-tests
    ::
    =/  num-extra-constraints=@
      %+  roll  (range num-tables)
      |=  [i=@ acc=@]
      =/  cs  (~(got by count-map.pre) i)
      ;:  add
        acc
        boundary.cs
        row.cs
        transition.cs
        terminal.cs
        extra.cs
      ==
    ::
    =/  total-cols=@
      %+  roll  tables
      |=  [[p=table-mary *] sum=@]
      (add sum step:p.p)
    ::
    ::  The constraints take variables for a full row plus the following row. So to evaluate them
    ::  the trace polys are not enough. We need to compose each trace poly with f(X)=g*X to create
    ::  polys that will give the value of the following row. Then we weld these second-row polys
    ::  to the original polys to get the double trace polys. These can then be used to compose with
    ::  the constraints and evaluate at the DEEP challenge later on.
    ::~&  %transposing-table
    ::  TODO: we already transposed the tables when we interpolated the polynomials and we should
    ::  just reuse that. But that requires changing the interface to the interpolation functions.
    =/  marys=(list table-mary)
      %+  turn  tables
      |=(t=table-dat p.t)
    =/  transposed-tables=(list mary)
      %+  turn  marys
      |=  =table-mary
      (transpose-bpolys p.table-mary)
    ::
    ::~&  %composing-trace-polys
    ::  each mary is a list of a table's columns, interpolated to polys
    =/  trace-polys
      %+  turn  (zip-up polys.base (zip-up polys.ext polys.mega-ext))
      |=  [bm=mary em=mary mem=mary]
      ^-  mary
      (~(weld ave bm) (~(weld ave em) mem))
    ::
    =/  second-row-trace-polys=(list mary)
      %+  turn  transposed-tables
      |=  polys=mary
      %-  zing-bpolys
      %+  turn  (range len.array.polys)
      |=  i=@
      =/  bp=bpoly  (~(snag-as-bpoly ave polys) i)
      (bp-ifft (bp-shift-by-unity bp 1))
    ::
    ::~&  %appending-first-and-second-row-trace-polys
    ::
    =/  tworow-trace-polys=(list mary)
      %^    zip
          trace-polys
        second-row-trace-polys
      |=  [t-poly=mary s-poly=mary]
      (~(weld ave t-poly) s-poly)
    ::
    ::
    ::  Compute trace and tworow-trace polynomials in eval form over a 4*d root of unity
    ::  (where d is the lowest power of 2 greater than the max degree of the constraints)
    ::~&  %extending-trace-polys
    ::
    ::  TODO: Save these variables in the preprocess step
    =/  max-constraint-degree  (get-max-constraint-degree cd.pre)
    =/  ntt-len
      %-  bex  %-  xeb  %-  dec
      (get-max-constraint-degree cd.pre)
    =/  max-height=@
      %-  bex  %-  xeb  %-  dec
      (roll heights max)
    =/  tworow-trace-polys-eval=(list bpoly)
      %+  iturn  tworow-trace-polys
      |=  [i=@ polys=mary]
      (precompute-ntts polys max-height ntt-len)
    ::
    ::
    ::  compute extra composition poly
    =/  omicrons-belt
      %+  turn  tables
      |=  [t=table-mary *]
      ~(omicron quot t)
    =/  omicrons-bpoly=bpoly  (init-bpoly omicrons-belt)
    =/  omicrons-fpoly=fpoly
      (init-fpoly (turn omicrons-belt lift))
    =^  extra-comp-weights=bpoly  rng
      =^  belt-list  rng  (belts:rng (mul 2 num-extra-constraints))
      [(init-bpoly belt-list) rng]
    =/  extra-composition-weights=(map @ bpoly)
      %-  ~(gas by *(map @ bpoly))
      =-  -<
      %+  roll  (range num-tables)
      |=  [i=@ acc=(list [@ bpoly]) num=@]
      =/  cs  (~(got by count-map.pre) i)
      =/  num-extra-constraints=@
        ;:  add
            boundary.cs
            row.cs
            transition.cs
            terminal.cs
            extra.cs
        ==
      :_  (add num (mul 2 num-extra-constraints))
      [[i (~(swag bop extra-comp-weights) num (mul 2 num-extra-constraints))] acc]
    ::~&  %computing-extra-composition-poly
    =/  extra-composition-poly=bpoly
      %-  compute-composition-poly
      :*  omicrons-bpoly
          heights
          tworow-trace-polys-eval
          constraint-map.pre
          count-map.pre
          extra-composition-weights
          augmented-chals
          dyn-list
          %.y
      ==
    =.  proof
      (~(push proof-stream proof) [%poly extra-composition-poly])
    =.  rng  ~(prover-fiat-shamir proof-stream proof)
    =^  extra-comp-eval-point  rng  $:felt:rng
    ::
    ::  compute extra trace evals
    ::~&  %evaluating-trace-at-new-comp-eval-point
    =/  extra-trace-evaluations=fpoly
      %-  init-fpoly
      %-  zing
      %+  turn  tworow-trace-polys
      |=  polys=mary
      %+  turn  (range len.array.polys)
      |=  i=@
      =/  b=bpoly  (~(snag-as-bpoly ave polys) i)
      (bpeval-lift b extra-comp-eval-point)
    ::
    =.  proof
      (~(push proof-stream proof) [%evals extra-trace-evaluations])
    ::
    ::  send mega extension columns to verifier
    =.  proof  (~(push proof-stream proof) [%m-root h.q.merk-heap.mega-ext])
    ::  reseed the rng
    =/  rng  ~(prover-fiat-shamir proof-stream proof)
    ::
    ::  compute the Composition Polynomial
    ::  This polynomial composes the trace polynomials with the constraints, takes quotients
    ::  over the rows where the constraint should be zero, adjusts the degree so they all
    ::  have the same maximal degree, and combines them into one big random linear combination.
    ::
    ::  compute weights used in linear combination of composition polynomial
    =/  num-constraints=@
      %+  roll  (range num-tables)
      |=  [i=@ acc=@]
      =/  cs  (~(got by count-map.pre) i)
      ;:  add
        acc
        boundary.cs
        row.cs
        transition.cs
        terminal.cs
      ==
    =^  comp-weights=bpoly  rng
      =^  belt-list  rng  (belts:rng (mul 2 num-constraints))
      [(init-bpoly belt-list) rng]
    ::
    =/  composition-weights=(map @ bpoly)
      %-  ~(gas by *(map @ bpoly))
      =-  -<
      %+  roll  (range num-tables)
      |=  [i=@ acc=(list [@ bpoly]) num=@]
      =/  cs  (~(got by count-map.pre) i)
      =/  num-constraints=@
        ;:  add
            boundary.cs
            row.cs
            transition.cs
            terminal.cs
        ==
      :_  (add num (mul 2 num-constraints))
      [[i (~(swag bop comp-weights) num (mul 2 num-constraints))] acc]
    ::
    ::~&  %computing-composition-poly
    =/  composition-poly=bpoly
      %-  compute-composition-poly
      :*  omicrons-bpoly
          heights
          tworow-trace-polys-eval
          constraint-map.pre
          count-map.pre
          composition-weights
          augmented-chals
          dyn-list
          %.n
      ==
    ::
    ::  decompose composition polynomial into one polynomial for each degree of the
    ::  constraints. If the max degree of the constraints is D, then this will produce
    ::  D polynomials each of degree table-height.
    ::~&  %decomposing-composition-poly
    =/  num-composition-pieces  (get-max-constraint-degree cd.pre)
    ::
    =/  composition-pieces=(list bpoly)
      (bp-decompose composition-poly num-composition-pieces)
    ::
    ::  turn composition pieces into codewords
    ::~&  %computing-composition-codewords
    =/  composition-codewords=mary
      %-  zing-bpolys
      %+  turn  composition-pieces
      |=  poly=bpoly
      (bp-coseword poly g fri-domain-len)
    =/  composition-codeword-array=mary
      (transpose-bpolys composition-codewords)
    =/  composition-merk=(pair @ merk-heap:merkle)
      (bp-build-merk-heap:merkle composition-codeword-array)
    =.  proof
      (~(push proof-stream proof) [%comp-m h.q.composition-merk num-composition-pieces])
    ::
    ::
    ::
    ::
    ::  reseed the rng
    =.  rng  ~(prover-fiat-shamir proof-stream proof)
    ::
    ::  compute DEEP challenge point from extension field
    =^  deep-challenge=felt  rng
      =^  deep-candidate  rng  $:felt:rng
      =/  n  fri-domain-len:clc
      =/  exp-offset  (lift (bpow generator:stark-engine n))
      |-
      =/  exp-deep-can  (fpow deep-candidate n)
      ?.  ?|(=(exp-deep-can f1) =(exp-deep-can exp-offset))
        [deep-candidate rng]
      =^  felt  rng  $:felt:rng
      $(deep-candidate felt)
    ::~&  %evaluating-trace-at-deep-challenge
    ::
    ::  trace-evaluations: list of evaluations of interpolated column polys and
    ::  shifted column polys at deep point, grouped in order by tables
    =/  trace-evaluations=fpoly
      %-  init-fpoly
      %-  zing
      %+  turn  tworow-trace-polys
      |=  polys=mary
      %+  turn  (range len.array.polys)
      |=  i=@
      =/  b=bpoly  (~(snag-as-bpoly ave polys) i)
      (bpeval-lift b deep-challenge)
    ::
    ::~&  %evaluating-pieces-at-deep-challenge
    =/  composition-pieces-fpoly  (turn composition-pieces bpoly-to-fpoly)
    =/  composition-piece-evaluations=fpoly
      =/  c  (fpow deep-challenge num-composition-pieces)
      %-  init-fpoly
      %+  turn  composition-pieces-fpoly
      |=(poly=fpoly (fpeval poly c))
    ::
    =.  proof
      (~(push proof-stream proof) [%evals trace-evaluations])
    =.  proof
      (~(push proof-stream proof) [%evals composition-piece-evaluations])
    ::
    ::  reseed the rng
    =.  rng  ~(prover-fiat-shamir proof-stream proof)
    ::
    ::  compute weights used in linear combination of deep polynomial. These
    ::  are from the extension field.
    =^  deep-weights=fpoly  rng
      =^  felt-list  rng
        %-  felts:rng
        (add (mul 4 total-cols) max-constraint-degree)
      [(init-fpoly felt-list) rng]
    =/  all-evals  (~(weld fop trace-evaluations) extra-trace-evaluations)
    ::~&  %computing-deep-poly
    =/  deep-poly=fpoly
      %-  compute-deep
      :*  trace-polys
          all-evals
          composition-pieces-fpoly
          composition-piece-evaluations
          deep-weights
          omicrons-fpoly
          deep-challenge
          extra-comp-eval-point
      ==
    ::
    ::  create DEEP codeword and push to proof
    ::~&  %computing-deep-codeword
    =/  deep-codeword=fpoly
      (coseword deep-poly (lift g) fri-domain-len)
    ::
    =^  fri-indices=(list @)  proof
      (prove:fri:clc deep-codeword proof)
    ::
    ::
    ::~&  %opening-codewords
    =.  proof
      %^  zip-roll  (range num-spot-checks)  fri-indices
      |=  [[i=@ idx=@] proof=_proof]
      ::
      ::  base trace codewords
      =/  elem=mary
        (~(change-step ave (~(snag-as-mary ave codewords.base) idx)) 1)
      =/  axis  (index-to-axis:merkle p.merk-heap.base idx)
      =/  opening=merk-proof:merkle
        (build-merk-proof:merkle q.merk-heap.base axis)
      =.  proof
        %-  ~(push proof-stream proof)
        m-pathbf+[(tail elem) path.opening]
      ::
      ::  ext trace codewords
      =.  elem
        (~(change-step ave (~(snag-as-mary ave codewords.ext) idx)) 1)
      =.  axis  (index-to-axis:merkle p.merk-heap.ext idx)
      =.  opening
        (build-merk-proof:merkle q.merk-heap.ext axis)
      =.  proof
        %-  ~(push proof-stream proof)
        m-pathbf+[(tail elem) path.opening]
      ::
      ::  mega-ext trace codewords
      =.  elem
        (~(change-step ave (~(snag-as-mary ave codewords.mega-ext) idx)) 1)
      =.  axis  (index-to-axis:merkle p.merk-heap.mega-ext idx)
      =.  opening
        (build-merk-proof:merkle q.merk-heap.mega-ext axis)
      =.  proof
        %-  ~(push proof-stream proof)
        m-pathbf+[(tail elem) path.opening]
      ::
      ::  piece codewords
      =.  elem
        (~(change-step ave (~(snag-as-mary ave composition-codeword-array) idx)) 1)
      =.  axis  (index-to-axis:merkle p.composition-merk idx)
      =.  opening  (build-merk-proof:merkle q.composition-merk axis)
      %-  ~(push proof-stream proof)
      m-pathbf+[(tail elem) path.opening]
    ::
    ::~&  %finished-proof
    ?-  version
      %0  [%& %0 objects.proof ~ 0]
      %1  [%& %1 objects.proof ~ 0]
      %2  [%& %2 objects.proof ~ 0]
      %3  [%& %3 objects.proof ~ 0]
    ==
  ::
  ::
  ++  build-table-dats
    ~/  %build-table-dats
    |=  return=fock-return
    ^-  (list table-dat)
    %-  sort
    :_  td-order
    %+  turn  gen-table-names:nock-common
    |=  name=term
    =/  t-funcs
      ~|  "table-funcs do not exist for {<name>}"
      (~(got by table-funcs-map) name)
    =/  v-funcs
      ~|  "verifier-funcs do not exist for {<name>}"
      (~(got by all-verifier-funcs-map:nock-common) name)
    =/  tm=table-mary  (build:t-funcs return)
    [(pad:t-funcs tm) t-funcs v-funcs]
  ::
  ++  table-funcs-map
    ~+
    ^-  (map term table-funcs)
    %-  ~(gas by *(map term table-funcs))
    :~  :-  name:compute-common
        compute-funcs
        :-  name:memory-common
        memory-funcs
    ==
  ++  build-mega-extend
    ~/  %build-mega-extend
    |=  [tables=(list table-dat) chals=(list belt) return=fock-return]
    ^-  (list table-mary)
    %+  turn  tables
    |=  t=table-dat
    ^-  table-mary
    (mega-extend:q.t p.t chals return)
  --
--
