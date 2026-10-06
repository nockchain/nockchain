::  p3: path segments for @if/@is/@r*/@t/@ta/@c/blob/many
::  (rend_with_rep 'i' ro_co, 'r' r_co/ed_co, 't' wood, 'c' tuft, blob
::  jam_simple backrefs/mat_bits and a >128-bit re-jam of a non-canonical
::  jam (bits_to_atom BigUint arm), many rend_many/wack incl. nested many,
::  wood_go '.' '~' and hex escapes, v_ne letters)
|%
++  main
  |=  a=@
  :*  !>(/.1.2.3.4)
      !>(/.0.0.0.0)
      !>(/.255.255.255.255)
      !>(/.0.0.0.0.0.0.0.1)
      !>(/.fe80.0.0.0.0.0.0.abcd)
      !>(/~.foo-bar)
      !>(/~.)
      !>(/~~foo)
      !>(/~~foo.bar~~baz)
      !>(/~-foo)
      !>(/~02)
      !>(/~019)
      !>(/._1_2__)
      !>(/._foo_~~.bar__)
      !>(/._~~.a~-b__)
      !>(/._1_.~-2~-3~-~-__)
      !>(/~0c)
      !>(/~014t5)
      !>(/~04jo3jk1)
      !>(/~038i3h)
      !>(/~012o861)
      !>(/~0ie9nfarfnf0g1)
      !>(/~04jv3cbgl)
      !>(/~02kmiqb9d5kmiqb9d5kmiqb9d5)
      !>(/~~a~.b)
      !>(/~~~41.)
      !>(/~~a_b)
      !>(/~~a1-b0)
      !>(/~~~~.~.)
      !>(/~-~41.)
      !>(/~-~2603.)
  ==
--
