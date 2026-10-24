::  ?~ refinement over unit and list: null branch vs unwrapped branch types.
|%
++  main
  |=  [u=(unit @ud) l=(list @t)]
  [!>(?~(u 0 u.u)) !>(?~(l %empty i.l))]
--
