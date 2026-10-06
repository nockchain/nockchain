::  c4: ++fond/++find walks. loot over an empty battery (find.rs ~774),
::  zinc read and iron write that search only the sample (peel sam & !con,
::  find.rs ~560), and a head-position %hold cycle cut to void (find.rs ~316)
::  that ++twin merges away (find.rs ~622).
|%
+$  rec  $@(~ [rec c=@])
++  main
  |=  [a=@ b=?]
  =/  x  5
  =/  z  ^&  |=(q=@ q)
  =/  i  ^|  |=(r=@ r)
  =/  e
    =>  |%
        --
    x
  :*  !>(e)
      !>(q.z)
      !>(i(r 7))
      !>(|=(s=rec ?~(s ~ c.s)))
  ==
--
