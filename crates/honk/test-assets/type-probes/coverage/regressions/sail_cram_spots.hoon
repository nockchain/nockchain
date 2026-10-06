::  regression: dbug spots of hoon embedded in sail markdown. hoon-138
::  reparses each paragraph from where it began, so the first line of a
::  paragraph that starts mid-line is shifted by its indentation.
|%
++  main  !>(..main)
++  s1
  ;>
    first {<1>} line
    second {<2>} line *bold {<3>}*

    - item {<4>}
      more {<5>}

    > quote {<6>}
    ;p: sail {<7>}
    after {<8>}
++  s2
  ;div
    text {<9>}
    ;p;
    [link {<10>}](u) and ;{b {<11>}}
  ==
--
