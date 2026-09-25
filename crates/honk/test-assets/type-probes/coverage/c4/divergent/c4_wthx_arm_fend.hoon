::  c4 divergence: ?# (%wthx) on a wing that names an arm. hoon-138 mints
::  %wthx with (fend %read [[%& 1] q.gen]) (hoon-138.hoon:10011, and mull at
::  10199-10200); the leading axis-1 limb turns the arm port into a leg on its
::  core, so hoonc compiles this. honk's mint_wthx (ut/mod.rs ~5048; mull at
::  ~11389) calls fend on the bare wing, gets an arm port, and rejects with
::  fend-fragment (find.rs ~78).
|%
++  main
  =>  |%  ++  b  1
      --
  ?#(@ b)
--
