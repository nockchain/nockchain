::  p3: postfix docs on a tall ;~ (utils.rs apply_hoon_docs MicSig arm:
::  token_count <= 1, arg index in range, arg index past the args)
|%
++  a  ;~  plug  (just 'a')  (just 'b')  ::  two rules
       ==
++  b  ;~  plug  (just 'a')  ==  ::  closed on one line
++  c  ;~  plug  ::  just the gate
         (just 'a')
       ==
++  main  [!>(a) !>(b) !>(c)]
--
