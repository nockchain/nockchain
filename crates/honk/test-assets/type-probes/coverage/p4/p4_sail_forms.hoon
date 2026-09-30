::  p4: sail forms stored as arm bodies (hatch runes/sail.rs mane_parser
::  TagSpace ~37, parsed_atom_to_cord/woof_to_beer/hoon_to_beers ~42-65,
::  tag_head id attr ~124, tuna_tail Tape ~173; utils.rs manx/mart/beer/
::  tuna encoders, mane_to_noun TagSpace ~13735, hoon_to_noun MicTis ~12587).
::  `!.` turns dbug off so attribute tapes reach hoon_to_beers as %knit.
|%
++  main  !>(..main)
++  s1  ;div;
++  s2  ;div.a.b;
++  s3  ;div#x;
++  s4  ;div(title "hi");
++  s5  ;foo_bar;
++  s9  ;div(title "a{<5>}b");
++  s6  ;-  "x"
++  s10
  !.
  ;div(title "hi", alt "a{<5>}b");
++  s7
  ;div
    ;p;
  ==
--
