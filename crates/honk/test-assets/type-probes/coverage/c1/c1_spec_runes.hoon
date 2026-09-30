::  c1: $< and $& specs, whose Sig64 cache-signature arms the corpus misses
::  (mod.rs write_spec ~792 BucGal, ~827 BucPam); also exercises their
::  ++ax openings end to end.
|%
+$  ab  $%([%a p=@] [%b q=@])
+$  only-a  $<(%b ab)
+$  only-b  $>(%b ab)
+$  norm
  $&  @
  |=(a=@ a)
++  main
  |=  [x=only-a y=only-b z=norm]
  :*  !>(x)
      !>(y)
      !>(z)
      !>(*only-a)
      !>(*norm)
      !>(`only-a`[%a 1])
  ==
--
