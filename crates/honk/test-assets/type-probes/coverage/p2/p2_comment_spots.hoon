::  p2: %dbug spots after comment gaps: expand_gap_start walk-back,
::  doccord_comment_anchors larg/smol shapes, match_en_link sigils (| . + $ %),
::  trailing_comment_offset skipping cords and tapes (hatch utils.rs
::  ~7966-8170)
|%
++  main
  :*  ::  +link-9: smol link with a digit
      1
      ::  %12.ab: cone link
      2
      ::  %3
      3
      ::  |chat .frag +funk $plan
      4
      ::  +foo:
      5
      ::  +foo: x
      6
      ::  +foo: 
      61
      ::    larg four aces
      7
      ::  plain prose
      8
      ::  %x: not a cone
      9
      ::  +Foo: uppercase
      10
      ::  +foo:  :: trailing
      11
      :-  'a::b'  ::    larg trailing after a cord
      12
      :-  "it's"  ::    larg trailing after a tape
      13
      :-  14  ::  +link: smol trailing
      15
      :-  16  ::  prose trailing
      17
  ==
--
