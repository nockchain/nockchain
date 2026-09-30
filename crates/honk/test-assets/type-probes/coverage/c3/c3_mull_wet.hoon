::  c3: mull_inner on wet gate bodies called with a narrower sample (mod.rs ~11114): .^ .* ^| ^& ^?
::  ^. ~! ~| =, =- !, != !; !< !@ !! ?#, and ?: whose gain or lose is void on the sut side only.
|%
++  w-dtkt  |*  a=*  .^(@ a)
++  w-dttr  |*  a=*  .*(a [0 1])
++  w-ktbr  |*  a=*  ^|(|.(a))
++  w-ktpm  |*  a=*  ^&(|.(a))
++  w-ktwt  |*  a=*  ^?(|.(a))
++  w-ktdt  |*  a=*  ^.(|=(b=* b) a)
++  w-sgzp  |*  a=*
  ~!  a
  a
++  w-sgbr  |*  a=*  ~|(%oops a)
++  w-tscm  |*  a=[b=* c=*]  =,(a b)
++  w-tshp  |*  a=*  =-(- a)
++  w-zpcm  |*  a=*  !,(*hoon a)
++  w-zpts  |*  a=*  !=(a)
++  w-zpmc  |*  a=*  !;(*type a)
++  w-zpgl  |*  a=vase  !<(@ a)
++  w-zppt  |*  a=*  [!@(a 1 2) !@(nope 3 4)]
++  w-zpzp  |*  a=*  ?:(=(a 0) !! a)
++  w-wthx  |*  a=*  ?#(@ a)
++  w-if1   |*  a=*  ?:(?=(@ a) !! 2)
++  w-if2   |*  a=*  ?:(?=(@ a) 1 !!)
++  w-if3   |*  a=@  ?:(?=(^ a) !! 2)
++  main
  |=  [x=@ y=[@ @] p=path]
  :*  !>((w-dttr x))
      !>((w-ktbr x))
      !>((w-ktpm x))
      !>((w-ktwt x))
      !>((w-ktdt x))
      !>((w-sgzp x))
      !>((w-sgbr x))
      !>((w-tscm y))
      !>((w-tshp x))
      !>((w-zpcm x))
      !>((w-zpts x))
      !>((w-zpmc x))
      !>((w-zpgl !>(x)))
      !>((w-zppt x))
      !>((w-zpzp x))
      !>((w-wthx x))
      !>((w-if1 y))
      !>((w-if2 x))
      !>((w-if3 x))
      !>((w-dtkt p))
  ==
--
