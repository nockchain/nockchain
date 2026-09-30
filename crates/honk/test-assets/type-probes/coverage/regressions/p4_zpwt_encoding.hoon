::  p4 DIVERGENT: `!?` hoon encoding. hoonc stores `[%zpwt p=$@(@ [@ @]) q]`
::  with the version numbers as plain atoms (138, [138 138]); hatch
::  utils.rs zpwt_arg_to_noun (~13716) emits `[%atom '138']` and
::  `[%pair '138' '138']` (tagged cords of the digit text), so any `!?` arm
::  body pinned into a type mismatches.
|%
++  main  !>(..main)
++  a-zpwt  !?  138  5
--
