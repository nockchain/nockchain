::  c3: fitz with size-suffixed auras (mod.rs fitz ~10716, rsh_bytes ~10759): a smaller-size
::  aura nests in a larger one, and a bare-size aura against a sized one; ?= fuse of a larger
::  sized atom with a smaller-sized pattern (fitz size check fails, the pattern aura wins).
|%
++  main
  |=  a=@
  :*  !>(^-(@uxE `@uxD`a))
      !>(^-(@uxD `@uxD`a))
      !>(^-(@ux `@uxC`a))
      !>(^-(@uxC `@ux`a))
      !>(^-(@D `@uD`a))
      =/  z  `@uxE`a
      !>(?:(?=(@uxD z) z !!))
  ==
--
