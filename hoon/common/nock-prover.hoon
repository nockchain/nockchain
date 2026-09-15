/=  *  /common/zeke
/=  stark-prover  /common/stark/prover
/#  softed-constraints
::
|%
::
++  prover
  =|  in=stark-input
  ::  +<+< = stark-engine door sample wrt stark-verifier core
  =/  sc=stark-config
    %*  .  *stark-config
      prep  softed-constraints
    ==
  %_    stark-prover
      +<+<
    %_  in
      stark-config        sc
    ==
  ==
::
++  prove
  |=  input=prover-input:stark-prover
  (prove:prover input)
::
++  snapshot
  |=  input=prover-input:stark-prover
  ^-  proof-snapshot:stark-prover
  (snapshot:prover input)
::
++  make-proof-stream-window
  |=  [input=prover-input:stark-prover range=proof-stream-range:stark-prover]
  ^-  proof-stream-window-result:stark-prover
  (make-proof-stream-window:prover input range)
::
++  assemble-proof-stream
  |=  windows=(list proof-stream-window:stark-prover)
  ^-  prove-result:stark-prover
  (assemble-proof-stream:prover windows)
:::
++  assemble-proof-continuation
  |=  [snapshot=proof-snapshot:stark-prover context=proof-stream-context:stark-prover windows=(list proof-stream-window:stark-prover)]
  ^-  prove-result:stark-prover
  (assemble-proof-continuation:prover snapshot context windows)
--
