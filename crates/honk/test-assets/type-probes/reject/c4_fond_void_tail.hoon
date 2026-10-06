::  !@ of a two-limb wing whose rest searches a void type: the subject head
::  is the %hold of an arm that returns itself, which the search cuts to
::  void. hoon-138 ++fond has no ?~ guard on the rest of the wing, so it
::  crashes instead of letting feel answer no.
|%
++  main
  |=  a=@
  =+  loop
  !@(x.zz 1 2)
++  loop  loop
--
