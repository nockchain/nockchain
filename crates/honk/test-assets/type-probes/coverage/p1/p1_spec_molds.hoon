::  p1: +ax ports in hatch utils.rs reached through mold gates and specs:
::  spore_recursion %bcgl/%bcgr/%bcpm/%bcpt-void (~410-466), relative %bcgl
::  (~1338) and %bcpm (~1190), basic %void (~949), basal %void via ?= (~287),
::  and the factory %bcmc short-circuit under ;; (~1653).
|%
+$  tag  $%([%a p=@] [%b q=?])
+$  rep
  $&  @
  |=(b=* ?@(b b 0))
++  main
  |=  a=tag
  :*  !>($<(%a tag))
      !>(($<(%a tag) [%b &]))
      !>($>(%a tag))
      !>(rep)
      !>((rep 5))
      !>($,($@(!! ^)))
      !>(|=(f=$@(!! ^) f))
      !>(?=(!! a))
      !>(;;($;(|=(b=* ?@(b b 0))) 7))
  ==
--
