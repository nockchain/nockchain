::  regression: a sail `"""` block (hoon-138 ++wide-quote under ++inde):
::  lines are indented as far as the `"""`, blank lines are kept, and the
::  tall form ends the text with a newline.
|%
++  main  !>(..main)
++  q1
  !.
  ;"""
   first line
     indented {<1>}

   last "quoted"
   """
++  q2
  ;div
    ;"""
     inside {<2>}
     """
  ==
++  q3
  ;div
    ;p:"""
       colon {<3>} block
       """
  ==
++  q4  (lent `marl`;"""
                     wide {<4>}
                     """)
--
