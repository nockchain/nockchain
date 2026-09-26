::  p1 regression (was HOONC-ONLY): ipv4_to_atom (utils.rs ~229) used std Ipv4Addr parsing, which rejects
::  octets over 255; hoonc's +lip:ag takes 1-3 digit groups in base 256.
|%
++  main
  |=  a=@
  !>(.1.2.3.300)
--
