::  p3: @if/@is literals (utils.rs zust ipv6/ipv4 try_maps)
|%
++  main
  |=  a=@
  :*  !>(.1.2.3.4)
      !>(.0.0.0.0)
      !>(.255.255.255.255)
      !>(.192.168.0.1)
      !>(.0.0.0.0.0.0.0.1)
      !>(.fe80.0.0.0.0.0.0.abcd)
      !>(.ffff.ffff.ffff.ffff.ffff.ffff.ffff.ffff)
      !>(.1.2.3.4.5.6.7.8)
      !>(.y)
      !>(.n)
  ==
--
