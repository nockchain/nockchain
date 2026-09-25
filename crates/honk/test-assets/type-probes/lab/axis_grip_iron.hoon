::  ^= cell-skin destructuring lowers to =+ gen [p=+4 q=+5]; the bare +5
::  reads the payload of an iron (^|) core, which ++peek %read blocks.
|%
++  main
  |=  [a=@ b=?]
  !>(^=([p q] ^|(|=(x=@ x))))
--
