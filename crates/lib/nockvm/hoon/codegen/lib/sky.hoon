/-  *sock
/+  ska
|%
:: mask axes in a noun to make a sock
++  dope
  |=  [mask=(list @) non=noun]
  ^-  boot
  =/  sack=boot  [%safe %know non]
  |-
  ^-  boot
  ?~  mask  sack
  $(sack (welt:ska i.mask [%safe %toss ~] sack), mask t.mask)
:: turn a hoon type into a boot
++  wove
  |=  kine=type
  ^-  boot
  =|  gil=(set type)
  ?@  kine
    ?-  kine
      %noun  [%risk %toss ~]
      %void  [%boom ~]
    ==
  ?-  -.kine
      %atom
    ?~  q.kine
      [%risk %dice ~]
    [%risk %know u.q.kine]
  ::
      %cell
    (cobb:ska $(kine p.kine) $(kine q.kine))
  ::
      %core
    %+  cobb:ska
      (spry p.r.q.kine) :: compiled battery
    $(kine p.kine) :: current payload
  ::
      %face
    $(kine q.kine)
  ::
      %fork
    =/  tins  ~(tap in p.kine)
    ?~  tins  [%boom ~]
    =/  hypo  $(kine i.tins)
    =/  tons  t.tins
    |-
    ^-  boot
    ?~  tons  hypo
    $(hypo (gnaw:ska ^$(kine i.tons) hypo), tons t.tons)
  ::
      %hint
    $(kine q.kine)
  ::
      %hold
    ?:  (~(has in gil) kine)
      [%risk %toss ~]
    $(gil (~(put in gil) kine), kine ~(repo ut kine))
  ==
:: turn a seminoun into a sock
++  spry
  |=  seminoun
  ^-  boot
  ?-  -.mask
      %half
    ?>  ?=(^ data)
    (cobb:ska $(mask left.mask, data -.data) $(mask rite.mask, data +.data))
  ::
      %full
    ?~  blocks.mask
      [%risk %know data]
    [%risk %toss ~]
  ::
      %lazy
    [%risk %toss ~]
  ==
::  for a stateful core, figure out what we can assume across all state
::  transitions
::  
::  step is a list of arm axes and result axes which are expected to produce gates
::  the gates will be simul-slammed with %toss
::  then the result axis will be intersected with the stateful core
::  knowledge
::  
::  fixed point termination argument: we can only know the same or less
::  than what we knew last time (intersection cannot add knowledge)
::  if we know the same, we stop now. We can only subtract finitely many
::  axes of knowledge from the tree before we know [%boom ~] or
::  [%risk %toss ~] at which point we will learn the same thing twice
::  and terminate
++  arid
  |=  [muck=boot step=(list [@ @])]
  ^-  boot
  =/  yuck  muck
  =/  stop  step
  ?:  ?=(%boom -.muck)
    [%boom ~]
  |-
  ^-  boot
  ?~  stop
    ?:  =(yuck muck)
      yuck
    ^$(muck yuck)
  =/  erm  (yank:ska -.i.stop muck)
  ?:  ?=(%boom -.erm)
    $(stop t.stop, yuck (gnaw:ska [%boom ~] yuck))
  =/  arm  (trip:ska erm)
  ?~  arm
    $(stop t.stop, yuck (gnaw:ska [%risk %toss ~] yuck))
  =/  cor
    ?-  -.muck
      %safe  sure.muck
      %risk  hope.muck
    ==
  =/  mat  (wash:ska cor u.arm)
  ?:  ?=(%boom -.mat)
    $(stop t.stop, yuck (gnaw:ska [%boom ~] yuck))
  =/  ear  (yank:ska 2 mat)
  ?:  ?=(%boom -.ear)
    $(stop t.stop, yuck (gnaw:ska [%boom ~] yuck))
  =/  gar  (trip:ska ear)
  ?~  gar
    $(stop t.stop, yuck (gnaw:ska [%risk %toss ~] yuck))
  =/  mar  (welt:ska 6 [%risk %toss ~] mat)
  ?:  ?=(%boom -.mar)
    $(stop t.stop, yuck (gnaw:ska [%boom ~] yuck))
  =/  gor
    ?-  -.mar
      %safe  sure.mar
      %risk  hope.mar
    ==
  =/  beg  (wash:ska gor u.gar)
  $(stop t.stop, yuck (gnaw:ska (yank:ska +.i.stop beg) yuck))
--
