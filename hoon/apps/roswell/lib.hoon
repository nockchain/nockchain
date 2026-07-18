/=  *  /common/zeke
/=  sp  /common/stark/prover
/=  np  /common/nock-prover
/=  nv  /common/nock-verifier
|%
++  prv  np
++  vrf  nv
::
++  test-puzzle
  |=  [v=proof-version len=@ override=(unit (list term))]
  ^-  ?
  =/  puzzle  (snag 0 puzzles-list)
  ~&  "testing puzzle len {<len>}: {<puzzle>}"
  ?:  (test v -.puzzle +.puzzle len override)
    ~&  "  puzzle len {<len>} passed"
    %.y
  ~&  >>  "  #puzzle len {<len>} failed"
  %.n
::
::
++  test-verify
  |=  =proof
  ^-  ?
  (verify:vrf proof ~ 0)
::
++  prove-puzzle
  |=  [v=proof-version len=@ override=(unit (list term))]
  ^-  prove-result:sp
  =/  puzzle  (snag 0 puzzles-list)
  =/  in=prover-input:sp
    ?-  v
      %0  [%0 -:puzzle +:puzzle len]
      %1  [%1 -:puzzle +:puzzle len]
      %2  [%2 -:puzzle +:puzzle len]
      %3  ~|(%zk-prover-cannot-generate-v3-ai-proof !!)
    ==
  (prove:prv in)
::
::
++  make-proof-snapshot
  |=  [v=proof-version len=@ override=(unit (list term))]
  ^-  proof-snapshot:sp
  =/  puzzle  (snag 0 puzzles-list)
  =/  in=prover-input:sp
    ?-  v
      %0  [%0 -:puzzle +:puzzle len]
      %1  [%1 -:puzzle +:puzzle len]
      %2  [%2 -:puzzle +:puzzle len]
      %3  ~|(%zk-prover-cannot-generate-v3-ai-proof !!)
    ==
  (snapshot:prv in)
::
::  +proof-snapshot-for: make-proof-snapshot for a caller-supplied input instead of the
::  hardcoded test puzzle. Same prover work (snapshot:prv); only the prover-input differs.
++  proof-snapshot-for
  |=  [v=proof-version header=noun-digest:tip5 nonce=noun-digest:tip5 len=@]
  ^-  proof-snapshot:sp
  =/  in=prover-input:sp
    ?-  v
      %0  [%0 header nonce len]
      %1  [%1 header nonce len]
      %2  [%2 header nonce len]
      %3  ~|(%zk-prover-cannot-generate-v3-ai-proof !!)
    ==
  (snapshot:prv in)
::
++  make-proof-stream-window
  |=  [v=proof-version len=@ range=proof-stream-range:sp override=(unit (list term))]
  ^-  proof-stream-window-result:sp
  =/  puzzle  (snag 0 puzzles-list)
  =/  in=prover-input:sp
    ?-  v
      %0  [%0 -:puzzle +:puzzle len]
      %1  [%1 -:puzzle +:puzzle len]
      %2  [%2 -:puzzle +:puzzle len]
      %3  ~|(%zk-prover-cannot-generate-v3-ai-proof !!)
    ==
  (make-proof-stream-window:prv in range)
::
++  assemble-proof-stream
  |=  windows=(list proof-stream-window:sp)
  ^-  prove-result:sp
  (assemble-proof-stream:prv windows)
:::
++  assemble-proof-continuation
  |=  [snapshot=proof-snapshot:sp context=proof-stream-context:sp windows=(list proof-stream-window:sp)]
  ^-  prove-result:sp
  (assemble-proof-continuation:prv snapshot context windows)
::
++  test
  |=  [v=proof-version header=noun-digest:tip5 nonce=noun-digest:tip5 len=@ override=(unit (list term))]
  ^-  ?
  ~&  %proving
  =/  in=prover-input:sp
    ?-  v
      %0  [%0 header nonce len]
      %1  [%1 header nonce len]
      %2  [%2 header nonce len]
      %3  ~|(%zk-prover-cannot-generate-v3-ai-proof !!)
    ==
  =/  res  (prove:prv in)
  ?>  ?=(%& -.res)
  ~&  %verifying
  (verify:vrf proof.p.res override 0)
::
::
++  compute
  |=  [s=* f=*]
  ^-  *
  -:(fink:fock [s f] ~)
::
++  puzzles-list
  ^-  (list [noun-digest:tip5 noun-digest:tip5])
  :~
    :-  [ 7.944.925.381.601.331.412
          11.010.936.557.463.758.866
          975.990.832.031.042.959
          13.385.244.201.508.724.730
          12.705.105.841.993.571.334
         ]
    [ 3.447.968.191.261.276.012
      6.052.632.301.823.254.318
      8.423.335.959.106.168.490
      11.097.589.918.442.157.217
      13.640.939.506.338.565.287
    ]
  ==
--
