/=  *  /common/zeke
/=  sp  /common/stark/prover
/=  *  /common/test
/=  *  /common/tx-engine
/=  bp  /tests/loki/bad-pow
/=  nv  /common/nock-verifier
/=  np  /common/nock-prover
/=  dk  /apps/dumbnet/lib/types
::
=<
|_  pf=proof
++  prv  np
++  vrf  nv
::
++  test-good-proof
  =/  res  (verify:vrf pf ~ 4)
  (expect !>(res))
::
++  test-v2-proof-relabelled-v3-is-bad
  =/  v3=proof  [%3 objects.pf ~ 0]
  =/  res  (verify:vrf v3 ~ 4)
  (expect !>(!res))
::
++  test-v3-pow-is-domain-separated-from-fiat-shamir
  =/  v2=proof  [%2 objects.pf ~ 0]
  =/  v3=proof  [%3 objects.pf ~ 0]
  =/  raw-pow=tip5-hash-atom
    (digest-to-atom:tip5 (hash-proof (get-pow v2)))
  %+  expect-eq
    !>([%.y %.n])
  !>  :_  =((proof-to-pow v3) raw-pow)
      =((proof-to-pow v2) raw-pow)
::
++  test-v3-version-is-bound-only-in-block-proof-digest
  =/  v2=proof  [%2 objects.pf ~ 0]
  =/  v3=proof  [%3 objects.pf ~ 0]
  =/  v3-hashes=proof  v3(hashes ~[*noun-digest:tip5])
  =/  v3-index=proof  v3(read-index 1)
  %+  expect-eq
    !>([%.y %.n %.n %.n])
  !>  :*  =((hash-proof v2) (hash-proof v3))
          =((hash-proof-for-block v2) (hash-proof-for-block v3))
          =((hash-proof-for-block v3) (hash-proof-for-block v3-hashes))
          =((hash-proof-for-block v3) (hash-proof-for-block v3-index))
      ==
::
++  test-bad-proof-empty-proof
  =/  pf  pf(objects *proof-objects)
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-no-puzzle
  =/  pf  (delete-proof-entry:bp pf %puzzle 1)
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-wrong-commitment
  =/  pzl
    =/  p  (grab-proof-entry:bp pf %puzzle 1)
    ?>  ?=(%puzzle -.p)  p
  =/  pf  (replace-proof-entry:bp pf %puzzle pzl(commitment (twiddle-digest:bp commitment.pzl)) 1)
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-unbased-commitment
  =/  pzl
    =/  p  (grab-proof-entry:bp pf %puzzle 1)
    ?>  ?=(%puzzle -.p)  p
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %puzzle
        pzl(commitment (unbase-digest:bp commitment.pzl))
        1
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-wrong-nonce
  =/  pzl
    =/  p  (grab-proof-entry:bp pf %puzzle 1)
    ?>  ?=(%puzzle -.p)  p
  =/  pf  (replace-proof-entry:bp pf %puzzle pzl(nonce (twiddle-digest:bp nonce.pzl)) 1)
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
::
++  test-bad-proof-unbased-nonce
  =/  pzl
    =/  p  (grab-proof-entry:bp pf %puzzle 1)
    ?>  ?=(%puzzle -.p)  p
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %puzzle
        pzl(nonce (unbase-digest:bp nonce.pzl))
        1
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-wrong-len
  =/  pzl
    =/  p  (grab-proof-entry:bp pf %puzzle 1)
    ?>  ?=(%puzzle -.p)  p
  =/  pf  (replace-proof-entry:bp pf %puzzle pzl(len +(len.pzl)) 1)
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-wrong-product
  =/  pzl
    =/  p  (grab-proof-entry:bp pf %puzzle 1)
    ?>  ?=(%puzzle -.p)
    ?>  ?=([@ *] p.p)  p
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %puzzle
        pzl(p [(succ -.p.pzl) +.p.pzl])
        1
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-product-not-based
  =/  pzl
    =/  p  (grab-proof-entry:bp pf %puzzle 1)
    ?>  ?=(%puzzle -.p)
    ?>  ?=([@ *] p.p)  p
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %puzzle
        pzl(p [(add -.p.pzl p) +.p.pzl])
        1
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-heights-not-after-puzzle
  =/  pf  (move-proof-entry:bp pf %heights 1)
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-no-heights
  =/  pf  (delete-proof-entry:bp pf %heights 1)
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-wrong-heights-1
  =/  heights
    =/  h  (grab-proof-entry:bp pf %heights 1)
    ?>  ?=(%heights -.h)  h
  =/  pf  (replace-proof-entry:bp pf %heights heights(p (turn p.heights succ)) 1)
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-wrong-heights-2
  =/  heights
    =/  h  (grab-proof-entry:bp pf %heights 1)
    ?>  ?=(%heights -.h)  h
  =/  pf  (replace-proof-entry:bp pf %heights heights(p (turn p.heights dec)) 1)
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-wrong-heights-3
  =/  heights
    =/  h  (grab-proof-entry:bp pf %heights 1)
    ?>  ?=(%heights -.h)  h
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %heights
        heights(p (turn p.heights |=(h=@ (mul 2 h))))
        1
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-too-many-heights
  =/  heights
    =/  h  (grab-proof-entry:bp pf %heights 1)
    ?>  ?=(%heights -.h)  h
  =/  pf  (replace-proof-entry:bp pf %heights heights(p (snoc p.heights 2)) 1)
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-long-proof
  =/  pf  pf(objects (snoc objects.pf *proof-data))
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-base-m-root-not-after-heights
  =/  pf  (replace-proof-entry:bp pf %m-root [%terms *bpoly] 1)
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-base-m-root-invalid
  =/  m-root
    =/  m  (grab-proof-entry:bp pf %m-root 1)
    ?>  ?=(%m-root -.m)  m
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %m-root
        m-root(p (twiddle-digest:bp p.m-root))
        1
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-base-m-root-not-based
  =/  m-root
    =/  m  (grab-proof-entry:bp pf %m-root 1)
    ?>  ?=(%m-root -.m)  m
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %m-root
        m-root(p (unbase-digest:bp p.m-root))
        1
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-ext-m-root-not-after-base-m-root
  =/  pf  (replace-proof-entry:bp pf %m-root [%terms *bpoly] 2)
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-ext-m-root-not-based
  =/  m-root
    =/  m  (grab-proof-entry:bp pf %m-root 2)
    ?>  ?=(%m-root -.m)  m
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %m-root
        m-root(p (unbase-digest:bp p.m-root))
        2
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-ext-m-root-invalid
  =/  m-root
    =/  m  (grab-proof-entry:bp pf %m-root 2)
    ?>  ?=(%m-root -.m)  m
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %m-root
        m-root(p (twiddle-digest:bp p.m-root))
        2
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-mega-m-root-invalid
  =/  m-root
    =/  m  (grab-proof-entry:bp pf %m-root 3)
    ?>  ?=(%m-root -.m)  m
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %m-root
        m-root(p (twiddle-digest:bp p.m-root))
        3
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-mega-m-root-not-based
  =/  m-root
    =/  m  (grab-proof-entry:bp pf %m-root 3)
    ?>  ?=(%m-root -.m)  m
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %m-root
        m-root(p (unbase-digest:bp p.m-root))
        3
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-mega-m-root-not-after-ext-m-root
  =/  pf  (replace-proof-entry:bp pf %m-root [%terms *bpoly] 3)
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-terminals-not-after-mega-m-root
  =/  pf  (replace-proof-entry:bp pf %terms [%evals *fpoly] 1)
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-terms-wrong-length-and-data-buffer
  =/  terms
    =/  t  (grab-proof-entry:bp pf %terms 1)
    ?>  ?=(%terms -.t)  t
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %terms
        terms(p (init-bpoly (weld (bpoly-to-list p.terms) ~[0])))
        1
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-terms-right-length-wrong-data-buffer
  =/  terms
    =/  t  (grab-proof-entry:bp pf %terms 1)
    ?>  ?=(%terms -.t)  t
  =/  ext-terms-poly  (init-bpoly (weld (bpoly-to-list p.terms) ~[0]))
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %terms
        terms(p [len.p.terms dat.ext-terms-poly])
        1
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-terms-wrong-length-right-data-buffer
  =/  terms
    =/  t  (grab-proof-entry:bp pf %terms 1)
    ?>  ?=(%terms -.t)  t
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %terms
        terms(p [+(len.p.terms) dat.p.terms])
        1
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-wrong-terms
  =/  terms
    =/  t  (grab-proof-entry:bp pf %terms 1)
    ?>  ?=(%terms -.t)  t
  %+  expect-eq  !>(%.n)
  !>
  %+  lien  (range len.p.terms)
  |=  idx=@
  %-  verify:vrf
  :_  [~ 4]
  %-  replace-proof-entry:bp
  :*  pf
      %terms
      terms(p (twiddle-bpoly:bp p.terms idx))
      1
  ==
::
++  test-bad-proof-comp-m-not-after-terms
  =/  pf  (move-proof-entry:bp pf %comp-m 1)
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-evals-not-after-comp-m
  =/  pf  (replace-proof-entry:bp pf %evals [%codeword *fpoly] 1)
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-comp-m-not-based
  =/  comp-m
    =/  c  (grab-proof-entry:bp pf %comp-m 1)
    ?>  ?=(%comp-m -.c)  c
  =/  pf  (replace-proof-entry:bp pf %comp-m comp-m(p (unbase-digest:bp p.comp-m)) 1)
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-trace-evals-wrong-length-and-data-buffer
  =/  trace-evals
    =/  e  (grab-proof-entry:bp pf %evals 1)
    ?>  ?=(%evals -.e)  e
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %evals
        trace-evals(p (init-fpoly (weld (fpoly-to-list p.trace-evals) ~[f0])))
        1
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-trace-evals-right-length-wrong-data-buffer
  =/  trace-evals
    =/  e  (grab-proof-entry:bp pf %evals 1)
    ?>  ?=(%evals -.e)  e
  =/  ext-trace-evals-poly  (init-fpoly (weld (fpoly-to-list p.trace-evals) ~[f0]))
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %evals
        trace-evals(p [len.p.trace-evals dat.ext-trace-evals-poly])
        1
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-trace-evals-wrong-length-right-data-buffer
  =/  trace-evals
    =/  e  (grab-proof-entry:bp pf %evals 1)
    ?>  ?=(%evals -.e)  e
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %evals
        trace-evals(p [+(len.p.trace-evals) dat.p.trace-evals])
        1
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-comp-evals-not-after-trace-evals
  =/  pf  (move-proof-entry:bp pf %evals 2)
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-comp-evals-wrong-length-and-data-buffer
  =/  comp-evals
    =/  e  (grab-proof-entry:bp pf %evals 2)
    ?>  ?=(%evals -.e)  e
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %evals
        comp-evals(p (init-fpoly (weld (fpoly-to-list p.comp-evals) ~[f0])))
        2
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-comp-evals-right-length-wrong-data-buffer
  =/  comp-evals
    =/  c  (grab-proof-entry:bp pf %evals 2)
    ?>  ?=(%evals -.c)  c
  =/  ext-comp-evals-poly  (init-fpoly (weld (fpoly-to-list p.comp-evals) ~[f0]))
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %evals
        comp-evals(p [len.p.comp-evals dat.ext-comp-evals-poly])
        2
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-comp-evals-wrong-length-right-data-buffer
  =/  comp-evals
    =/  e  (grab-proof-entry:bp pf %evals 2)
    ?>  ?=(%evals -.e)  e
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %evals
        comp-evals(p [+(len.p.comp-evals) dat.p.comp-evals])
        2
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-wrong-trace-evals
  =/  trace-evals
    =/  e  (grab-proof-entry:bp pf %evals 1)
    ?>  ?=(%evals -.e)  e
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %evals
        trace-evals(p (twiddle-fpoly:bp p.trace-evals 0))
        1
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-wrong-comp-evals
  =/  comp-evals
    =/  e  (grab-proof-entry:bp pf %evals 2)
    ?>  ?=(%evals -.e)  e
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %evals
        comp-evals(p (twiddle-fpoly:bp p.comp-evals 0))
        1
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-wrong-extra-evals
  =/  comp-evals
    =/  e  (grab-proof-entry:bp pf %evals 3)
    ?>  ?=(%evals -.e)  e
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %evals
        comp-evals(p (twiddle-fpoly:bp p.comp-evals 0))
        3
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-extra-trace-evals-wrong-length-and-data-buffer
  =/  trace-evals
    =/  e  (grab-proof-entry:bp pf %evals 3)
    ?>  ?=(%evals -.e)  e
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %evals
        trace-evals(p (init-fpoly (weld (fpoly-to-list p.trace-evals) ~[f0])))
        3
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-extra-trace-evals-right-length-wrong-data-buffer
  =/  trace-evals
    =/  e  (grab-proof-entry:bp pf %evals 3)
    ?>  ?=(%evals -.e)  e
  =/  ext-trace-evals-poly  (init-fpoly (weld (fpoly-to-list p.trace-evals) ~[f0]))
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %evals
        trace-evals(p [len.p.trace-evals dat.ext-trace-evals-poly])
        3
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-extra-trace-evals-wrong-length-right-data-buffer
  =/  trace-evals
    =/  e  (grab-proof-entry:bp pf %evals 3)
    ?>  ?=(%evals -.e)  e
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %evals
        trace-evals(p [+(len.p.trace-evals) dat.p.trace-evals])
        3
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-deep-m-root-not-after-comp-evals
  =/  pf  (replace-proof-entry:bp pf %m-root [%codeword *fpoly] 4)
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-deep-m-root-not-based
  =/  m-root
    =/  m  (grab-proof-entry:bp pf %m-root 4)
    ?>  ?=(%m-root -.m)  m
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %m-root
        m-root(p (unbase-digest:bp p.m-root))
        4
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-deep-m-root-invalid
  =/  m-root
    =/  m  (grab-proof-entry:bp pf %m-root 4)
    ?>  ?=(%m-root -.m)  m
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %m-root
        m-root(p (twiddle-digest:bp p.m-root))
        4
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-not-enough-m-roots-before-final-codeword
  =/  num-m-roots  (lent (grab-proof-entries:bp pf %m-root))
  =/  pf  (move-proof-entry:bp pf %m-root num-m-roots)
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
::  crashes if proof is small and FRI only sends one codeword in the clear
++  test-bad-proof-fri-m-root-invalid
  =/  m-root
    =/  m  (grab-proof-entry:bp pf %m-root 5)
    ?>  ?=(%m-root -.m)  m
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %m-root
        m-root(p (twiddle-digest:bp p.m-root))
        5
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-fri-m-root-not-based
  =/  m-root
    =/  m  (grab-proof-entry:bp pf %m-root 5)
    ?>  ?=(%m-root -.m)  m
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %m-root
        m-root(p (unbase-digest:bp p.m-root))
        4
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-codeword-doesnt-follow-m-roots
  =/  pf  (move-proof-entry:bp pf %codeword 1)
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-codeword-high-degree
  =/  codeword
    =/  c  (grab-proof-entry:bp pf %codeword 1)
    ?>  ?=(%codeword -.c)  c
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %codeword
        codeword(p (~(snoc fop p.codeword) f1))
        1
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-codeword-wrong-len
  =/  codeword
    =/  c  (grab-proof-entry:bp pf %codeword 1)
    ?>  ?=(%codeword -.c)  c
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %codeword
        codeword(len.p +(len.p.codeword))
        1
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-not-enough-m-paths-before-m-pathbfs
  =/  num-m-paths  (lent (grab-proof-entries:bp pf %m-path))
  =/  pf  (delete-proof-entry:bp pf %m-path num-m-paths)
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-m-pathbf-doesnt-follow-m-paths
  =/  pf  (replace-proof-entry:bp pf %m-pathbf [%m-path *proof-path] 1)
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-m-path-not-based
  =/  m-path
    =/  m  (grab-proof-entry:bp pf %m-path 1)
    ?>  ?=(%m-path -.m)  m
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %m-path
        m-path(path.p (unbase-noun-digests:bp path.p.m-path))
        1
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-m-pathbf-not-based
  =/  m-pathbf
    =/  m  (grab-proof-entry:bp pf %m-pathbf 1)
    ?>  ?=(%m-pathbf -.m)  m
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %m-pathbf
        m-pathbf(path.p (unbase-noun-digests:bp path.p.m-pathbf))
        1
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
::
++  test-bad-proof-spot-checks-fail-and-deep-evals-dont-match
  =/  m-paths  (grab-proof-entries:bp pf %m-path)
  %+  expect-eq  !>(%.n)
  !>
  %+  lien  (range (lent m-paths))
  |=  idx=@
  =/  m-path
    =/  m  (snag idx m-paths)
    ?>  ?=(%m-path -.m)  m
  %-  ~(. verify:vrf %.y)
  :_  [~ 4]
  %-  replace-proof-entry:bp
  :*  pf
      %m-path
      m-path(leaf.p (twiddle-fpoly:bp leaf.p.m-path 0))
      +(idx)
  ==
++  test-extra-composition-poly-canonicality
  =/  poly
    =/  m  (grab-proof-entry:bp pf %poly 1)
    ?>  ?=(%poly -.m)  m
  =/  padded=bpoly  (init-bpoly (snoc (bpoly-to-list p.poly) 0))
  =/  padded-proof=proof:sp
    %-  replace-proof-entry:bp
    :*  pf
        %poly
        [%poly padded]
        1
    ==
  %+  expect-eq  !>  [%.y %.n]
  !>
  :*  (canonical-pow-proof:dk [112.499 padded-proof])
      (canonical-pow-proof:dk [canonical-pow-polynomial-height:dk padded-proof])
  ==
:::
++  test-bad-proof-poly-wrong
  =/  poly
    =/  m  (grab-proof-entry:bp pf %poly 1)
    ?>  ?=(%poly -.m)  m
  =/  pf
    %-  replace-proof-entry:bp
    :*  pf
        %poly
        [%poly (twiddle-bpoly:bp p.poly 1)]
        1
    ==
  =/  res  (verify:vrf pf ~ 4)
  (expect-eq !>(%.n) !>(res))
--
::
::  helper functions
|_  stark-config
++  prv   ~(. np +<)
++  vrf   ~(. nv +<)
--
