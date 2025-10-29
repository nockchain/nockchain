/=  nock-common-v0-v1  /common/v0-v1/nock-common
/=  nock-common-v2     /common/v2/nock-common
/=  *  /common/zeke
::
=>  :*  stark-engine
        nock-common-v0-v1=nock-common-v0-v1
        nock-common-v2=nock-common-v2
    ==
~%  %stark-verifier  ..stark-engine-jet-hook  ~
|%
::  copied from sur/verifier.hoon because of =>  stark-engine
+$  verify-result  [commitment=noun-digest:tip5 nonce=noun-digest:tip5]
+$  elem-list  (list [idx=@ trace-elems=(list belt) comp-elems=(list felt) deep-elem=felt])
::
++  verify
  =|  test-mode=_|
  |=  [=proof override=(unit (list term)) verifier-eny=@]
  ^-  ?
  =/  nock-common=_nock-common-v0-v1
    ?-  version.proof
      %0  nock-common-v0-v1
      %1  nock-common-v0-v1
      %2  nock-common-v2
    ==
  =/  pre=preprocess-data
    ?-  version.proof
      %0  p.pre-0-1.prep.stark-config
      %1  p.pre-0-1.prep.stark-config
      %2  p.pre-2.prep.stark-config
    ==
  ::
  =/  verify  ~(verify verify-door [nock-common pre])
  %-  ~(. verify test-mode)
  [proof override verifier-eny]
::
++  verify-door
  ~/  %verify-door
  |_  [nock-common=_nock-common-v0-v1 pre=preprocess-data]
  ::
  ++  verify
    =|  test-mode=_|
    |=  [=proof override=(unit (list term)) verifier-eny=@]
    ^-  ?
    =/  args  [proof override verifier-eny test-mode]
    -:(mule |.((verify-inner args)))
  ::
  ++  verify-inner
    ~/  %verify-inner
    |=  [=proof override=(unit (list term)) verifier-eny=@ test-mode=?]
    ^-  verify-result
    ?>  =(~ hashes.proof)
    =^  puzzle  proof
      =^(c proof ~(pull proof-stream proof) ?>(?=(%puzzle -.c) c^proof))
    =/  [s=* f=*]  (puzzle-nock commitment.puzzle nonce.puzzle len.puzzle)
    ::
    ::  get computation in raw noun form
    ?>  (based-noun p.puzzle)
    ::
    =/  table-names  %-  sort
                    :_  t-order
                    ?~  override
                      gen-table-names:nock-common
                    u.override
    ::~&  table-names+table-names
    ::
    ::
    ::  compute dynamic table widths
    =/  table-base-widths  (compute-base-widths override)
    =/  table-full-widths  (compute-full-widths override)
    ::
    ::  get table heights
    =^  heights  proof
      =^(h proof ~(pull proof-stream proof) ?>(?=(%heights -.h) p.h^proof))
    =/  num-tables  (lent heights)
    ?>  =(num-tables (lent core-table-names:nock-common))
    ::~&  table-heights+heights
    ::
    =/  c  constraints
    ::
    ::
    =/  clc  ~(. calc heights cd.pre)
    ::
    ::  verify size of proof
    =/  expected-num-proof-items=@ud
      ;:  add
      ::
      ::  number of static items in proof-data
      ::
        12
      ::
      ::  number of items written by FRI
      ::
        %+  add  num-rounds:fri:clc
        (mul num-rounds:fri:clc num-spot-checks:fri:clc)
      ::
      ::  number of merkle lookups into the deep codeword
      ::
      (mul 4 num-spot-checks:fri:clc)
      ==
    =/  actual-num-proof-items=@ud  (lent objects.proof)
    ?>  =(expected-num-proof-items actual-num-proof-items)
    ::
    ::  get merkle root of base tables
    =^  base-root   proof
      =^(b proof ~(pull proof-stream proof) ?>(?=(%m-root -.b) p.b^proof))
    ::
    =/  rng  ~(verifier-fiat-shamir proof-stream proof)
    ::
    ::  get coefficients for table extensions
    ::  challenges: a, b, c, alpha
    =^  chals-rd1=(list belt)  rng  (belts:rng num-chals-rd1:chal)
    ::
    ::  get merkle root of extension tables
    =^  ext-root   proof
      =^(e proof ~(pull proof-stream proof) ?>(?=(%m-root -.e) p.e^proof))
    ::
    =.  rng  ~(verifier-fiat-shamir proof-stream proof)
    ::
    ::  get coefficients for table extensions
    ::  challenges: beta, z
    =^  chals-rd2=(list belt)  rng  (belts:rng num-chals-rd2:chal)
    =/  challenges  (weld chals-rd1 chals-rd2)
    ::  augment challenges with derived challenges
    =/  augmented-chals=bpoly
      (augment-challenges:chal challenges s f)
    =/  chal-map=(map term belt)
      (bp-zip-chals-list:chal chal-names-basic:chal challenges)
    ::
    :: TODO: read these out of the augmented-chals bpoly and dont waste
    :: time building the map
    =/  [alf=pelt j=pelt k=pelt l=pelt m=pelt z=pelt]
      :*  (got-pelt chal-map %alf)
          (got-pelt chal-map %j)
          (got-pelt chal-map %k)
          (got-pelt chal-map %l)
          (got-pelt chal-map %m)
          (got-pelt chal-map %z)
      ==
    ::
    =/  subj-data
      (build-tree-data:fock s alf)
    =/  form-data
      (build-tree-data:fock f alf)
    =/  prod-data
      (build-tree-data:fock p.puzzle alf)
    ::
    ::  get terminals
    =^  terminals  proof
      =^(t proof ~(pull proof-stream proof) ?>(?=(%terms -.t) p.t^proof))
    ::
    =.  rng  ~(verifier-fiat-shamir proof-stream proof)
    ::
    ::  verify that len.terminals is as expected
    ?.  .=  len.terminals
        %-  lent
        %+  roll  all-terminal-names:nock-common
        |=  [terms=(list term) acc=(list term)]
        ^-  (list term)
        (weld acc terms)
      ~&  "len.terminals is wrong"
      !!
    ::
    ::  verify that len.terminals is the length of the data buffer of the bpoly
    ?.  ~(chck bop terminals)
      ~&  "len.terminals is not equal to the data buffer"
      !!
    ::
    ::
    =/  [terminal-map=(map term belt) dyn-list=(list bpoly)]
      =-  [term-map (flop dyn-list)]
      %+  roll  all-terminal-names:nock-common
      |=  [terms=(list term) table-num=@ idx=@ term-map=(map term belt) dyn-list=(list bpoly)]
      :^    +(table-num)
          (add idx (lent terms))
        %-  ~(gas by term-map)
        %+  iturn  terms
        |=  [i=@ t=term]
        [t (~(snag bop terminals) (add idx i))]
      [(~(swag bop terminals) idx (lent terms)) dyn-list]
    ::
    ::
    ?.  (linking-checks subj-data form-data prod-data j k l m z terminal-map)
      ~&  "failed input linking checks"  !!
    ::
    ::
    ::  evaluate the second composition poly
    =/  total-extra-constraints=@
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
    =^  extra-comp-weights=bpoly  rng
      =^  belts  rng  (belts:rng (mul 2 total-extra-constraints))
      [(init-bpoly belts) rng]
    =/  extra-composition-weights=(map @ bpoly)
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
            extra.cs
        ==
      :_  (add num (mul 2 num-constraints))
      [[i (~(scag bop (~(slag bop extra-comp-weights) num)) (mul 2 num-constraints))] acc]
    ::
    =^  extra-comp-bpoly  proof
      =^(c proof ~(pull proof-stream proof) ?>(?=(%poly -.c) p.c^proof))
    ::
    =.  rng  ~(verifier-fiat-shamir proof-stream proof)
    ::
    =^  extra-comp-eval-point  rng  $:felt:rng
    ::
    =^  extra-trace-evaluations=fpoly  proof
      =^(t proof ~(pull proof-stream proof) ?>(?=(%evals -.t) p.t^proof))
    ::
    ::  check that the size of the evaluations is exactly twice the total number of
    ::  columns across all tables
    =/  total-cols=@
      %+  roll  table-full-widths
      |=  [w=@ acc=@]
      (add w acc)
    ?>  =(len.extra-trace-evaluations (mul 2 total-cols))
    ?>  ~(chck fop extra-trace-evaluations)
    ::
    =/  extra-composition-eval=felt
      %-  eval-composition-poly
      :*  extra-trace-evaluations
          heights
          constraint-map.pre
          count-map.pre
          dyn-list
          extra-composition-weights
          augmented-chals
          extra-comp-eval-point
          table-full-widths
          %.y
      ==
    ::
    =/  extra-comp-bpoly-eval  (bpeval-lift extra-comp-bpoly extra-comp-eval-point)
    ::
    ::  check that the extra composition eval equals the eval pt
    ?>  =(extra-composition-eval extra-comp-bpoly-eval)
    ::
    ::  get merkle root of mega-extension tables
    =^  mega-ext-root   proof
      =^(m proof ~(pull proof-stream proof) ?>(?=(%m-root -.m) p.m^proof))
    ::
    =.  rng  ~(verifier-fiat-shamir proof-stream proof)
    ::
    ::  We now use the randomness to compute the expected fingerprints of the compute stack and product stack based on the given [s f] and product, respectively.
    ::  We then dynamically generate constraints that force the cs and ps to be equivalent to the expected fingerprints.
    ::  As long as the prover replicates this exact protocol, the opened indicies should match up.
    ::  The boundary constraint then ensures that the computation in cleartext is linked to the computation in the trace.
    ::
    ::  generate scalars for the random linear combination of the composition polynomial
    =/  total-constraints=@
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
      =^  belts  rng  (belts:rng (mul 2 total-constraints))
      [(init-bpoly belts) rng]
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
      [[i (~(scag bop (~(slag bop comp-weights) num)) (mul 2 num-constraints))] acc]
    ::~&  max-degree+max-degree:clc
    ::
    ::  read the composition piece codewords
    =^  comp-root  proof
      =^(c proof ~(pull proof-stream proof) ?>(?=(%comp-m -.c) [p.c num.c]^proof))
    ::
    ::
    =.  rng  ~(verifier-fiat-shamir proof-stream proof)
    ::
    ::  generate the DEEP challenge
    =^  deep-challenge=felt  rng
      =^  deep-candidate=felt  rng  $:felt:rng
      =/  n  fri-domain-len:clc
      =/  exp-offset  (lift (bpow generator:stark-engine n))
      |-
      =/  exp-deep-can  (fpow deep-candidate n)
      ::  reject if deep-candidate is in the evaluation domain H or FRI domain D
      ?.  ?|(=(exp-deep-can f1) =(exp-deep-can exp-offset))
        [deep-candidate rng]
      =^  felt  rng  $:felt:rng
      $(deep-candidate felt)
    ::
    ::  read the trace evaluations at the DEEP challenge point
    =^  trace-evaluations=fpoly  proof
      =^(t proof ~(pull proof-stream proof) ?>(?=(%evals -.t) p.t^proof))
    ::
    ::
    ::  check that the size of the evaluations is exactly twice the total number of
    ::  columns across all tables
    ?>  =(len.trace-evaluations (mul 2 total-cols))
    ?>  ~(chck fop trace-evaluations)
    ::
    ::  read the composition piece evaluations at the DEEP challenge point
    =^  composition-piece-evaluations=fpoly  proof
      =^(c proof ~(pull proof-stream proof) ?>(?=(%evals -.c) p.c^proof))
    ::
    ::  check that there are the correct number of composition piece evaluations and no more
    ?.  ?&  =(len.composition-piece-evaluations +.comp-root)
            ~(chck fop composition-piece-evaluations)
        ==
        ~&  >>  %num-composition-piece-evals-wrong
        !!
    ::
    =.  rng  ~(verifier-fiat-shamir proof-stream proof)
    :: verify the composition polynomial equals the composition pieces by evaluating each side
    :: at the DEEP challenge point
    =/  composition-eval=felt
      %-  eval-composition-poly
      :*  trace-evaluations
          heights
          constraint-map.pre
          count-map.pre
          dyn-list
          composition-weights
          augmented-chals
          deep-challenge
          table-full-widths
          %.n
      ==
    ::
    =/  decomposition-eval=felt
      %+  roll  (range +.comp-root)
      |=  [i=@ acc=_(lift 0)]
      ~|  'A crash here sometimes indicates that one of the constraints is degree 5 or higher'
      %+  fadd  acc
      (fmul (fpow deep-challenge i) (~(snag fop composition-piece-evaluations) i))
    ::
    ?.  =(composition-eval decomposition-eval)
      ~&  %composition-eval-failed
      ~&  %invalid-proof  !!
    ::~&  %composition-eval-passed
    ::
    ::
    ::  generate random weights for DEEP composition polynomial
    =^  deep-weights=fpoly  rng
      =^  felt-list  rng
        %-  felts:rng
        :(add len.trace-evaluations len.extra-trace-evaluations len.composition-piece-evaluations)
      [(init-fpoly felt-list) rng]
    ::
    ::  read the merkle root of the DEEP composition polynomial
    =^  deep-root   proof
      =^(d proof ~(pull proof-stream proof) ?>(?=(%m-root -.d) p.d^proof))
    ::
    ::  verify the DEEP composition polynomial is low degree
    =^  [fri-indices=(list @) merks=(list merk-data:merkle) deep-cosets=(map @ fpoly) fri-res=?]  proof
      (verify:fri:clc proof deep-root)
    ::
    ?.  =(fri-res %.y)
      ~&  %deep-composition-polynomial-is-not-low-degree
      ~&  %invalid-proof  !!
    ::~&  %deep-composition-polynomial-is-low-degree
    ::
    ::
    ::  verify the DEEP codeword is actually the codeword of the DEEP composition polynomial by evaluating
    ::  it at all the top level FRI points by opening the trace and piece polynomials and comparing with the
    ::  deep codeword. This convinces the verifier that it ran FRI on the correct polynomial.
    ::
    ::  Open trace and composition piece polynomials at the top level FRI indices
    ::
    =^  [elems=elem-list merk-proofs=(list merk-data:merkle)]
        proof
      %+  roll  fri-indices
      |=  $:  idx=@
              $:  l=(list [idx=@ trace-elems=(list belt) comp-elems=(list felt) deep-elem=felt])
                  proofs=_merks
              ==
              proof=_proof
          ==
      =/  axis  (index-to-axis:merkle (xeb fri-domain-len:clc) idx)
      =^  base-trace-opening  proof
        =^(mp proof ~(pull proof-stream proof) ?>(?=(%m-pathbf -.mp) p.mp^proof))
      =^  ext-opening  proof
        =^(mp proof ~(pull proof-stream proof) ?>(?=(%m-pathbf -.mp) p.mp^proof))
      =^  mega-ext-opening  proof
        =^(mp proof ~(pull proof-stream proof) ?>(?=(%m-pathbf -.mp) p.mp^proof))
      =^  comp-opening  proof
        =^(mp proof ~(pull proof-stream proof) ?>(?=(%m-pathbf -.mp) p.mp^proof))
      ::
      =.  proofs
        :*
          :*  (hash-hashable:tip5 (hashable-bpoly:tip5 leaf.base-trace-opening))
              axis  base-root  path.base-trace-opening
          ==
        ::
          :*  (hash-hashable:tip5 (hashable-bpoly:tip5 leaf.ext-opening))
              axis  ext-root  path.ext-opening
          ==
        ::
          :*  (hash-hashable:tip5 (hashable-bpoly:tip5 leaf.mega-ext-opening))
              axis  mega-ext-root  path.mega-ext-opening
          ==
        ::
          :*  (hash-hashable:tip5 (hashable-bpoly:tip5 leaf.comp-opening))
              axis  -.comp-root  path.comp-opening
          ==
        ::
          proofs
        ==
      =/  base-elems  (bpoly-to-list leaf.base-trace-opening)
      ?>  (levy base-elems based)
      =/  ext-elems  (bpoly-to-list leaf.ext-opening)
      ?>  (levy ext-elems based)
      =/  mega-ext-elems  (bpoly-to-list leaf.mega-ext-opening)
      ?>  (levy mega-ext-elems based)
      =/  trace-elems=(list belt)
        %-  zing
        ::  combines base, ext, and mega-ext openings, divided by table
        ^-  (list (list belt))
        %-  turn  :_  weld
        %+  zip-up
          (clev base-elems table-base-widths-static:nock-common)
        %-  turn  :_  weld
        %+  zip-up
          (clev ext-elems table-ext-widths-static:nock-common)
        (clev mega-ext-elems table-mega-ext-widths-static:nock-common)
      =/  comp-elems  (bpoly-to-list leaf.comp-opening)
      ?>  (levy comp-elems based)
      ::
      ::  The openings to the deep codeword itself were already read out of the proof
      ::  during FRI. verify:fri returned deep-cosets=(map @ fpoly), which is all the cosets
      ::  read from the deep codeword, keyed by coset-idx. We will use this map to find the
      ::  deep codeword point instead of wasting proof space by writing it into the proof twice.
      ::
      =/  coset-idx  (mod idx (div fri-domain-len:clc folding-deg:fri:clc))
      =/  entry  (div idx (div fri-domain-len:clc folding-deg:fri:clc))
      =/  coset=fpoly  (~(got by deep-cosets) coset-idx)
      =/  deep-elem=felt  (~(snag fop coset) entry)
      ::
      :_  proof
      :-  [[idx trace-elems comp-elems deep-elem] l]
      proofs
    ::
    ?:  &(=(test-mode %.n) !(verify-merk-proofs merk-proofs verifier-eny))
      ~&  %failed-to-verify-merk-proofs  !!
    ::
    :: evaluate DEEP polynomial at the indices
    =/  omega=felt  (lift omega:clc)
    =/  all-evals  (~(weld fop trace-evaluations) extra-trace-evaluations)
    =/  eval-res=?
      %+  roll  elems
      |=  [[idx=@ trace-elems=(list belt) comp-elems=(list belt) deep-elem=felt] acc=?]
      ^-  ?
      =/  deep-eval
        %-  evaluate-deep
        :*  all-evals
            composition-piece-evaluations
            trace-elems
            comp-elems
            +.comp-root
            deep-weights
            heights
            table-full-widths
            omega
            idx
            deep-challenge
            extra-comp-eval-point
        ==
      ~|  "DEEP codeword doesn't match evaluation"
      ?>  =(deep-eval deep-elem)
      &(acc %.y)
    ~|  "DEEP codeword doesn't match evaluation"
    ::
    ?>  =(eval-res %.y)
    ::~&  %deep-codeword-matches
    ::~&  %proof-verified
    [commitment nonce]:puzzle
    ::
  ++  compute-base-widths
    ~/  %compute-base-widths
    |=  override=(unit (list term))
    ^-  (list @)
    ?~  override
      core-table-base-widths-static:nock-common
    (custom-table-base-widths-static:nock-common all-table-names:nock-common)
  ::
  ++  compute-full-widths
    ~/  %compute-full-widths
    |=  override=(unit (list term))
    ?~  override
      core-table-full-widths-static:nock-common
    (custom-table-full-widths-static:nock-common all-table-names:nock-common)
  ::
  ++  linking-checks
    ~/  %linking-checks
    |=  $:  s=tree-data  f=tree-data  p=tree-data
            j=pelt  k=pelt  l=pelt  m=pelt  z=pelt
            mp=(map term belt)
        ==
    ^-  ?
    =/  ifp-f  (compress-pelt ~[j k l] ~[size dyck leaf]:f)
    =/  ifp-s  (compress-pelt ~[j k l] ~[size dyck leaf]:s)
    ?&
        =;  bool
          ?:  bool  bool
          ~&("memory table node count input check failed" bool)
        .=  ?@  n.s
              z
            (pmul z z)
        (got-pelt mp %memory-nc)
      ::
        =;  bool
          ?:  bool  bool
          ~&("memory table kvs input check failed" bool)
        .=  ?@  n.s
              (pmul z (padd ifp-f (pscal 0 m)))
            %+  padd
              (pmul z (padd ifp-s (pscal 1 m)))
            :(pmul z z (padd ifp-f (pscal 0 m)))
        (got-pelt mp %memory-kvs)
      ::
        =;  bool
          ?:  bool  bool
          ~&("compute table subject size input check failed" bool)
        =(size.s (got-pelt mp %compute-s-size))
      ::
        =;  bool
          ?:  bool  bool
          ~&("compute table subject dyck word input check failed" bool)
        .=  dyck.s
        (got-pelt mp %compute-s-dyck)
      ::
        =;  bool
          ?:  bool  bool
          ~&("compute table subject leaf vector input check failed" bool)
        =(leaf.s (got-pelt mp %compute-s-leaf))
      ::
        =;  bool
          ?:  bool  bool
          ~&("compute table formula size input check failed" bool)
        =(size.f (got-pelt mp %compute-f-size))
      ::
        =;  bool
          ?:  bool  bool
          ~&("compute table formula dyck word input check failed" bool)
        =(dyck.f (got-pelt mp %compute-f-dyck))
      ::
        =;  bool
          ?:  bool  bool
          ~&("compute table formula leaf vector input check failed" bool)
        =(leaf.f (got-pelt mp %compute-f-leaf))
      ::
        =;  bool
          ?:  bool  bool
          ~&("compute table product size input check failed" bool)
        =(size.p (got-pelt mp %compute-e-size))
      ::
        =;  bool
          ?:  bool  bool
          ~&("compute table product dyck word input check failed" bool)
        =(dyck.p (got-pelt mp %compute-e-dyck))
      ::
        =;  bool
          ?:  bool  bool
          ~&("compute table product leaf vector input check failed" bool)
        =(leaf.p (got-pelt mp %compute-e-leaf))
      ::
        =;  bool
          ?:  bool  bool
          ~&("decode multiset terminal comparison check failed" bool)
        =((got-pelt mp %compute-decode-mset) (got-pelt mp %memory-decode-mset))
      ::
        =;  bool
          ?:  bool  bool
          ~&("Nock 0 multiset terminal comparison check failed" bool)
        =((got-pelt mp %compute-op0-mset) (got-pelt mp %memory-op0-mset))
    ==
  ::
  ++  eval-composition-poly
    ~/  %eval-composition-poly-wrapper
    |=  $:  trace-evaluations=fpoly
            heights=(list @)
            constraint-map=(map @ constraints)
            constraint-counts=(map @ constraint-counts)
            dyn-list=(list bpoly)
            weight-map=(map @ bpoly)
            challenges=bpoly
            deep-challenge=felt
            table-full-widths=(list @)
            is-extra=?
        ==
    ^-  felt
    (do-eval-composition-poly +<)
  ::
  :: Jets dont show up in the trace so we wrap it in a hoon function that will
  :: show up
  ++  do-eval-composition-poly
    ~/  %eval-composition-poly
    |=  $:  trace-evaluations=fpoly
            heights=(list @)
            constraint-map=(map @ constraints)
            constraint-counts=(map @ constraint-counts)
            dyn-list=(list bpoly)
            weight-map=(map @ bpoly)
            challenges=bpoly
            deep-challenge=felt
            table-full-widths=(list @)
            is-extra=?
        ==
    ^-  felt
    =/  dp  (degree-processing heights constraint-map is-extra)
    =/  boundary-zerofier=felt
      (finv (fsub deep-challenge (lift 1)))
    |^
    =-  -<
    %^  zip-roll  (range (lent heights))  heights
    |=  [[i=@ height=@] acc=_(lift 0) evals=_trace-evaluations]
    =/  width=@  (snag i table-full-widths)
    =/  omicron  (lift (ordered-root height))
    =/  last-row  (fsub deep-challenge (finv omicron))
    =/  terminal-zerofier  (finv last-row)                                   ::  f(X)=1/(X-g^{-1})
    =/  weights=bpoly  (~(got by weight-map) i)
    =/  constraints  (~(got by constraint-w-deg-map.dp) i)
    =/  counts  (~(got by constraint-counts) i)
    =/  dyns  (snag i dyn-list)
    =/  row-zerofier  (finv (fsub (fpow deep-challenge height) (lift 1)))    ::  f(X)=1/(X^N-1)
    =/  transition-zerofier                                                  ::  f(X)=(X-g^{-1})/(X^N-1)
      (fmul last-row row-zerofier)
    ::
    =/  current-evals=fpoly  (~(scag fop evals) (mul 2 width))
    :_  (~(slag fop evals) (mul 2 width))
    ;:  fadd
      acc
    ::
      %+  fmul  boundary-zerofier
      %-  evaluate-constraints
      :*  boundary.constraints
          dyns
          current-evals
          (~(scag bop weights) (mul 2 boundary.counts))
      ==
    ::
      %+  fmul  row-zerofier
      %-  evaluate-constraints
      :*  row.constraints
          dyns
          current-evals
        ::
          %+  ~(swag bop weights)
            (mul 2 boundary.counts)
          (mul 2 row.counts)
        ::
      ==
    ::
      %+  fmul  transition-zerofier
      %-  evaluate-constraints
      :*  transition.constraints
          dyns
          current-evals
        ::
          %+  ~(swag bop weights)
            (mul 2 (add boundary.counts row.counts))
          (mul 2 transition.counts)
        ::
      ==
    ::
      %+  fmul  terminal-zerofier
      %-  evaluate-constraints
      :*  terminal.constraints
          dyns
          current-evals
        ::
          %+  ~(swag bop weights)
            (mul 2 :(add boundary.counts row.counts transition.counts))
          (mul 2 terminal.counts)
        ::
      ==
    ::
      ?.  is-extra  (lift 0)
      %+  fmul  row-zerofier
      %-  evaluate-constraints
      :*  extra.constraints
          dyns
          current-evals
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
      ==
    ==
    ::
    ++  evaluate-constraints
      |=  $:  constraints=(list [(list @) mp-ultra])
              dyns=bpoly
              evals=fpoly
              weights=bpoly
          ==
      ^-  felt
      =-  acc
      %+  roll  constraints
      |=  [[degs=(list @) c=mp-ultra] [idx=@ acc=_(lift 0)]]
      ::
      ::  evaled is a list because the %comp constraint type
      ::  can contain multiple mp-mega constraints.
      =/  evaled=(list felt)  (mpeval-ultra %ext c evals challenges dyns)
      %+  roll
        (zip-up degs evaled)
      |=  [[deg=@ eval=felt] [idx=_idx acc=_acc]]
      :-  +(idx)
      ::
      ::  Each constraint corresponds to two weights: alpha and beta. The verifier
      ::  samples 2*num_constraints random values and we assume that the alpha
      ::  and beta weights for a given constraint are situated next to each other
      ::  in the array.
      ::
      =/  alpha  (~(snag bop weights) (mul 2 idx))
      =/  beta   (~(snag bop weights) (add 1 (mul 2 idx)))
      ::
      %+  fadd  acc
      %+  fmul  eval
      %+  fadd  (lift beta)
      %+  fmul  (lift alpha)
      (fpow deep-challenge (sub fri-deg-bound.dp deg))
    --  ::+eval-composition-poly
  ::
  ++  evaluate-deep
    ~/  %evaluate-deep-wrapper
    |=  $:  trace-evaluations=fpoly
            comp-evaluations=fpoly
            trace-elems=(list belt)
            comp-elems=(list belt)
            num-comp-pieces=@
            weights=fpoly
            heights=(list @)
            full-widths=(list @)
            omega=felt
            index=@
            deep-challenge=felt
            new-comp-eval=felt
        ==
    ^-  felt
    (do-evaluate-deep +<)
  ::
  ++  do-evaluate-deep
    ~/  %evaluate-deep
    |=  $:  trace-evaluations=fpoly
            comp-evaluations=fpoly
            trace-elems=(list belt)
            comp-elems=(list belt)
            num-comp-pieces=@
            weights=fpoly
            heights=(list @)
            full-widths=(list @)
            omega=felt
            index=@
            deep-challenge=felt
            new-comp-eval=felt
        ==
    ^-  felt
    =/  omega-pow  (fmul (lift g) (fpow omega index))
    |^
    =/  [acc=felt num=@ @]
      %^  zip-roll  (range (lent heights))  heights
      |=  [[i=@ height=@] acc=_(lift 0) num=@ total-full-width=@]
      =/  full-width  (snag i full-widths)
      =/  omicron  (lift (ordered-root height))
      =/  current-trace-elems  (swag [total-full-width full-width] trace-elems)
      =/  dat=[acc=felt num=@]  [acc num]
      ::  first row trace columns
      =/  denom  (fsub omega-pow deep-challenge)
      =.  dat
        %-  process-belt
        :*  current-trace-elems
            trace-evaluations
            weights
            full-width
            num.dat
            denom
            acc.dat
        ==
      ::  second row trace columns obtained by shifting by omicron
      =.  denom  (fsub omega-pow (fmul deep-challenge omicron))
      =.  dat
        %-  process-belt
        :*  current-trace-elems
            trace-evaluations
            weights
            full-width
            num.dat
            denom
            acc.dat
        ==
      [acc.dat num.dat (add total-full-width full-width)]
    ::
    ::
    =/  [acc=felt num=@ @]
      %^  zip-roll  (range (lent heights))  heights
      |=  [[i=@ height=@] acc=_acc num=_num total-full-width=@]
      =/  full-width  (snag i full-widths)
      =/  omicron  (lift (ordered-root height))
      =/  current-trace-elems  (swag [total-full-width full-width] trace-elems)
      =/  dat=[acc=felt num=@]  [acc num]
      ::  first row trace columns
      ::  evaluate new evals
      =/  denom  (fsub omega-pow new-comp-eval)
      =.  dat
        %-  process-belt
        :*  current-trace-elems
            trace-evaluations
            weights
            full-width
            num.dat
            denom
            acc.dat
        ==
      ::  second row trace columns obtained by shifting by omicron
      =.  denom  (fsub omega-pow (fmul new-comp-eval omicron))
      =.  dat
        %-  process-belt
        :*  current-trace-elems
            trace-evaluations
            weights
            full-width
            num.dat
            denom
            acc.dat
        ==
      [acc.dat num.dat (add total-full-width full-width)]
    ::
    =/  denom  (fsub omega-pow (fpow deep-challenge num-comp-pieces))
    =-  -<
    %-  process-belt
    :*  comp-elems
        comp-evaluations
        (~(slag fop weights) num)
        num-comp-pieces
        0
        denom
        acc
    ==
    ::
    ++  process-belt
      |=  $:  elems=(list belt)
              evals=fpoly
              weights=fpoly
              width=@
              num=@
              denom=felt
              acc=felt
          ==
      ^-  [felt @]
      %+  roll  (range width)
      |=  [i=@ acc=_acc num=_num]
      :_  +(num)
      %+  fadd  acc
      %+  fmul  (~(snag fop weights) num)
      %+  fdiv
        (fsub (lift (snag i elems)) (~(snag fop evals) num))
      denom
    --  ::+evaluate-deep
  ::
  ::  verify a list of merkle proofs in a random order. This is to guard against DDOS attacks.
  ++  verify-merk-proofs
    ~/  %verify-merk-proofs
    |=  [ps=(list merk-data:merkle) eny=@]
    ^-  ?
    =/  tog-eny  (new:tog:tip5 sponge:(absorb:(new:sponge:tip5) (mod eny p)^~))
    =/  lst=(list [@ merk-data:merkle])
      =-  l
      %+  roll  ps
      |=  [m=merk-data:merkle rng=_tog-eny l=(list [@ merk-data:merkle])]
      ^+  [tog:tip5 *(list [@ merk-data:merkle])]
      =^  rnd  rng  (belts:rng 1)
      :-  rng
      [[(head rnd) m] l]
    =/  sorted=(list [@ m=merk-data:merkle])
      %+  sort  lst
      |=  [x=[@ *] y=[@ *]]
      (lth -.x -.y)
    |-
    ?~  sorted  %.y
    =/  res  (verify-merk-proof:merkle m.i.sorted)
    ?.  res
      %.n
    $(sorted t.sorted)
  --
--
