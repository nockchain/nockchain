::  c6 (was divergence 32): the entry sits under a different open/hoon root
::  than the dependency root. hoonc keys it by its canonical absolute path;
::  honk used to match the roots by their open|closed/hoon marker and key it
::  as /app/kernel.hoon.
/=  raw  /common/raw-thing
|=  hash=@uvI
^-  *
[hash raw]
