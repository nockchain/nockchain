::  regression: wide sail (hoon-138 ++wide-top, ++wide-tail,
::  ++wrapped-elems, ++wide-paren-elems, ++tag-head).
|%
++  main  !>(..main)
++  w1  !>(;div;)
++  w2  `manx`;div:"text {<2>}"
++  w3  `manx`;div:'cord'
++  w4  `manx`;div:(p "x" -"y" +;b; *~[;i;])
++  w5  `marl`;"just {<3>} text"
++  w6  `marl`;("a" b -"c")
++  w7  `manx`;a/"url"(title "t")
++  w8  `manx`;div#i.c1.c2;
++  w9  `manx`;img@"pic"
++  w10  `manx`;div
++  w11  `manx`;div:"with \"escapes\" \{ \41"
--
