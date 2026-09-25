::  p4 DIVERGENT (HOONC-ONLY): the sail text tail `;p: text` (hoon-138
::  ++tall-tail `;~(pfix col ace (cook collapse-chars quote-innards))`).
::  hatch runes/sail.rs `tag_tail` only takes `:` before a braced hoon, so
::  honk fails to parse.
|%
++  main  !>(..main)
++  s1
  ;div
    ;p: hello
  ==
--
