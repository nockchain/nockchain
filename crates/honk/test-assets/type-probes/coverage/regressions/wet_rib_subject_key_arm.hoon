::  wet arm re-entered from a call site whose subject equals the outer call
::  site's (both `=>  ..foo`): hoon-138 ++fire keys rib on [sut dox arm], so the
::  inner (foo [0 0]) is not re-mulled; keying on the redone core instead
::  mulls x=[@ud @ud] and .+(x) fails.
|%
++  foo
  |*  x=@
  :-  .+(x)
  =>  ..foo
  (foo [0 0])
++  main
  |=  [a=@ b=?]
  =>  ..foo
  (foo 0)
--
