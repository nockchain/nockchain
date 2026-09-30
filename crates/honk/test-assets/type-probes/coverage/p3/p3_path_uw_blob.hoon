::  p3: w_ne (utils.rs ~9229) on @uw digits 36-63 in a path segment, and ~0
::  blobs whose jam is wider than 128 bits (atom_to_bits Big arm), whose
::  noun holds an atom wider than 128 bits (rub_atom), and such a blob in a
::  path segment, re-jammed through atom_bit_len and atom_get_bit.
|%
++  main
  |=  a=@
  :*  !>(/0wA)
      !>(/0wZ.-~AbC)
      !>(0wZ.-~AbC)
      !>(~04000000000000000000000000003g0)
      !>(~07og0000000000000000000000000001m01)
      !>(~0l30pm1ic32o61gfi1s8790s43cgd21i8610vgtgrgpgt1i3h)
      !>(/~04000000000000000000000000003g0)
      !>(/~07og0000000000000000000000000001m01)
      !>(/~0l30pm1ic32o61gfi1s8790s43cgd21i8610vgtgrgpgt1i3h)
  ==
--
