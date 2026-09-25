::  A zero head group takes no further digit groups: ++ape parses a lone 0
::  and ends the number, so both compilers reject 0x0.0000 (hatch
::  hexadecimal_number).
|%
++  main  0x0.0000
--
