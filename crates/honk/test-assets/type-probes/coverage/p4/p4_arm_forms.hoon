::  p4: forms stored as arm bodies: a wide `|$` (bar.rs barbuc_wide), an
::  obsolete `$5` leaf (utils.rs hoon_to_noun_uncached %leaf), `=a=$`,
::  whose autoname %$ names the face `a-` (buc.rs buctis_irregular), and a
::  `/foo` spec (utils.rs spec_to_noun %loop).
|%
++  main  !>(..main)
++  $  @ud
++  foo  @ud
++  w  |$(a (list a))
++  l  $5
++  t  $:(=a=$ b=@)
++  g  |=(a=/foo a)
--
