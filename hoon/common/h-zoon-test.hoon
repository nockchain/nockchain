::  common/h-zoon-test.hoon
::
::    compile-time checks for hashed-key container boundaries.
::
/=  *  /common/h-zoon
/=  transact  /common/tx-engine-0
|%
++  h-test1
  |=  [a=(h-set hashed) b=hashed]
  ^-  ?
  (~(has h-in a) b)
++  h-test2
  |=  [a=(h-set noun-digest:tip5:z)]
  ^-  ?
  (~(has h-in a) (atom-to-digest:tip5:z `@ux`5))
:: this will not compile
:: ++  h-test3
::   |=  [a=(h-set @)]
::   ^-  ?
::   %.n
++  h-test4
  |=  [a=(h-set nname:transact)]
  ^-  ?
  %.n
++  h-test5
  |=  [a=(h-set noun-digest:tip5:z)]
  ^-  (z-set noun-digest:tip5:z)
  (hz-silt a)
++  h-test6
  |=  [a=(z-set noun-digest:tip5:z)]
  ^-  (h-set noun-digest:tip5:z)
  (zh-silt a)
:: this won't compile either
:: ++  h-test7
::   |=  [a=(z-set @)]
::   ^-  (h-set @)
::   (zh-silt a)
++  h-test8
  |=  [a=(h-map noun-digest:tip5:z @tas)]
  %-  ~(rep h-by a)
  |=  [[=noun-digest:tip5:z v=@tas] sum=_0]
  %+  add  sum
  `@`v
++  h-test9
  |=  [a=(h-map hashed @tas)]
  (~(put h-by a) [~ 'abc'])
:: this will not compile either
:: ++  h-test10
::   |=  [a=(h-map noun-digest:tip5:z @tas)]
::   (~(put h-by a) [~ 'abc'])
++  h-test11
  |=  [a=(h-map nname:transact @tas)]
  (~(put h-by a) [[(atom-to-digest:tip5:z 0x0) (atom-to-digest:tip5:z 0x0) ~] 'abc'])
:: this will not compile either
:: ++  h-test12
::   |=  [a=(h-map nname:transact @tas)]
::   (~(put h-by a) [~ 'abc'])
++  h-test13
  |=  [a=(h-set noun-digest:tip5:z)]
  (~(put h-in a) (atom-to-digest:tip5:z 0x0))
++  h-test14
  |=  [a=(z-jug noun-digest:tip5:z noun-digest:tip5:z)]
  (zh-jult a)
++  h-test15
  |=  [a=(z-mip noun-digest:tip5:z noun-digest:tip5:z ~)]
  (zh-milt a)
++  h-test16
  |=  [a=~]
  ^-  (h-map ~ @)
  =/  b=(h-map ~ @)  ~
  (~(put h-by b) [~ 4])
:: ++  h-test17
::   |=  [a=(h-map @ @)]
::   ^-  (h-map @ @)
::   a
++  h-test18
  |=  [a=(h-map noun-digest:tip5:z ~)]
  (hz-molt a)
++  h-test19
  |=  [a=(h-mip noun-digest:tip5:z noun-digest:tip5:z ~)]
  (h-test18 (~(got h-by a) (atom-to-digest:tip5:z 0x5)))
++  h-test20
  |=  [a=(h-mip noun-digest:tip5:z noun-digest:tip5:z ~)]
  (hz-molt (~(got h-by a) (atom-to-digest:tip5:z 0x5)))
:: This won't compile either
:: ++  h-test21
::   |=  [a=~]
::   ^-  (h-map noun-digests:z @)
::   =/  b=(h-map noun-digests:z @)  ~
::   (~(put h-by b) [(atom-to-digest:tip5:z 0x1) 0x5])
:: ++  h-test22
::   |=  [a=(h-set noun-digest:tip5:z)]
::   ^-  ?
::   (~(has z-in a) (atom-to-digest:tip5:z `@ux`5))
::  This won't compile: z-set uses ztree, h-in expects hset-tree
:: ++  h-test23
::   |=  [a=(z-set noun-digest:tip5:z)]
::   ^-  ?
::   (~(has h-in a) (atom-to-digest:tip5:z `@ux`5))
--
