::  c6 rejection (both compilers): hoonc's header rule takes /- /+ /= /*
::  /# in that order, so a /- after a /+ is left for the body, where it does
::  not parse.
/+  util
/-  kinds
`*`42
