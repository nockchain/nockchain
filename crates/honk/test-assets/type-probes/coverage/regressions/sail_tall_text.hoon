::  regression: tall sail text (hoon-138 ++tall-top, ++tall-tail,
::  ++quote-innards, ++collapse-chars). `;p: text` keeps `"` and embeds,
::  `; text` lines lose trailing spaces and gain a newline, escapes give
::  bytes, a bare `;` is a newline node, and `:` also takes a quote, a
::  cord, an element, or a parenthesized list.
|%
++  main  !>(..main)
++  t1
  ;div
    ;p: hello world
    ;p: say "hi" {<5>} and \{x} \3b\41\-
    ; a line{"  "}  
    ; {"x"}
    ;
  ==
++  t2
  ;p: x ;{b "bold" -"t"} y -{"tape"} +{;i;} *{~[;u;]}
++  t3
  ;div
    ;p: %{|=(a=marl a)} end
  ==
++  t4
  ;div
    ;p:"quoted {<1>}"
    ;p:'cord'
    ;p:b
    ;p:(a "t" -"u" +;v;)
  ==
++  t5  ;
++  t6  ;p: trailing spaces stay  
--
