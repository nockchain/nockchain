::  Block emission schedule.
::
::  Returns NOCK-atom emission for a given block number, spanning genesis
::  through the 2^32-NOCK hard cap. Pre-activation blocks (1..=65,500)
::  preserve the original chain history with three power-of-two halvings
::  at heights 13,150 / 39,448 / 65,500. Eon 2 is truncated from its
::  original end at block 78,895 to the new activation block 65,500;
::  the 13,395 blocks of legacy eon-2 emission that would have followed
::  are absorbed into the new post-activation budget.
::
::  Post-activation (block > 65,500), the schedule is the unified
::  Aletheia table: a 6-month activation era at 2,048 NOCK/block, then
::  nine 1-year eras with alternating 25% / 33% drops down to a 64-NOCK
::  floor, then a long flat tail until the 2^32 hard cap is met exactly.
::
::  Cap accounting. Eons 0/1/2 emit 65,536 / 32,768 / 16,384 NOCK per
::  block (powers of two, the actual on-chain history). The Aletheia
::  schedule doc rounds these to multiples of 5 (65,540 / 32,770 /
::  16,380) for table readability, but the chain has already emitted
::  the power-of-two values, so the implementation preserves them.
::  Cumulative supply through end-of-decay (block 2,060,500) =
::  3,393,567,232 NOCK. Remaining budget to cap = 901,400,064 NOCK,
::  which is exactly 14,084,376 tail blocks at 64 NOCK each. Therefore
::  emission-tail-end = 2,060,500 + 14,084,376 = 16,144,876, every tail
::  block emits 64 NOCK, and no per-block carve-out for "dust" is
::  needed.
|%
++  atoms-per-nock  ^~((bex 16))
::
::  Era boundaries (block heights, inclusive end of each era).
::    eon-0-end     :: end of original eon 0 (3-month halving)
::    eon-1-end     :: end of original eon 1 (9-month halving)
::    activation    :: end of (truncated) eon 2 + Aletheia activation
::    eon-3-end     :: end of 6-month activation era at 2,048 NOCK
::    decay-end     :: end of decay phase (eon 12, last 96-NOCK block)
::    tail-end      :: last block at which emission is non-zero
++  eon-0-end       13.150
++  eon-1-end       39.448
++  activation      65.500
++  eon-3-end       170.500
++  decay-end       2.060.500
++  tail-end        16.144.876
++  era-blocks      210.000
::
::  Per-block reward in NOCK for each post-activation 1-year era beyond
::  eon 3. Index = era - 4 (era 4 = 1,536 NOCK, era 12 = 96 NOCK).
++  decay-rewards
  ^-  (list @)
  ~[1.536 1.024 768 512 384 256 192 128 96]
::
++  schedule
  |=  block-num=@
  ^-  @  :: emission is number of atoms
  ?:  =(0 block-num)  0
  ?:  (gth block-num tail-end)  0
  ::  Pre-activation eons preserve the actual on-chain emission
  ::  (powers of two, not the rounded doc values).
  ?:  (lte block-num eon-0-end)
    ^~((mul (bex 16) atoms-per-nock))         :: 65,536 NOCK
  ?:  (lte block-num eon-1-end)
    ^~((mul (bex 15) atoms-per-nock))         :: 32,768 NOCK
  ?:  (lte block-num activation)
    ^~((mul (bex 14) atoms-per-nock))         :: 16,384 NOCK
  ::  Eon 3: 6-month activation era at 2,048 NOCK.
  ?:  (lte block-num eon-3-end)
    ^~((mul 2.048 atoms-per-nock))
  ::  Decay phase (eons 4..=12): 9 one-year eras of 210k blocks each.
  ?:  (lte block-num decay-end)
    =/  era-idx=@  (div (sub block-num +(eon-3-end)) era-blocks)
    (mul (snag era-idx decay-rewards) atoms-per-nock)
  ::  Tail: 64 NOCK/block until the cap is hit at tail-end.
  ^~((mul 64 atoms-per-nock))
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
