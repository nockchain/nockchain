::  ?+ switch with default: crop of matched cases from the default branch.
|%
++  main
  |=  t=?(%a %b %c %d)
  !>  ?+  t  %other
        %a  %first
        %b  %second
      ==
--
