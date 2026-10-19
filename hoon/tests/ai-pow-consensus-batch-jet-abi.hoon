::  Production height-selected admission through the mandatory verifier jet.
::  Standard compilation supplies the dependency hash to the outer gate;
::  evaluating the compiled trap returns the height-selected verifier gate.
/=  mine  /common/pow
/=  txe  /common/tx-engine
|=  hash=@uvI
=+  t=~(. txe *blockchain-constants:txe)
|=  [height=@ud artifact=* commit=* target=@]
^-  ?
%:  ai-pow-verify:mine
  (ai-pow-proof-rules:page:t height)
  artifact
  commit
  target
==
