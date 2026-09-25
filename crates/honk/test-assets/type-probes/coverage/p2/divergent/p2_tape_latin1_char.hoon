::  p2 divergence: a non-ASCII character in a wide tape becomes one element
::  holding its code point (0xe9) in honk, but hoonc splits it into its UTF-8
::  bytes [0xc3 0xa9]; soil wide_char utils.rs:5071-5074 filters chars, not bytes
|%
++  main  "é"
--
