::  c5: forks whose members are zinc cores, so fork members decode through
::  ty.rs Garb::from_noun (~198) with a %zinc vair
|%
++  zc  ^&(|=(a=@ a))
++  zd  ^&(|=(a=@ +(a)))
++  ic  ^|(|=(a=@ a))
++  main
  |=  [b=? c=@]
  :*  !>(+<:?:(b zc zd))
      !>(^-(?(_zc _zd) zc))
      !>(=/(f ?:(b zc zd) +<.f))
      !>(=/(f ?:(b zc ic) -.f))
  ==
--
