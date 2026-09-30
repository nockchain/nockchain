::  c1: play_inner arms reached through ^+ (mod.rs ~4463-4716): %lost from a
::  non-exhaustive ?-, ^& (KetPam), .^ (DotKet), !, (ZapCom), !< (ZapGal),
::  != (ZapTis), !@ taken and untaken (ZapPat/feel), ?# (WutHax), and a
::  multi-item =~ (TisSig).
|%
++  main
  |=  [a=@ b=? c=*]
  :*  !>(^+(?-(b %.y 1) 7))
      !>(^+(^&(|.(a)) |.(a)))
      !>(^+(.^(@ %cx /foo) 5))
      !>(^+(!,(5 a) 6))
      !>(^+(!<(@ !>(a)) 8))
      !>(^+(!=(a) c))
      !>(^+(!@(a [a a] ~) [1 2]))
      !>(^+(!@(zzz [a a] ~) ~))
      !>(^+(?#(%foo c) &))
      !>  ^+
          =~  [x=a y=b]
              [y x]
          ==
        [| 3]
  ==
--
