::  p4: parser forms landing in arm bodies (utils.rs basetype_to_noun Void
::  ~13041, spec_to_noun BucGal/BucPam ~13316/13349, chum_to_noun StdKel/
::  VenProKel ~13570-13579; runes/buc.rs bucwut_wide/buccen_wide ~246/409,
::  sig.rs sigwut_wide ~106 siggal/siggal_wide ~305/319-320, tis.rs
::  tistar_wide ~321, wut.rs wuthax ~241, col.rs list_syntax `]~` ~292;
::  hoon_to_noun Yell ~12241)
|%
++  main  !>(..main)
++  v1  $@(!! @)
++  g1  $<(%a $%([%a p=@] [%b q=@]))
++  p1
  $&  @
  |=(a=@ a)
++  w1  $?(%a %b)
++  c1  $%([%a p=@] [%b q=@])
++  j1  ~/  %foo.1  |=(a=@ a)
++  j2  ~/  %foo:bar.1  |=(a=@ a)
++  j3  ~/  %foo:bar..1  |=(a=@ a)
++  sw  ~?(& %hi 5)
++  sw2  ~?(> & %hi 5)
++  sl  ~<  %foo  5
++  sl2  ~<(%foo 5)
++  sl3  ~<(%foo.5 6)
++  ts  =*(a 5 a)
++  wh
  |=  b=@
  ?#  %a  b
++  ls  ~[1 2]~
++  kt  ^=  [a b]  [1 2]
++  ye  >5<
++  bg  0xffff.ffff.ffff.ffff.ffff.ffff.ffff.ffff.ffff
--
