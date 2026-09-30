::  c2: %= edit batteries through hike_insert (~6848-6882): a later edit on
::  an ancestor axis drops earlier descendant edits, a descendant after its
::  ancestor is skipped, and sibling edits merge in both orders; plus a
::  ?- with a missing case minted with vet off (mint_lost ~8303-8310).
|%
++  main
  |=  b=?
  =/  x  [[1 2] 3]
  :*  !>(x(- [4 5], -< 6))
      !>(x(-< 6, - [4 5]))
      !>(x(-< 6, -> 7))
      !>(x(-> 7, -< 6))
      !>(x(-< 6, -> 7, + 8))
      !>(!=(?-(b %.y 1)))
  ==
--
