::  After a rule in a sail markdown list item, a line whose text starts
::  inside the item's indentation is an error (hoon-138 ++cram
::  ++read-line, expected-indent).
|%
++  main
  ;>
    - item
      ---
     x
--
