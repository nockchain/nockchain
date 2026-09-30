::  control: ?! with both branches live, and ?! inside ?:/?&/?| conditions
::  (neither compiler refines through ?!).
|%
++  main
  |=  [a=@ b=? u=(unit @)]
  :*  !>(?!(%.y))
      !>(?!(b))
      !>(?!(?=(~ u)))
      !>(?:(?!(?=(~ u)) u u))
      !>(?:(?&(?!(?=(~ u)) b) u u))
      !>(?:(?|(?!(?=(~ u)) b) u u))
      !=(?!(%.y))
      !=(?!(=(a 1)))
  ==
--
