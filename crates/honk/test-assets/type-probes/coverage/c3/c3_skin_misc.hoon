::  c3: gain_skin_inner / lose_skin_inner for %name, %spec (^*), mold-name terms and %over skins under ?:
::  (mod.rs ~9679 Term, ~9739 Name, ~9750 Spec, ~10019, ~10065).
|%
+$  num  @ud
+$  duo  [@ @]
++  main
  |=  [a=* b=@ud c=[@ @] d=$@(@ [@ @])]
  :*  !>(?:(?#(x=@ a) a a))
      !>(?:(?#(x=[y=@ z=@] a) a a))
      !>(?:(?#(^*(@ud) b) b !!))
      !>(?:(?#(^*([@ @]) c) c !!))
      !>(?:(?#(num b) b !!))
      !>(?:(?#(duo c) c !!))
      !>(?:(?#(=>(+3 @) a) a a))
  ==
--
