::  p2: named tikis on ?^ ?@ ?~: a faced wing (b=a) and a faced hoon
::  (b=(head ...)), tall and wide (hatch utils.rs tiki_wide ~4662-4685)
|%
++  main
  |=  a=*
  :*  ?^  b=a
        -.b
      b
      ?^(b=a -.b b)
      ?@  b=(head [a a])
        b
      -.b
      ?~(b=(head [a a]) %nil b)
      ?^  a
        -.a
      a
  ==
--
