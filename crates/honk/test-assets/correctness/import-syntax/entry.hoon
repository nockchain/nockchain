::  Every import header form hoonc's +pile-rule accepts: a kelvin pin,
::  wild and faced /- and /+ items, a comma list that carries a comment
::  and continues onto an unindented line, comments between directives,
::  /= paths, /* leaves under marks that hoonc ignores (a Hoon leaf under
::  %jam, a byte leaf under %txt) and a /# data file that hoonc evaluates at
::  build time.
::
/?  138
/-  *types, t=types
/+  *alpha,  ::  a comment inside the list
    beta,
gamma
::  a comment between directives
/=  raw  /lib/alpha
/*  src  %jam  /lib/gamma/hoon
/*  text-leaf  %txt  /dat/note/jam
/#  konst
::
:*  alpha-val
    beta-val:beta
    gamma-val:gamma
    `thing:t`[1 2]
    `thing`[3 4]
    alpha-val:raw
    gamma-val:src
    p.text-leaf
    konst
==
