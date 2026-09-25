::  Honest-proof integration harness: use the production rule selector and gate.
::  Compile with honk and pass its jam to the Rust verifier_jet_abi test.
/=  mine  /common/pow
/=  txe  /common/tx-engine
=+  t=~(. txe *blockchain-constants:txe)
|=  [height=@ud artifact=* commit=* target=@]
^-  ?
%:  ai-pow-verify:mine
  (ai-pow-proof-rules:page:t height)
  artifact
  commit
  target
==
