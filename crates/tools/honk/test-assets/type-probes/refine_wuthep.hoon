::  ?- exhaustive switch over a $% union: per-case fuse results and the
::  joined product fork.
|%
++  main
  |=  f=fruit
  !>  ?-  -.f
        %apple   p.f
        %banana  q.f
        %cherry  [p.f p.f]
      ==
+$  fruit
  $%  [%apple p=@ud]
      [%banana q=@t]
      [%cherry p=@ud]
  ==
--
