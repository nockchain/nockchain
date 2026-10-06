::  p1 regression: =@ names the sample atom. hatch autoname once tested the
::  aura only against "$", not the parser's "" for a bare @, and named it %$;
::  honk then segfaulted building a zero-byte atom for that face.
|%
++  main
  |=  a=@
  !>(|=(=@ atom))
--
