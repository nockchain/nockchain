::  Multi-option %fork treaps (3-6 members) built by the normal compiler
::  paths: $? spec molds, nested ?:, ?= / ?@ / ?~ refinement (crop/fuse), and
::  a fork under a list hold. Control for type_to_noun's list-encoded %fork,
::  which only a %hand gene reaches.
|%
++  main
  |=  [a=? b=? c=? x=?(%a %b %c %d %e %f) y=?(~ @ [@ @] [%q @])]
  :*  !>(x)
      !>(?:(a %p ?:(b %q ?:(c %r ?:(a %s %t)))))
      !>(?:(?=(?(%a %b) x) x x))
      !>(?@(y y y))
      !>(?~(y y y))
      !>(`(list ?(%a %b %c))`~)
  ==
--
