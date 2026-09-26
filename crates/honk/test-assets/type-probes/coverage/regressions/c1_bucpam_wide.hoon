::  c1 divergence: wide-form $& in a +$ arm.  hoonc parses $&(@ |=(a=@ a))
::  as [%bcpm p=spec q=hoon]; hatch rejects it ("found '(' expected Gap, or
::  Spec Wide"), though the tall form parses.  Would cover write_spec's
::  BucPam arm (mod.rs ~827) through the wide parser.
|%
+$  norm  $&(@ |=(a=@ a))
++  main
  |=  z=norm
  z
--
