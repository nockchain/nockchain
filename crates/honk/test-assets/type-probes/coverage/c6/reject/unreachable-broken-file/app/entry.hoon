::  c6 rejection (both compilers): hoonc parses every source file in the
::  dependency tree (hoonc.hoon +parse-dir) and fails on junk/bad.hoon although
::  nothing imports it (honk.rs check_dependency_tree).
/=  raw  /common/raw
`*`raw
