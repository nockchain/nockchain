::  c1 divergence: the hoon noun for !? (%zpwt).  hoon-138 types p as
::  $@(p=@ [p=@ q=@]), so !?(138 a) is [%zpwt 138 hoon]; hatch's
::  zpwt_arg_to_noun (utils.rs:13716) emits [%zpwt [%atom '138'] hoon].  The
::  arm gene is embedded in the core type, so the artifacts differ.  Would
::  cover write_zpwt_arg / write_hoon's %zpwt arm (mod.rs ~1335, ~2047).
|%
++  main
  |=  a=@
  !?(138 a)
--
