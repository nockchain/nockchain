::  p4 DIVERGENT (HOONC-ONLY): sail markdown. hoon-138 parses bare text
::  lines among an element's tall children, and `;>` blocks, with ++cram;
::  hatch runes/sail.rs has no markdown parser, so honk fails to parse.
|%
++  main  !>(..main)
++  m1
  ;div
    some *text*
  ==
--
