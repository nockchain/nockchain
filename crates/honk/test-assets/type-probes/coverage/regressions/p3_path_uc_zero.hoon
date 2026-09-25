::  p3 regression (was MISMATCH): @uc zero path segment. hoonc renders
::  '0c1111111111111111111114oLvT2'; honk renders '0c' + 21 '1' + '0'
::  (utils.rs rend_with_rep 'u' 'c' arm special-cases q=0).
|%
++  main
  |=  a=@
  !>(/0c1111111111111111111114oLvT2)
--
