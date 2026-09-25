::  p3: relative @dr literals (utils.rs relative_date: every unit arm,
::  repeated units, hex fraction list; yule)
|%
++  main
  |=  a=@
  :*  !>(~s0)
      !>(~s1)
      !>(~m1)
      !>(~h1)
      !>(~d1)
      !>(~d1.h2.m3.s4)
      !>(~s4.m3.h2.d1)
      !>(~s1.s2.s3)
      !>(~h25.m70.s3)
      !>(~s1..8000)
      !>(~s0..0001.0002)
      !>(~d7..ffff.ffff.ffff.ffff)
  ==
--
