::  Mold-shaped types: gate molds ($-), defaulted molds ($~), pair/union
::  shapes, and the mold-gate type itself via $, .
|%
++  main
  |=  g=$-(@ud @t)
  =/  d  *$~(42 @ud)
  [!>(g) !>(d) !>($,(spec))]
+$  spec  $%([%base p=@tas] [%cell p=$~(%noun @tas) q=@])
--
