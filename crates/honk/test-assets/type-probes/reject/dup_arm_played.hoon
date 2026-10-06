::  Nesting against a hold on an arm that the parser replaced with [%eror
::  "duplicate arm: +x"] (++whap) plays the arm, which crashes: hoon-138
::  +open has no expansion for %eror.
|%
++  main
  ^-  @
  ^+  =<  x
      |%
      ++  x  1
      ++  x  2
      --
  !!
--
