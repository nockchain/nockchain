::  p3: path elements other than knots (utils.rs path hasp: [wide],
::  (gate args) cncl, $, 'cord', and limp's extra-fas insertion; gasp tis runs)
|%
++  main
  |=  a=@
  :*  !>(/foo/[a]/bar)
      !>(/(add a 1))
      !>(/foo/(add 1 2)/baz)
      !>(/$/foo)
      !>(/'cord'/'hello world')
      !>(/foo//bar)
      !>(/foo///bar)
  ==
--
