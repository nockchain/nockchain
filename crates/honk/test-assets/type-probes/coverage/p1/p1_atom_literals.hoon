::  p1: atom-literal conversions in hatch utils.rs: base64_to_atom (~179),
::  base32_to_atom (~199), base58_to_atom (~217), ipv4_to_atom (~229),
::  ipv6_to_atom (~237), and the >128-bit path of binary_to_atom (~147).
|%
++  main
  |=  a=@
  :*  !>(0w1.abcde)
      !>(0wA)
      !>(0vab.cdefg)
      !>(0v1)
      !>(.127.0.0.1)
      !>(.0.0.0.0.0.0.0.1)
      !>(0b1.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000.0000)
      !>(0c1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa)
  ==
--
