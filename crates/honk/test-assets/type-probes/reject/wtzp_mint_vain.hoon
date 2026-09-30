::  ?!(p) where lose(p) is void: hoon-138 opens ?! to ?:, whose dead %.y
::  branch is minted against a void subject under vet (mint-vain).
|%
++  main
  |=  [a=@ b=?]
  =/  x  ~
  !>(?!(?=(~ x)))
--
