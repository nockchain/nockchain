::  c2 regression: a +| chapter named twice keeps the FIRST chapter's doc
::  (none here) and a lone [%eror "duplicate chapter: |aa"] arm $ (++wisp);
::  ^+ only plays the core, so the build succeeds with that in the type.
|%
++  main
  ^+  |%
      +|  %aa
      ++  x  1
      ::    second chapter doc
      +|  %aa
      ++  y  2
      --
  !!
--
