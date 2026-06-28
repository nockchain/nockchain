::  Minimal extraction for the hoon-138 failure rooted from line 3820 (`++tail`)
::  that reduces all the way down to `++ob`/`++raku`.
::  Repro target: `arm ob: arm raku: mint-nice`.
|%
++  ob
  |%
  ++  raku
    ^-  (list @ux)
    :~  0xb76d.5eed
        0xee28.1300
        0x85bc.ae01
        0x4b38.7af7
    ==
  --
--
