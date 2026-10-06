::  c2: %= with edits at different depths, in both orders, so hike_insert
::  (mod.rs ~7006) folds an edit into one above it (+16 inside +2) and
::  keeps edits that sit side by side (+2 and +7). axis_contains decides
::  which; mutation testing found no earlier probe or test that checks it.
=/  a  [[[[1 2] 3] 4] [5 6]]
:*  a(+16 9, +2 [7 8])
    a(+2 [7 8], +16 9)
    a(+2 0, +7 1)
    a(+7 1, +2 0)
    a(+4 0, +7 1, +13 2)
==
