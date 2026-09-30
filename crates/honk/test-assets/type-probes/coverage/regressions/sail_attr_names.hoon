::  regression: sail attributes (hoon-138 ++tag-head, ++tall-attrs). Names
::  are manes (mixed case, and `a_b` for a namespace), the #id comes before
::  the .classes, `=name  value` lines follow a tall tag head, and
::  attribute text keeps one beer char per byte.
|%
++  main  !>(..main)
++  s1  !.  ;a(xlink_href "x", dataFoo "y");
++  s2  !.  ;div(title "h\c3\a9");
++  s5  !.  ;div(title "a€{"é"}");
++  s3  ;div#x.y.z;
++  s4
  ;div#x.y(lang "en")
    =title  "t"
    =xml_space  "preserve"
    ;p;
  ==
--
