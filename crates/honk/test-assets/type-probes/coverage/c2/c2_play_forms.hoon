::  c2: play-only paths reached through ^+ and != (vet off):
::  %sand %n/%f via tic-aura casts (play_sand ~8369-8384), a door with +*
::  aliases played as a ^+ goal (play_brcb/barcab_apply_alas ~4809-4858),
::  and =, (busk) and ?: in play position.
|%
++  main
  |=  [a=@ b=?]
  =/  q  [c=1 d=2]
  :*  !>(!=(`@n`5))
      !>(!=(`@f`5))
      !>(^+(`@n`~ ~))
      !>(^+(`@f`a &))
      !>(^+(=,(q c) 3))
      !>(^+(?:(b [1 2] [3 4]) [5 6]))
      !>(^+(?.(b 1 2) 3))
      ^+  |_  c=@
          +*  d  c
          ++  e  d
          --
      |_  c=@
      +*  d  c
      ++  e  d
      --
  ==
--
