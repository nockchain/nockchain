::  c2: mint_wtcl where gain and lose are both void (mod.rs ~4888-4890):
::  a face bound to a crashing arm is a non-void %hold, but refining it
::  collapses both branches, so the test becomes [%0 0] with no %toss hint.
|%
++  bad  !!
++  main
  |=  b=?
  =+  a=bad
  ?:  ?=(@ a)  !!  !!
--
