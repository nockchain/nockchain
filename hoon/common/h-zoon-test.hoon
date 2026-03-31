/=  *  /common/h-zoon
/=  transact  /common/tx-engine
|%
++  h-test1
  |=  [a=(h-set hashed) b=hashed]
  ^-  ?
  (~(has h-in a) b)
++  h-test2
  |=  [a=(h-set noun-digest:tip5:z)]
  ^-  ?
  (~(has h-in a) (atom-to-digest:tip5:z `@ux`5))
:: This will not compile
:: ++  h-test3
::   |=  [a=(h-set @)]
::   ^-  ?
::   %.n
++  h-test4
  |=  [a=(h-set nname:transact)]
  ^-  ?
  %.n
--
