::  p4 regression: irregular spec `=a=@`. hoonc (+scad `=` branch) names it
::  `(cat 3 'a' (cat 3 '-' (autoname @)))` = %a-atom; hatch runes/buc.rs
::  buctis_irregular once used the bare prefix %a for the `$:` face.
|%
++  main  !>(..main)
++  t  $:(=a=@ b=@)
--
