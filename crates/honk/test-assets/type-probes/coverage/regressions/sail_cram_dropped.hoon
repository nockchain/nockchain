::  regression: sail markdown that hoon-138 drops. The last paragraph of a
::  markdown block is closed after the error check, so one that does not
::  parse (a tab, a heading of seven `#`) is silently left out.
|%
++  main  !>(..main)
++  d1
  !.
  ;>
    kept

    dropped	tab
++  d2
  !.
  ;div
    kept

    ####### seven
  ==
--
