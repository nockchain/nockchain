::  p4 DIVERGENT (MISMATCH): a non-ASCII sail attribute value parsed without
::  dbug. hoonc keeps each UTF-8 byte of "hé" as its own beer char (195 169);
::  hatch runes/sail.rs parsed_atom_to_cord (~42-54) turns the multi-byte
::  woof atom into one Unicode scalar via char::from_u32, so the beer list
::  differs.
|%
++  main  !>(..main)
++  s4
  !.
  ;div(title "hé");
--
