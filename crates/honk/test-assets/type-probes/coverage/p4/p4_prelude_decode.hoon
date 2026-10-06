::  p4: noun_to_hoon decoding of prelude arm bodies. Firing a prelude arm
::  makes honk decode that arm's hoon noun from the embedded hoon-138 subject
::  type (hatch utils.rs noun_to_hoon %cntr ~15020 via +ma:rd's `%*`, %cnkt
::  ~15033 via +ruv:at's `%^`, %dtkt ~15072 via +reck's `.^`).
|%
++  main
  |=  [a=@ p=path c=@rd]
  :*  !>(~(ruv at a))
      !>((reck p))
      !>((sea:rd c))
  ==
--
