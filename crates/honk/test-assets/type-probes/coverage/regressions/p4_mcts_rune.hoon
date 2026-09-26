::  p4 DIVERGENT: the `;=` (mcts) rune. hoonc's +expression `;` table has no
::  `=` entry but +sail parses `;=  marl  ==` into [%mcts marl]; hatch has
::  no `;=` parser (runes/mic.rs mic_runes_tall, runes/sail.rs), so honk
::  fails to parse what hoonc accepts.
|%
++  main  !>(..main)
++  s8  ;=  ;p;  ==
--
