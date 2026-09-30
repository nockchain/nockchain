::  p4 regression: a non-ASCII sail attribute value parsed without dbug.
::  hoonc keeps each UTF-8 byte of "hé" as its own beer char (195 169): the
::  tape lexer (utils.rs soil) yields one woof per byte and runes/sail.rs
::  keeps one beer char per woof byte.
|%
++  main  !>(..main)
++  s4
  !.
  ;div(title "hé");
--
