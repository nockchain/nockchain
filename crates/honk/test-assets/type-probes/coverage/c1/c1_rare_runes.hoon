::  c1: a rare rune whose Sig64 cache-signature arm the corpus misses
::  (mod.rs write_hoon ~1784 ~=), and a %spec skin in ?# whose example is
::  played through play_noun (~4457).
|%
++  main
  |=  [a=@ b=*]
  :*  !>(~=(a a))
      !>(?#(*@ a))
      !>(?#(*@ +<-))
  ==
--
