!:
::  p3: %dbug spots for gap-glued children preceded by doc lines
::  (utils.rs wrap_hoon_with_trace -> unanchor_gap_glued_children for
::  ?^ ?@ ?~ ?- ?+ %~ %* =^ =* ~% ~> |_ +*; unanchor_hoon_spot through
::  %note; unanchor_spec_spot through %gist; unanchor_spot_start doc skip)
|%
++  cor
  |_  s=@
  +*  t
    ::  alias doc
    s
  ++  get  t
  --
++  main
  |=  [a=* d=*]
  =*  b
    ::  doc on =* value
    a
  =^  c  a
    ::  doc on =^ value
    [1 2]
  :*  ?~  d
        ::  doc on ?~ yes
        0
      ::  doc on ?~ no
      1
    ::
      ?^  d
        ::  doc on ?^ yes
        2
      3
    ::
      ?@  d
        ::  doc on ?@ yes
        4
      5
    ::
      ?@  d
        ::    larg doc on a colhep child
        :-  4  5
      ::    larg doc on the no branch
      [6 7]
    ::
      ?-  d
        ::  doc on first ?- clause
        @  6
        ^  7
      ==
    ::
      ?+  d
        ::  doc on ?+ default
        8
        ::  doc on first ?+ clause
        @  9
      ==
    ::
      %~  get
        ::  doc on %~ door
        cor
      ::  doc on %~ sample
      11
    ::
      %*  .
        ::  doc on %* base
        [x=12 y=13]
        x  14
      ==
    ::
      ~>  %foo
        ::  doc on ~> body
        15
    ::
      ~>  %foo.16
      17
    ::
      ~%  %bar
          ::  doc on ~% subject
          .
        ~
      18
    ::
      ~%  %baz  .
        ==
          %qux
          ::  doc on ~% hint value
          19
        ==
      20
    ::
      b
  ==
--
