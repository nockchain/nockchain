::  p2 divergence: honk rejects a tape holding a character above U+00FF
::  (hoonc accepts its UTF-8 bytes); soil wide_char utils.rs:5071-5074
::  only admits code points 0x80-0xff
|%
++  main  "€"
--
