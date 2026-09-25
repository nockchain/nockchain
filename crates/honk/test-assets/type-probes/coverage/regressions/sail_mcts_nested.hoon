::  regression: sail `;=` (%mcts) and `;%` (%call). A `;=` among an
::  element's tall children is spliced into them, `;=;` is an empty list,
::  and `;%` also works as a child of `;=` and at the top of a sail
::  expression (hoon-138 ++tall-top, ++tuna-mode).
|%
++  main  !>(..main)
++  s1
  ;div
    ;=  ;p;  ;q;  ==
    ;r;
  ==
++  s2  ;=;
++  s3
  ;=
    ;p;
    ;%  |=(a=marl:hoot a)
    ;q;
  ==
++  s4
  ;%  |=(a=marl:hoot a)
--
