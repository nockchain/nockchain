|%
++  bmonth  4.383
++  byear   ^~((mul 12 4.383))
++  atoms-per-nock  ^~((bex 16))
::
++  schedule
  |=  block-num=@
  ^-  @  :: emission is number of atoms
  ?:  =(0 block-num)  0  :: no coins in genesis block
  :: least inconvenient offset to deal with coinless
  :: genesis block
  =.  block-num  (dec block-num)
  =;  emit=@
    ?:  (gte block-num (add 2 (mul byear 191)))
      :: rate goes to 0 at 191 years and 2 blocks. not strictly
      :: necessary since the algorithm would do so anyways, but
      :: it makes it clear exactly when emissions stop.
      ?>  =(0 emit)  0
    ?>  !=(0 emit)  emit
  =/  rate  ^~((mul (bex 16) atoms-per-nock))
  =?  rate  (gth block-num (mul bmonth 3))
    (div rate 2)
  =?  rate  (gth block-num (mul bmonth 9))
    (div rate 2)
  =?  rate  (gth block-num (mul bmonth 18))
    (div rate 2)
  =?  rate  (gth block-num (mul byear 3))
    (div rate 2)
  =?  rate  (gth block-num (mul byear 5))
    (div rate 2)
  =?  rate  (gth block-num (mul byear 8))
    (div rate 2)
  =?  rate  (gth block-num (mul byear 12))
    (div rate 2)
  =?  rate  (gth block-num (mul byear 17))
    (div rate 2)
  =?  rate  (gth block-num (mul byear 23))
    (div rate 2)
  ?.  (gth block-num (mul byear 30))
    rate
  =:  rate       (div rate 2)
      block-num  (sub block-num (mul byear 30))
    ==
  |-
  ?:  (gth block-num (mul byear 7))
    $(rate (div rate 2), block-num (sub block-num (mul byear 7)))
  rate
::
++  total-supply
  |=  max-block=@
  ^-  @
  =/  cur-block  0
  =/  sum-atoms  0
  |-
  ?:  =(cur-block max-block)
    sum-atoms
  %_  $
    cur-block  +(cur-block)
    sum-atoms  (add sum-atoms (schedule cur-block))
  ==
::
++  supply-evolution
  |=  max-block=@
  ^-  (list @)
  =/  cur-block  0
  =/  sum-atoms  0
  =/  lis=(list @)  ~[0]
  |-
  ?:  =(cur-block max-block)
    (flop lis)
  =:  cur-block  +(cur-block)
      sum-atoms  (add sum-atoms (schedule cur-block))
    ==
  =.  lis  [sum-atoms lis]
  $
--
