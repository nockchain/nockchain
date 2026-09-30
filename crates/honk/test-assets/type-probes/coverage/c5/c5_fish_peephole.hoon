::  c5: formula_dag flan/flor/and peepholes reached through ?= and ?# fish
::  (formula_dag.rs flan ~554, flor ~572, and ~590): void-fishing hold
::  members, fork members with equal fish, and forks containing %noun in
::  either treap position.
|%
++  vd  !!
++  main
  |=  [b=? c=*]
  :*  !>(?=([_vd *] c))
      !>(?=([* _vd] c))
      !>(?=(_?:(b ^-(@ud 0) ^-(@ux 0)) c))
      !>(?=(_?:(b ^-(* 0) ^-(@ 0)) c))
      !>(?=(_?:(b ^-(* 0) ^-(^ [0 0])) c))
      !>(?=(_?:(b ^-(* 0) ^-(@ud 0)) c))
      !>(?=(_?:(b ^-(* 0) ^-(@ux 0)) c))
      !>(?=(_?:(b ^-(* 0) ^-(? &)) c))
      !>(?=(_?:(b ^-(* 0) ^-([@ @] [0 0])) c))
      !>(?=(_?:(b ^-(* 0) %foo) c))
      !>(?=(_?:(b [vd 0] ^-(@ 0)) c))
      !>(?=(_?:(b ^-(@ 0) [vd 0]) c))
      !>(?#([@ @] c))
      !>(?#([^ ?] c))
  ==
--
