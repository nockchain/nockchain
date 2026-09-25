::  p4 DIVERGENT: irregular spec `=a=@`. hoonc (+scad `=` branch) names it
::  `(cat 3 'a' (cat 3 '-' (autoname @)))` = %a-atom; hatch runes/buc.rs
::  buctis_irregular (~619-622) uses the bare prefix %a, so the face in the
::  `$:` mold differs.
|%
++  main  !>(..main)
++  t  $:(=a=@ b=@)
--
