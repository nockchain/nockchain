::  p4 DIVERGENT (HONK-ONLY): duplicate arm names in one battery. hoonc's
::  parser stores [%eror "duplicate arm: +a"] for the repeat and the build
::  fails; hatch utils.rs chapters (~4967-4975) inserts both arms into a
::  HashMap, the later one silently wins, and honk writes an artifact.
|%
++  main  !>(..main)
++  a  1
++  a  2
--
