::  p1: autoname (utils.rs ~2994) and name_ax (~2971) naming =spec samples.
|%
+$  tag  $%([%a p=@] [%b q=?])
++  main
  |=  a=@
  =/  a  [1 2]
  =/  g  |=(* 5)
  :*  !>(|=(=@ud ud))
      !>(|=(=%foo tas))
      !>(|=(=(list @) list))
      !>(|=(=[@ud @] ud))
      !>(|=(=$%([%x p=@] [%y q=?]) tas))
      !>(|=(=?(%x %y) tas))
      !>(|=(=$@(@ [n=@ud r=@]) ud))
      !>(|=(=$-(@ud @) ud))
      !>(|=(=$^([@ud @] @ud) ud))
      !>(|=(=$+(foo @ud) ud))
      !>(|=(=$|(@ud |=(* &)) ud))
      !>(|=(=$~(5 @ud) ud))
      !>(|=(=$=(n @ud) ud))
      !>(|=(=$_(a) a))
      !>(|=(=$_(^a) a))
      !>(|=(=$<(%a tag) tag))
      !>(|=(=$>(%a tag) tag))
      !>(|=(=$;(g) g))
  ==
--
