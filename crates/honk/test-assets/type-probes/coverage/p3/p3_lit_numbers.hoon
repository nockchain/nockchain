::  p3: number literals (utils.rs number: ud/ux/ub/uv/uw/ui/uc and the
::  signed choice with its sx/sb/sv/sw/si/sd arms)
|%
++  main
  |=  a=@
  :*  !>(0)
      !>(1.000.000)
      !>(0x0)
      !>(0xdead.beef)
      !>(0b0)
      !>(0b1.0101)
      !>(0v0)
      !>(0v1f.00000)
      !>(0w0)
      !>(0w-~.AAAAA)
      !>(0i0)
      !>(0i123)
      !>(0c1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa)
      !>(0c1111111111111111111114oLvT2)
      !>(-1)
      !>(--1)
      !>(-0x10)
      !>(--0x10)
      !>(-0b101)
      !>(--0b1)
      !>(-0v1f)
      !>(--0vv)
      !>(-0w1)
      !>(--0wA)
      !>(-0i7)
      !>(--0i7)
  ==
--
