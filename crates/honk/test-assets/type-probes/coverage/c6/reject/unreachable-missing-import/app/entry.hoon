::  c6 rejection (both compilers): junk/x.hoon imports a library that
::  does not exist. Nothing imports junk/x.hoon, but hoonc resolves the imports
::  of every file in the tree (+resolve-pile).
`*`42
