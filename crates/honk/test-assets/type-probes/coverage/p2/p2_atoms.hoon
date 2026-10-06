::  p2: atom literal helpers: yawn/yelp date arithmetic, apply_sign on small
::  and big signed atoms, ipv4/ipv6, base32/base64 groups, urs/urx knot and
::  cord text, @c via taft/tuft, %many and ~0 blob coins via jock, @uc
::  base58check (bass_58/tok/shay/den_fa)
::  (hatch utils.rs ~5504-6260, 8250-8279)
|%
++  main
  :*  :*  ~2024.3.1  ~2023.12.31  ~1900.1.1  ~2100.6.15  ~1904.2.29
          ~2000.1.1  ~1.1.1  ~2001.1.1  ~1999.7.4  ~2400.12.1
      ==
      :*  .1.2.3.4  .0.0.0.0  .255.255.255.255  .10.0.0.1
          .0.0.0.0.0.0.0.0  .1.2.3.4.5.6.7.8  .ffff.0.abcd.0.0.0.0.1
      ==
      :*  0v0  0v1  0vabc.defgh.ijklm  0v1.23456  0w0  0wa  0w1.-~aBc.Zz09y  0x0  0xabc  0x1.0000
          -0  --0  -1  --1  -0x10  --0x10  -0b1  -0v1  -0w1  -0i5  --0i5
          --0x1.0000.0000.0000.0000.0000.0000.0000.0000.0000
          -0x1.0000.0000.0000.0000.0000.0000.0000.0000.0000
          -0x1.0000.0000.0000.0000.0000.0000.0000.0000.0001
      ==
      :*  ~.foo  ~.a.b_c  ~~foo  ~~a.b  ~~~~.x  ~-foo  ~-a.b  ~-~2605.  ~-~~  ~-~.
          ~-~61.b  ~-~e9.~1f600.  ~-abcde-fgh  ~-~0.
      ==
      :*  0c1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2  0c3J98t1WpEZ73CNmQviecrnyiWrnqRhWNLy
          0c1111111111111111111114oLvT2
      ==
      :*  ._1_2__  ._foo_~~.bar_~~.a~-b__  ._.1.5_0x10__  %._1_~~.a__  ~02  ~0c  %~0c  ~04hh  %~04hh  ~038i65
      ==
  ==
--
