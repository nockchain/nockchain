::  p4 DIVERGENT (HONK-ONLY): a repeated `+|` chapter name. hoonc's parser
::  replaces the battery with [%eror "duplicate chapter: |x"] and the build
::  fails; hatch utils.rs chapters (~4967, `.entry(key).or_insert_with`)
::  merges both chapters and honk writes an artifact.
|%
+|  %x
++  main  !>(..main)
+|  %x
++  b  2
--
