::  p1: the sail tuna mode ;% (%call) is in hoon-138's +tuna-mode but not in
::  hatch's sail parser, so open %mcts's TunaTail::Call arm (utils.rs ~2233)
::  is unreachable from source.
|%
++  main
  |=  a=@
  !>  ;div
        ;%  |=(m=marl m)
        ;p;
      ==
--
