::  p1: autoname (utils.rs ~2999) tests the aura against "$", but the parser
::  gives @ the aura "", so =@ names the sample %$ instead of atom; honk then
::  crashes (SIGSEGV) compiling the gate. hoonc names it atom.
|%
++  main
  |=  a=@
  !>(|=(=@ atom))
--
