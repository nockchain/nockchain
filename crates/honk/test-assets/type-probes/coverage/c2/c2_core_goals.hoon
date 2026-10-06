::  c2: cores minted against goals (mint_core, goal_core_for_mine,
::  check_goal_core_chapter_counts ~7798-7850): played core goals, +$ %hint
::  goals, %face goals, fork-of-cores goals, noun-headed cell goals, and a
::  multi-chapter core against a matching multi-chapter goal.
|%
+$  gt  $-(@ @)
++  main
  |=  b=?
  :*  ^+(|=(a=@ a) |=(a=@ +(a)))
      ^-(gt |=(a=@ a))
      ^+(?:(b |=(a=@ a) |=(a=@ 1)) |=(a=@ a))
      ^+(f=|=(a=@ a) |=(a=@ a))
      ^-  [* *]
      |%
      ++  foo  1
      --
      ^+  |%
          +|  %one
          ++  x  1
          +|  %two
          ++  y  2
          --
      |%
      +|  %one
      ++  x  3
      +|  %two
      ++  y  4
      --
  ==
--
