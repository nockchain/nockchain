::  play of ?!(p) with gain(p) void: hoon-138's %wtcl play drops the void
::  branch, so ^+ takes the constant %.y rather than a full ?.
|%
++  main
  |=  a=@
  !>(^+(?!(?=(^ a)) %.y))
--
