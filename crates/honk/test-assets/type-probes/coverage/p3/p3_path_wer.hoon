::  p3: paths relative to the file path (utils.rs path: cen_fas, multi_cen,
::  rood with a %-suffix, gasp tis runs, posh/poon)
|%
++  main
  |=  a=@
  :*  !>(%/foo)
      !>(%%)
      !>(%%%)
      !>(%%/bar)
      !>(/foo%/bar)
      !>(/foo%%/bar/baz)
      !>(/=/foo)
      !>(/==/foo)
      !>(/=foo/bar)
      !>(/foo=/bar)
  ==
--
