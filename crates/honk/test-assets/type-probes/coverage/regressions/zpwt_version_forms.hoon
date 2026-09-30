::  regression: `!?` version forms (hoon-138 ++hinh). The version is plain
::  decimal digits (leading zeros allowed, no 64-bit limit) or a pair [p q]
::  admitting q <= 138 <= p, stored as bare atoms.
|%
++  main  !>(..main)
++  z1  !?  0138  5
++  z2  !?(123456789012345678901234567890 5)
++  z3  !?([138 138] 5)
++  z4  !?  [200 0]  5
--
