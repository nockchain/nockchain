::  A sail markdown paragraph that does not parse (a tab) is an error
::  when a list item ends it rather than a blank line (hoon-138 ++cram
::  ++line, ++close-par).
|%
++  main
  ;>
    a	b
    - item
--
