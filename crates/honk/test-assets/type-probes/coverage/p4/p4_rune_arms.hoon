::  p4: hoon_to_noun encodings of rune forms stored raw as arm bodies
::  (hatch utils.rs hoon_to_noun_uncached ~12295-12837: brkt brsg brwt clkt
::  cnkt dtkt ktdt sgpm sgts sgwt sgzp mcfs mcgl mcmc tscl tsmc tswt tshp
::  tskt tssg tstr+spec tscm zpcm zpgl zpmc zpts). `!>(..main)` pins the
::  file core type, whose tomes carry every arm's unexpanded hoon.
|%
++  main  !>(..main)
++  a-brkt
  |^  (inc 1)
  ++  inc  |=(b=@ +(b))
  --
++  a-brsg  |~  a=@  a
++  a-brwt  |?  5
++  a-clkt  :^  1  2  3  4
++  a-cnkt
  %^  |=([a=@ b=@ c=@] (add a (add b c)))  1  2  3
++  a-dtkt  .^(@ %cx /a/b)
++  a-ktdt  ^.(|=(a=@ a) 5)
++  a-sgpm  ~&  %hi  5
++  a-sgpm2  ~&  >>  %hi  5
++  a-sgts  ~=  5  5
++  a-sgwt  ~?  &  %hi  5
++  a-sgwt3  ~?  >>>  &  %hi  5
++  a-sgzp  ~!  5  5
++  a-mcfs  ;/  "x"
++  a-mcgl
  ;<  a=@  |=(* |=([a=@ f=$-(@ @)] (f a)))  5
  +(a)
++  a-mcmc  ;;  @  5
++  a-tscl
  =/  a  1
  =/  b  2
  =:  a  3
      b  4
    ==
  (add a b)
++  a-tsmc
  =;  a=@
    +(a)
  5
++  a-tswt
  =/  a  1
  =?  a  &  2
  a
++  a-tshp  =-  +(-)  5
++  a-tskt
  =/  s  5
  =^  a  s  [1 2]
  [a s]
++  a-tssg
  =~  5
      +(.)
  ==
++  a-tstr
  =*  a=@  5
  a
++  a-tscm
  =/  x  [p=1 q=2]
  =,  x
  p
++  a-zpcm  !,(*hoon 5)
++  a-zpgl  !<(@ !>(5))
++  a-zpmc  !;(*type 5)
++  a-zpts  !=(5)
--
