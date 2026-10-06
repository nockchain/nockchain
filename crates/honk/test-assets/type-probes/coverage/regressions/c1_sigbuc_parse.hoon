::  c1 divergence: hoonc parses ~$ (%sgbc, profiler hit; hoon-138.hoon:13308)
::  but hatch has no ~$ rune, so honk rejects the file with a parse error.
::  Would cover write_hoon's Hoon::SigBuc signature arm (mod.rs ~1768).
|%
++  main
  |=  a=@
  ~$  %foo
  a
--
