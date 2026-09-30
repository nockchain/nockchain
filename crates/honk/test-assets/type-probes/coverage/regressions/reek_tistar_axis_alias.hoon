::  =* aliases to axis wings (+6, +<, +13, .): reek yields a wing, so fund resolves them as legs; reads and edits (x(..), =.) go through find/tack.
|%
++  main
  |=  [a=@ b=?]
  =*  x  +6
  =*  y  +<
  =*  z  +13
  =*  w  .
  :*  !>(x)
      !>([y z])
      !>(a.x)
      !>(x(a 5))
      !>(=.(x [7 |] x))
      !>(=.(a.x 9 [x a]))
      !>(=.(z | [b z]))
      !>(+<.w)
  ==
--
