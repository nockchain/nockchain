::  A repeated `+|` chapter name: the parser (++wisp) makes the chapter
::  [%$ [%eror "duplicate chapter: |x"]], and minting it fails (hatch once
::  merged both chapters, and honk wrote an artifact).
|%
+|  %x
++  main  !>(..main)
+|  %x
++  b  2
--
