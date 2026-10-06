::  p4 DIVERGENT: wide `$&(spec hoon)`. hoonc's `$&` rune has a wide form
::  (rune pam %bcpm exqc); hatch runes/buc.rs bucpam_wide (~354) and
::  bucpam_spec_wide (~805) omit `.delimited_by('(' ')')`, so honk fails to
::  parse `$&(@ |=(a=@ a))`.
|%
++  main  !>(..main)
++  p1  $&(@ |=(a=@ a))
--
