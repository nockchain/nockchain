::  p3 DIVERGENT (HOONC-ONLY): a tall hoon that starts with a signed path
::  segment. hoonc parses /-0x10 as a path; hatch fas_runes_tall
::  (runes/fas.rs) treats any /- /+ /= /* /# /? /% line in tall position as
::  an import header, skips it, and fails on the following --.
|%
++  main  /-0x10
--
