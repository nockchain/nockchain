::  wet gate whose formal sample is a fork of cells, called with an atom:
::  every fork case misses, wec is empty, hoon-138 ++dear yields `~ (faceless
::  sample) and the body never names the sample, so hoonc accepts; honk's
::  redo_dear treats an empty wec as redo-match.
|%
++  wig
  |*  a=?([p=@ q=@] [r=@ s=@])
  1
++  main
  |=  [x=@ud b=?]
  !>((wig x))
--
