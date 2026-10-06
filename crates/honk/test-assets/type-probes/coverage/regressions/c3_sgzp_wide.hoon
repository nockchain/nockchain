::  c3 divergent (parser): wide-form ~!(p q). hoon-138 parses it (rune zap %sgzp expb); hatch's
::  sigzap_wide (crates/hatch/src/runes/sig.rs ~140) delegates to two_hoons_tall and rejects '('.
|%
++  main
  |=  a=@
  ~!(a a)
--
