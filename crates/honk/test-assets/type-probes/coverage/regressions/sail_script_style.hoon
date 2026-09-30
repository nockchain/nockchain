::  regression: sail `;script` and `;style` bodies are `;` lines of raw
::  text (hoon-138 ++script-or-style, ++script-style-tail); a `;script;`
::  is an ordinary element.
|%
++  main  !>(..main)
++  s1
  ;script(type "text/javascript")
    ; var x = "a{b}";
    ;
    ; done
  ==
++  s2
  ;style
    ; p { color: red; }
  ==
++  s3  ;script;
++  s4
  ;scripts
    ;p;
  ==
--
