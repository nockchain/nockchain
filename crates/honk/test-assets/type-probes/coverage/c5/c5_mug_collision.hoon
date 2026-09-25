::  c5: 31-bit mug collisions among quoted constants and type leaves.
::  0x1.0000.0000.0000.3717 and 0x1.0000.0000.0000.d5d0 share a mug (so do
::  [0 a] and [0 b]), as do the terms mugclashwlza and mugclasheydf.  Every
::  hash bucket must still tell them apart: formula_dag.rs
::  intern_with_materialized (~157), value_dag.rs intern_atom (~97), intern.rs
::  node_eq (~976 auras, ~1001 face tools) and intern_node (~903).
|%
++  main
  |=  c=@
  =/  a  0x1.0000.0000.0000.3717
  =/  b  0x1.0000.0000.0000.d5d0
  :*  !>([0x1.0000.0000.0000.3717 0x1.0000.0000.0000.d5d0])
      !>(^~([0x1.0000.0000.0000.3717 0x1.0000.0000.0000.d5d0]))
      !>(^~([0x1.0000.0000.0000.d5d0 0x1.0000.0000.0000.3717]))
      !>(%mugclashwlza)
      !>(%mugclasheydf)
      !>([mugclashwlza=1 mugclasheydf=1])
      !>([`@mugclashwlza`0 `@mugclasheydf`0])
      !>([p=0x1.0000.0000.0000.3717 q=0x1.0000.0000.0000.d5d0])
      !>(?:(=(c a) a b))
      !>(?:(=(c a) %mugclashwlza %mugclasheydf))
      !=([0x1.0000.0000.0000.3717 0x1.0000.0000.0000.d5d0])
  ==
--
