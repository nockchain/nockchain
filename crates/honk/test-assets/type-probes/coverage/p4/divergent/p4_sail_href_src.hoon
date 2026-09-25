::  p4 DIVERGENT (HOONC-ONLY): the sail `/"url"` (href) and `@"url"` (src)
::  tag-head shorthands (hoon-138 ++tag-head). hatch runes/sail.rs
::  `tag_head` has neither, so honk fails to parse.
|%
++  main  !>(..main)
++  s1  ;a/"x";
++  s2  ;img@"y";
--
