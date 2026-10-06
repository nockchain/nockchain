::  Duplicate arm names in one battery: the parser (++whap) stores
::  [%eror "duplicate arm: +a"] for the repeat, and minting it fails (hatch
::  once let the later arm win, and honk wrote an artifact).
|%
++  main  !>(..main)
++  a  1
++  a  2
--
