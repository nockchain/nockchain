::  p3 (was HOONC-ONLY): tall hoons that are paths starting with `/` and an
::  import-rune character. hatch used to skip any such line in tall position
::  as an import header (runes/fas.rs fas_runes_tall) and then failed on the
::  closing `--`; now only a block at the top of the file is one, as in
::  hoonc's +pile-rule.
|%
++  cur  /=/foo
++  sig  /--2
++  neg  /-1
--
