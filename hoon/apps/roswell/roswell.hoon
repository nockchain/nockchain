::  a kernel intended as a NockApp to replace the -test thread in Urbit
::
::  for now, it is specialized to whatever tests you put in the tests import.
/=  *  /common/wrapper
/=  *  /common/test
/=  sp  /common/stark/prover
/=  lib  /apps/roswell/lib
/=  np  /common/nock-prover
/=  nock-common  /common/v0-v1/nock-common
/=  dumb-tests  /tests/dumb/main
/=  h-zoon-benches  /tests/dumb/mod/benchmarks/h-zoon-hot-path
/=  crypto-tests  /tests/crypto/main
/=  wallet-tests  /tests/wallet/main
/=  zkvm-tests    /tests/zkvm/main
/=  zoon-tests  /tests/zoon/main
/=  bridge-tests  /tests/bridge/main
/=  bp  /tests/loki/bad-pow
/=  z  /common/zeke
=>
|%
+$  test-state
  $%  $:  %0
          proof=(unit proof:sp)
          test-proof=(unit proof:sp)
          snapshot=(unit proof-snapshot:sp)
          stream-window=(unit proof-stream-window:sp)
      ==
  ==
++  digest  noun-digest:tip5:z
++  moat  (keep test-state)
+$  cause
  $%  [%test name=term]                                 ::  test arms beginning with .name
      [%test-ci ~]                                     ::  test all
      [%test-crypto ~]
      [%test-dumb ~]
      [%bench-dumb ~]
      [%bench-h-zoon-noop salt=@]
      [%bench-h-zoon-z-map-build salt=@]
      [%bench-h-zoon-h-map-build salt=@]
      [%bench-h-zoon-z-map-read salt=@]
      [%bench-h-zoon-h-map-read salt=@]
      [%bench-h-zoon-z-map-update salt=@]
      [%bench-h-zoon-h-map-update salt=@]
      [%test-wallet ~]
      [%test-wallet-shard shard=@ total=@]
      [%test-zoon ~]
      [%test-bridge ~]
      [%test-verifier ~]
      [%prepare-verifier-bench v=proof-version:z n=@]
      [%bench-verifier v=proof-version:z]
      [%verify-proof p=(unit (unit proof:sp))]
      [%test-puzzle v=proof-version:z n=@ override=(unit (list term))]
      $:  %prove-puzzle
          v=proof-version:z
          n=@
          filename=(unit @t)
          override=(unit (list term))
      ==
      $:  %make-proof-snapshot
          v=proof-version:z
          n=@
          filename=(unit @t)
          override=(unit (list term))
      ==
      $:  %proof-snapshot-for
          v=proof-version:z
          header=digest
          nonce=digest
          len=@
      ==
      $:  %make-proof-stream-window
          v=proof-version:z
          n=@
          range=proof-stream-range:sp
          filename=(unit @t)
          override=(unit (list term))
      ==
      $:  %assemble-proof-stream
          windows=(list proof-stream-window:sp)
          filename=(unit @t)
      ==
      $:  %assemble-proof-continuation
          snapshot=proof-snapshot:sp
          context=proof-stream-context:sp
          windows=(list proof-stream-window:sp)
          filename=(unit @t)
      ==
      ::NOTE the remaining commands are not yet implemented on the rust side
      $:  %test-custom
          header=digest
          nonce=digest
          len=@
          override=(unit (list term))
      ==
      $:  %prove
          header=digest
          nonce=digest
          len=@
          override=(unit (list term))
      ==
      [%compute nock=*]                                 ::  compute nock
      [%file %write path=@t contents=@ success=?]       ::  file driver result poke
      [%dec-benchmark n=@]
      [%test-soft p=(pair @ merk-heap:merkle:z)]
  ==
::
::  exit codes:
::    0  -  success. all non-zero codes are failures.
::    1  -  generic failure.
+$  effect
  $%  [%exit code=@]                                    ::  instruction to exit
      [%file %write path=@t contents=@]
  ==
--
%-  (moat |)
^-  fort:moat
=<
|_  k=test-state
+*  util  +>
::
::  +load: upgrade from previous state
::
++  load
  |=  arg=test-state
  ^-  test-state
  arg
::
::  +peek: external inspect
::
++  peek
  |=  arg=path
  ^-  (unit (unit *))
  =/  =(pole)  arg
  ?+  pole  ~
      [%proof ~]
    `test-proof.k
      [%snapshot ~]
    `snapshot.k
      [%proof-stream-window ~]
    `stream-window.k
  ::
  ::
  ==
::
::  +poke: external apply
::
++  poke
  |=  [=wire eny=@ our=@ux now=@da dat=*]
  ^-  [(list effect) test-state]
  =/  soft-cau  ((soft cause) dat)
  ?~  soft-cau
    ~&  "could not mold poke type:"  !!
  =/  c=cause  u.soft-cau
  ::  The verifier benchmark times this entire poke.  Keep its path free of
  ::  slog rendering even when the tracing filter would suppress emission.
  =.  c
    ?:  =(%bench-verifier -.c)
      c
    ~&  "poked at {<now>}"
    c
  ?-    -.c    ::TODO why does this not work with ?+ ~|("invalid cause {<c>}" !!)
  ::
      %test-soft
    ~&  %soft-passed
    `k
  ::
      %file
    `k
  ::
      %test
    =.  proof.k  (some ~(get-proof util k))
    =/  test-arms=(list test-arm)
      (~(all-tests util k) name.c)
    =/  res=(list [ok=? =tang])
      (run-tests test-arms)
    =/  suc=?  (succeed res)
    :_  k
    [%exit suc]~
  ::
      %test-verifier
    =.  proof.k  (some ~(get-proof util k))
    =/  res=(list [ok=? =tang])
      (run-tests (zkvm-tests 'test' (need proof.k)))
    =/  suc=?  (succeed res)
    :_  k
    [%exit suc]~
  ::
      %prepare-verifier-bench
    ::  Proof construction is intentionally outside the measured benchmark
    ::  pokes.  +proof holds v2 and +test-proof holds v3.  Preparing them in
    ::  separate events avoids retaining both prover computations at once.
    =/  res=prove-result:sp  (prove-puzzle:lib v.c n.c ~)
    ?>  ?=(%& -.res)
    =/  p=proof:sp  p.res
    :_  ?-  v.c
          %2  k(proof `p)
          %3  k(test-proof `p)
          %0  ~|("verifier benchmark only supports proof versions 2 and 3" !!)
          %1  ~|("verifier benchmark only supports proof versions 2 and 3" !!)
          %4  ~|("verifier benchmark cannot construct AI proof artifacts" !!)
        ==
    [%exit -.res]~
  ::
      %bench-verifier
    =/  bench-proof=proof:sp
      ?-  v.c
        %2  (need proof.k)
        %3  (need test-proof.k)
        %0  ~|("verifier benchmark only supports proof versions 2 and 3" !!)
        %1  ~|("verifier benchmark only supports proof versions 2 and 3" !!)
        %4  ~|("verifier benchmark cannot verify AI proof artifacts" !!)
      ==
    =/  suc=?  (test-verify:lib bench-proof)
    :_  k
    [%exit suc]~
  ::
      %verify-proof
    =/  proof=proof:sp  (need (need p.c))
    =/  suc=?  ~>(%bout (test-verify:lib proof))
    ~&  res+suc
    :_  k
    [%exit suc]~
  ::
      %test-ci
    =.  proof.k  (some ~(get-proof util k))
    =/  res=(list [ok=? =tang])
      (run-tests ~(ci-tests util k))
    =/  suc=?  (succeed res)
    :_  k
    [%exit suc]~
  ::
      %test-dumb
    =/  res=(list [ok=? =tang])
      (run-tests (dumb-tests 'test-'))
    =/  suc=?  (succeed res)
    :_  k
    [%exit suc]~
  ::
      %bench-dumb
    =/  res=(list [ok=? =tang])
      (run-tests (dumb-tests 'bench-'))
    =/  suc=?  (succeed res)
    :_  k
    [%exit suc]~
  ::
      %bench-h-zoon-noop
    =/  res  (mule |.((bench-h-zoon-noop:h-zoon-benches salt.c)))
    =/  suc=?  ?=(%& -.res)
    :_  k
    [%exit suc]~
  ::
      %bench-h-zoon-z-map-build
    =/  res  (mule |.((bench-h-zoon-z-map-build:h-zoon-benches salt.c)))
    =/  suc=?  ?=(%& -.res)
    :_  k
    [%exit suc]~
  ::
      %bench-h-zoon-h-map-build
    =/  res  (mule |.((bench-h-zoon-h-map-build:h-zoon-benches salt.c)))
    =/  suc=?  ?=(%& -.res)
    :_  k
    [%exit suc]~
  ::
      %bench-h-zoon-z-map-read
    =/  res  (mule |.((bench-h-zoon-z-map-read:h-zoon-benches salt.c)))
    =/  suc=?  ?=(%& -.res)
    :_  k
    [%exit suc]~
  ::
      %bench-h-zoon-h-map-read
    =/  res  (mule |.((bench-h-zoon-h-map-read:h-zoon-benches salt.c)))
    =/  suc=?  ?=(%& -.res)
    :_  k
    [%exit suc]~
  ::
      %bench-h-zoon-z-map-update
    =/  res  (mule |.((bench-h-zoon-z-map-update:h-zoon-benches salt.c)))
    =/  suc=?  ?=(%& -.res)
    :_  k
    [%exit suc]~
  ::
      %bench-h-zoon-h-map-update
    =/  res  (mule |.((bench-h-zoon-h-map-update:h-zoon-benches salt.c)))
    =/  suc=?  ?=(%& -.res)
    :_  k
    [%exit suc]~
  ::
      %test-crypto
    =/  res=(list [ok=? =tang])
      (run-tests (crypto-tests 'test-'))
    =/  suc=?  (succeed res)
    :_  k
    [%exit suc]~
  ::
      %test-wallet
    =/  res=(list [ok=? =tang])
      (run-tests (wallet-tests 'test-'))
    =/  suc=?  (succeed res)
    :_  k
    [%exit suc]~
  ::
      %test-wallet-shard
    =/  tests=(list test-arm)  (wallet-tests 'test-')
    =/  res=(list [ok=? =tang])
      (run-tests (shard-tests tests shard.c total.c))
    =/  suc=?  (succeed res)
    :_  k
    [%exit suc]~
  ::
      %test-zoon
    =/  res=(list [ok=? =tang])
      (run-tests (zoon-tests 'test-'))
    =/  suc=?  (succeed res)
    :_  k
    [%exit suc]~
  ::
      %test-bridge
    =/  res=(list [ok=? =tang])
      (run-tests (bridge-tests 'test'))
    =/  suc=?  (succeed res)
    :_  k
    [%exit suc]~
  ::
      %test-puzzle
    ~&  >>>  test-puzzle-len+n.c
    ~&  %test-puzzle
    ~&  v+v.c
    =/  suc=?  ~>(%bout (test-puzzle:lib v.c n.c override.c))
    :_  k
    [%exit suc]~
      %prove-puzzle
    ~&  >>>  prove-puzzle+[len=n.c filename=filename.c]
    =/  res=prove-result:sp
      ~>(%bout (prove-puzzle:lib v.c n.c override.c))
    ?>  ?=(%& -.res)
    =/  p=proof:sp  p.res
    =/  size  (met 3 (jam p))
    ~&  size+size
    :_  k(test-proof `p)
    ?~  filename.c
      ~[[%exit -.res]]
    =/  filename=@t
      (cat 3 u.filename.c (crip ".jam"))
    :~  [%file %write filename (jam p.res)]
        [%exit -.res]
    ==
  ::
      %make-proof-snapshot
    ~&  >>>  make-proof-snapshot+[len=n.c filename=filename.c]
    =/  res=proof-snapshot:sp
      ~>(%bout (make-proof-snapshot:lib v.c n.c override.c))
    =/  size  (met 3 (jam res))
    ~&  size+size
    :_  k(snapshot `res)
    ?~  filename.c
      ~[[%exit %.y]]
    =/  filename=@t
      (cat 3 u.filename.c (crip ".jam"))
    :~  [%file %write filename (jam res)]
        [%exit %.y]
    ==
  ::
      %proof-snapshot-for
    =/  res=proof-snapshot:sp
      ~>(%bout (proof-snapshot-for:lib v.c header.c nonce.c len.c))
    :_  k(snapshot `res)
    [%exit %.y]~
  ::
      %make-proof-stream-window
    ~&  >>>  make-proof-stream-window+[len=n.c range=range.c filename=filename.c]
    =/  res=proof-stream-window-result:sp
      ~>(%bout (make-proof-stream-window:lib v.c n.c range.c override.c))
    ?-    -.res
        %|
      :_  k
      [%exit %.n]~
    ::
        %&
      =/  window=proof-stream-window:sp  p.res
      =/  size  (met 3 (jam window))
      ~&  size+size
      :_  k(stream-window `window)
      ?~  filename.c
        ~[[%exit %.y]]
      =/  filename=@t
        (cat 3 u.filename.c (crip ".jam"))
      :~  [%file %write filename (jam window)]
          [%exit %.y]
      ==
    ==
  ::
      %assemble-proof-stream
    ~&  >>>  assemble-proof-stream+[filename=filename.c]
    =/  res=prove-result:sp
      ~>(%bout (assemble-proof-stream:lib windows.c))
    ?-    -.res
        %|
      :_  k
      [%exit %.n]~
    ::
        %&
      =/  p=proof:sp  p.res
      =/  size  (met 3 (jam p))
      ~&  size+size
      :_  k(test-proof `p)
      ?~  filename.c
        ~[[%exit %.y]]
      =/  filename=@t
        (cat 3 u.filename.c (crip ".jam"))
      :~  [%file %write filename (jam p)]
          [%exit %.y]
      ==
    ==
  ::
      %assemble-proof-continuation
    ~&  >>>  assemble-proof-continuation+[filename=filename.c]
    =/  res=prove-result:sp
      ~>(%bout (assemble-proof-continuation:lib snapshot.c context.c windows.c))
    ?-    -.res
        %|
      :_  k
      [%exit %.n]~
    ::
        %&
      =/  p=proof:sp  p.res
      =/  size  (met 3 (jam p))
      ~&  size+size
      :_  k(test-proof `p)
      ?~  filename.c
        ~[[%exit %.y]]
      =/  filename=@t
        (cat 3 u.filename.c (crip ".jam"))
      :~  [%file %write filename (jam p)]
          [%exit %.y]
      ==
    ==
  ::
  ::
      %test-custom
    ~&  test+[header nonce len]:c
    =/  res
      ~>(%bout (test:lib %2 header.c nonce.c len.c override.c))
    :_  k
    [%exit -.res]~
  ::
      %prove
    ~&  prove+[header nonce len]:c
    =/  res
      ~>(%bout (prove:np %0 header.c nonce.c len.c))
    =/  size  (met 3 (jam res))
    ~&  size+size
    :_  k
    [%exit -.res]~
  ::
      %compute
    ~&  compute+nock.c
    =/  res  (compute:lib ;;([* *] nock.c))
    ~&  >>  res
    :_  k
    [%exit 0]~
  ::
      %dec-benchmark
    |^
    =/  d  (unjetted-dec n.c)
    ~>  %slog.[0 %leaf^"finished running dec"]
    [[%exit 0]~ k]
    ::
    ++  unjetted-dec
      |=  a=@
      ~_  leaf+"decrement-underflow"
      ?<  =(0 a)
      =+  b=0
      ::  decremented integer
      |-  ^-  @
      ?:  =(a +(b))  b
      $(b +(b))
    --
  ==
--
::
|_  k=test-state
::
++  get-proof
  ^-  proof:sp
  ?^  proof.k  ~&  >>  %proof-exists  u.proof.k
  ~&  %generating-proof
  =/  res  (prove-puzzle:lib %2 testing-pow-len:bp ~)
  ?>  ?=(%& -.res)
  +.res
::
++  all-tests
  |=  name=term
  ^-  (list test-arm)
  ;:  weld
      (dumb-tests name)
      (wallet-tests name)
      (crypto-tests name)
      (zoon-tests name)
      (bridge-tests name)
      (zkvm-tests name (need proof.k))
  ==
::  deterministically shard wallet tests by index modulo total
++  shard-tests
  |=  [tests=(list test-arm) shard=@ total=@]
  ^-  (list test-arm)
  ~|  [%invalid-shard shard total]
  ?>  ?&((gth total 0) (lth shard total))
  =/  acc=(list test-arm)  ~
  =/  idx=@  0
  |-  ^-  (list test-arm)
  ?~  tests  (flop acc)
  =/  keep=?  =(shard (mod idx total))
  $(tests t.tests, idx +(idx), acc ?:(keep [i.tests acc] acc))
::
++  ci-tests
  ^-  (list test-arm)
  ;:  weld
      (dumb-tests 'test-')
      (crypto-tests 'test-')
      (wallet-tests 'test-')
      (zoon-tests 'test-')
      (bridge-tests 'test')
      (zkvm-tests 'test-' (need proof.k))
  ==
--
