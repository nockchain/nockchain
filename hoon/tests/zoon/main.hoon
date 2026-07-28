/=  tt  /common/test
::  Roswell aggregation excludes z/h container suites while their cold-hint
::  paths emit per-operation `unknown jet` logs.
:: /=  z-map-tests  /tests/zoon/mod/z-map
:: /=  z-set-tests  /tests/zoon/mod/z-set
/=  ordering-tests  /tests/zoon/mod/ordering
:: /=  balance-tests  /tests/zoon/mod/balance
:: /=  h-zoon-tests  /tests/zoon/mod/h-zoon
|=  name=term
^-  (list test-arm:tt)
;:  weld
::  (get-prefix-arms:tt name !>(z-map-tests))
::  (get-prefix-arms:tt name !>(z-set-tests))
  (get-prefix-arms:tt name !>(ordering-tests))
::  (get-prefix-arms:tt name !>(balance-tests))
::  (get-prefix-arms:tt name !>(h-zoon-tests))
==
