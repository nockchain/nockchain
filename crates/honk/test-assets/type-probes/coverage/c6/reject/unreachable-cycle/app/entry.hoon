::  c6 rejection (both compilers): lib/a.hoon and lib/b.hoon import each
::  other. Nothing imports them, but hoonc's +build-merk-dag sorts the whole
::  tree and fails on the cycle.
`*`42
