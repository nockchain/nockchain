::  => and =< subject composition: types across re-rooted subjects.
::  (the !> pin stays outside the => — vase-building needs the prelude
::  in the subject, so both compilers reject !> under a re-rooted one)
|%
++  main
  |=  x=@ud
  !>(=>([val=x ctx=%fixed] [val ctx]))
--
